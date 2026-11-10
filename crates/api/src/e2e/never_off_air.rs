// SPDX-License-Identifier: AGPL-3.0-or-later

//! Never off air (#221): scenarios that try to break the air with the
//! modules on, their stored data broken or their state stuck. In each,
//! something must be on air or waiting at every moment: the queue manager
//! keeps the engine's queue from running dry, whatever a module does.
//!
//! The engine here has no fallback directory: a queue that runs dry is
//! silence (on a station, its fallback directory's music).

use std::sync::atomic::{AtomicU64, Ordering};

use heron_core::PlaySource;

use super::*;
use crate::engine::PlayerState;

/// How long the queue may stay empty before the station counts as off
/// air: the queue manager fills on every change of the engine's state and
/// every 2 s.
const OFF_AIR_AFTER: Duration = Duration::from_secs(8);

/// Long enough to carry a match's vote (60 s at least).
const VOTE_TRACK_SECONDS: f64 = 75.0;

/// Adds an analyzed track to the library, as the scanner and the analyzer
/// leave it: `name` is its title and its artist.
async fn add_track(station: &Station, name: &str, seconds: f64, freq: f64) -> u64 {
    let path = format!("{name}/{name}.wav");
    std::fs::create_dir_all(station.library.path().join(name)).unwrap();
    write_wav(&station.library.path().join(&path), seconds, freq);
    let ms = (seconds * 1_000.0) as u32;
    let id = sqlx::query(
        "INSERT INTO tracks (path, title, artist, album, duration_ms) VALUES (?, ?, ?, 'Album', ?)",
    )
    .bind(&path)
    .bind(name)
    .bind(name)
    .bind(ms)
    .execute(&station.db.pool)
    .await
    .unwrap()
    .last_insert_id();
    sqlx::query(
        "INSERT INTO track_analysis (track_id, analysis_version, params_hash, cue_in_ms, cue_out_ms,
                                     integrated_lufs, true_peak_db)
         VALUES (?, 1, ?, 0, ?, -13.0, -7.0)",
    )
    .bind(id)
    .bind([0u8; 32].as_slice())
    .bind(ms)
    .execute(&station.db.pool)
    .await
    .unwrap();
    id
}

