// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-api`: HTTP and WebSocket API, library scanner and queue manager.
//!
//! It applies the database migrations on startup, scans the music library,
//! feeds the engine and serves the API and the ops endpoints. See
//! `docs/api.md`.

mod app;
mod artists;
mod auth;
mod backups;
mod categories;
mod category_types;
mod clocks;
#[cfg(test)]
mod e2e;
mod engine;
mod eq;
mod error;
mod history;
mod import;
mod labels;
mod librarian;
mod library;
mod listeners;
mod live;
mod medleys;
mod outputs;
mod player;
mod public;
mod queue;
mod restrictions;
mod rotation;
mod schedules;
mod selection;
mod settings;
mod staging;
mod stations;
mod sync;
mod tracks;
mod users;
mod webhooks;

use std::io::{BufRead, IsTerminal, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use app::{AppState, SERVICE};
use auth::{Passwords, Role};
use clap::{Parser, Subcommand};
use heron_service::db::{self, DatabaseHealth, MIGRATOR};
use heron_service::{LogArgs, MetricsArgs};
use library::{Library, ScanOptions};

#[derive(Debug, Parser)]
#[command(name = SERVICE, version, about = "Heron Broadcaster API")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Address of the HTTP server (API, `/healthz`, `/metrics`).
    #[arg(
        long,
        env = "HERON_HTTP_ADDR",
        default_value = "0.0.0.0:8080",
        global = true
    )]
    http_addr: SocketAddr,

    /// MariaDB URL, e.g. `mysql://heron:secret@mariadb:3306/heron`.
    #[arg(
        long,
        env = "HERON_DATABASE_URL",
        hide_env_values = true,
        global = true
    )]
    database_url: Option<String>,

    /// gRPC address of the engine.
    #[arg(
        long,
        env = "HERON_ENGINE_URL",
        default_value = "http://heron-engine:50051"
    )]
    engine_url: String,

    /// Root of the music library. Track paths are stored relative to it.
    #[arg(long, env = "HERON_LIBRARY_DIR", default_value = "/music")]
    library_dir: PathBuf,

    /// The buffer zone, where uploads are written (read-write, shared with
    /// heron-librarian, which imports them).
    #[arg(long, env = "HERON_STAGING_DIR", default_value = "/staging")]
    staging_dir: PathBuf,

    /// Where backups are written (docs/backups.md).
    #[arg(
        long,
        env = "HERON_BACKUP_DIR",
        default_value = "/var/lib/heron/backups"
    )]
    backup_dir: PathBuf,

    /// Seconds between full library scans (file system events also trigger scans).
    #[arg(long, env = "HERON_SCAN_INTERVAL", default_value_t = 3600)]
    scan_interval: u64,

    /// Watch the library for changes. Turn off on file systems without
    /// change notifications (some network shares); periodic scans still run.
    #[arg(long, env = "HERON_LIBRARY_WATCH", default_value_t = true, action = clap::ArgAction::Set)]
    library_watch: bool,

    /// Seconds a file must stay unmodified before it is imported, so files
    /// still being copied are not read half-written.
    #[arg(long, env = "HERON_IMPORT_SETTLE", default_value_t = 5)]
    import_settle: u64,

    #[command(flatten)]
    log: LogArgs,

    #[command(flatten)]
    metrics: MetricsArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Exit 0 if the running server answers `/healthz` (used by Docker).
    Healthcheck,
    /// Print the OpenAPI document (JSON).
    Openapi,
    /// Manage users. Passwords are read from the terminal, or from the
    /// first line of stdin when it is not a terminal.
    #[command(subcommand)]
    User(UserCommand),
    /// Back up, list and restore backups (docs/backups.md).
    #[command(subcommand)]
    Backup(BackupCommand),
}

