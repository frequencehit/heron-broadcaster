// SPDX-License-Identifier: AGPL-3.0-or-later

//! The database migrations, with the steps SQL cannot do.
//!
//! Cover images were columns of `covers` before they became files of the
//! covers folder (docs/library.md#where-the-images-are). The migration that
//! drops the columns ([`COVERS_ON_DISK`]) runs only once every image they
//! hold is a file: the migrations before it run first, then the images are
//! written, then the rest. Writing an image twice does no harm, so an
//! interrupted start does it again; one that cannot write stops before
//! anything is dropped.

use anyhow::Context;
use heron_library::covers::{self, Files};
use heron_service::db::MIGRATOR;
use sqlx::MySqlPool;

/// The migration that drops `covers.image` and `covers.thumbnail`.
pub const COVERS_ON_DISK: i64 = 20261225000000;

/// Images read from the database at a time.
const BATCH: u32 = 50;

/// Applies every migration, writing the cover images of the database to
/// `files` before their columns are dropped.
pub async fn run(pool: &MySqlPool, files: &Files) -> anyhow::Result<()> {
    MIGRATOR
        .run_to(COVERS_ON_DISK - 1, pool)
        .await
        .context("failed to apply database migrations")?;
    let written = export_cover_images(pool, files).await.with_context(|| {
        format!(
            "cannot move the cover images to the covers folder ({}); nothing was dropped",
            files.root().display()
        )
    })?;
    if written > 0 {
        tracing::info!(
            written,
            dir = %files.root().display(),
            "cover images moved from the database to the covers folder"
        );
    }
    MIGRATOR
        .run(pool)
        .await
        .context("failed to apply database migrations")?;
    Ok(())
}

/// Writes the images still in `covers` to their files, those already
/// there left alone. Returns how many covers were written.
async fn export_cover_images(pool: &MySqlPool, files: &Files) -> anyhow::Result<u64> {
    let columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns
         WHERE table_schema = DATABASE() AND table_name = 'covers'
           AND column_name IN ('image', 'thumbnail')",
    )
    .fetch_one(pool)
    .await?;
    if columns < 2 {
        return Ok(0);
    }
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covers")
        .fetch_one(pool)
        .await?;
    if pending == 0 {
        return Ok(0);
    }
    files.check()?;
    tracing::info!(
        covers = pending,
        "moving the cover images from the database to the covers folder"
    );
    let mut written = 0;
    let mut after: Vec<u8> = Vec::new();
    loop {
        type Row = (Vec<u8>, Vec<u8>, Vec<u8>);
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT hash, image, thumbnail FROM covers WHERE hash > ? ORDER BY hash LIMIT ?",
        )
        .bind(&after)
        .bind(BATCH)
        .fetch_all(pool)
        .await?;
        let Some((last, _, _)) = rows.last() else {
            break;
        };
        after = last.clone();
        for (hash, image, thumbnail) in rows {
            let Ok(hash) = covers::Hash::try_from(hash.as_slice()) else {
                continue;
            };
            if files.has(&hash).await {
                continue;
            }
            let files = files.clone();
            tokio::task::spawn_blocking(move || {
                files.write(&hash, covers::LARGE, &image)?;
                files.write(&hash, covers::SMALL, &thumbnail)
            })
            .await??;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Heron whose cover images are in the database (an earlier edge
    /// build) has them as files after the upgrade, with nothing else lost.
    #[tokio::test]
    async fn cover_images_move_from_the_database_to_files() {
        let Ok(admin_url) = std::env::var("HERON_TEST_DATABASE_URL") else {
            return;
        };
        let admin_url = admin_url.trim_end_matches('/').to_owned();
        let name = format!("heron_covers_{}", std::process::id());
        let admin = sqlx::MySqlPool::connect(&admin_url).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE `{name}`")))
            .execute(&admin)
            .await
            .unwrap();
        let pool = sqlx::MySqlPool::connect(&format!("{admin_url}/{name}"))
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path().join("covers"));

        // Covers stored the old way: an album's, set by hand, and a logo.
        MIGRATOR.run_to(COVERS_ON_DISK - 1, &pool).await.unwrap();
        let (album, logo) = ([1u8; 32], [2u8; 32]);
        for (hash, image, thumbnail) in [
            (album, &b"album 600"[..], &b"album 96"[..]),
            (logo, b"logo 600", b"logo 96"),
        ] {
            sqlx::query(
                "INSERT INTO covers (hash, width, height, image, thumbnail)
                 VALUES (?, 1200, 1200, ?, ?)",
            )
            .bind(&hash[..])
            .bind(image)
            .bind(thumbnail)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO album_covers (album_key, cover_hash, source)
             VALUES ('0123456789abcdef0123456789abcdef', ?, 'hand')",
        )
        .bind(&album[..])
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE stations SET logo_hash = ? WHERE id = 1")
            .bind(&logo[..])
            .execute(&pool)
            .await
            .unwrap();

        // A folder that cannot be written: the start fails, nothing dropped.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "").unwrap();
        assert!(run(&pool, &Files::new(&blocked)).await.is_err());
        let image: Vec<u8> = sqlx::query_scalar("SELECT image FROM covers WHERE hash = ?")
            .bind(&album[..])
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(image, b"album 600");

        run(&pool, &files).await.unwrap();
        let read = |hash: [u8; 32], size| {
            let files = files.clone();
            async move { files.read(&hash, size).await.unwrap() }
        };
        assert_eq!(
            read(album, covers::LARGE).await.as_deref(),
            Some(&b"album 600"[..])
        );
        assert_eq!(
            read(album, covers::SMALL).await.as_deref(),
            Some(&b"album 96"[..])
        );
        assert_eq!(
            read(logo, covers::LARGE).await.as_deref(),
            Some(&b"logo 600"[..])
        );
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT CAST(column_name AS CHAR) FROM information_schema.columns
             WHERE table_schema = DATABASE() AND table_name = 'covers'
             ORDER BY ordinal_position",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(columns, ["hash", "width", "height", "created_at"]);
        let kept: Vec<(Vec<u8>, u32)> =
            sqlx::query_as("SELECT hash, width FROM covers ORDER BY hash")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(kept, [(album.to_vec(), 1200), (logo.to_vec(), 1200)]);
        let linked: Vec<u8> = sqlx::query_scalar("SELECT cover_hash FROM album_covers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(linked, album);

        // Started again: nothing to do.
        run(&pool, &files).await.unwrap();

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE `{name}`")))
            .execute(&admin)
            .await
            .unwrap();
    }

    /// A new database has no images to move.
    #[tokio::test]
    async fn a_new_database_is_migrated_whole() {
        let Ok(admin_url) = std::env::var("HERON_TEST_DATABASE_URL") else {
            return;
        };
        let admin_url = admin_url.trim_end_matches('/').to_owned();
        let name = format!("heron_covers_new_{}", std::process::id());
        let admin = sqlx::MySqlPool::connect(&admin_url).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE `{name}`")))
            .execute(&admin)
            .await
            .unwrap();
        let pool = sqlx::MySqlPool::connect(&format!("{admin_url}/{name}"))
            .await
            .unwrap();
        // Not even a folder: it is not needed.
        run(&pool, &Files::new("/nonexistent/covers"))
            .await
            .unwrap();
        let applied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = ? AND success",
        )
        .bind(COVERS_ON_DISK)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(applied, 1);
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE `{name}`")))
            .execute(&admin)
            .await
            .unwrap();
    }
}