async fn category(admin: &Client, name: &str, body: Value, members: &[u64]) -> u64 {
    let mut body = body;
    body["name"] = json!(name);
    let (status, category) = admin.send(Method::POST, "/categories", body).await;
    assert_eq!(status, StatusCode::CREATED, "{category}");
    let id = category["id"].as_u64().unwrap();
    for track in members {
        let (status, _) = admin
            .send(
                Method::PUT,
                &format!("/tracks/{track}/categories"),
                json!([{"category_id": id}]),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    id
}

/// A clock of `slots`, every hour of the week; looping when it has no
/// exact time.
async fn clock_all_week(admin: &Client, slots: Value) -> u64 {
    let looping = !slots
        .as_array()
        .unwrap()
        .iter()
        .any(|slot| slot.get("exact").is_some());
    let (status, clock) = admin
        .send(
            Method::POST,
            "/clocks",
            json!({"name": "Air", "looping": looping, "slots": slots}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{clock}");
    let clock_id = clock["id"].as_u64().unwrap();
    let week: Vec<Value> = (1..=7)
        .flat_map(|day| {
            (0..24).map(move |hour| json!({"day_of_week": day, "hour": hour, "clock_id": clock_id}))
        })
        .collect();
    let (status, _) = admin.send(Method::PUT, "/schedule", json!(week)).await;
    assert_eq!(status, StatusCode::OK);
    clock_id
}

async fn put_ok(admin: &Client, path: &str, body: Value) -> Value {
    let (status, saved) = admin.send(Method::PUT, path, body).await;
    assert_eq!(status, StatusCode::OK, "PUT {path}: {saved}");
    saved
}

/// Turns modules on or off, the others as they are (`PUT /modules`
/// replaces them all).
async fn modules(admin: &Client, switches: Value) {
    let mut modules = admin.get("/modules").await;
    for (module, on) in switches.as_object().unwrap() {
        modules[module] = on.clone();
    }
    put_ok(admin, "/modules", modules).await;
}

/// Near the end of the hour, waits for the next one: a scenario's show
/// hours and exact times stay in one hour.
async fn away_from_the_hour_end(margin: Duration) {
    let margin = i64::try_from(margin.as_millis()).unwrap();
    let left = 3_600_000 - crate::rotation::now_ms().rem_euclid(3_600_000);
    if left < margin {
        tokio::time::sleep(Duration::from_millis(left as u64 + 2_000)).await;
    }
}

fn player(station: &Station) -> Option<Arc<PlayerState>> {
    station
        .state
        .stations
        .first()
        .engine
        .state()
        .borrow()
        .clone()
}

/// The entry on air.
fn on_air_entry(state: &PlayerState) -> Option<u64> {
    state
        .decks
        .iter()
        .find(|deck| deck.on_air)
        .and_then(|deck| deck.entry.as_ref())
        .map(|entry| entry.entry_id)
}

/// Something from the API waits to play (not the fallback's).
fn fed(state: &PlayerState) -> bool {
    state
        .waiting()
        .any(|entry| entry.track.source != PlaySource::Fallback)
}

/// What plays and waits, for the messages.
fn describe(state: Option<&PlayerState>) -> String {
    let Some(state) = state else {
        return "no engine state".to_owned();
    };
    let entry = |entry: &crate::engine::QueueEntry| {
        format!(
            "{}:{}({:?})",
            entry.entry_id,
            entry
                .track
                .title
                .clone()
                .unwrap_or_else(|| entry.track.path.clone()),
            entry.track.source
        )
    };
    let decks: Vec<String> = state
        .decks
        .iter()
        .filter_map(|deck| {
            deck.entry
                .as_ref()
                .map(|e| format!("{}{}", entry(e), if deck.on_air { "*" } else { "" }))
        })
        .collect();
    let waiting: Vec<String> = state.waiting().map(entry).collect();
    format!(
        "decks [{}], waiting [{}]",
        decks.join(", "),
        waiting.join(", ")
    )
}

/// Watches the engine while a scenario runs: the longest time nothing
/// played, and the longest time nothing waited.
struct Watch {
    task: tokio::task::JoinHandle<()>,
    silence_ms: Arc<AtomicU64>,
    dry_ms: Arc<AtomicU64>,
}

impl Watch {
    fn start(station: &Station) -> Self {
        let state = station.state.stations.first().engine.state();
        let silence_ms = Arc::new(AtomicU64::new(0));
        let dry_ms = Arc::new(AtomicU64::new(0));
        let (silence, dry) = (Arc::clone(&silence_ms), Arc::clone(&dry_ms));
        let task = tokio::spawn(async move {
            let mut silent_since: Option<Instant> = None;
            let mut dry_since: Option<Instant> = None;
            let span = |since: &mut Option<Instant>, now: bool, longest: &AtomicU64| {
                if now {
                    let start = *since.get_or_insert_with(Instant::now);
                    let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                    longest.fetch_max(ms, Ordering::Relaxed);
                } else {
                    *since = None;
                }
            };
            loop {
                let player = state.borrow().clone();
                if let Some(player) = player {
                    let playing =
                        !player.paused && player.decks.iter().any(|deck| deck.entry.is_some());
                    span(&mut silent_since, !playing, &silence);
                    span(&mut dry_since, !fed(&player), &dry);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        Self {
            task,
            silence_ms,
            dry_ms,
        }
    }

    /// The longest silence and the longest dry queue, in ms.
    fn stop(self) -> (u64, u64) {
        self.task.abort();
        (
            self.silence_ms.load(Ordering::Relaxed),
            self.dry_ms.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone, Copy)]
enum Skip {
    /// An operator's skip (`POST /player/skip`): an open vote closes first.
    Operator,
    /// The engine moves on by itself, as at the end of a track: the API
    /// does nothing first.
    End,
}

/// Waits for something waiting to play: an error, with what plays and
/// waits, after [`OFF_AIR_AFTER`].
async fn fed_within(station: &Station, what: &str) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let state = player(station);
        if state.as_deref().is_some_and(fed) {
            return Ok(());
        }
        if start.elapsed() > OFF_AIR_AFTER {
            return Err(format!(
                "{what}: the queue stayed empty for {OFF_AIR_AFTER:?}: {}",
                describe(state.as_deref())
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Moves on `count` tracks, each after `dwell` on air (the match's loop
/// opens a vote then): each time something must be waiting, and the next
/// track come on air. The tracks played, or how the air broke.
async fn skip_through(
    station: &Station,
    admin: &Client,
    how: Skip,
    count: usize,
    dwell: Duration,
) -> Result<Vec<u64>, String> {
    let mut played = Vec::new();
    for n in 0..count {
        tokio::time::sleep(dwell).await;
        fed_within(station, &format!("before move {n}")).await?;
        let before = player(station).as_deref().and_then(on_air_entry);
        match how {
            Skip::Operator => {
                let (status, body) = admin.request(Method::POST, "/player/skip", None).await;
                if !status.is_success() {
                    return Err(format!("move {n}: skip refused: {status} {body}"));
                }
            }
            Skip::End => {
                if let Err(error) = station.state.stations.first().engine.skip().await {
                    return Err(format!("move {n}: the engine cannot skip: {error}"));
                }
            }
        }
        let start = Instant::now();
        loop {
            let state = player(station);
            let now = state.as_deref().and_then(on_air_entry);
            if now.is_some() && now != before {
                let track = state
                    .as_deref()
                    .and_then(|state| state.decks.iter().find(|deck| deck.on_air))
                    .and_then(|deck| deck.entry.as_ref())
                    .and_then(|entry| entry.track.track_id);
                played.extend(track);
                break;
            }
            if start.elapsed() > OFF_AIR_AFTER {
                return Err(format!(
                    "move {n}: nothing new on air: {}",
                    describe(state.as_deref())
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    Ok(played)
}

/// The failures of a scenario's steps, all reported at once.
#[derive(Default)]
struct Failures(Vec<String>);

impl Failures {
    fn check<T>(&mut self, step: &str, result: Result<T, String>) {
        match result {
            Ok(_) => eprintln!("#221 {step}: on air"),
            Err(error) => {
                eprintln!("#221 {step}: OFF AIR: {error}");
                self.0.push(format!("{step}: {error}"));
            }
        }
    }

    fn watched(&mut self, step: &str, watch: Watch) {
        let (silence, dry) = watch.stop();
        eprintln!("#221 {step}: longest silence {silence} ms, longest dry queue {dry} ms");
        if Duration::from_millis(dry) >= OFF_AIR_AFTER {
            self.0
                .push(format!("{step}: the queue stayed empty for {dry} ms"));
        }
        if silence >= 3_000 {
            self.0.push(format!("{step}: {silence} ms of silence"));
        }
    }

    fn assert(self) {
        assert!(self.0.is_empty(), "off air:\n{}", self.0.join("\n"));
    }
}

/// The station of the match scenarios: Hits (six tracks long enough to
/// carry a vote), the jingles (track 5: the match's announcement, and a
/// jingle linked to the next track in the clock), the clock `slots` (Hits,
/// a linked jingle, Hits, Hits when none) every hour; the match on every
/// track, from the clock, all week, its loop running when `run_loop`.
struct MatchStation {
    hits: u64,
    hits_tracks: Vec<u64>,
    jingles: u64,
}

async fn match_station(
    station: &mut Station,
    admin: &Client,
    run_loop: bool,
    slots: impl FnOnce(u64, u64) -> Value,
) -> MatchStation {
    let mut hits_tracks = Vec::new();
    for n in 1..=6 {
        hits_tracks.push(
            add_track(
                station,
                &format!("Hit {n}"),
                VOTE_TRACK_SECONDS,
                330.0 + 55.0 * f64::from(n),
            )
            .await,
        );
    }
    let types = admin.get("/category-types").await;
    let jingle_type = types[1]["id"].as_u64().unwrap();
    let hits = category(
        admin,
        "Hits",
        json!({"separation": {"track_min": 1}}),
        &hits_tracks,
    )
    .await;
    let jingles = category(
        admin,
        "Jingles",
        json!({"type_id": jingle_type}),
        &[station.tracks[4]],
    )
    .await;
    put_ok(admin, "/settings/station", json!({"timezone": "UTC"})).await;
    clock_all_week(admin, slots(hits, jingles)).await;
    put_ok(
        admin,
        "/settings/queue",
        json!({"autofill": true, "min_queued": 2}),
    )
    .await;
    modules(admin, json!({"match": true})).await;
    put_ok(
        admin,
        "/match/settings",
        json!({"every_tracks": 1, "tracks_from": "clock", "category_id": hits,
               "jingle_category_id": jingles, "announce_every": 2, "loser_same_day": true}),
    )
    .await;
    put_ok(
        admin,
        "/shows/match",
        json!({"hours": [{"days": [1, 2, 3, 4, 5, 6, 7], "start_hour": 0, "end_hour": 24}]}),
    )
    .await;
    if run_loop {
        station.tasks.push(tokio::spawn(crate::matches::run(
            station.db.pool.clone(),
            Arc::clone(&station.state.stations),
            Arc::clone(&station.state.webhooks),
        )));
    }
    MatchStation {
        hits,
        hits_tracks,
        jingles,
    }
}

fn default_slots(hits: u64, jingles: u64) -> Value {
    json!([
        {"category_id": hits},
        {"category_id": jingles, "enforce_rules": false, "linked_to_next": true},
        {"category_id": hits},
        {"category_id": hits},
    ])
}

/// Waits for `entries` to leave the engine's queue (the queue manager
/// follows the library every 5 s).
async fn left_the_queue(station: &Station, entries: &[u64]) {
    eventually(
        "the entries out of the queue",
        Duration::from_secs(15),
        || async {
            player(station)
                .is_some_and(|state| {
                    !state
                        .waiting()
                        .any(|entry| entries.contains(&entry.entry_id))
                })
                .then_some(())
        },
    )
    .await;
}

/// The match waiting or open, as the API shows it.
async fn current_match(admin: &Client) -> Value {
    admin.get("/match/current").await
}

/// Moves on (as `how`) until the match is `status`: its JSON.
async fn until_match(station: &Station, admin: &Client, status: &str, how: Skip) -> Value {
    for _ in 0..12 {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(4) {
            let current = current_match(admin).await;
            if current["status"] == status {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        skip_through(station, admin, how, 1, Duration::ZERO)
            .await
            .unwrap_or_else(|error| panic!("waiting for a match {status}: {error}"));
    }
    panic!("no match {status}");
}

/// A top's chart: `ranked` (number 1 first), voted in the week frozen
/// three days ago at noon (whatever the day the test runs), its tracks from
/// `music`, its jingles from `jingles`. Returns the top and its settings.
async fn top30_chart(
    station: &Station,
    admin: &Client,
    music: u64,
    jingles: u64,
    ranked: &[u64],
) -> (u64, Value) {
    let pool = &station.db.pool;
    modules(admin, json!({"top30": true})).await;
    let today = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .weekday()
        .to_monday_one_offset();
    let reset_weekday = (today + 3) % 7 + 1;
    let size = [3, 5, 10, 20, 30, 40, 50]
        .into_iter()
        .find(|size| *size >= ranked.len())
        .unwrap_or(50);
    let settings = json!({"name": "Top 30", "min_votes": 2, "size": size, "category_ids": [music],
                          "jingle_category_id": jingles, "period": "week",
                          "reset_weekday": reset_weekday, "reset_time": "12:00"});
    let (status, top) = admin.send(Method::POST, "/tops", settings.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{top}");
    let parsed: crate::tops::TopSettings = serde_json::from_value(settings.clone()).unwrap();
    let closed =
        crate::tops::latest_frozen(&parsed, &jiff::tz::TimeZone::UTC, jiff::Timestamp::now())
            .unwrap();
    for (rank, track) in ranked.iter().enumerate() {
        let started = closed.from.as_millisecond() as f64 / 1_000.0 + 3_600.0;
        let play = sqlx::query(
            "INSERT INTO play_history (station_id, track_id, title, started_at, ended_at, deck, source)
             VALUES (1, ?, 'x', FROM_UNIXTIME(?), FROM_UNIXTIME(?), 'a', 'rotation')",
        )
        .bind(track)
        .bind(started)
        .bind(started + 3.0)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
        sqlx::query(
            "INSERT INTO play_votes (play_id, station_id, track_id, started_at, likes, dislikes)
             VALUES (?, 1, ?, FROM_UNIXTIME(?), ?, 1)",
        )
        .bind(play)
        .bind(track)
        .bind(started)
        .bind(20 - u32::try_from(rank).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }
    (top["id"].as_u64().unwrap(), settings)
}

/// The hours of `show`: the current hour (UTC) today.
async fn this_hour(admin: &Client, show: &str) {
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    let (day, hour) = (now.weekday().to_monday_one_offset(), now.hour());
    put_ok(
        admin,
        &format!("/shows/{show}"),
        json!({"hours": [{"days": [day], "start_hour": hour, "end_hour": hour + 1}]}),
    )
    .await;
}

/// The top's show: the current hour (UTC), today's day of the week; none
/// with `on` off.
async fn top30_this_hour(admin: &Client, top: &(u64, Value), on: bool) {
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    let (day, hour) = (now.weekday().to_monday_one_offset(), now.hour());
    let mut settings = top.1.clone();
    settings["slot"] = if on {
        json!({"every": "week", "weekday": day, "start": format!("{hour:02}:00"),
               "end": format!("{:02}:00", hour + 1)})
    } else {
        Value::Null
    };
    put_ok(admin, &format!("/tops/{}", top.0), settings).await;
}

/// The top's programme on air: its items and the next one to queue.
async fn top30_programme(pool: &sqlx::MySqlPool) -> Option<(Vec<u64>, u64)> {
    let row: Option<(String, u64)> = sqlx::query_as(
        "SELECT CAST(programme AS CHAR), CAST(next_item AS UNSIGNED) FROM top_shows
         ORDER BY opens_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    let (programme, next) = row?;
    let programme: Value = serde_json::from_str(&programme).unwrap();
    let items = programme["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] != "jingle")
        .map(|item| item["track_id"].as_u64().unwrap())
        .collect();
    Some((items, next))
}

/// Writes a top's stored settings with an unknown field, as a restore, an
/// import or a downgrade can leave them. Returns what they were.
async fn corrupt_top(pool: &sqlx::MySqlPool, top: u64) -> String {
    let before: String = sqlx::query_scalar("SELECT CAST(settings AS CHAR) FROM tops WHERE id = ?")
        .bind(top)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tops SET settings = JSON_SET(settings, '$.unknown_field', 1) WHERE id = ?")
        .bind(top)
        .execute(pool)
        .await
        .unwrap();
    before
}

async fn restore_top(pool: &sqlx::MySqlPool, top: u64, before: String) {
    sqlx::query("UPDATE tops SET settings = ? WHERE id = ?")
        .bind(before)
        .bind(top)
        .execute(pool)
        .await
        .unwrap();
}

/// Writes a station's settings group as stored, with an unknown field:
/// as a restore, an import or a downgrade can leave it. Returns what it
/// was.
async fn corrupt_settings(pool: &sqlx::MySqlPool, name: &str) -> Option<String> {
    let before: Option<String> = sqlx::query_scalar(
        "SELECT CAST(value AS CHAR) FROM station_settings WHERE station_id = 1 AND name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO station_settings (station_id, name, value) VALUES (1, ?, '{\"unknown_field\": 1}')
         ON DUPLICATE KEY UPDATE value = JSON_SET(value, '$.unknown_field', 1)",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
    before
}

async fn restore_settings(pool: &sqlx::MySqlPool, name: &str, before: Option<String>) {
    match before {
        Some(value) => {
            sqlx::query("UPDATE station_settings SET value = ? WHERE station_id = 1 AND name = ?")
                .bind(value)
                .bind(name)
        }
        None => {
            sqlx::query("DELETE FROM station_settings WHERE station_id = 1 AND name = ?").bind(name)
        }
    }
    .execute(pool)
    .await
    .unwrap();
}

/// Scenario 1: every module on at once (a match on every track from the
/// clock, the Top 30 in its hours, a dedication accepted, an exact time,
/// linked jingles, a medley), and twenty tracks skipped through.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_every_module_on() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    away_from_the_hour_end(Duration::from_secs(240)).await;
    let admin = station.signed_in("admin").await;
    let t = station.tracks.clone();
    let medley = add_track(&station, "Medley", VOTE_TRACK_SECONDS, 300.0).await;
    let hour_start = crate::rotation::now_ms() - crate::rotation::now_ms().rem_euclid(3_600_000);
    // After the tracks skipped through: the exact time comes while the
    // match chains.
    let exact_s = (crate::rotation::now_ms() - hour_start) / 1_000 + 75;
    let free =
        json!({"separation": {"track_min": 0, "title_min": 0, "artist_min": 0, "album_min": 0}});
    let news = category(&admin, "News", free.clone(), &[t[3]]).await;
    let medleys = category(&admin, "Medleys", free, &[medley]).await;
    let pool = station.db.pool.clone();
    sqlx::query(
        "INSERT INTO medleys (name, recipe, path, track_id, rendered_at)
         VALUES ('The medley', '{}', 'Medley/Medley.wav', ?, NOW(6))",
    )
    .bind(medley)
    .execute(&pool)
    .await
    .unwrap();
    put_ok(&admin, "/settings/medleys", json!({"category_id": medleys})).await;
    let setup = match_station(&mut station, &admin, true, |hits, jingles| {
        json!([
            {"category_id": hits},
            {"category_id": jingles, "enforce_rules": false, "linked_to_next": true},
            {"category_id": hits},
            {"category_id": medleys, "enforce_rules": false},
            {"category_id": hits},
            {"category_id": news, "enforce_rules": false,
             "exact": {"at_s": exact_s, "mode": "hard"}},
        ])
    })
    .await;
    let h = setup.hits_tracks.clone();
    // The clock plays once in the hour (its exact time): Hits after it.
    let mut rotation = admin.get("/settings/rotation").await;
    rotation["filler_category_id"] = json!(setup.hits);
    put_ok(&admin, "/settings/rotation", rotation).await;
    modules(
        &admin,
        json!({"medleys": true, "dedications": true, "top30": true, "match": true}),
    )
    .await;

    // The Top 30 in this hour, the match in the others: an hour belongs to
    // one show.
    let hour = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .hour();
    let all_week = [1, 2, 3, 4, 5, 6, 7];
    let mut others = Vec::new();
    if hour > 0 {
        others.push(json!({"days": all_week, "start_hour": 0, "end_hour": hour}));
    }
    if hour < 23 {
        others.push(json!({"days": all_week, "start_hour": hour + 1, "end_hour": 24}));
    }
    put_ok(&admin, "/shows/match", json!({"hours": others})).await;

    // A dedication asked in this hour, then accepted in the Top 30's.
    let code = station.state.stations.first().code();
    this_hour(&admin, "dedications").await;
    let mut gateway = station.client();
    gateway.bearer = Some(GATEWAY_TOKEN.to_owned());
    let (status, asked) = gateway_write(
        &gateway,
        &format!("/gateway/stations/{code}/dedications"),
        Some("dev-a"),
        json!({"track_id": h[4], "from": "Marc", "to": "Julie"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    put_ok(&admin, "/shows/dedications", json!({"hours": []})).await;
    let top = top30_chart(&station, &admin, setup.hits, setup.jingles, &h[..3]).await;
    top30_this_hour(&admin, &top, true).await;
    let list = admin.get("/dedications").await;
    let id = list["dedications"][0]["id"].clone();
    let (status, accepted) = admin
        .send(
            Method::POST,
            &format!("/dedications/{id}/accept"),
            json!({"place": "best"}),
        )
        .await;
    eprintln!("#221 dedication accepted at the best place: {status} {accepted}");

    let watch = Watch::start(&station);
    let mut failures = Failures::default();
    // Ten tracks in the Top 30's hour; then the hour goes to the match, in
    // the middle of the show, and ten more.
    let mut played = skip_through(
        &station,
        &admin,
        Skip::Operator,
        10,
        Duration::from_millis(1_500),
    )
    .await;
    if let Ok(first) = &mut played {
        top30_this_hour(&admin, &top, false).await;
        put_ok(
            &admin,
            "/shows/match",
            json!({"hours": [{"days": all_week, "start_hour": 0, "end_hour": 24}]}),
        )
        .await;
        let then = skip_through(
            &station,
            &admin,
            Skip::Operator,
            10,
            Duration::from_millis(1_500),
        )
        .await;
        played = then.map(|then| [first.clone(), then].concat());
    }
    // The exact time, the tracks ending by themselves around it.
    let exact_at = hour_start + exact_s * 1_000;
    if played.is_ok() {
        while crate::rotation::now_ms() < exact_at + 3_000 {
            if let Err(error) = fed_within(&station, "before the exact time").await {
                played = Err(error);
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    if let Ok(first) = &mut played {
        let then = skip_through(&station, &admin, Skip::End, 3, Duration::from_millis(1_500)).await;
        played = then.map(|then| [first.clone(), then].concat());
    }
    if let Ok(played) = &played {
        let matches: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT CAST(status AS CHAR), CAST(decided_by AS CHAR) FROM matches ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let programme = top30_programme(&pool).await;
        let exact_played: Vec<i64> = sqlx::query_scalar(
            "SELECT CAST(UNIX_TIMESTAMP(started_at) * 1000 AS SIGNED) - ? FROM play_history
             WHERE track_id = ? ORDER BY id",
        )
        .bind(exact_at)
        .bind(t[3])
        .fetch_all(&pool)
        .await
        .unwrap();
        eprintln!(
            "#221 played {played:?}\n  matches {matches:?}\n  top 30 {programme:?}\n  \
             medley played: {}, dedication's track played: {}, the exact time's track started (ms from its time): {exact_played:?}",
            played.contains(&medley),
            played.contains(&h[4]),
        );
    }
    failures.check("every module on: 20 tracks skipped", played);
    failures.watched("every module on", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 2a: a top's stored settings with an unknown field, outside
/// its show and in it before its programme is built.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_the_top30_settings_corrupted() {
    let Some(station) = Station::start().await else {
        return;
    };
    away_from_the_hour_end(Duration::from_secs(120)).await;
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    let t = station.tracks.clone();
    let music = category(&admin, "Music", json!({}), &t[..4]).await;
    let jingles = category(&admin, "Jingles", json!({}), &t[4..]).await;
    put_ok(&admin, "/settings/station", json!({"timezone": "UTC"})).await;
    put_ok(
        &admin,
        "/settings/queue",
        json!({"autofill": true, "min_queued": 2}),
    )
    .await;
    let top = top30_chart(&station, &admin, music, jingles, &t[..3]).await;
    let mut failures = Failures::default();

    // Outside its show: read on every fill.
    let before = corrupt_top(&pool, top.0).await;
    let watch = Watch::start(&station);
    let result = skip_through(&station, &admin, Skip::End, 4, Duration::ZERO).await;
    failures.check("top settings broken, outside the show", result);
    failures.watched("top settings broken", watch);
    restore_top(&pool, top.0, before).await;
    fed_within(&station, "top settings restored").await.unwrap();

    // In its show, its settings broken before the programme is built.
    top30_this_hour(&admin, &top, true).await;
    let before = corrupt_top(&pool, top.0).await;
    let watch = Watch::start(&station);
    let result = skip_through(&station, &admin, Skip::End, 4, Duration::ZERO).await;
    failures.check("top settings broken, in the show", result);
    failures.watched("top settings broken in the show", watch);
    restore_top(&pool, top.0, before).await;
    failures.assert();
    station.stop().await;
}

/// Scenario 2b: a top's programme item made unplayable during the show
/// (a cue override leaving less than 1 s): the show goes on without it.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_a_top30_item_made_unplayable() {
    let Some(station) = Station::start().await else {
        return;
    };
    away_from_the_hour_end(Duration::from_secs(180)).await;
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    let t = station.tracks.clone();
    let mut ranked = Vec::new();
    for n in 1..=3 {
        ranked.push(
            add_track(
                &station,
                &format!("Chart {n}"),
                40.0,
                300.0 + 50.0 * f64::from(n),
            )
            .await,
        );
    }
    let mut music_tracks = ranked.clone();
    music_tracks.extend(&t[..4]);
    let music = category(&admin, "Music", json!({}), &music_tracks).await;
    let jingles = category(&admin, "Jingles", json!({}), &t[4..]).await;
    put_ok(&admin, "/settings/station", json!({"timezone": "UTC"})).await;
    put_ok(
        &admin,
        "/settings/queue",
        json!({"autofill": true, "min_queued": 1}),
    )
    .await;
    let top = top30_chart(&station, &admin, music, jingles, &ranked).await;
    top30_this_hour(&admin, &top, true).await;
    let (items, _) = eventually("the show built", Duration::from_secs(20), || async {
        top30_programme(&pool).await
    })
    .await;
    eprintln!("#221 the top 30's programme: {items:?}");
    // Number 1, last: made unplayable once the show has started.
    let number_one = *items
        .iter()
        .find(|track| **track == ranked[0])
        .expect("number 1 in the programme");
    sqlx::query(
        "INSERT INTO track_overrides (track_id, cue_in_ms, cue_out_ms) VALUES (?, 0, 500)
         ON DUPLICATE KEY UPDATE cue_in_ms = 0, cue_out_ms = 500",
    )
    .bind(number_one)
    .execute(&pool)
    .await
    .unwrap();
    let watch = Watch::start(&station);
    let mut failures = Failures::default();
    let result = skip_through(&station, &admin, Skip::End, 6, Duration::from_millis(500)).await;
    eprintln!("#221 the top 30 after: {:?}", top30_programme(&pool).await);
    failures.check("a top 30 item unplayable", result);
    failures.watched("a top 30 item unplayable", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 3a: a match stuck in `queued`, its loop not running: the
/// vote's track, A and B play, and the queue must not wait for it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "off air: #221 a match stuck queued holds the queue for good (holds_queue)"]
async fn never_off_air_with_a_match_stuck_queued() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    match_station(&mut station, &admin, false, default_slots).await;
    let queued = until_match(&station, &admin, "queued", Skip::End).await;
    eprintln!("#221 match queued: {}", queued["entry_ids"]);
    let watch = Watch::start(&station);
    let mut failures = Failures::default();
    // Past the vote's track, A and B, and what waits before them.
    let result = skip_through(&station, &admin, Skip::End, 10, Duration::from_millis(500)).await;
    failures.check("a match stuck queued", result);
    failures.watched("a match stuck queued", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 3b: a match stuck in `open` (status forced in the database),
/// its loop not running: its tracks play by themselves.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "off air: #221 a match stuck open holds the queue for good (holds_queue)"]
async fn never_off_air_with_a_match_stuck_open() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    match_station(&mut station, &admin, false, default_slots).await;
    let queued = until_match(&station, &admin, "queued", Skip::End).await;
    sqlx::query(
        "UPDATE matches SET status = 'open', opened_at = NOW(6),
                closes_at = NOW(6) + INTERVAL 1 HOUR, transition_at = NOW(6) + INTERVAL 1 HOUR
         WHERE id = ?",
    )
    .bind(queued["id"].as_u64())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(current_match(&admin).await["status"], "open");
    let watch = Watch::start(&station);
    let mut failures = Failures::default();
    // Past the vote's track, A and B, and what waits before them.
    let result = skip_through(&station, &admin, Skip::End, 10, Duration::from_millis(500)).await;
    failures.check("a match stuck open", result);
    failures.watched("a match stuck open", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 4: the `matches` table failing (renamed) during a chain of
/// matches: the music goes on without the match, and an operator can still
/// skip and clear.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "off air: #221 a match table error refuses the operator's skip and clear (matches::pending `?`)"]
async fn never_off_air_with_the_matches_table_failing() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    match_station(&mut station, &admin, true, default_slots).await;
    // A chain: a match open, the next one queued behind its winner.
    until_match(&station, &admin, "open", Skip::Operator).await;
    skip_through(&station, &admin, Skip::Operator, 1, Duration::ZERO)
        .await
        .unwrap();
    until_match(&station, &admin, "open", Skip::Operator).await;
    sqlx::query("RENAME TABLE matches TO matches_gone")
        .execute(&pool)
        .await
        .unwrap();
    let watch = Watch::start(&station);
    let mut failures = Failures::default();
    let result = skip_through(&station, &admin, Skip::End, 6, Duration::from_millis(500)).await;
    failures.check("the matches table gone: tracks end", result);
    let result = skip_through(&station, &admin, Skip::Operator, 2, Duration::ZERO).await;
    failures.check("the matches table gone: an operator skips", result);
    let (status, body) = admin.request(Method::DELETE, "/queue", None).await;
    failures.check(
        "the matches table gone: an operator clears the queue",
        status
            .is_success()
            .then_some(())
            .ok_or(format!("{status} {body}")),
    );
    let result = skip_through(&station, &admin, Skip::End, 2, Duration::ZERO).await;
    failures.check("the matches table gone: after the clear", result);
    failures.watched("the matches table gone", watch);
    sqlx::query("RENAME TABLE matches_gone TO matches")
        .execute(&pool)
        .await
        .unwrap();
    let watch = Watch::start(&station);
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        6,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("the matches table back", result);
    failures.watched("the matches table back", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 5: an invalid restriction rule and settings groups with an
/// unknown field, written straight into the database (a restore, an
/// import, a downgrade): each in turn, the queue must still fill.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "off air: #221 one bad restriction or settings group fails every fill"]
async fn never_off_air_with_bad_stored_data() {
    let Some(station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    let t = station.tracks.clone();
    let music = category(
        &admin,
        "Music",
        json!({"separation": {"track_min": 0, "title_min": 0, "artist_min": 0, "album_min": 0}}),
        &t,
    )
    .await;
    clock_all_week(&admin, json!([{"category_id": music}])).await;
    put_ok(&admin, "/settings/station", json!({"timezone": "UTC"})).await;
    put_ok(
        &admin,
        "/settings/queue",
        json!({"autofill": true, "min_queued": 2}),
    )
    .await;
    // The modules whose settings the queue reads: on, outside their hours.
    modules(
        &admin,
        json!({"match": true, "top30": true, "medleys": true, "restrictions": true}),
    )
    .await;
    put_ok(
        &admin,
        "/match/settings",
        json!({"every_tracks": 4, "category_id": music}),
    )
    .await;
    // Every group saved once, as a station that set them up has them.
    for (path, name) in [
        ("/settings/rotation", "rotation"),
        ("/settings/playout", "playout"),
        ("/settings/medleys", "medleys"),
    ] {
        let current = admin.get(path).await;
        let (status, body) = admin.send(Method::PUT, path, current).await;
        assert_eq!(status, StatusCode::OK, "{name}: {body}");
    }
    let mut failures = Failures::default();
    fed_within(&station, "the start").await.unwrap();

    // An invalid restriction rule.
    let restriction = sqlx::query(
        "INSERT INTO restrictions (target, target_id, rule) VALUES ('track', ?, '{\"kind\": \"no_such_rule\"}')",
    )
    .bind(t[0])
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_id();
    station.state.stations.first().engine.clear().await.unwrap();
    let watch = Watch::start(&station);
    let result = skip_through(&station, &admin, Skip::End, 3, Duration::ZERO).await;
    failures.check("an invalid restriction rule", result);
    failures.watched("an invalid restriction rule", watch);
    sqlx::query("DELETE FROM restrictions WHERE id = ?")
        .bind(restriction)
        .execute(&pool)
        .await
        .unwrap();
    let _ = fed_within(&station, "the restriction deleted").await;

    // Each settings group with an unknown field.
    for name in [
        "queue",
        "rotation",
        "playout",
        "station",
        "medley_categories",
        "station_modules",
        "show_match",
        "match",
    ] {
        let before = corrupt_settings(&pool, name).await;
        station.state.stations.first().engine.clear().await.unwrap();
        let watch = Watch::start(&station);
        let result = skip_through(&station, &admin, Skip::End, 3, Duration::ZERO).await;
        let step = format!("{name} settings with an unknown field");
        failures.check(&step, result);
        failures.watched(&step, watch);
        restore_settings(&pool, name, before).await;
        let _ = fed_within(&station, &format!("{name} restored")).await;
    }
    failures.assert();
    station.stop().await;
}

/// Scenario 6: the match module turned off and on during a vote, and in a
/// chain (the next match queued behind the winner).
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_the_match_turned_off_and_on() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    match_station(&mut station, &admin, true, default_slots).await;
    let mut failures = Failures::default();
    let toggle = |pause: Duration| {
        let admin = &admin;
        async move {
            modules(admin, json!({"match": false})).await;
            tokio::time::sleep(pause).await;
            modules(admin, json!({"match": true})).await;
        }
    };

    // During a vote, the loop ticking while off.
    until_match(&station, &admin, "open", Skip::Operator).await;
    let watch = Watch::start(&station);
    toggle(Duration::from_millis(2_500)).await;
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        5,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("off and on during a vote", result);
    failures.watched("off and on during a vote", watch);

    // During a vote, on again before the loop ticks.
    until_match(&station, &admin, "open", Skip::Operator).await;
    let watch = Watch::start(&station);
    toggle(Duration::ZERO).await;
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        5,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("off and on at once during a vote", result);
    failures.watched("off and on at once during a vote", watch);

    // In a chain: the winner on air, the next match queued behind it.
    until_match(&station, &admin, "open", Skip::Operator).await;
    skip_through(&station, &admin, Skip::Operator, 1, Duration::ZERO)
        .await
        .unwrap();
    let watch = Watch::start(&station);
    toggle(Duration::from_millis(2_500)).await;
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        5,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("off and on in a chain", result);
    failures.watched("off and on in a chain", watch);

    // Off, the tracks ending by themselves, then on again.
    modules(&admin, json!({"match": false})).await;
    let watch = Watch::start(&station);
    let result = skip_through(&station, &admin, Skip::End, 4, Duration::from_millis(500)).await;
    failures.check("off", result);
    modules(&admin, json!({"match": true})).await;
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        5,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("on again", result);
    failures.watched("off, then on", watch);
    failures.assert();
    station.stop().await;
}

const DISABLE: &str = "UPDATE tracks SET enabled = FALSE WHERE id = ?";
const ENABLE: &str = "UPDATE tracks SET enabled = TRUE WHERE id = ?";
/// Its file gone, as when deleted.
const GONE: &str = "UPDATE tracks SET missing_since = NOW(6) WHERE id = ?";
const BACK: &str = "UPDATE tracks SET missing_since = NULL WHERE id = ?";

async fn set_track(pool: &sqlx::MySqlPool, sql: &'static str, track: u64) {
    sqlx::query(sql).bind(track).execute(pool).await.unwrap();
}

/// A match's two tracks and their entries.
fn sides(found: &Value) -> [(u64, u64); 2] {
    [0, 1].map(|side| {
        (
            found["choices"][side]["track_id"].as_u64().unwrap(),
            found["entry_ids"][side + 1].as_u64().unwrap(),
        )
    })
}

/// Scenario 7a: one of a match's tracks disabled, or deleted (its file
/// gone), while it waits: the queue manager takes it out of the queue.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_a_match_s_track_disabled_or_deleted() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    match_station(&mut station, &admin, true, default_slots).await;
    let mut failures = Failures::default();

    // A disabled while the match is queued.
    let [(a, entry_a), _] = sides(&until_match(&station, &admin, "queued", Skip::Operator).await);
    let watch = Watch::start(&station);
    set_track(&pool, DISABLE, a).await;
    left_the_queue(&station, &[entry_a]).await;
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        5,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("A disabled while queued", result);
    failures.watched("A disabled while queued", watch);
    set_track(&pool, ENABLE, a).await;

    // B's file gone while the vote is open.
    let [_, (b, entry_b)] = sides(&until_match(&station, &admin, "open", Skip::Operator).await);
    let watch = Watch::start(&station);
    set_track(&pool, GONE, b).await;
    left_the_queue(&station, &[entry_b]).await;
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        5,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("B deleted while open", result);
    failures.watched("B deleted while open", watch);
    set_track(&pool, BACK, b).await;
    failures.assert();
    station.stop().await;
}

/// Scenario 7b: both of a match's tracks gone (one disabled, one deleted)
/// while its vote is open: the queue must not stay empty until the vote
/// closes, and the vote's track ending must not be silence.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "off air: #221 a match whose two tracks are gone holds an empty queue until its vote closes"]
async fn never_off_air_with_both_of_a_match_s_tracks_gone() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    // Nothing between the vote's track and A: no linked jingle.
    match_station(
        &mut station,
        &admin,
        true,
        |hits, _| json!([{"category_id": hits}]),
    )
    .await;
    let mut failures = Failures::default();
    let [(a, entry_a), (b, entry_b)] =
        sides(&until_match(&station, &admin, "open", Skip::Operator).await);
    let watch = Watch::start(&station);
    set_track(&pool, DISABLE, a).await;
    set_track(&pool, GONE, b).await;
    left_the_queue(&station, &[entry_a, entry_b]).await;
    eprintln!(
        "#221 A and B gone: {}",
        describe(player(&station).as_deref())
    );
    let result = fed_within(&station, "A and B gone, the vote open").await;
    failures.check("A and B gone while open: the queue", result);
    // The vote's track ends, the queue empty: how long until music again.
    let vote_entry = player(&station).as_deref().and_then(on_air_entry);
    station.state.stations.first().engine.skip().await.unwrap();
    let ended = Instant::now();
    let result = loop {
        let now = player(&station).as_deref().and_then(on_air_entry);
        if now.is_some() && now != vote_entry {
            eprintln!("#221 A and B gone: music again after {:?}", ended.elapsed());
            break skip_through(&station, &admin, Skip::End, 3, Duration::from_millis(1_500)).await;
        }
        if ended.elapsed() > OFF_AIR_AFTER {
            break Err(format!(
                "silence after the vote's track: {}",
                describe(player(&station).as_deref())
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    failures.check("A and B gone while open: the vote's track ends", result);
    failures.watched("A and B gone while open", watch);
    set_track(&pool, ENABLE, a).await;
    set_track(&pool, BACK, b).await;
    failures.assert();
    station.stop().await;
}

/// Scenario 8a: the queue cleared, tracks skipped and removed during a
/// match.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_the_queue_changed_during_a_match() {
    let Some(mut station) = Station::start().await else {
        return;
    };
    let admin = station.signed_in("admin").await;
    match_station(&mut station, &admin, true, default_slots).await;
    let mut failures = Failures::default();

    // Cleared while the match is queued, then while open.
    until_match(&station, &admin, "queued", Skip::Operator).await;
    let watch = Watch::start(&station);
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        4,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("cleared while queued", result);
    until_match(&station, &admin, "open", Skip::Operator).await;
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        4,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("cleared while open", result);
    failures.watched("cleared during a match", watch);

    // Every entry that can go removed, while open.
    until_match(&station, &admin, "open", Skip::Operator).await;
    let watch = Watch::start(&station);
    for entry in admin.get("/queue").await.as_array().unwrap() {
        let _ = admin
            .request(
                Method::DELETE,
                &format!("/queue/{}", entry["entry_id"]),
                None,
            )
            .await;
    }
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        4,
        Duration::from_millis(1_500),
    )
    .await;
    failures.check("removed while open", result);
    // Skipped fast, the loop barely ticking.
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        8,
        Duration::from_millis(300),
    )
    .await;
    failures.check("skipped fast", result);
    failures.watched("removed and skipped during a match", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 8b: the queue cleared, tracks skipped and removed during the
/// show of a top.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_the_queue_changed_during_the_top30() {
    let Some(station) = Station::start().await else {
        return;
    };
    away_from_the_hour_end(Duration::from_secs(180)).await;
    let admin = station.signed_in("admin").await;
    let pool = station.db.pool.clone();
    let t = station.tracks.clone();
    let mut ranked = Vec::new();
    for n in 1..=5 {
        ranked.push(
            add_track(
                &station,
                &format!("Chart {n}"),
                30.0,
                300.0 + 50.0 * f64::from(n),
            )
            .await,
        );
    }
    let mut music_tracks = ranked.clone();
    music_tracks.extend(&t[..4]);
    let music = category(&admin, "Music", json!({}), &music_tracks).await;
    let jingles = category(&admin, "Jingles", json!({}), &t[4..]).await;
    put_ok(&admin, "/settings/station", json!({"timezone": "UTC"})).await;
    put_ok(
        &admin,
        "/settings/queue",
        json!({"autofill": true, "min_queued": 2}),
    )
    .await;
    let top = top30_chart(&station, &admin, music, jingles, &ranked).await;
    top30_this_hour(&admin, &top, true).await;
    eventually("the show built", Duration::from_secs(20), || async {
        top30_programme(&pool).await
    })
    .await;
    let mut failures = Failures::default();
    let watch = Watch::start(&station);
    let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        2,
        Duration::from_millis(300),
    )
    .await;
    failures.check("cleared during the top 30", result);
    for entry in admin.get("/queue").await.as_array().unwrap() {
        let _ = admin
            .request(
                Method::DELETE,
                &format!("/queue/{}", entry["entry_id"]),
                None,
            )
            .await;
    }
    let result = skip_through(
        &station,
        &admin,
        Skip::Operator,
        8,
        Duration::from_millis(300),
    )
    .await;
    failures.check("removed and skipped during the top 30", result);
    eprintln!("#221 the top 30 after: {:?}", top30_programme(&pool).await);
    failures.watched("the queue changed during the top 30", watch);
    failures.assert();
    station.stop().await;
}

/// Scenario 8c: the queue cleared, tracks skipped and removed just before
/// an exact time.
#[tokio::test(flavor = "multi_thread")]
async fn never_off_air_with_the_queue_changed_before_an_exact_time() {
    let Some(station) = Station::start().await else {
        return;
    };
    away_from_the_hour_end(Duration::from_secs(240)).await;
    let admin = station.signed_in("admin").await;
    let t = station.tracks.clone();
    let mut songs = Vec::new();
    for n in 1..=4 {
        songs.push(
            add_track(
                &station,
                &format!("Song {n}"),
                20.0,
                300.0 + 50.0 * f64::from(n),
            )
            .await,
        );
    }
    let free =
        json!({"separation": {"track_min": 0, "title_min": 0, "artist_min": 0, "album_min": 0}});
    let news = category(&admin, "News", free.clone(), &[t[4]]).await;
    let songs = category(&admin, "Songs", free, &songs).await;
    let mut rotation = admin.get("/settings/rotation").await;
    rotation["filler_category_id"] = json!(songs);
    put_ok(&admin, "/settings/rotation", rotation).await;
    let now = crate::rotation::now_ms();
    let hour_start = now - now.rem_euclid(3_600_000);
    let first_s = (now - hour_start) / 1_000 + 45;
    let second_s = first_s + 60;
    let mut failures = Failures::default();
    clock_all_week(
        &admin,
        json!([
            {"category_id": songs, "enforce_rules": false},
            {"category_id": news, "enforce_rules": false, "exact": {"at_s": first_s, "mode": "hard"}},
            {"category_id": songs, "enforce_rules": false},
            {"category_id": news, "enforce_rules": false,
             "exact": {"at_s": second_s, "mode": "soft", "before_s": 0, "after_s": 0}},
            {"category_id": songs, "enforce_rules": false},
        ]),
    )
    .await;
    put_ok(
        &admin,
        "/settings/queue",
        json!({"autofill": true, "min_queued": 3}),
    )
    .await;
    let watch = Watch::start(&station);
    for (what, at_s) in [("hard", first_s), ("soft", second_s)] {
        let at = hour_start + at_s * 1_000;
        // Just before it: the queue cleared, its entry removed by hand.
        while crate::rotation::now_ms() < at - 8_000 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let (status, _) = admin.request(Method::DELETE, "/queue", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let result = fed_within(&station, "cleared").await;
        failures.check(&format!("cleared before the {what} exact time"), result);
        for entry in admin.get("/queue").await.as_array().unwrap() {
            if entry["track"]["start_at_unix_ms"] == at {
                let _ = admin
                    .request(
                        Method::DELETE,
                        &format!("/queue/{}", entry["entry_id"]),
                        None,
                    )
                    .await;
            }
        }
        let result = skip_through(
            &station,
            &admin,
            Skip::Operator,
            3,
            Duration::from_millis(500),
        )
        .await;
        failures.check(&format!("skipped before the {what} exact time"), result);
        while crate::rotation::now_ms() < at + 2_000 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let result = skip_through(&station, &admin, Skip::End, 3, Duration::from_millis(500)).await;
        failures.check(&format!("after the {what} exact time"), result);
    }
    failures.watched("the queue changed before exact times", watch);
    failures.assert();
    station.stop().await;
}
