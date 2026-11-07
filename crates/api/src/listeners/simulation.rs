// SPDX-License-Identifier: AGPL-3.0-or-later

//! The collector against a simulated Icecast server: listeners of known
//! connections, minute after minute, recorded in a scratch database (see
//! `HERON_TEST_DATABASE_URL`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use heron_service::db::ScratchDatabase;
use jiff::civil::date;
use jiff::tz::TimeZone;

use super::collector::Collector;
use super::totals;
use crate::settings::{self, StationSettings, StatisticsSettings};

const MIN: i64 = 60_000;
/// `admin:pw`.
const ADMIN: &str = "Basic YWRtaW46cHc=";

#[derive(Clone)]
struct Connection {
    mount: &'static str,
    id: u64,
    address: &'static str,
    player: &'static str,
    from_ms: i64,
    to_ms: i64,
}

#[derive(Default)]
struct Server {
    now_ms: i64,
    down: bool,
    /// The mounts it has; `/live` and `/other` when empty.
    mounts: Vec<&'static str>,
    connections: Vec<Connection>,
}

impl Server {
    fn connected(&self, mount: &str) -> Vec<&Connection> {
        self.connections
            .iter()
            .filter(|c| c.mount == mount && c.from_ms <= self.now_ms && self.now_ms < c.to_ms)
            .collect()
    }
}

type Shared = Arc<Mutex<Server>>;

