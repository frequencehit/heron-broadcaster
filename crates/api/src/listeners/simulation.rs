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
    let sources: Vec<_> = ["/live", "/other"]
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

async fn add_output(
    pool: &sqlx::MySqlPool,
    name: &str,
    port: u16,
    mount: &str,
    admin_password: Option<&str>,
) -> u64 {
    let config = serde_json::json!({"host": "127.0.0.1", "port": port, "mount": mount, "password": "hackme"});
    let statistics =
        admin_password.map(|password| format!(r#"{{"admin_password": "{password}"}}"#));
    sqlx::query(
        "INSERT INTO outputs (station_id, name, kind, encoder_profile_id, config, statistics, enabled)
         SELECT 1, ?, 'icecast', MIN(id), ?, ?, TRUE FROM encoder_profiles",
    )
    .bind(name)
    .bind(config.to_string())
    .bind(statistics)
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
    sqlx::query_as(
        "SELECT hour, minutes, peak, listening_hours, sessions, distinct_listeners
         FROM listener_totals WHERE day = ? AND output_key = COALESCE(?, 0)
           AND hour_key = COALESCE(?, 255)",
    )
    .bind(day)
    .bind(output)
    .bind(hour)
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
            gateway_url: None,
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
    sqlx::query(
        "INSERT INTO outputs (station_id, name, kind, encoder_profile_id, config, enabled)
         SELECT 1, 'HLS', 'hls', MIN(id), '{}', TRUE FROM encoder_profiles
         UNION ALL SELECT 1, 'Off', 'icecast', MIN(id), '{\"host\": \"127.0.0.1\", \"mount\": \"/live\", \"password\": \"x\"}', FALSE FROM encoder_profiles
         UNION ALL SELECT 1, 'Broken', 'icecast', MIN(id), '{\"host\": \"127.0.0.1\", \"mount\": \"live\", \"password\": \"x\"}', TRUE FROM encoder_profiles",
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
    let collector = Collector::new(pool.clone()).unwrap();
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
    let tracks = super::track_listeners(&pool, 1, (at(0, 0, 0), at(24, 0, 0)), 60, 1)
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
