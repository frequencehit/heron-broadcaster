// SPDX-License-Identifier: AGPL-3.0-or-later

//! Covers looked up on the Cover Art Archive through library tasks: by
//! release ID, by a sure search, and the candidates of an unsure one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Mock, MockState, temp_roots};
use heron_core::library::TaskRequest;
use heron_librarian::tasks::TaskRunner;
use heron_service::db::ScratchDatabase;
use serde_json::json;
use sqlx::MySqlPool;

fn jpeg(shade: u8) -> Vec<u8> {
    let picture = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        700,
        700,
        image::Rgb([shade, 0, 0]),
    ));
    let mut out = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut out),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    out
}

async fn track(pool: &MySqlPool, path: &str, album: &str, release: Option<&str>) -> u64 {
    sqlx::query(
        "INSERT INTO tracks (path, artist, album, musicbrainz_release_id) VALUES (?, 'Band', ?, ?)",
    )
    .bind(path)
    .bind(album)
    .bind(release)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_id()
}

async fn run(runner: &TaskRunner, pool: &MySqlPool, request: &TaskRequest) -> serde_json::Value {
    let id = sqlx::query("INSERT INTO library_tasks (kind, params) VALUES (?, ?)")
        .bind(request.kind())
        .bind(serde_json::to_string(request).unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_id();
    assert!(runner.run_next().await.unwrap());
    let (status, result, error): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT CAST(status AS CHAR), CAST(result AS CHAR), error FROM library_tasks WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(status, "done", "{error:?}");
    serde_json::from_str(&result.unwrap()).unwrap()
}

/// The album cover of a track: its source and detail.
async fn cover(pool: &MySqlPool, track_id: u64) -> Option<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT CAST(c.source AS CHAR), c.detail FROM tracks t
         JOIN album_covers c ON c.album_key = t.album_key WHERE t.id = ?",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn albums_get_covers_or_candidates() {
    let Some(db) = ScratchDatabase::create().await else {
        return;
    };
    let pool = &db.pool;
    let id = |n: u32| format!("00000000-0000-4000-8000-{n:012}");
    let mut state = MockState::default();
    state.covers.insert(id(1), jpeg(10));
    state.covers.insert(id(2), jpeg(20));
    state.covers.insert(id(3), jpeg(30));
    let release = |n: u32, score: u32, title: &str| {
        json!({"id": id(n), "score": score, "title": title, "date": "1999", "country": "FR",
               "artist-credit": [{"name": "Band", "joinphrase": "", "artist": {"id": id(99)}}]})
    };
    state
        .searches
        .insert("Sure".into(), json!([release(2, 100, "Sure Thing")]));
    state.searches.insert(
        "Doubt".into(),
        json!([release(3, 70, "In Doubt (Deluxe)"), release(4, 20, "Other")]),
    );
    let mock = Mock::start(state).await;
    let (_dirs, roots) = temp_roots();
    let runner = TaskRunner::new(pool.clone(), roots, mock.endpoints());

    let by_release = track(pool, "a/1.mp3", "Known", Some(&id(1))).await;
    let sure = track(pool, "b/1.mp3", "Sure Thing", None).await;
    let sure_too = track(pool, "b/2.mp3", "sure thing", None).await;
    let unsure = track(pool, "c/1.mp3", "In Doubt", None).await;
    let unknown = track(pool, "d/1.mp3", "Nowhere", None).await;

    let report = run(
        &runner,
        pool,
        &TaskRequest::FetchCovers {
            track_ids: Vec::new(),
            candidates: false,
        },
    )
    .await;
    assert_eq!(
        report,
        json!({"albums": 4, "found": 2, "candidates": 1, "not_found": 1, "failed": 0})
    );
    let caa = |n: u32| Some(("cover_art_archive".to_owned(), Some(id(n))));
    assert_eq!(cover(pool, by_release).await, caa(1));
    assert_eq!(cover(pool, sure).await, caa(2));
    assert_eq!(cover(pool, sure_too).await, caa(2), "one album");
    assert_eq!(
        cover(pool, unsure).await,
        None,
        "an unsure search sets nothing"
    );
    assert_eq!(cover(pool, unknown).await, None);

    let candidates: Vec<(u64, String, u8, String)> = sqlx::query_as(
        "SELECT id, release_id, score, thumbnail_url FROM cover_candidates
         WHERE status = 'pending'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(candidates.len(), 1, "the result under 50 is left out");
    let (candidate, release_id, score, thumbnail) = &candidates[0];
    assert_eq!((release_id.as_str(), *score), (id(3).as_str(), 70));
    assert!(thumbnail.ends_with(&format!("/release/{}/front-250", id(3))));

    // Looked up once: not again in the background.
    let looked_up: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cover_lookups")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(looked_up, 4);

    let report = run(
        &runner,
        pool,
        &TaskRequest::ApplyCover {
            candidate_id: *candidate,
            track_id: None,
        },
    )
    .await;
    assert_eq!(report["found"], 1);
    assert_eq!(cover(pool, unsure).await, caa(3));
    let status: String =
        sqlx::query_scalar("SELECT CAST(status AS CHAR) FROM cover_candidates WHERE id = ?")
            .bind(candidate)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "accepted");

    // Asked from a track's page: candidates, even for an album with a cover.
    let report = run(
        &runner,
        pool,
        &TaskRequest::FetchCovers {
            track_ids: vec![sure],
            candidates: true,
        },
    )
    .await;
    assert_eq!(report["candidates"], 1);
    assert_eq!(cover(pool, sure).await, caa(2), "kept until one is picked");
    db.drop().await;
}
