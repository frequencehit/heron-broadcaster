// SPDX-License-Identifier: AGPL-3.0-or-later

//! `heron-librarian`: library management service and its command line.
//!
//! Without a subcommand, it finishes operations interrupted by a crash,
//! then moves files dropped into the incoming folder into the buffer zone,
//! identifies the files waiting there and runs the library tasks asked
//! through the API (imports, deletions, …) until stopped. The
//! subcommands reorganize the library (after a dry run), undo a batch and
//! show what needs review; the same work asked through the API (the web
//! UI) runs as library tasks. It never applies migrations: `heron-api` owns
//! the schema.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use heron_core::LibrarySettings;
use heron_librarian::identify::Endpoints;
use heron_librarian::import::{self, Importer};
use heron_librarian::journal::{self, Roots};
use heron_librarian::layout::Layout;
use heron_librarian::organize;
use heron_librarian::staging::{self, Stager};
use heron_librarian::tasks::TaskRunner;
use heron_library::LibraryLock;
use heron_service::about::ConfigItem;
use heron_service::db::{self, DatabaseHealth};
use heron_service::{LogArgs, MetricsArgs};
use sqlx::{MySqlPool, Row};

const SERVICE: &str = "heron-librarian";

#[derive(Debug, Parser)]
#[command(name = SERVICE, version, about = "Heron Broadcaster library management")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Address of the ops HTTP server (`/healthz`, `/metrics`).
    #[arg(
        long,
        env = "HERON_HTTP_ADDR",
        default_value = "0.0.0.0:8083",
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

    /// Root of the music library, as mounted in this container (read-write).
    #[arg(
        long,
        env = "HERON_LIBRARY_DIR",
        default_value = "/music",
        global = true
    )]
    library_dir: PathBuf,

    /// The incoming folder, where files to import are dropped (read-write).
    #[arg(
        long,
        env = "HERON_INCOMING_DIR",
        default_value = "/incoming",
        global = true
    )]
    incoming_dir: PathBuf,

    /// The buffer zone, where new files wait until they are imported
    /// (read-write, shared with heron-api).
    #[arg(
        long,
        env = "HERON_STAGING_DIR",
        default_value = "/staging",
        global = true
    )]
    staging_dir: PathBuf,

    /// Seconds a file must stay unmodified before it moves into the buffer zone.
    #[arg(long, env = "HERON_IMPORT_SETTLE", default_value_t = 5)]
    import_settle: u64,

    /// Seconds between looks at the incoming folder when no event arrives.
    #[arg(long, env = "HERON_INCOMING_INTERVAL", default_value_t = 60)]
    incoming_interval: u64,

    /// Watch the incoming folder for changes. Turn off on file systems
    /// without change notifications; periodic looks still happen.
    #[arg(long, env = "HERON_INCOMING_WATCH", default_value_t = true, action = clap::ArgAction::Set)]
    incoming_watch: bool,

    /// AcoustID lookup URL (for mirrors and tests).
    #[arg(long, env = "HERON_ACOUSTID_URL", hide = true)]
    acoustid_url: Option<String>,

    /// MusicBrainz web service root, ending with `/ws/2`.
    #[arg(long, env = "HERON_MUSICBRAINZ_URL", hide = true)]
    musicbrainz_url: Option<String>,

    /// Cover Art Archive root.
    #[arg(long, env = "HERON_COVER_ART_URL", hide = true)]
    cover_art_url: Option<String>,

    /// Niceness, 0 to 19 (0 leaves it as is). The librarian yield the CPU and
    /// the disk to the engine; they also get the lowest best-effort I/O priority.
    #[arg(long, env = "HERON_NICE", default_value = "10", global = true)]
    nice: String,

    #[command(flatten)]
    log: LogArgs,

    #[command(flatten)]
    metrics: MetricsArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Exit 0 if the running service answers `/healthz` (used by Docker).
    Healthcheck,
    /// Move every track where the layout says. A dry run unless `--apply`.
    Organize {
        /// Carry out the moves (as one batch, which `undo` can revert).
        #[arg(long)]
        apply: bool,
        /// Print the plan or the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Revert a batch: files go back to their previous paths and tags.
    Undo { batch: u64 },
    /// List the most recent batches.
    Batches {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// List the tracks whose identification needs review.
    Review,
    /// Show the library settings, or merge JSON into them, e.g.
    /// `settings --set '{"acoustid_key": "..."}'`.
    Settings {
        #[arg(long)]
        set: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Healthcheck) => {
            heron_service::probe::probe(cli.http_addr, "/healthz", Duration::from_secs(3))?;
            Ok(())
        }
        _ => {
            // Before the runtime starts: its threads inherit the priority.
            let lowered = heron_service::priority::lower_current_thread_to(&cli.nice);
            run(cli, lowered)
        }
    }
}

