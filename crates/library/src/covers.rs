// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cover art kept in Heron (docs/library.md#cover-art).
//!
//! Each image is stored once, by the hash of the image as found, decoded
//! and re-encoded as two JPEGs (600 and 96 px on the longest side): the
//! JPEGs are files of the covers folder ([`Files`]), `covers` keeps the
//! hash and the size of the image as found.
//! An album (`tracks.album_key`) has its cover in `album_covers`; a track
//! can have its own in `track_covers`, in place of its album's.
//!
//! The scanner and the librarian call [`from_file`] when they read a file:
//! a cover embedded in it, or an image beside it, becomes its album's or
//! its own, in the order of [`Source`]. A cover set by hand always wins.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits};
use sqlx::{MySqlConnection, MySqlPool};

/// Largest image read (uploaded, embedded or beside a file).
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
/// Largest width or height of an image read.
pub const MAX_SIDE: u32 = 10_000;
/// The longest side of the large copy.
pub const LARGE: u32 = 600;
/// The longest side of the thumbnail.
pub const SMALL: u32 = 96;
const QUALITY: u8 = 85;

/// Images beside a file that are its album's cover, as imports save them.
const BESIDE: [&str; 3] = ["cover", "folder", "front"];
const BESIDE_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "png"];

/// A cover's hash: BLAKE3 of the image as found.
pub type Hash = [u8; 32];

/// Where a cover comes from, from the lowest to the highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    CoverArtArchive,
    /// An image beside the file (`cover.jpg`).
    File,
    Embedded,
    Hand,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CoverArtArchive => "cover_art_archive",
            Self::File => "file",
            Self::Embedded => "embedded",
            Self::Hand => "hand",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::CoverArtArchive,
            Self::File,
            Self::Embedded,
            Self::Hand,
        ]
        .into_iter()
        .find(|source| source.as_str() == name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoverError {
    #[error("the image is larger than {} MB", MAX_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("not a JPEG, PNG or WebP image")]
    Unsupported,
    #[error("the image cannot be read: {0}")]
    Decode(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot write the image to the covers folder: {0}")]
    Write(std::io::Error),
    #[error("the image was not processed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// An image, re-encoded.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub hash: Hash,
    /// Of the image as found.
    pub width: u32,
    pub height: u32,
    /// [`LARGE`] px JPEG.
    pub image: Vec<u8>,
    /// [`SMALL`] px JPEG.
    pub thumbnail: Vec<u8>,
}

pub fn hash(bytes: &[u8]) -> Hash {
    *blake3::hash(bytes).as_bytes()
}

/// Lowercase hexadecimal, as in cover URLs.
pub fn hex(hash: &[u8]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The hash a hexadecimal string names, if it is one.
pub fn parse_hex(text: &str) -> Option<Hash> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut hash = [0; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(hash)
}

/// Decodes a JPEG, PNG or WebP image and makes its two copies. Blocking:
/// call it from a blocking thread.
pub fn encode(bytes: &[u8]) -> Result<Encoded, CoverError> {
    if bytes.len() > MAX_BYTES {
        return Err(CoverError::TooLarge);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| CoverError::Decode(error.to_string()))?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return Err(CoverError::Unsupported);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let decode = |error: image::ImageError| CoverError::Decode(error.to_string());
    let mut decoder = reader.into_decoder().map_err(decode)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut found = DynamicImage::from_decoder(decoder).map_err(decode)?;
    found.apply_orientation(orientation);
    let (width, height) = found.dimensions();
    if width == 0 || height == 0 {
        return Err(CoverError::Decode("the image is empty".to_owned()));
    }
    let flat = flatten(&found);
    let large = fit(&flat, LARGE, image::imageops::FilterType::Lanczos3);
    let small = fit(&large, SMALL, image::imageops::FilterType::Triangle);
    Ok(Encoded {
        hash: hash(bytes),
        width,
        height,
        image: jpeg(&large)?,
        thumbnail: jpeg(&small)?,
    })
}

/// RGB, transparent parts on white.
fn flatten(found: &DynamicImage) -> DynamicImage {
    if !found.color().has_alpha() {
        return DynamicImage::ImageRgb8(found.to_rgb8());
    }
    let rgba = found.to_rgba8();
    let rgb = image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let over = |channel: u8| {
            let alpha = u16::from(a);
            ((u16::from(channel) * alpha + 255 * (255 - alpha)) / 255) as u8
        };
        image::Rgb([over(r), over(g), over(b)])
    });
    DynamicImage::ImageRgb8(rgb)
}

/// At most `side` px on its longest side, never enlarged.
fn fit(found: &DynamicImage, side: u32, filter: image::imageops::FilterType) -> DynamicImage {
    if found.width() <= side && found.height() <= side {
        found.clone()
    } else {
        found.resize(side, side, filter)
    }
}

fn jpeg(found: &DynamicImage) -> Result<Vec<u8>, CoverError> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY)
        .encode_image(found)
        .map_err(|error| CoverError::Decode(error.to_string()))?;
    Ok(out)
}