async fn status(State(server): State<Shared>) -> Response {
    let server = server.lock().unwrap();
    if server.down {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mounts = if server.mounts.is_empty() {
        vec!["/live", "/other"]
    } else {
        server.mounts.clone()
    };
    let sources: Vec<_> = mounts
        .iter()
        .map(|mount| {
            serde_json::json!({
                "listeners": server.connected(mount).len(),
                "listenurl": format!("http://localhost:8000{mount}"),
            })
        })
        .collect();
    axum::Json(serde_json::json!({"icestats": {"source": sources}})).into_response()
}

async fn list_clients(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let server = server.lock().unwrap();
    if server.down {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(ADMIN) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mount = query.get("mount").cloned().unwrap_or_default();
    let mut xml = format!("<?xml version=\"1.0\"?><icestats><source mount=\"{mount}\">");
    for c in server.connected(&mount) {
        xml.push_str(&format!(
            "<listener><IP>{}</IP><UserAgent>{}</UserAgent><Connected>{}</Connected><ID>{}</ID></listener>",
            c.address,
            c.player,
            (server.now_ms - c.from_ms) / 1_000,
            c.id
        ));
    }
    xml.push_str("</source></icestats>");
    xml.into_response()
}

async fn fake_icecast(server: Shared) -> u16 {
    let router = axum::Router::new()
        .route("/status-json.xsl", get(status))
        .route("/admin/listclients", get(list_clients))
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    port
}

/// A stream server on the fake's port; a relay of `relay_of`.
async fn add_server(
    pool: &sqlx::MySqlPool,
    name: &str,
    port: u16,
    admin_password: Option<&str>,
    relay_of: Option<u64>,
) -> u64 {
    sqlx::query(
        "INSERT INTO stream_servers (station_id, name, host, port, source_password,
                                     admin_password, relay_of)
         VALUES (1, ?, '127.0.0.1', ?, 'hackme', ?, ?)",
    )
    .bind(name)
    .bind(port)
    .bind(admin_password)
    .bind(relay_of)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_id()
}

/// An output, on a server of its own.
async fn add_output(
    pool: &sqlx::MySqlPool,
    name: &str,
    port: u16,
    mount: &str,
    admin_password: Option<&str>,
) -> u64 {
    let server = add_server(pool, &format!("{name} server"), port, admin_password, None).await;
    add_output_on(pool, name, server, mount).await
}

async fn add_output_on(pool: &sqlx::MySqlPool, name: &str, server: u64, mount: &str) -> u64 {
    sqlx::query(
        "INSERT INTO outputs (station_id, name, kind, server_id, encoder_profile_id, config, enabled)
         SELECT 1, ?, 'icecast', ?, MIN(id), ?, TRUE FROM encoder_profiles",
    )
    .bind(name)
    .bind(server)
    .bind(serde_json::json!({"mount": mount}).to_string())
    .execute(pool)
    .await
    .unwrap()
    .last_insert_id()
}

async fn add_play(pool: &sqlx::MySqlPool, title: &str, from_ms: i64, to_ms: i64) -> u64 {
    let track = sqlx::query("INSERT INTO tracks (path, artist, title) VALUES (?, 'Artist', ?)")
        .bind(format!("{title}.mp3"))
        .bind(title)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
    sqlx::query(
        "INSERT INTO play_history (station_id, track_id, started_at, ended_at, deck, source)
         VALUES (1, ?, FROM_UNIXTIME(?), FROM_UNIXTIME(?), 'a', 'rotation')",
    )
    .bind(track)
    .bind(from_ms as f64 / 1_000.0)
    .bind(to_ms as f64 / 1_000.0)
    .execute(pool)
    .await
    .unwrap();
    track
}

#[derive(Debug, sqlx::FromRow)]
struct TotalRow {
    hour: Option<u8>,
    minutes: u32,
    peak: u32,
    listening_hours: f64,
    sessions: Option<u32>,
    distinct_listeners: Option<u32>,
}

async fn total(
    pool: &sqlx::MySqlPool,
    day: &str,
    output: Option<u64>,
    hour: Option<u8>,
) -> TotalRow {
    server_total(pool, day, output, None, hour).await
}

/// A row of one server's totals (`None`: every server's).
async fn server_total(
    pool: &sqlx::MySqlPool,
    day: &str,
    output: Option<u64>,
    server: Option<u64>,
    hour: Option<u8>,
) -> TotalRow {
    sqlx::query_as(
        "SELECT hour, minutes, peak, listening_hours, sessions, distinct_listeners
         FROM listener_totals WHERE day = ? AND output_key = COALESCE(?, 0)
           AND hour_key = COALESCE(?, 255) AND server_key = COALESCE(?, 0)",
    )
    .bind(day)
    .bind(output)
    .bind(hour)
    .bind(server)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn simulated_listeners_are_counted() {
    let Some(db) = ScratchDatabase::create().await else {
        return;
    };
    let pool = db.pool.clone();
    settings::save_for(
        &pool,
        1,
        StationSettings::SETTINGS_NAME,
        &StationSettings {
            timezone: "UTC".to_owned(),
            public_url: None,
        },
    )
    .await
    .unwrap();
    let server: Shared = Arc::default();
    let port = fake_icecast(Arc::clone(&server)).await;
    let live = add_output(&pool, "Live", port, "/live", Some("pw")).await;
    let other = add_output(&pool, "Other", port, "/other", None).await;
    // Wrong admin credentials: listener counts only, and the error said.
    let refused = add_output(&pool, "Refused", port, "/none", Some("wrong")).await;
    // Not read: HLS has no server to ask, a disabled output, settings the
    // engine would refuse.
    let off = add_output(&pool, "Off", port, "/live", Some("pw")).await;
    sqlx::query("UPDATE outputs SET enabled = FALSE WHERE id = ?")
        .bind(off)
        .execute(&pool)
        .await
        .unwrap();
    add_output(&pool, "Broken", port, "live", Some("pw")).await;
    sqlx::query(
        "INSERT INTO outputs (station_id, name, kind, encoder_profile_id, config, enabled)
         SELECT 1, 'HLS', 'hls', MIN(id), '{}', TRUE FROM encoder_profiles",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (day, _) = totals::day_bounds(date(2026, 10, 20), &TimeZone::UTC);
    let at = |hour: i64, minute: i64, second: i64| {
        day + hour * 3_600_000 + minute * MIN + second * 1_000
    };
    let connection = |mount, id, address, player, from_ms, to_ms| Connection {
        mount,
        id,
        address,
        player,
        from_ms,
        to_ms,
    };
    server.lock().unwrap().connections = vec![
        // A: 10:00 to 10:30, back 40 s later with a new connection until 10:45.
        connection(
            "/live",
            1,
            "192.0.2.1",
            "VLC/3.0",
            at(10, 0, 0),
            at(10, 30, 0),
        ),
        connection(
            "/live",
            5,
            "192.0.2.1",
            "VLC/3.0",
            at(10, 30, 40),
            at(10, 45, 0),
        ),
        // B: an hour.
        connection(
            "/live",
            2,
            "192.0.2.2",
            "mpv/0.38",
            at(10, 5, 0),
            at(11, 5, 0),
        ),
        // A probe of 30 s, seen once.
        connection(
            "/live",
            3,
            "198.51.100.7",
            "Probe/1.0",
            at(10, 10, 50),
            at(10, 11, 20),
        ),
        // A's address with another player: another listener.
        connection(
            "/live",
            4,
            "192.0.2.1",
            "Firefox/131",
            at(10, 20, 0),
            at(10, 25, 0),
        ),
        // Without admin credentials: two listeners for 10 minutes.
        connection(
            "/other",
            1,
            "203.0.113.1",
            "VLC/3.0",
            at(10, 0, 0),
            at(10, 10, 0),
        ),
        connection(
            "/other",
            2,
            "203.0.113.2",
            "VLC/3.0",
            at(10, 0, 0),
            at(10, 10, 0),
        ),
        // E: across midnight.
        connection(
            "/live",
            6,
            "192.0.2.9",
            "Radio/2",
            at(23, 58, 30),
            at(24, 2, 0),
        ),
    ];
    // Countries from a local database: 192.0.2.0/24 is in France.
    let geoip_dir = tempfile::tempdir().unwrap();
    let geoip_path = geoip_dir.path().join("country.mmdb");
    std::fs::write(
        &geoip_path,
        super::geoip::test_database(&[("192.0.2.0/24", "FR")]),
    )
    .unwrap();
    let collector =
        Collector::with_geoip(pool.clone(), super::geoip::GeoIp::new(Some(geoip_path))).unwrap();
    assert_eq!(collector.geoip_database().as_deref(), Some("Test-Country"));
    let mut polls: Vec<i64> = (0..=70).map(|m| at(10, m, 1)).collect();
    polls.extend([at(23, 59, 1), at(24, 0, 1), at(24, 1, 1)]);
    let mut next_purge = 0;
    for now in polls {
        {
            let mut fake = server.lock().unwrap();
            fake.now_ms = now;
            // Down from 10:50 to 10:52.
            fake.down = (at(10, 50, 0)..at(10, 53, 0)).contains(&now);
        }
        next_purge = collector.minute(now, next_purge).await;
        if now == at(10, 20, 1) {
            let last = collector.last();
            assert_eq!(last.len(), 3, "{last:?}");
            assert_eq!(
                (last[2].output_id, last[2].listeners, last[2].sessions),
                (refused, Some(0), false)
            );
            assert_eq!(
                last[2].error.as_deref(),
                Some("listener list: wrong admin credentials")
            );
            assert_eq!(last[0].listeners, Some(3));
            assert!(last[0].sessions);
            assert_eq!(last[1].listeners, Some(0));
            assert!(!last[1].sessions);
        }
        if now == at(10, 51, 1) {
            assert_eq!(collector.last()[0].listeners, None);
            assert!(collector.last()[0].error.is_some());
        }
    }

    // Sessions: A's reconnection continued it, the probe is kept but too short.
    let sessions: Vec<(String, i64, Option<i64>, u32, String)> = sqlx::query_as(
        "SELECT user_agent, CAST(UNIX_TIMESTAMP(started_at) * 1000 AS SIGNED),
                CAST(UNIX_TIMESTAMP(ended_at) * 1000 AS SIGNED), duration_s, CAST(day AS CHAR)
         FROM listener_sessions WHERE output_id = ? ORDER BY started_at",
    )
    .bind(live)
    .fetch_all(&pool)
    .await
    .unwrap();
    let find = |player: &str| -> Vec<&(String, i64, Option<i64>, u32, String)> {
        sessions.iter().filter(|s| s.0 == player).collect()
    };
    let a = find("VLC/3.0");
    assert_eq!(a.len(), 1, "{sessions:?}");
    assert_eq!((a[0].1, a[0].2), (at(10, 0, 0), Some(at(10, 44, 1))));
    let b = find("mpv/0.38");
    // Connected through the server's outage: one session.
    assert_eq!(b.len(), 1);
    assert_eq!(
        (b[0].1, b[0].2, b[0].3),
        (at(10, 5, 0), Some(at(11, 4, 1)), 3_541)
    );
    assert_eq!(find("Probe/1.0")[0].3, 11);
    let e = find("Radio/2");
    assert_eq!(e.len(), 2);
    assert_eq!(
        (e[0].2, e[0].3, e[0].4.as_str()),
        (Some(at(24, 0, 0)), 90, "2026-10-20")
    );
    assert_eq!(
        (e[1].1, e[1].2, e[1].4.as_str()),
        (at(24, 0, 0), None, "2026-10-21")
    );
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM listener_sessions WHERE output_id = ?")
            .bind(other)
            .fetch_one(&pool)
            .await
            .unwrap()
            == 0
    );

    // The country, looked up when each session started.
    let countries: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT user_agent, country FROM listener_sessions WHERE output_id = ? ORDER BY id",
    )
    .bind(live)
    .fetch_all(&pool)
    .await
    .unwrap();
    for (player, country) in &countries {
        let expected = (player != "Probe/1.0").then_some("FR");
        assert_eq!(country.as_deref(), expected, "{player}");
    }
    let by_country = super::breakdown::shares(
        &super::counted(
            &pool,
            &[1],
            (date(2026, 10, 20), date(2026, 10, 20)),
            super::Filter::default(),
            super::breakdown::By::Country,
        )
        .await
        .unwrap(),
        super::breakdown::By::Country.min_listeners(),
    );
    // 4 listeners in France: fewer than 5, Other; the probe not found.
    let kinds: Vec<(super::breakdown::ShareKind, u32)> =
        by_country.iter().map(|s| (s.kind, s.listeners)).collect();
    assert_eq!(
        kinds,
        [
            (super::breakdown::ShareKind::Unknown, 1),
            (super::breakdown::ShareKind::Other, 4)
        ]
    );

    // The gap: three minutes without samples.
    let gap: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listener_samples
         WHERE sampled_at >= FROM_UNIXTIME(?) AND sampled_at < FROM_UNIXTIME(?)",
    )
    .bind(at(10, 50, 0) / 1_000)
    .bind(at(10, 53, 0) / 1_000)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(gap, 0);

    // The day of the output with sessions. Listening, as simulated: A 44 min
    // 20 s, B 60 min (3 in the gap), D 5 min, the probe 30 s, E 1.5 min.
    let day_live = total(&pool, "2026-10-20", Some(live), None).await;
    assert_eq!((day_live.minutes, day_live.peak), (68 + 1, 3));
    let simulated_minutes = 44.33 + 60.0 - 3.0 + 5.0 + 0.5 + 1.5;
    assert!(
        (day_live.listening_hours * 60.0 - simulated_minutes).abs() <= 2.0,
        "{day_live:?}"
    );
    // A, B, D and E (90 s); not the probe.
    assert_eq!(day_live.sessions, Some(4));
    assert_eq!(day_live.distinct_listeners, Some(4));
    let ten = total(&pool, "2026-10-20", Some(live), Some(10)).await;
    assert_eq!(
        (ten.hour, ten.minutes, ten.sessions, ten.distinct_listeners),
        (Some(10), 57, Some(3), Some(3))
    );
    let eleven = total(&pool, "2026-10-20", Some(live), Some(11)).await;
    assert_eq!(
        (eleven.minutes, eleven.sessions, eleven.distinct_listeners),
        (11, Some(0), Some(1))
    );

    // Without admin credentials: concurrent listeners only.
    let day_other = total(&pool, "2026-10-20", Some(other), None).await;
    assert_eq!(
        (
            day_other.peak,
            day_other.sessions,
            day_other.distinct_listeners
        ),
        (2, None, None)
    );
    assert!((day_other.listening_hours - 20.0 / 60.0).abs() < 1e-9);

    // The station: both outputs, minute by minute.
    let station = total(&pool, "2026-10-20", None, None).await;
    assert_eq!(station.peak, 4);
    assert!(
        (station.listening_hours - day_live.listening_hours - day_other.listening_hours).abs()
            < 1e-9
    );
    assert_eq!(station.distinct_listeners, Some(4));

    // The next day: a new key, the old one is gone.
    let keys: Vec<String> = sqlx::query_scalar("SELECT CAST(day AS CHAR) FROM listener_day_keys")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(keys, ["2026-10-21"]);
    let e_keys: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT listener_key FROM listener_sessions WHERE user_agent = 'Radio/2' ORDER BY day",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_ne!(e_keys[0], e_keys[1]);

    // No address anywhere in the database.
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT CAST(table_name AS CHAR), CAST(column_name AS CHAR) FROM information_schema.columns
         WHERE table_schema = DATABASE()
           AND data_type IN ('char', 'varchar', 'text', 'mediumtext', 'longtext', 'json',
                             'binary', 'varbinary', 'blob', 'longblob')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        columns
            .iter()
            .any(|(table, _)| table == "listener_sessions")
    );
    for (table, column) in columns {
        let sql = format!(
            "SELECT COUNT(*) FROM `{table}` WHERE CAST(`{column}` AS CHAR) LIKE '%192.0.2.%'
                OR CAST(`{column}` AS CHAR) LIKE '%198.51.100.%' OR CAST(`{column}` AS CHAR) LIKE '%203.0.113.%'"
        );
        let found: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(found, 0, "an address in {table}.{column}");
    }

    // Departures go to the track on air: D left at 10:25, A at 10:45, B at 11:05.
    let first = add_play(&pool, "First", at(10, 20, 0), at(10, 30, 0)).await;
    let second = add_play(&pool, "Second", at(10, 40, 0), at(10, 50, 0)).await;
    let third = add_play(&pool, "Third", at(11, 0, 0), at(11, 10, 0)).await;
    let whole_day = (at(0, 0, 0), at(24, 0, 0));
    let tracks = super::track_listeners(&pool, 1, whole_day, super::Filter::default(), 60, 1)
        .await
        .unwrap();
    let ranking: Vec<(u64, f64, Option<u32>)> = tracks
        .iter()
        .map(|t| (t.track_id, t.listeners_start, t.departures))
        .collect();
    assert_eq!(
        ranking,
        [
            (third, 1.0, Some(1)),
            (second, 2.0, Some(1)),
            (first, 3.0, Some(1))
        ]
    );

    // The tracks of the other output: no listener lists, no departures.
    let other_only = super::Filter {
        output_id: Some(other),
        server_id: None,
    };
    let tracks = super::track_listeners(&pool, 1, whole_day, other_only, 60, 1)
        .await
        .unwrap();
    assert!(tracks.iter().all(|t| t.departures.is_none()), "{tracks:?}");

    // The audience figures of the output with sessions: every session of
    // 5 s or more, the probe's 11 s too.
    let live_only = super::Filter {
        output_id: Some(live),
        server_id: None,
    };
    let day20 = (date(2026, 10, 20), date(2026, 10, 20));
    let figures = super::audience_of(&pool, &[1], day20, &TimeZone::UTC, live_only)
        .await
        .unwrap();
    assert_eq!(
        (
            figures.sessions,
            figures.listener_days,
            figures.cume,
            figures.peak
        ),
        (Some(5), Some(5), Some(5.0), Some(3)),
        "{figures:?}"
    );
    let lengths = figures.session_lengths.unwrap();
    // The probe; D (4 min) and E (90 s); A (44 min) and B (59 min).
    assert_eq!(
        (
            lengths.under_1_min,
            lengths.from_1_to_5_min,
            lengths.from_5_to_30_min,
            lengths.from_30_min_to_2_h
        ),
        (1, 2, 0, 2)
    );
    assert!(figures.aqh.unwrap() > 0.0);
    // The day before: nothing measured.
    let before = super::audience_of(
        &pool,
        &[1],
        super::previous_period(day20),
        &TimeZone::UTC,
        live_only,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            before.from.as_str(),
            before.days,
            before.sessions,
            before.peak
        ),
        ("2026-10-19", 0, None, None)
    );

    // A shorter minimum session counts the probe once totals are recomputed.
    settings::save(
        &pool,
        StatisticsSettings::SETTINGS_NAME,
        &StatisticsSettings {
            min_session_s: 10,
            retention_days: 90,
        },
    )
    .await
    .unwrap();
    collector.recompute_now(at(24, 2, 0)).await;
    let recomputed = total(&pool, "2026-10-20", Some(live), None).await;
    assert_eq!(recomputed.sessions, Some(5));
    assert_eq!(recomputed.distinct_listeners, Some(5));

    // Past the retention, sessions and samples go and the totals stay, even
    // when recomputed.
    let later = at(24, 2, 0) + 91 * 86_400_000;
    assert_eq!(collector.minute(later, next_purge).await, later + 3_600_000);
    let (sessions, samples): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM listener_sessions), (SELECT COUNT(*) FROM listener_samples)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // E's session of the 21st is still open; the minute just read stays.
    assert_eq!((sessions, samples), (1, 3));
    collector.recompute_now(later).await;
    assert_eq!(
        total(&pool, "2026-10-20", Some(live), None).await.sessions,
        Some(5)
    );
    // Not due yet: no purge.
    assert_eq!(
        collector.minute(later + 60_000, later + 3_600_000).await,
        later + 3_600_000
    );
    db.drop().await;
}