#[tokio::main]
async fn run(cli: Cli, lowered: Result<(), String>) -> anyhow::Result<()> {
    let _telemetry = heron_service::telemetry::init_tracing(&cli.log, SERVICE)?;
    match lowered {
        Ok(()) => {
            tracing::info!(nice = ?heron_service::priority::current_nice(), "priority lowered")
        }
        Err(problem) => {
            tracing::warn!(nice = ?heron_service::priority::current_nice(), "priority: {problem}")
        }
    }
    let database_url = cli
        .database_url
        .clone()
        .context("HERON_DATABASE_URL (or --database-url) is required")?;
    let pool = db::connect(&database_url, db::ConnectOptions::default()).await?;
    let roots = Roots {
        library: cli.library_dir.clone(),
        incoming: cli.incoming_dir.clone(),
        staging: cli.staging_dir.clone(),
    };

    // Finish what a crash interrupted before doing anything else.
    let lock = LibraryLock::acquire(&pool, SERVICE).await?;
    let recovered = match journal::recover(&pool, &roots).await {
        Ok(_) => staging::recover(&pool, &roots)
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from),
        Err(error) => Err(error.into()),
    };
    lock.release().await?;
    recovered?;

    let result = match cli.command {
        None => serve(&cli, pool.clone(), roots).await,
        Some(Command::Organize { apply, json }) => {
            organize_library(&pool, &roots, apply, json).await
        }
        Some(Command::Undo { batch }) => {
            let report = organize::undo(&pool, &roots, batch).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        Some(Command::Batches { limit }) => list_batches(&pool, limit).await,
        Some(Command::Review) => list_review(&pool).await,
        Some(Command::Settings { set }) => settings(&pool, set).await,
        Some(Command::Healthcheck) => Ok(()),
    };
    pool.close().await;
    result
}

async fn serve(cli: &Cli, pool: MySqlPool, roots: Roots) -> anyhow::Result<()> {
    let metrics = heron_service::ops::install_metrics(SERVICE, &cli.metrics)?;
    let defaults = Endpoints::default();
    let endpoints = Endpoints {
        acoustid: cli.acoustid_url.clone().unwrap_or(defaults.acoustid),
        musicbrainz: cli.musicbrainz_url.clone().unwrap_or(defaults.musicbrainz),
        cover_art: cli.cover_art_url.clone().unwrap_or(defaults.cover_art),
    };
    let settings = import::load_settings(&pool).await?;
    if settings.acoustid_key.is_none() {
        tracing::info!("no AcoustID key set: new files keep their own tags");
    }
    tracing::info!(
        library = %roots.library.display(),
        incoming = %roots.incoming.display(),
        staging = %roots.staging.display(),
        "watching the incoming folder and the buffer zone"
    );

    let (stop, stopped) = tokio::sync::watch::channel(false);
    // Says on the About page that the librarian runs, and how.
    let mut about = heron_service::about::common_config(&cli.log, &cli.metrics, cli.http_addr);
    about.extend([
        ConfigItem::text("HERON_LIBRARY_DIR", roots.library.display()),
        ConfigItem::text("HERON_INCOMING_DIR", roots.incoming.display()),
        ConfigItem::text("HERON_STAGING_DIR", roots.staging.display()),
        ConfigItem::flag("HERON_INCOMING_WATCH", cli.incoming_watch),
        ConfigItem::text("HERON_INCOMING_INTERVAL", cli.incoming_interval),
        ConfigItem::text("HERON_IMPORT_SETTLE", cli.import_settle),
        ConfigItem::text("HERON_NICE", &cli.nice),
    ]);
    let instance = std::env::var("HOSTNAME").unwrap_or_else(|_| SERVICE.to_owned());
    tokio::spawn(heron_service::about::run_heartbeat(
        pool.clone(),
        SERVICE,
        instance,
        about,
        stopped.clone(),
    ));
    let tasks = TaskRunner::new(pool.clone(), roots.clone(), endpoints.clone());
    let running_tasks = tokio::spawn(tasks.run(stopped.clone()));
    let stager = Stager::new(pool.clone(), roots.clone(), endpoints);
    let importer = Importer::new(pool.clone(), roots, Duration::from_secs(cli.import_settle));
    let importing = tokio::spawn(importer.run(
        stager,
        Duration::from_secs(cli.incoming_interval),
        cli.incoming_watch,
        stopped,
    ));

    let router = heron_service::ops_router(SERVICE, metrics, DatabaseHealth(pool));
    let served = heron_service::serve(cli.http_addr, router).await;
    // A file operation in progress finishes: the importer stops between files.
    let _ = stop.send(true);
    if tokio::time::timeout(Duration::from_secs(30), importing)
        .await
        .is_err()
    {
        tracing::warn!("import still running at shutdown, recovery will finish it");
    }
    if tokio::time::timeout(Duration::from_secs(30), running_tasks)
        .await
        .is_err()
    {
        tracing::warn!("a library task is still running at shutdown, recovery will finish it");
    }
    Ok(served?)
}