/// The covers folder (`HERON_COVERS_DIR`), where the two JPEGs of each
/// cover are: `<first two hex digits>/<hash>-600.jpg` and `…-96.jpg`.
#[derive(Clone, Debug)]
pub struct Files {
    root: PathBuf,
}

/// Temporary files of this process, each its own name.
static TEMPORARY: AtomicU64 = AtomicU64::new(0);

impl Files {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file of a cover's copy, [`LARGE`] or [`SMALL`].
    pub fn path(&self, hash: &[u8], size: u32) -> PathBuf {
        let hex = hex(hash);
        self.root
            .join(hex.get(..2).unwrap_or("00"))
            .join(format!("{hex}-{size}.jpg"))
    }

    /// Creates the folder if it is missing, and checks it can be written.
    pub fn check(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let probe = self
            .root
            .join(format!(".write-check-{}", std::process::id()));
        std::fs::write(&probe, b"")?;
        std::fs::remove_file(&probe)
    }

    /// Whether both copies of a cover are there.
    pub async fn has(&self, hash: &Hash) -> bool {
        let paths = [self.path(hash, LARGE), self.path(hash, SMALL)];
        tokio::task::spawn_blocking(move || paths.iter().all(|path| path.is_file()))
            .await
            .unwrap_or(false)
    }

    /// One copy of a cover, `None` when its file is missing.
    pub async fn read(&self, hash: &[u8], size: u32) -> std::io::Result<Option<Vec<u8>>> {
        let path = self.path(hash, size);
        let read = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(std::io::Error::other)?;
        match read {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Writes one copy, to a temporary file renamed over its name: a crash
    /// never leaves half an image under a cover's name. Blocking.
    pub fn write(&self, hash: &[u8], size: u32, bytes: &[u8]) -> std::io::Result<()> {
        let path = self.path(hash, size);
        let dir = path.parent().unwrap_or(&self.root);
        std::fs::create_dir_all(dir)?;
        let temporary = dir.join(format!(
            ".{}-{size}.{}-{}.tmp",
            hex(hash),
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)
        })();
        if written.is_err() {
            std::fs::remove_file(&temporary).ok();
        }
        written
    }

    /// Writes both copies of an encoded image.
    pub async fn write_encoded(&self, encoded: &Encoded) -> std::io::Result<()> {
        let files = self.clone();
        let encoded = encoded.clone();
        tokio::task::spawn_blocking(move || {
            files.write(&encoded.hash, LARGE, &encoded.image)?;
            files.write(&encoded.hash, SMALL, &encoded.thumbnail)
        })
        .await
        .map_err(std::io::Error::other)?
    }

    /// Removes both copies of a cover; missing ones are no error.
    pub async fn remove(&self, hash: &[u8]) -> std::io::Result<()> {
        let paths = [self.path(hash, LARGE), self.path(hash, SMALL)];
        tokio::task::spawn_blocking(move || {
            for path in paths {
                match std::fs::remove_file(path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error);
                    }
                    _ => {}
                }
            }
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)?
    }
}