#[derive(Debug, Subcommand)]
enum BackupCommand {
    /// Back up now: the configuration file and the database dump.
    Create,
    /// List the backups, newest first.
    List,
    /// Restore a backup's configuration file, by its name
    /// (`2026-11-03T03-30-00Z-daily`). Restart heron-api afterwards, so the
    /// engines get the restored configuration; everyone signs in again.
    Restore { name: String },
}

#[derive(Debug, Subcommand)]
enum UserCommand {
    /// Create a user, e.g. the first admin: `user add admin --role admin`.
    Add {
        username: String,
        /// viewer, operator or admin.
        #[arg(long, default_value = "viewer")]
        role: Role,
    },
    /// Set a user's password (and sign them out everywhere).
    Passwd { username: String },
    /// List users.
    List,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Healthcheck) => {
            heron_service::probe::probe(cli.http_addr, "/healthz", Duration::from_secs(3))?;
            Ok(())
        }
        Some(Command::Openapi) => {
            let document = serde_json::to_string_pretty(&app::openapi())?;
            println!("{document}");
            Ok(())
        }
        Some(Command::User(command)) => manage_users(cli.database_url, command),
        Some(Command::Backup(command)) => manage_backups(cli.database_url, cli.backup_dir, command),
        None => run(cli),
    }
}

#[tokio::main]
async fn run(cli: Cli) -> anyhow::Result<()> {
    let _telemetry = heron_service::telemetry::init_tracing(&cli.log, SERVICE)?;
    let metrics = heron_service::ops::install_metrics(SERVICE, &cli.metrics)?;
    let database_url = cli
        .database_url
        .context("HERON_DATABASE_URL (or --database-url) is required")?;

    let pool = db::connect(&database_url, db::ConnectOptions::default()).await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;
    tracing::info!(
        migrations = MIGRATOR.iter().count(),
        "database schema is up to date"
    );
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE enabled")
        .fetch_one(&pool)
        .await?;
    if users == 0 {
        tracing::warn!(
            "no user yet: create the first admin with \
             `docker compose exec heron-api heron-api user add admin --role admin`"
        );
    }

    let library = Arc::new(Library::new(pool.clone(), cli.library_dir));
    let webhooks = webhooks::Webhooks::new(pool.clone()).context("cannot set up webhooks")?;
    let stations = stations::Stations::start(
        pool.clone(),
        cli.engine_url,
        stations::engines_from_env(),
        std::env::var("HERON_STATION_NAME").ok(),
        Arc::clone(&webhooks),
    )
    .await
    .context("cannot start the stations")?;
    let listeners =
        listeners::Collector::new(pool.clone()).context("cannot set up listener statistics")?;
    let backups = backups::Backups::new(cli.backup_dir, database_url.clone());

    let mut tasks = vec![
        tokio::spawn(library.clone().run(ScanOptions {
            interval: Duration::from_secs(cli.scan_interval.max(60)),
            watch: cli.library_watch,
            settle: Duration::from_secs(cli.import_settle),
        })),
        tokio::spawn(Arc::clone(&webhooks).send_loop()),
    ];
    tasks.push(tokio::spawn(Arc::clone(&listeners).run()));
    tasks.push(tokio::spawn(artists::run_linker(
        pool.clone(),
        Arc::clone(&stations),
    )));
    tasks.push(tokio::spawn(medleys::run_linker(pool.clone())));
    tasks.push(tokio::spawn(users::purge_sessions(pool.clone())));
    tasks.push(tokio::spawn(
        Arc::clone(&backups).run_daily(pool.clone(), Arc::clone(&stations)),
    ));

    let health = DatabaseHealth(pool.clone());
    let state = AppState {
        pool: pool.clone(),
        health: health.clone(),
        library,
        stations,
        passwords: Arc::new(Passwords::default()),
        staging: Arc::new(cli.staging_dir),
        webhooks,
        listeners,
        backups,
    };
    let router = app::router(state).merge(heron_service::ops_router(SERVICE, metrics, health));

    let served = heron_service::serve(cli.http_addr, router).await;
    for task in tasks {
        task.abort();
    }
    pool.close().await;
    Ok(served?)
}