async fn organize_library(
    pool: &MySqlPool,
    roots: &Roots,
    apply: bool,
    json: bool,
) -> anyhow::Result<()> {
    let settings = import::load_settings(pool).await?;
    let layout = Layout::from_settings(&settings);
    let plan = organize::plan(pool, roots, &layout).await?;
    if !apply {
        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            for planned in &plan.moves {
                println!("{} -> {}", planned.from, planned.to);
            }
            for path in &plan.unreadable {
                println!("unreadable, not moved: {path}");
            }
            println!(
                "{} moves, {} tracks already in place. Dry run: nothing was changed. \
                 Run with --apply to move the files.",
                plan.moves.len(),
                plan.unchanged
            );
        }
        return Ok(());
    }
    let report = organize::apply(pool, roots, &plan).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for skipped in &report.skipped {
            println!("skipped {}: {}", skipped.path, skipped.reason);
        }
        println!(
            "Batch {}: {} files moved, {} skipped. Undo with `{SERVICE} undo {}`.",
            report.batch_id,
            report.done,
            report.skipped.len(),
            report.batch_id
        );
    }
    Ok(())
}

async fn list_batches(pool: &MySqlPool, limit: u32) -> anyhow::Result<()> {
    let rows = sqlx::query(
        "SELECT b.id, CAST(b.kind AS CHAR) AS kind, b.undoes,
                CAST(b.created_at AS CHAR) AS created_at,
                (SELECT COUNT(*) FROM library_moves m WHERE m.batch_id = b.id AND m.status = 'done') AS done,
                (SELECT u.id FROM library_batches u WHERE u.undoes = b.id LIMIT 1) AS undone_by
         FROM library_batches b ORDER BY b.id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    for row in rows {
        let id: u64 = row.try_get("id")?;
        let kind: String = row.try_get("kind")?;
        let created: String = row.try_get("created_at")?;
        let done: i64 = row.try_get("done")?;
        let undoes: Option<u64> = row.try_get("undoes")?;
        let undone_by: Option<u64> = row.try_get("undone_by")?;
        let mut line = format!("{id:>6}  {created}  {kind:<8}  {done} files");
        if let Some(undoes) = undoes {
            line.push_str(&format!(", undoes {undoes}"));
        }
        if let Some(by) = undone_by {
            line.push_str(&format!(", undone by {by}"));
        }
        println!("{line}");
    }
    Ok(())
}

async fn list_review(pool: &MySqlPool) -> anyhow::Result<()> {
    let rows = sqlx::query(
        "SELECT t.id, t.path, i.reason FROM track_identifications i
         JOIN tracks t ON t.id = i.track_id
         WHERE i.status = 'needs_review' ORDER BY t.path",
    )
    .fetch_all(pool)
    .await?;
    for row in &rows {
        let id: u64 = row.try_get("id")?;
        let path: String = row.try_get("path")?;
        let reason: Option<String> = row.try_get("reason")?;
        println!("{id:>8}  {path}  ({})", reason.unwrap_or_default());
    }
    println!("{} tracks need review.", rows.len());
    Ok(())
}

async fn settings(pool: &MySqlPool, set: Option<String>) -> anyhow::Result<()> {
    let current = import::load_settings(pool).await?;
    let Some(set) = set else {
        let mut shown = serde_json::to_value(&current)?;
        if current.acoustid_key.is_some() {
            shown["acoustid_key"] = "<set>".into();
        }
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(());
    };
    let mut merged = serde_json::to_value(&current)?;
    let changes: serde_json::Value =
        serde_json::from_str(&set).context("--set takes a JSON object")?;
    let (Some(merged_fields), Some(changes)) = (merged.as_object_mut(), changes.as_object()) else {
        anyhow::bail!("--set takes a JSON object");
    };
    for (key, value) in changes {
        merged_fields.insert(key.clone(), value.clone());
    }
    let settings: LibrarySettings =
        serde_json::from_value(merged).context("invalid library settings")?;
    Layout::parse(&settings.layout).context("invalid layout")?;
    sqlx::query(
        "INSERT INTO settings (name, value) VALUES (?, ?)
         ON DUPLICATE KEY UPDATE value = VALUES(value)",
    )
    .bind(LibrarySettings::SETTINGS_NAME)
    .bind(serde_json::to_string(&settings)?)
    .execute(pool)
    .await?;
    println!("Library settings saved.");
    Ok(())
}