/// Stores `bytes` as a cover, unless the same image is stored already, and
/// returns its hash. A cover whose files are missing has them written
/// again. The image is decoded on a blocking thread.
pub async fn store(
    connection: &mut MySqlConnection,
    files: &Files,
    bytes: Vec<u8>,
) -> Result<Hash, CoverError> {
    if bytes.len() > MAX_BYTES {
        return Err(CoverError::TooLarge);
    }
    let hash = hash(&bytes);
    let known: Option<i32> = sqlx::query_scalar("SELECT 1 FROM covers WHERE hash = ?")
        .bind(&hash[..])
        .fetch_optional(&mut *connection)
        .await?;
    if known.is_some() {
        if files.has(&hash).await {
            return Ok(hash);
        }
        tracing::warn!(
            cover = hex(&hash),
            "the cover's files were missing: written again"
        );
    }
    let encoded = tokio::task::spawn_blocking(move || encode(&bytes)).await??;
    save(connection, files, &encoded).await?;
    Ok(hash)
}

/// Saves an encoded image (once): its files, then its row.
pub async fn save(
    connection: &mut MySqlConnection,
    files: &Files,
    encoded: &Encoded,
) -> Result<(), CoverError> {
    files
        .write_encoded(encoded)
        .await
        .map_err(CoverError::Write)?;
    sqlx::query("INSERT IGNORE INTO covers (hash, width, height) VALUES (?, ?, ?)")
        .bind(&encoded.hash[..])
        .bind(encoded.width)
        .bind(encoded.height)
        .execute(connection)
        .await?;
    Ok(())
}

/// A cover as linked to an album or a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub hash: Hash,
    pub source: Source,
    pub detail: Option<String>,
}

type LinkRow = (Vec<u8>, String, Option<String>);

fn link((hash, source, detail): LinkRow) -> Option<Link> {
    Some(Link {
        hash: hash.try_into().ok()?,
        source: Source::parse(&source)?,
        detail,
    })
}

pub async fn album_cover(
    executor: impl sqlx::MySqlExecutor<'_>,
    album_key: &str,
) -> Result<Option<Link>, sqlx::Error> {
    let row: Option<LinkRow> = sqlx::query_as(
        "SELECT cover_hash, CAST(source AS CHAR), detail FROM album_covers WHERE album_key = ?",
    )
    .bind(album_key)
    .fetch_optional(executor)
    .await?;
    Ok(row.and_then(link))
}

pub async fn track_cover(
    executor: impl sqlx::MySqlExecutor<'_>,
    track_id: u64,
) -> Result<Option<Link>, sqlx::Error> {
    let row: Option<LinkRow> = sqlx::query_as(
        "SELECT cover_hash, CAST(source AS CHAR), detail FROM track_covers WHERE track_id = ?",
    )
    .bind(track_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.and_then(link))
}

/// The album a track belongs to, for its cover.
pub async fn album_key(
    executor: impl sqlx::MySqlExecutor<'_>,
    track_id: u64,
) -> Result<Option<String>, sqlx::Error> {
    Ok(
        sqlx::query_scalar("SELECT album_key FROM tracks WHERE id = ?")
            .bind(track_id)
            .fetch_optional(executor)
            .await?
            .flatten(),
    )
}

/// The update of a link: a cover set by hand is only replaced by another.
const KEEP_HAND: &str = " ON DUPLICATE KEY UPDATE
    cover_hash = IF(source = 'hand' AND VALUES(source) <> 'hand', cover_hash, VALUES(cover_hash)),
    detail = IF(source = 'hand' AND VALUES(source) <> 'hand', detail, VALUES(detail)),
    updated_by = IF(source = 'hand' AND VALUES(source) <> 'hand', updated_by, VALUES(updated_by)),
    source = IF(source = 'hand' AND VALUES(source) <> 'hand', source, VALUES(source))";

