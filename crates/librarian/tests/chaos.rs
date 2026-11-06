// SPDX-License-Identifier: AGPL-3.0-or-later

//! Acceptance (chaos): interrupting the librarian during moves loses no
//! file, overwrites none, and leaves no track pointing at a missing path.
//!
//! The `heron-librarian` binary is crashed at every step of an operation
//! (`HERON_LIBRARIAN_CRASH_AT`, an abort like a power cut) and killed with
//! SIGKILL at random moments, while moving incoming files into the buffer
//! zone, importing from it (copies, because identified files are re-tagged;
//! renames for the others), organizing (renames), undoing, deleting tracks
//! to the trash, restoring them and purging the trash. After each crash it
//! is restarted, recovers, and carries on.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{
    Album, Mock, MockState, acoustid_result, fingerprint, temp_roots, write_tagged, write_wav,
};
use heron_core::LibrarySettings;
use heron_core::library::TaskRequest;
use heron_librarian::journal::Roots;
use heron_library::{Tags, is_audio, is_hidden};
use heron_service::db::ScratchDatabase;
use sqlx::MySqlPool;

/// The collision target that must never be overwritten.
const PRE_EXISTING: &str = "Band 1/Album 1/01 Song 1 of 1.wav";

struct Setup {
    db_url: String,
    roots: Roots,
    mock_url: String,
}

impl Setup {
    fn command(&self, args: &[&str], crash_at: Option<&str>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_heron-librarian"));
        command
            .args(args)
            .env("HERON_DATABASE_URL", &self.db_url)
            .env("HERON_LIBRARY_DIR", &self.roots.library)
            .env("HERON_INCOMING_DIR", &self.roots.incoming)
            .env("HERON_STAGING_DIR", &self.roots.staging)
            .env("HERON_COVERS_DIR", self.roots.covers.root())
            .env("HERON_HTTP_ADDR", "127.0.0.1:0")
            .env("HERON_IMPORT_SETTLE", "0")
            .env("HERON_INCOMING_INTERVAL", "1")
            .env("HERON_INCOMING_WATCH", "false")
            .env("HERON_ACOUSTID_URL", format!("{}/v2/lookup", self.mock_url))
            .env("HERON_MUSICBRAINZ_URL", format!("{}/ws/2", self.mock_url))
            .env("HERON_COVER_ART_URL", format!("{}/caa", self.mock_url))
            .env("RUST_LOG", "warn")
            .env_remove("HERON_LIBRARIAN_CRASH_AT")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(crash_at) = crash_at {
            command.env("HERON_LIBRARIAN_CRASH_AT", crash_at);
        }
        command
    }

    fn spawn(&self, args: &[&str], crash_at: Option<&str>) -> Child {
        self.command(args, crash_at).spawn().unwrap()
    }

    /// Runs a command to completion (or to its crash).
    fn run(&self, args: &[&str], crash_at: Option<&str>) -> bool {
        let mut command = self.command(args, crash_at);
        let output = command.stderr(Stdio::piped()).output().unwrap();
        eprintln!("chaos: {args:?} crash_at={crash_at:?} -> {}", output.status);
        if crash_at.is_none() && !output.status.success() {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        }
        output.status.success()
    }
}

fn wait(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the librarian did not finish in time");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Fingerprints of the audio files found in `dirs` (several copies of a
/// file can exist while a copy is in progress).
fn audio(dirs: &[&Path]) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for dir in dirs {
        let entries = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_hidden(entry.file_name()));
        for entry in entries.flatten() {
            if entry.file_type().is_file()
                && is_audio(entry.path())
                && !entry.path().ends_with(PRE_EXISTING)
            {
                *found.entry(fingerprint(entry.path())).or_default() += 1;
            }
        }
    }
    found
}

/// Every place audio can be: the incoming folder, the buffer zone, the
/// library and its trash.
fn everywhere(roots: &Roots) -> BTreeMap<String, usize> {
    audio(&[
        &roots.incoming,
        &roots.staging,
        &roots.library,
        &roots.library.join(".heron-trash"),
    ])
}

/// Nothing is lost and nothing is overwritten, whatever the moment.
fn assert_nothing_lost(
    roots: &Roots,
    originals: &BTreeMap<String, usize>,
    pre_existing: &[u8],
    when: &str,
) {
    let found = everywhere(roots);
    for fingerprint in originals.keys() {
        assert!(found.contains_key(fingerprint), "{when}: a file was lost");
    }
    assert_eq!(found.len(), originals.len(), "{when}: unexpected audio");
    assert_eq!(
        std::fs::read(roots.library.join(PRE_EXISTING)).unwrap(),
        pre_existing,
        "{when}: a file was overwritten"
    );
}