/// A master and two relays: one known by its address, one behind NAT
/// whose connection counts as one listener. The output's listeners add up
/// the three, without the relays' own connections; a relay that cannot be
/// read is a gap for its part only.
#[tokio::test]
async fn relays_add_their_listeners_without_their_connections() {
    let Some(db) = ScratchDatabase::create().await else {
        return;
    };
    let pool = db.pool.clone();
    settings::save_for(
        &pool,
        1,
        StationSettings::SETTINGS_NAME,
        &StationSettings {
            timezone: "UTC".to_owned(),
            public_url: None,
        },
    )
    .await
    .unwrap();
    let (master, known, nat): (Shared, Shared, Shared) =
        (Arc::default(), Arc::default(), Arc::default());
    let master_port = fake_icecast(Arc::clone(&master)).await;
    let known_port = fake_icecast(Arc::clone(&known)).await;
    let nat_port = fake_icecast(Arc::clone(&nat)).await;
    let master_id = add_server(&pool, "Master", master_port, Some("pw"), None).await;
    // Relays /live as /r-live; the master sees it at 127.0.0.1.
    let known_id = add_server(&pool, "Relay", known_port, Some("pw"), Some(master_id)).await;
    sqlx::query(
        "UPDATE stream_servers SET relay_mounts = '[{\"mount\": \"/live\", \"relay_mount\": \"/r-live\"}]'
         WHERE id = ?",
    )
    .bind(known_id)
    .execute(&pool)
    .await
    .unwrap();
    // Behind NAT, without admin credentials: every mount.
    let nat_id = add_server(&pool, "NAT relay", nat_port, None, Some(master_id)).await;
    sqlx::query("UPDATE stream_servers SET relay_subtract_one = TRUE WHERE id = ?")
        .bind(nat_id)
        .execute(&pool)
        .await
        .unwrap();
    let live = add_output_on(&pool, "Live", master_id, "/live").await;

    let (day, _) = totals::day_bounds(date(2026, 10, 20), &TimeZone::UTC);
    let at = |minute: i64, second: i64| day + 10 * 3_600_000 + minute * MIN + second * 1_000;
    let connection = |mount, id, address, player, from_ms, to_ms| Connection {
        mount,
        id,
        address,
        player,
        from_ms,
        to_ms,
    };
    master.lock().unwrap().connections = vec![
        connection("/live", 1, "192.0.2.1", "VLC/3.0", at(0, 0), at(10, 0)),
        // The relays' own connections.
        connection(
            "/live",
            2,
            "127.0.0.1",
            "Icecast 2.4.4",
            at(0, 0),
            at(10, 0),
        ),
        connection(
            "/live",
            3,
            "198.51.100.9",
            "Icecast 2.5.0",
            at(0, 0),
            at(5, 0),
        ),
        connection(
            "/live",
            4,
            "198.51.100.9",
            "Icecast 2.5.0",
            at(7, 0),
            at(10, 0),
        ),
    ];
    {
        let mut relay = known.lock().unwrap();
        relay.mounts = vec!["/r-live"];
        relay.connections = vec![
            connection("/r-live", 1, "192.0.2.2", "mpv/0.38", at(0, 0), at(10, 0)),
            // The master's listener, here too for 5 minutes: one listener.
            connection("/r-live", 2, "192.0.2.1", "VLC/3.0", at(0, 0), at(5, 0)),
        ];
    }
    nat.lock().unwrap().connections = vec![
        connection("/live", 1, "203.0.113.1", "VLC/3.0", at(0, 0), at(10, 0)),
        connection("/live", 2, "203.0.113.2", "VLC/3.0", at(0, 0), at(10, 0)),
    ];

    let collector = Collector::new(pool.clone()).unwrap();
    let mut next_purge = 0;
    for minute in 0..10 {
        let now = at(minute, 1);
        for server in [&master, &known, &nat] {
            server.lock().unwrap().now_ms = now;
        }
        // The NAT relay is down from 10:05 to 10:07, and so is its
        // connection to the master.
        nat.lock().unwrap().down = (at(5, 0)..at(7, 0)).contains(&now);
        next_purge = collector.minute(now, next_purge).await;
        let last = collector.last();
        assert_eq!(last.len(), 1, "{last:?}");
        let parts: Vec<(u64, Option<u32>)> = last[0]
            .servers
            .iter()
            .map(|part| (part.server_id, part.listeners))
            .collect();
        match minute {
            0..=4 => {
                assert_eq!(
                    parts,
                    [(master_id, Some(1)), (known_id, Some(2)), (nat_id, Some(2))]
                );
                assert_eq!(last[0].listeners, Some(5));
                assert!(!last[0].sessions, "the NAT relay has no listener list");
            }
            5 | 6 => {
                assert_eq!(
                    parts,
                    [(master_id, Some(1)), (known_id, Some(1)), (nat_id, None)]
                );
                assert_eq!(last[0].listeners, Some(2));
                assert!(last[0].error.as_deref().unwrap().starts_with("NAT relay: "));
            }
            _ => assert_eq!(last[0].listeners, Some(4)),
        }
    }

    // Samples per server: the NAT relay's gap is its own.
    let samples: Vec<(u64, i64)> = sqlx::query_as(
        "SELECT server_id, COUNT(*) FROM listener_samples GROUP BY server_id ORDER BY server_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(samples, [(master_id, 10), (known_id, 10), (nat_id, 8)]);
    // No session for the relays' connections.
    let sessions: Vec<(u64, String)> = sqlx::query_as(
        "SELECT server_id, user_agent FROM listener_sessions ORDER BY server_id, started_at, id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        sessions,
        [
            (master_id, "VLC/3.0".to_owned()),
            (known_id, "mpv/0.38".to_owned()),
            (known_id, "VLC/3.0".to_owned()),
        ]
    );

    // The output's day adds up its servers, minute by minute; a listener
    // on the master and a relay is one listener.
    let day_live = total(&pool, "2026-10-20", Some(live), None).await;
    assert_eq!((day_live.minutes, day_live.peak), (10, 5));
    assert!((day_live.listening_hours - 41.0 / 60.0).abs() < 1e-9);
    assert_eq!(day_live.sessions, Some(3));
    assert_eq!(day_live.distinct_listeners, Some(2));
    let station = total(&pool, "2026-10-20", None, None).await;
    assert_eq!(station.peak, 5);

    // Each server's own figures: the relay's listeners, without its
    // connection to the master.
    let relay = server_total(&pool, "2026-10-20", Some(live), Some(known_id), None).await;
    assert_eq!(
        (relay.peak, relay.sessions, relay.distinct_listeners),
        (2, Some(2), Some(2))
    );
    let on_master = server_total(&pool, "2026-10-20", None, Some(master_id), None).await;
    assert_eq!((on_master.peak, on_master.distinct_listeners), (1, Some(1)));
    let nat_part = server_total(&pool, "2026-10-20", Some(live), Some(nat_id), None).await;
    assert_eq!((nat_part.minutes, nat_part.sessions), (8, None));
    let relay_only = super::Filter {
        output_id: None,
        server_id: Some(known_id),
    };
    let day20 = (date(2026, 10, 20), date(2026, 10, 20));
    let figures = super::audience_of(&pool, &[1], day20, &TimeZone::UTC, relay_only)
        .await
        .unwrap();
    assert_eq!((figures.peak, figures.sessions), (Some(2), Some(2)));
    db.drop().await;
}