/// Gives an album its cover. A cover set by hand is replaced only by
/// another one set by hand.
pub async fn set_album(
    connection: &mut MySqlConnection,
    album_key: &str,
    link: &Link,
    user: Option<u64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO album_covers (album_key, cover_hash, source, detail, updated_by)
         VALUES (?, ?, ?, ?, ?){KEEP_HAND}"
    )))
    .bind(album_key)
    .bind(&link.hash[..])
    .bind(link.source.as_str())
    .bind(&link.detail)
    .bind(user)
    .execute(&mut *connection)
    .await?;
    if link.source == Source::Hand {
        // Set by hand for the album: its tracks show it, not what their
        // files had.
        sqlx::query(
            "DELETE c FROM track_covers c JOIN tracks t ON t.id = c.track_id
             WHERE t.album_key = ? AND c.source <> 'hand'",
        )
        .bind(album_key)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Gives a track its own cover, in place of its album's. A cover set by
/// hand is replaced only by another one set by hand.
pub async fn set_track(
    connection: &mut MySqlConnection,
    track_id: u64,
    link: &Link,
    user: Option<u64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO track_covers (track_id, cover_hash, source, detail, updated_by)
         VALUES (?, ?, ?, ?, ?){KEEP_HAND}"
    )))
    .bind(track_id)
    .bind(&link.hash[..])
    .bind(link.source.as_str())
    .bind(&link.detail)
    .bind(user)
    .execute(connection)
    .await?;
    Ok(())
}

/// Removes a track's own cover, unless it was set by hand.
async fn drop_found_track_cover(
    connection: &mut MySqlConnection,
    track_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM track_covers WHERE track_id = ? AND source <> 'hand'")
        .bind(track_id)
        .execute(connection)
        .await?;
    Ok(())
}

/// Finds the covers of the track `track_id` in its file: `embedded`, the
/// cover in it ([`crate::tags::FileTags::cover`]), and, when `file` is
/// given, an image beside it. Called after the track's tags were saved.
pub async fn from_file(
    connection: &mut MySqlConnection,
    files: &Files,
    track_id: u64,
    file: Option<&Path>,
    embedded: Option<&[u8]>,
) -> Result<(), CoverError> {
    let album_key = album_key(&mut *connection, track_id).await?;
    let own = track_cover(&mut *connection, track_id).await?;
    let own_by_hand = own.as_ref().is_some_and(|own| own.source == Source::Hand);
    let mut album = match &album_key {
        Some(key) => album_cover(&mut *connection, key).await?,
        None => None,
    };

    let embedded = match embedded {
        Some(bytes) => match store(connection, files, bytes.to_vec()).await {
            Ok(hash) => Some(hash),
            Err(CoverError::Database(error)) => return Err(error.into()),
            Err(error) => {
                tracing::warn!(track_id, %error, "cannot read the cover embedded in the file");
                None
            }
        },
        None => None,
    };
    match (embedded, &album_key) {
        (Some(hash), None) => {
            if !own_by_hand {
                let link = Link {
                    hash,
                    source: Source::Embedded,
                    detail: None,
                };
                set_track(connection, track_id, &link, None).await?;
            }
        }
        (Some(hash), Some(key)) => match &album {
            Some(cover) if cover.source == Source::Embedded && cover.hash != hash => {
                // Not the album's: the track keeps its own.
                if !own_by_hand {
                    let link = Link {
                        hash,
                        source: Source::Embedded,
                        detail: None,
                    };
                    set_track(connection, track_id, &link, None).await?;
                }
            }
            Some(cover) if cover.source >= Source::Embedded => {
                drop_found_track_cover(connection, track_id).await?;
            }
            _ => {
                let link = Link {
                    hash,
                    source: Source::Embedded,
                    detail: None,
                };
                set_album(connection, key, &link, None).await?;
                drop_found_track_cover(connection, track_id).await?;
                album = Some(link);
            }
        },
        (None, _) => {
            if own
                .as_ref()
                .is_some_and(|own| own.source == Source::Embedded)
            {
                drop_found_track_cover(connection, track_id).await?;
            }
        }
    }

    // An image beside the file, while nothing better is known.
    let Some(file) = file else {
        return Ok(());
    };
    let wanted = match &album_key {
        Some(_) => album
            .as_ref()
            .is_none_or(|cover| cover.source < Source::File),
        None => embedded.is_none() && own.as_ref().is_none_or(|own| own.source < Source::File),
    };
    if !wanted {
        return Ok(());
    }
    let Some(dir) = file.parent().map(Path::to_path_buf) else {
        return Ok(());
    };
    let Some((name, bytes)) = tokio::task::spawn_blocking(move || beside(&dir)).await? else {
        return Ok(());
    };
    let hash = match store(connection, files, bytes).await {
        Ok(hash) => hash,
        Err(CoverError::Database(error)) => return Err(error.into()),
        Err(error) => {
            tracing::warn!(track_id, name, %error, "cannot read the cover beside the file");
            return Ok(());
        }
    };
    let link = Link {
        hash,
        source: Source::File,
        detail: Some(name),
    };
    match &album_key {
        Some(key) => set_album(connection, key, &link, None).await?,
        None => set_track(connection, track_id, &link, None).await?,
    }
    Ok(())
}