async fn assert_settled(
    pool: &MySqlPool,
    roots: &Roots,
    originals: &BTreeMap<String, usize>,
    when: &str,
) {
    let found = everywhere(roots);
    assert_eq!(&found, originals, "{when}: every file exactly once");
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM library_moves WHERE status IN ('pending', 'moved')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(unfinished, 0, "{when}: unfinished operations");
    let leftovers: Vec<_> = walkdir::WalkDir::new(&roots.library)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.file_name().to_string_lossy().starts_with(".heron-")
        })
        .map(|entry| entry.path().display().to_string())
        .collect();
    assert!(
        leftovers.is_empty(),
        "{when}: temporary files left: {leftovers:?}"
    );
    common::assert_tracks_match_files(pool, &roots.library).await;
    assert_trash_matches_files(pool, &roots.library, when).await;
}

/// Every track in the trash has its file there.
async fn assert_trash_matches_files(pool: &MySqlPool, library: &Path, when: &str) {
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT path FROM tracks WHERE deleted_at IS NOT NULL")
            .fetch_all(pool)
            .await
            .unwrap();
    for path in paths {
        assert!(
            common::path_buf(library, &path).is_file(),
            "{when}: deleted track {path} has no file in the trash"
        );
    }
}

fn incoming_is_empty(roots: &Roots) -> bool {
    audio(&[&roots.incoming]).is_empty()
}