/// Reads a password without echoing it on a terminal.
fn read_password(prompt: &str) -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    let terminal = stdin.is_terminal();
    let mut saved = None;
    if terminal {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        if let Ok(mut termios) = rustix::termios::tcgetattr(&stdin) {
            saved = Some(termios.clone());
            termios
                .local_modes
                .remove(rustix::termios::LocalModes::ECHO);
            let _ =
                rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &termios);
        }
    }
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if let Some(saved) = saved {
        let _ = rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &saved);
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

#[tokio::main(flavor = "current_thread")]
async fn manage_users(database_url: Option<String>, command: UserCommand) -> anyhow::Result<()> {
    let database_url =
        database_url.context("HERON_DATABASE_URL (or --database-url) is required")?;
    let pool = db::connect(
        &database_url,
        db::ConnectOptions {
            max_connections: 1,
            max_attempts: 3,
        },
    )
    .await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;
    let passwords = Passwords::default();
    match command {
        UserCommand::Add { username, role } => {
            let password = read_password("Password: ")?;
            let user = users::add_user(&pool, &passwords, &username, &password, role)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            println!("user {} created ({})", user.username, user.role.as_str());
        }
        UserCommand::Passwd { username } => {
            let id = users::user_id(&pool, &username)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let password = read_password("New password: ")?;
            users::set_password(&pool, &passwords, id, &password, None)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            println!("password of {username} changed");
        }
        UserCommand::List => {
            let users = users::all_users(&pool)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            for user in users {
                println!(
                    "{:<24} {:<9} {}",
                    user.username,
                    user.role.as_str(),
                    if user.enabled { "" } else { "disabled" }
                );
            }
        }
    }
    pool.close().await;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn manage_backups(
    database_url: Option<String>,
    backup_dir: PathBuf,
    command: BackupCommand,
) -> anyhow::Result<()> {
    let database_url =
        database_url.context("HERON_DATABASE_URL (or --database-url) is required")?;
    let pool = db::connect(
        &database_url,
        db::ConnectOptions {
            max_connections: 2,
            max_attempts: 3,
        },
    )
    .await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("failed to apply database migrations")?;
    let backups = backups::Backups::new(backup_dir, database_url);
    let fail = |error: error::ApiError| anyhow::anyhow!("{error}");
    match command {
        BackupCommand::Create => {
            let backup = backups
                .create(&pool, backups::Kind::Manual)
                .await
                .map_err(fail)?;
            println!("{}", backup.name);
            if let Some(error) = backup.database_error {
                eprintln!("the database dump failed: {error}");
            }
        }
        BackupCommand::List => {
            for backup in backups.list().map_err(fail)? {
                println!(
                    "{:<40} config {:>10} B  database {}",
                    backup.name,
                    backup.config_size,
                    match (backup.database_size, &backup.database_error) {
                        (Some(size), _) => format!("{size} B"),
                        (None, Some(error)) => format!("failed: {error}"),
                        (None, None) => "none".to_owned(),
                    }
                );
            }
        }
        BackupCommand::Restore { name } => {
            let bytes = std::fs::read(backups.dir().join(format!("heron-config-{name}.json")))
                .with_context(|| format!("no backup {name}: see `heron-api backup list`"))?;
            let report = backups
                .restore(&pool, None, &bytes, false)
                .await
                .map_err(fail)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    pool.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use heron_service::db::ScratchDatabase;

    /// Applies the migrations to a scratch MariaDB database (see
    /// `HERON_TEST_DATABASE_URL`). Skipped when no test database is configured.
    #[tokio::test]
    async fn migrations_apply_and_are_idempotent() {
        let Some(db) = ScratchDatabase::create().await else {
            return;
        };
        let pool = db.pool.clone();
        MIGRATOR.run(&pool).await.unwrap();

        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT CAST(table_name AS CHAR) FROM information_schema.tables \
             WHERE table_schema = DATABASE() ORDER BY table_name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        for expected in [
            "analysis_jobs",
            "api_tokens",
            "artist_names",
            "artist_similar",
            "artist_suggestions",
            "artists",
            "categories",
            "category_stations",
            "medley_stations",
            "restriction_stations",
            "clock_schedule",
            "schedules",
            "schedule_periods",
            "clock_slots",
            "clocks",
            "encoder_profiles",
            "listener_day_keys",
            "listener_samples",
            "listener_sessions",
            "listener_totals",
            "outputs",
            "play_history",
            "restrictions",
            "rotation_picks",
            "sessions",
            "settings",
            "station_settings",
            "stations",
            "track_analysis",
            "track_artists",
            "track_categories",
            "track_overrides",
            "tracks",
            "user_station_roles",
            "users",
            "webhook_deliveries",
            "webhooks",
        ] {
            assert!(tables.iter().any(|t| t == expected), "missing {expected}");
        }

        // The default encoder profile (docs/outputs.md#default-profiles).
        let (name, encoder, bitrate, rate, channels, mode): (String, String, u16, u32, u8, String) =
            sqlx::query_as(
                "SELECT name, CAST(encoder AS CHAR), bitrate_kbps, sample_rate, channels, \
                 JSON_UNQUOTE(JSON_EXTRACT(options, '$.mode')) FROM encoder_profiles",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                encoder.as_str(),
                bitrate,
                rate,
                channels,
                mode.as_str()
            ),
            ("MP3 128k", "mp3", 128, 44_100, 2, "cbr")
        );

        // Long paths stay unique through the generated hash column.
        let mut tx = pool.begin().await.unwrap();
        let long_path = format!("/music/{}.flac", "a".repeat(3000));
        sqlx::query("INSERT INTO tracks (path) VALUES (?)")
            .bind(&long_path)
            .execute(&mut *tx)
            .await
            .unwrap();
        let duplicate = sqlx::query("INSERT INTO tracks (path) VALUES (?)")
            .bind(&long_path)
            .execute(&mut *tx)
            .await;
        assert!(duplicate.is_err());
        tx.rollback().await.unwrap();
        db.drop().await;
    }

    /// An existing Heron upgrades to one station holding all it had: its
    /// clocks, schedule, history, selection log, outputs, webhooks,
    /// listener totals and per-station settings.
    #[tokio::test]
    async fn an_existing_heron_becomes_its_first_station() {
        const STATIONS: i64 = 20261117000000;
        let Ok(admin_url) = std::env::var("HERON_TEST_DATABASE_URL") else {
            return;
        };
        let admin_url = admin_url.trim_end_matches('/').to_owned();
        let name = format!("heron_upgrade_{}", std::process::id());
        let admin = sqlx::MySqlPool::connect(&admin_url).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE `{name}`")))
            .execute(&admin)
            .await
            .unwrap();
        let pool = sqlx::MySqlPool::connect(&format!("{admin_url}/{name}"))
            .await
            .unwrap();

        // A Heron of one station, before M13.
        MIGRATOR.run_to(STATIONS - 1, &pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO settings (name, value) VALUES
                 ('station', '{\"timezone\": \"Europe/Paris\"}'),
                 ('playout', '{}'), ('queue', '{\"autofill\": false, \"min_queued\": 4}'),
                 ('library', '{}'),
                 ('medleys', '{\"category_id\": 9, \"name_from\": \"titles\"}');
             INSERT INTO categories (id, name) VALUES (9, 'Medleys');
             INSERT INTO medleys (id, recipe) VALUES (5, '{}');
             INSERT INTO users (id, username, password_hash, role) VALUES (3, 'op', 'x', 'operator');
             INSERT INTO clocks (id, name) VALUES (7, 'Morning');
             INSERT INTO clock_schedule (day_of_week, hour, clock_id) VALUES (1, 6, 7);
             INSERT INTO play_history (title, started_at, deck, source)
                 VALUES ('Song', NOW(6), 'a', 'rotation');
             INSERT INTO rotation_picks (planned_at, step, explanation)
                 VALUES (NOW(6), 0, '{}');
             INSERT INTO outputs (name, kind, encoder_profile_id, config)
                 SELECT 'Main', 'icecast', MIN(id), '{}' FROM encoder_profiles;
             INSERT INTO webhooks (name, url, events, secret)
                 VALUES ('Site', 'https://example.org/hook', '[]', 'whsec');
             INSERT INTO listener_totals (output_id, day, minutes, peak, average, listening_hours)
                 VALUES (NULL, '2026-10-01', 60, 3, 1.5, 1.5);",
        )
        .execute(&pool)
        .await
        .unwrap();

        MIGRATOR.run(&pool).await.unwrap();

        let stations: Vec<(u64, String, String, bool)> =
            sqlx::query_as("SELECT id, name, code, enabled FROM stations")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(stations, [(1, String::new(), "main".to_owned(), true)]);
        // The schedule became the first station's default schedule.
        let hours: Vec<(u64, bool, u8, u8, u64)> = sqlx::query_as(
            "SELECT s.station_id, s.is_default, cs.day_of_week, cs.hour, cs.clock_id
             FROM clock_schedule cs JOIN schedules s ON s.id = cs.schedule_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(hours, [(1, true, 1, 6, 7)]);
        for table in [
            "clocks",
            "play_history",
            "rotation_picks",
            "outputs",
            "webhooks",
            "listener_totals",
        ] {
            let elsewhere: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table} WHERE station_id <> 1"
            )))
            .fetch_one(&pool)
            .await
            .unwrap();
            let first: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM {table} WHERE station_id = 1"
            )))
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!((first, elsewhere), (1, 0), "{table}");
        }
        let moved: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM station_settings WHERE station_id = 1 ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(moved, ["medley_categories", "playout", "queue", "station"]);
        let shared: Vec<(String, String)> =
            sqlx::query_as("SELECT name, CAST(value AS CHAR) FROM settings ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            shared,
            [
                ("library".to_owned(), "{}".to_owned()),
                (
                    "medleys".to_owned(),
                    r#"{"name_from": "titles"}"#.to_owned()
                )
            ]
        );
        let medleys = medleys::categories_of(&pool, 1).await.unwrap();
        assert_eq!(medleys.category_id, Some(9));
        // Categories and medleys are the first station's.
        let tags: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM category_stations WHERE category_id = 9 AND station_id = 1),
                    (SELECT COUNT(*) FROM medley_stations WHERE medley_id = 5 AND station_id = 1),
                    (SELECT COUNT(*) FROM restriction_stations)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(tags, (1, 1, 0));
        let role: String = sqlx::query_scalar(
            "SELECT CAST(role AS CHAR) FROM user_station_roles WHERE user_id = 3 AND station_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role, "operator");
        let queue: settings::QueueSettings =
            settings::load_for(&pool, 1, settings::QueueSettings::SETTINGS_NAME)
                .await
                .unwrap();
        assert_eq!((queue.autofill, queue.min_queued), (false, 4));

        // The API names the station from HERON_STATION_NAME.
        let webhooks = webhooks::Webhooks::new(pool.clone()).unwrap();
        let stations = stations::Stations::start(
            pool.clone(),
            "http://127.0.0.1:9".to_owned(),
            std::collections::BTreeMap::new(),
            Some("Hits".to_owned()),
            webhooks,
        )
        .await
        .unwrap();
        assert_eq!(stations.first().info().name, "Hits");
        drop(stations);

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE `{name}`")))
            .execute(&admin)
            .await
            .unwrap();
    }
}