/// The first image of `dir` named like a cover (`cover.jpg`, `folder.png`,
/// …, any case), with its name. Images too large are skipped.
pub fn beside(dir: &Path) -> Option<(String, Vec<u8>)> {
    let mut found: Vec<(usize, String, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let lower = name.to_lowercase();
            let (stem, extension) = lower.rsplit_once('.')?;
            let rank = BESIDE.iter().position(|wanted| *wanted == stem)?;
            BESIDE_EXTENSIONS
                .contains(&extension)
                .then(|| (rank, name, entry.path()))
        })
        .collect();
    found.sort();
    found.into_iter().find_map(|(_, name, path)| {
        let size = std::fs::metadata(&path).ok()?.len();
        if size == 0 || size > MAX_BYTES as u64 {
            return None;
        }
        std::fs::read(&path).ok().map(|bytes| (name, bytes))
    })
}

/// Deletes album covers no track uses (but those set by hand), and the
/// images nothing uses that were stored more than an hour ago (an image
/// just stored is about to be linked), with their files.
pub async fn prune(pool: &MySqlPool, files: &Files) -> Result<u64, sqlx::Error> {
    const UNUSED: &str = "created_at < NOW(6) - INTERVAL 1 HOUR
           AND NOT EXISTS (SELECT 1 FROM album_covers a WHERE a.cover_hash = covers.hash)
           AND NOT EXISTS (SELECT 1 FROM track_covers t WHERE t.cover_hash = covers.hash)
           AND NOT EXISTS (SELECT 1 FROM stations s WHERE s.logo_hash = covers.hash)";
    sqlx::query(
        "DELETE FROM album_covers WHERE source <> 'hand' AND NOT EXISTS
             (SELECT 1 FROM tracks t WHERE t.album_key = album_covers.album_key)",
    )
    .execute(pool)
    .await?;
    let unused: Vec<Vec<u8>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT hash FROM covers WHERE {UNUSED}"
    )))
    .fetch_all(pool)
    .await?;
    let mut deleted = 0;
    for hash in unused {
        // Still unused: it may have been linked since.
        let gone = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM covers WHERE hash = ? AND {UNUSED}"
        )))
        .bind(&hash)
        .execute(pool)
        .await?
        .rows_affected();
        if gone == 0 {
            continue;
        }
        deleted += 1;
        if let Err(error) = files.remove(&hash).await {
            tracing::warn!(cover = hex(&hash), %error, "cannot delete the cover's files");
        }
    }
    if deleted > 0 {
        tracing::info!(deleted, "covers nothing uses deleted");
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let picture = if alpha {
            DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([255, 0, 0, 0]),
            ))
        } else {
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                width,
                height,
                image::Rgb([10, 200, 30]),
            ))
        };
        picture
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn images_are_re_encoded_in_two_sizes() {
        let encoded = encode(&png(1200, 800, false)).unwrap();
        assert_eq!((encoded.width, encoded.height), (1200, 800));
        let large = image::load_from_memory(&encoded.image).unwrap();
        assert_eq!(large.dimensions(), (600, 400));
        let small = image::load_from_memory(&encoded.thumbnail).unwrap();
        assert_eq!(small.dimensions(), (96, 64));
        assert_eq!(
            image::guess_format(&encoded.image).unwrap(),
            ImageFormat::Jpeg
        );
    }

    #[test]
    fn small_images_are_not_enlarged_and_transparency_is_white() {
        let encoded = encode(&png(50, 50, true)).unwrap();
        let large = image::load_from_memory(&encoded.image).unwrap().to_rgb8();
        assert_eq!(large.dimensions(), (50, 50));
        assert!(
            large
                .get_pixel(25, 25)
                .0
                .iter()
                .all(|&channel| channel > 240)
        );
    }

    #[test]
    fn other_files_are_refused() {
        assert!(matches!(
            encode(b"GIF89a\x01\x00\x01\x00"),
            Err(CoverError::Unsupported)
        ));
        assert!(matches!(
            encode(b"\xff\xd8\xff\xe0 not really"),
            Err(CoverError::Decode(_))
        ));
        assert!(matches!(
            encode(&vec![0; MAX_BYTES + 1]),
            Err(CoverError::TooLarge)
        ));
    }

    #[test]
    fn hashes_read_back_from_hex() {
        let hash = hash(b"cover");
        assert_eq!(parse_hex(&hex(&hash)), Some(hash));
        assert_eq!(parse_hex("zz"), None);
        assert_eq!(parse_hex(&"g".repeat(64)), None);
    }

    async fn add_track(pool: &MySqlPool, path: &str, album: Option<&str>) -> u64 {
        sqlx::query("INSERT INTO tracks (path, artist, album) VALUES (?, 'Artist', ?)")
            .bind(path)
            .bind(album)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_id()
    }

    async fn shown(pool: &MySqlPool, track_id: u64) -> Option<(Hash, String)> {
        let row: Option<(Vec<u8>, String)> = sqlx::query_as(
            "SELECT COALESCE(tc.cover_hash, ac.cover_hash),
                    CAST(COALESCE(tc.source, ac.source) AS CHAR)
             FROM tracks t
             LEFT JOIN track_covers tc ON tc.track_id = t.id
             LEFT JOIN album_covers ac ON ac.album_key = t.album_key
             WHERE t.id = ? AND COALESCE(tc.cover_hash, ac.cover_hash) IS NOT NULL",
        )
        .bind(track_id)
        .fetch_optional(pool)
        .await
        .unwrap();
        row.map(|(hash, source)| (hash.try_into().unwrap(), source))
    }

    #[tokio::test]
    async fn covers_follow_their_sources() {
        let Some(db) = heron_service::db::ScratchDatabase::create().await else {
            return;
        };
        let pool = &db.pool;
        let dir = tempfile::tempdir().unwrap();
        let covers_dir = tempfile::tempdir().unwrap();
        let files = Files::new(covers_dir.path());
        let file = dir.path().join("01.mp3");
        let (red, green, blue) = (png(10, 10, false), png(20, 10, false), png(10, 20, false));
        let first = add_track(pool, "a/01.mp3", Some("Album")).await;
        let second = add_track(pool, "a/02.mp3", Some("album ")).await;
        let loose = add_track(pool, "loose.mp3", None).await;
        let mut connection = pool.acquire().await.unwrap();

        // An image beside the file, then a cover embedded in it, which is better.
        std::fs::write(dir.path().join("cover.jpg"), &red).unwrap();
        from_file(&mut connection, &files, first, Some(&file), None)
            .await
            .unwrap();
        assert_eq!(shown(pool, second).await, Some((hash(&red), "file".into())));
        from_file(&mut connection, &files, first, Some(&file), Some(&green))
            .await
            .unwrap();
        assert_eq!(
            shown(pool, second).await,
            Some((hash(&green), "embedded".into()))
        );
        // Another embedded cover in the same album: the track's own.
        from_file(&mut connection, &files, second, Some(&file), Some(&blue))
            .await
            .unwrap();
        assert_eq!(
            shown(pool, first).await,
            Some((hash(&green), "embedded".into()))
        );
        assert_eq!(
            shown(pool, second).await,
            Some((hash(&blue), "embedded".into()))
        );
        // Gone from the file: the album's again.
        from_file(&mut connection, &files, second, Some(&file), None)
            .await
            .unwrap();
        assert_eq!(
            shown(pool, second).await,
            Some((hash(&green), "embedded".into()))
        );

        // Set by hand for the album: it wins, over what the files have.
        from_file(&mut connection, &files, second, Some(&file), Some(&blue))
            .await
            .unwrap();
        let key = album_key(pool, first).await.unwrap().unwrap();
        let by_hand = Link {
            hash: store(&mut connection, &files, red.clone()).await.unwrap(),
            source: Source::Hand,
            detail: None,
        };
        set_album(&mut connection, &key, &by_hand, None)
            .await
            .unwrap();
        assert_eq!(shown(pool, second).await, Some((hash(&red), "hand".into())));
        from_file(&mut connection, &files, first, Some(&file), Some(&green))
            .await
            .unwrap();
        from_file(&mut connection, &files, second, Some(&file), Some(&blue))
            .await
            .unwrap();
        assert_eq!(shown(pool, first).await, Some((hash(&red), "hand".into())));
        assert_eq!(shown(pool, second).await, Some((hash(&red), "hand".into())));

        // A track without an album has its own.
        from_file(&mut connection, &files, loose, None, Some(&blue))
            .await
            .unwrap();
        assert_eq!(
            shown(pool, loose).await,
            Some((hash(&blue), "embedded".into()))
        );

        // Each image once; nothing pruned while in use.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covers")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(count, 3);
        sqlx::query("UPDATE covers SET created_at = NOW(6) - INTERVAL 2 HOUR")
            .execute(pool)
            .await
            .unwrap();
        drop(connection);
        assert!(files.has(&hash(&green)).await);
        assert_eq!(
            prune(pool, &files).await.unwrap(),
            1,
            "green: nothing uses it"
        );
        assert!(!files.has(&hash(&green)).await, "its files went with it");
        assert!(files.has(&hash(&red)).await);

        // Files lost: storing the image again writes them back.
        files.remove(&hash(&red)).await.unwrap();
        let mut connection = pool.acquire().await.unwrap();
        store(&mut connection, &files, red.clone()).await.unwrap();
        assert!(files.has(&hash(&red)).await);
        drop(connection);
        db.drop().await;
    }

    #[tokio::test]
    async fn files_are_written_whole_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path().join("covers"));
        files.check().unwrap();
        let encoded = encode(&png(1200, 800, false)).unwrap();
        let hash = encoded.hash;
        assert_eq!(files.read(&hash, LARGE).await.unwrap(), None);
        assert!(!files.has(&hash).await);
        files.write_encoded(&encoded).await.unwrap();
        assert!(files.has(&hash).await);
        let large = files.path(&hash, LARGE);
        assert_eq!(
            large.strip_prefix(dir.path()).unwrap(),
            Path::new("covers")
                .join(&hex(&hash)[..2])
                .join(format!("{}-600.jpg", hex(&hash)))
        );
        assert_eq!(
            files.read(&hash, LARGE).await.unwrap(),
            Some(encoded.image.clone())
        );
        assert_eq!(
            files.read(&hash, SMALL).await.unwrap(),
            Some(encoded.thumbnail.clone())
        );
        // No temporary file left behind.
        let names: Vec<_> = std::fs::read_dir(large.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        files.remove(&hash).await.unwrap();
        files.remove(&hash).await.unwrap();
        assert!(!files.has(&hash).await);
    }

    #[test]
    fn images_beside_files_are_found_by_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(beside(dir.path()).is_none());
        std::fs::write(dir.path().join("Front.PNG"), b"front").unwrap();
        std::fs::write(dir.path().join("back.jpg"), b"back").unwrap();
        assert_eq!(
            beside(dir.path()),
            Some(("Front.PNG".to_owned(), b"front".to_vec()))
        );
        std::fs::write(dir.path().join("cover.jpg"), b"cover").unwrap();
        assert_eq!(beside(dir.path()).unwrap().0, "cover.jpg");
    }
}