/// Asks the librarian for a task, as the API does.
async fn ask(pool: &MySqlPool, request: &TaskRequest) -> u64 {
    sqlx::query("INSERT INTO library_tasks (kind, params) VALUES (?, ?)")
        .bind(request.kind())
        .bind(serde_json::to_string(request).unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id()
}

async fn task_status(pool: &MySqlPool, id: u64) -> String {
    sqlx::query_scalar("SELECT CAST(status AS CHAR) FROM library_tasks WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Runs the service until the task is over (then stops it as `docker
/// compose stop` does) or until it crashes at `crash_at`.
async fn run_task(setup: &Setup, pool: &MySqlPool, task: u64, crash_at: Option<&str>) {
    let mut child = setup.spawn(&[], crash_at);
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if matches!(task_status(pool, task).await.as_str(), "done" | "failed") {
            break;
        }
        assert!(Instant::now() < deadline, "task {task} did not finish");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    wait(&mut child, Duration::from_secs(60));
}

async fn ids(pool: &MySqlPool, query: &str) -> Vec<u64> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(query.to_owned()))
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Every track has what its buffer file was prepared with, from the
/// moment it exists.
async fn assert_prepared(pool: &MySqlPool, when: &str) {
    let unprepared: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks t
         WHERE NOT EXISTS (SELECT 1 FROM track_categories c WHERE c.track_id = t.id)
            OR NOT EXISTS (SELECT 1 FROM track_labels l WHERE l.track_id = t.id)
            OR NOT EXISTS (SELECT 1 FROM track_programming p
                           WHERE p.track_id = t.id AND p.mood = 'calm')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(unprepared, 0, "{when}: tracks without their preparation");
}

async fn ready_files(pool: &MySqlPool) -> Vec<u64> {
    ids(
        pool,
        "SELECT id FROM staged_files WHERE status = 'ready' ORDER BY id",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_operations_lose_and_overwrite_nothing() {
    let Some(db) = ScratchDatabase::create().await else {
        return;
    };
    let pool = &db.pool;
    let (_dir, roots) = temp_roots();

    // 4 albums of 4 tracks to identify (re-tagged: copies) and 4 unknown
    // files (renames).
    let mut mock = MockState::default();
    let mut seed = 0;
    for index in 1..=4 {
        let album = Album::new(index, 4, false);
        for (n, (recording, _)) in album.tracks.iter().enumerate() {
            seed += 1;
            let path = roots.incoming.join(format!("drop {index}/{n}.wav"));
            write_tagged(
                &path,
                seed,
                &Tags {
                    title: Some(format!("t{n}")),
                    ..Tags::default()
                },
            );
            mock.acoustid.insert(
                fingerprint(&path),
                acoustid_result(0.99, recording, vec![album.acoustid_group(n)]),
            );
        }
        mock.releases
            .insert(album.release_id.clone(), album.musicbrainz());
    }
    for n in 0..4 {
        seed += 1;
        write_wav(&roots.incoming.join(format!("unknown/{n}.wav")), seed);
    }
    let pre_existing = b"a file already where track 1 of album 1 wants to go".to_vec();
    std::fs::create_dir_all(roots.library.join("Band 1/Album 1")).unwrap();
    std::fs::write(roots.library.join(PRE_EXISTING), &pre_existing).unwrap();
    let originals = audio(&[&roots.incoming]);
    assert_eq!(originals.len(), 20);

    let mock = Mock::start(mock).await;
    sqlx::query("INSERT INTO settings (name, value) VALUES ('library', ?)")
        .bind(
            serde_json::to_string(&LibrarySettings {
                acoustid_key: Some("test-key".into()),
                save_cover: false,
                ..LibrarySettings::default()
            })
            .unwrap(),
        )
        .execute(pool)
        .await
        .unwrap();
    let setup = Setup {
        db_url: db.url(),
        roots: roots.clone(),
        mock_url: mock.url.clone(),
    };

    // Into the buffer zone: crash at every step, then at random.
    for crash_at in ["staging-recorded:1", "staging-recorded:7"] {
        setup.run(&[], Some(crash_at));
        assert_nothing_lost(
            &roots,
            &originals,
            &pre_existing,
            &format!("staging, crash at {crash_at}"),
        );
    }
    let mut round = 0;
    while !incoming_is_empty(&roots) && round < 3 {
        round += 1;
        let mut child = setup.spawn(&[], None);
        std::thread::sleep(Duration::from_millis(200 * round));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_nothing_lost(
            &roots,
            &originals,
            &pre_existing,
            &format!("staging, killed in round {round}"),
        );
    }
    // Then let it move and identify everything.
    let mut child = setup.spawn(&[], None);
    let deadline = Instant::now() + Duration::from_secs(120);
    while !incoming_is_empty(&roots) || ready_files(pool).await.len() < 20 {
        assert!(Instant::now() < deadline, "the files never got ready");
        std::thread::sleep(Duration::from_millis(200));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let tracks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(tracks, 0, "nothing imported before it is asked");

    // Prepare every file, as the API does.
    let category = sqlx::query("INSERT INTO categories (name) VALUES ('Chaos')")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
    let label = sqlx::query("INSERT INTO labels (name) VALUES ('chaos')")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
    sqlx::query(
        "INSERT INTO staged_file_categories (staged_file_id, category_id)
         SELECT id, ? FROM staged_files WHERE status = 'ready'",
    )
    .bind(category)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO staged_file_labels (staged_file_id, label_id)
         SELECT id, ? FROM staged_files WHERE status = 'ready'",
    )
    .bind(label)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE staged_files SET mood = 'calm' WHERE status = 'ready'")
        .execute(pool)
        .await
        .unwrap();

    // Import from the buffer zone: crash at every step of the first
    // operations, then at random, asking again for what is left.
    for step in ["recorded", "transferred", "finished", "removed"] {
        for n in [1, 3] {
            let ready = ready_files(pool).await;
            if ready.is_empty() {
                break;
            }
            let crash_at = format!("{step}:{n}");
            let task = ask(pool, &TaskRequest::StagingImport { file_ids: ready }).await;
            run_task(&setup, pool, task, Some(&crash_at)).await;
            assert_nothing_lost(
                &roots,
                &originals,
                &pre_existing,
                &format!("import, crash at {crash_at}"),
            );
            assert_prepared(pool, &format!("import, crash at {crash_at}")).await;
        }
    }
    let mut round = 0;
    loop {
        // A run recovers interrupted imports first.
        let ready = ready_files(pool).await;
        if ready.is_empty() {
            let importing = ids(
                pool,
                "SELECT id FROM staged_files WHERE status = 'importing'",
            )
            .await;
            if importing.is_empty() {
                break;
            }
        }
        round += 1;
        assert!(round < 20, "the import never finished");
        let task = ask(pool, &TaskRequest::StagingImport { file_ids: ready }).await;
        if round <= 3 {
            let mut child = setup.spawn(&[], None);
            tokio::time::sleep(Duration::from_millis(300 + 400 * round)).await;
            child.kill().unwrap();
            child.wait().unwrap();
            assert_nothing_lost(
                &roots,
                &originals,
                &pre_existing,
                &format!("import, killed in round {round}"),
            );
            assert_prepared(pool, &format!("import, killed in round {round}")).await;
        } else {
            run_task(&setup, pool, task, None).await;
        }
    }
    setup.run(&["batches"], None);
    assert_settled(pool, &roots, &originals, "after the import").await;
    assert_eq!(
        std::fs::read(roots.library.join(PRE_EXISTING)).unwrap(),
        pre_existing,
        "the collision got a new name"
    );
    let tracks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(tracks, 20, "every file imported once");
    assert_prepared(pool, "after the import").await;

    // Organize under another layout, crashing as it goes.
    sqlx::query(
        "UPDATE settings SET value = JSON_SET(value, '$.layout', '{artist}/{title}.{ext}') WHERE name = 'library'",
    )
    .execute(pool)
    .await
    .unwrap();
    let before_organize = common::snapshot(&roots.library);
    for crash_at in ["recorded:1", "transferred:2", "recorded:5", "transferred:7"] {
        setup.run(&["organize", "--apply"], Some(crash_at));
        assert_nothing_lost(
            &roots,
            &originals,
            &pre_existing,
            &format!("organize, crash at {crash_at}"),
        );
    }
    for _ in 0..3 {
        let mut child = setup.spawn(&["organize", "--apply"], None);
        std::thread::sleep(Duration::from_millis(150));
        let _ = child.kill();
        child.wait().unwrap();
        assert_nothing_lost(&roots, &originals, &pre_existing, "organize, killed");
    }
    assert!(setup.run(&["organize", "--apply"], None));
    assert_settled(pool, &roots, &originals, "after organizing").await;
    let organized = common::snapshot(&roots.library);
    assert_ne!(organized, before_organize, "files moved");
    assert!(roots.library.join("Band 2/Song 3 of 2.wav").exists());

    // Undo every organize batch, latest first, crashing in the first one
    // with two moves or more: how many moves each batch holds depends on
    // how far the organize runs killed above got, so the latest may hold
    // one, and never reach a crash at its second transfer.
    let batches: Vec<(u64, i64)> = sqlx::query_as(
        "SELECT b.id, (SELECT COUNT(*) FROM library_moves m WHERE m.batch_id = b.id AND m.status = 'done')
         FROM library_batches b WHERE b.kind = 'organize'
           AND EXISTS (SELECT 1 FROM library_moves m WHERE m.batch_id = b.id AND m.status = 'done')
         ORDER BY b.id DESC",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut crashed = false;
    for (batch, moves) in &batches {
        let batch = batch.to_string();
        if !crashed && *moves >= 2 {
            setup.run(&["undo", &batch], Some("transferred:2"));
            assert_nothing_lost(&roots, &originals, &pre_existing, "undo, crashed");
            crashed = true;
        }
        assert!(setup.run(&["undo", &batch], None), "undo {batch}");
    }
    assert!(crashed, "no organize batch with two moves: {batches:?}");
    assert_settled(pool, &roots, &originals, "after undoing").await;
    assert_eq!(
        common::snapshot(&roots.library),
        before_organize,
        "undone byte for byte"
    );

    // Delete half the tracks to the trash, crashing as it goes.
    let deleting = ids(pool, "SELECT id FROM tracks ORDER BY id LIMIT 10").await;
    for crash_at in ["recorded:2", "transferred:4", "finished:6"] {
        let task = ask(
            pool,
            &TaskRequest::Delete {
                track_ids: deleting.clone(),
            },
        )
        .await;
        run_task(&setup, pool, task, Some(crash_at)).await;
        assert_nothing_lost(
            &roots,
            &originals,
            &pre_existing,
            &format!("delete, crash at {crash_at}"),
        );
    }
    let task = ask(
        pool,
        &TaskRequest::Delete {
            track_ids: deleting.clone(),
        },
    )
    .await;
    run_task(&setup, pool, task, None).await;
    assert_settled(pool, &roots, &originals, "after deleting").await;
    let deleted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE deleted_at IS NOT NULL")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(deleted, 10);

    // Restore every deletion batch, crashing once.
    let batches = ids(
        pool,
        "SELECT b.id FROM library_batches b WHERE b.kind = 'delete'
           AND EXISTS (SELECT 1 FROM library_moves m WHERE m.batch_id = b.id AND m.status = 'done')
         ORDER BY b.id",
    )
    .await;
    for (index, batch) in batches.iter().enumerate() {
        let batch = batch.to_string();
        if index == 0 {
            setup.run(&["undo", &batch], Some("transferred:1"));
            assert_nothing_lost(&roots, &originals, &pre_existing, "restore, crashed");
        }
        assert!(setup.run(&["undo", &batch], None), "restore {batch}");
    }
    assert_settled(pool, &roots, &originals, "after restoring").await;
    assert_eq!(
        common::snapshot(&roots.library),
        before_organize,
        "restored byte for byte"
    );

    // Delete them again and purge the trash, crashing in the middle: what
    // is purged is gone, the rest stays whole.
    let task = ask(
        pool,
        &TaskRequest::Delete {
            track_ids: deleting.clone(),
        },
    )
    .await;
    run_task(&setup, pool, task, None).await;
    let task = ask(pool, &TaskRequest::EmptyTrash).await;
    run_task(&setup, pool, task, Some("purged:4")).await;
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE deleted_at IS NOT NULL")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(left < 10, "the purge started");
    assert_trash_matches_files(pool, &roots.library, "purge, crashed").await;
    common::assert_tracks_match_files(pool, &roots.library).await;
    let task = ask(pool, &TaskRequest::EmptyTrash).await;
    run_task(&setup, pool, task, None).await;
    let tracks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(tracks, 10, "the purged tracks are gone");
    assert_eq!(everywhere(&roots).len(), 10, "and their files");
    assert!(
        !roots
            .library
            .join(".heron-trash")
            .join(batches[0].to_string())
            .exists()
    );
    common::assert_tracks_match_files(pool, &roots.library).await;
    db.drop().await;
}
