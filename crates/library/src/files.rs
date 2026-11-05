// SPDX-License-Identifier: AGPL-3.0-or-later

//! Which files are audio, and how a file's content is recognized.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Extensions picked up in the library and the incoming folder, compared
/// case-insensitively.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "mp4", "aac", "flac", "ogg", "oga", "opus", "wav", "aif", "aiff",
];

/// Bytes read from each end of a file for its [`content_hash`].
const CONTENT_HASH_CHUNK: u64 = 64 * 1024;

/// Hidden files and directories (including macOS `._` resource forks) are
/// never imported. Heron's own temporary files are hidden too.
pub fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with('.'))
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            AUDIO_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// Identifies a file's content cheaply, to recognize it under a new path:
/// BLAKE3 of the size and the first and last 64 KiB. Reading whole files
/// would double the I/O of an import, which hurts on network shares.
///
/// Tags usually sit in the first or last bytes, so re-tagging a file
/// changes its hash.
pub fn content_hash(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut buffer = Vec::new();
    file.by_ref()
        .take(CONTENT_HASH_CHUNK)
        .read_to_end(&mut buffer)?;
    if size > 2 * CONTENT_HASH_CHUNK {
        file.seek(SeekFrom::End(-(CONTENT_HASH_CHUNK as i64)))?;
    }
    file.read_to_end(&mut buffer)?;
    hasher.update(&buffer);
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_audio_extensions_in_any_case() {
        assert!(is_audio(Path::new("a/b.MP3")));
        assert!(is_audio(Path::new("a/b.m4a")));
        assert!(!is_audio(Path::new("a/b.jpg")));
        assert!(!is_audio(Path::new("a/mp3")));
    }

    #[test]
    fn content_hash_depends_on_both_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        let mut bytes = vec![0_u8; 300 * 1024];
        std::fs::write(&path, &bytes).unwrap();
        let before = content_hash(&path).unwrap();
        *bytes.last_mut().unwrap() = 1;
        std::fs::write(&path, &bytes).unwrap();
        assert_ne!(content_hash(&path).unwrap(), before);
    }
}
