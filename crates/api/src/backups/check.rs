// SPDX-License-Identifier: AGPL-3.0-or-later

//! What tells a backup is whole (docs/backups.md#testing-backups): the
//! checksums of its files, the last line of the dump, the configuration
//! file read back as a restore reads it.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::file::{self, ConfigFile};
use crate::error::{ApiError, ApiResult};

/// The last line of a whole dump: `-- Dump completed on <time>`.
const DUMP_END: &[u8] = b"-- Dump completed";
/// How much of the end of the dump is kept to find it.
const TAIL: usize = 256;

pub type Checksum = [u8; 32];

pub fn hex(checksum: &Checksum) -> String {
    checksum
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The checksums file: `sha256sum`'s format, which `sha256sum -c` reads.
pub fn checksums_text(files: &[(String, Checksum)]) -> String {
    files
        .iter()
        .map(|(name, checksum)| format!("{}  {name}\n", hex(checksum)))
        .collect()
}

/// The files a checksums file names, with their checksums in hexadecimal.
pub fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (checksum, name) = line.split_once("  ")?;
            (checksum.len() == 64 && checksum.bytes().all(|byte| byte.is_ascii_hexdigit())).then(
                || {
                    (
                        name.trim_start_matches('*').to_owned(),
                        checksum.to_ascii_lowercase(),
                    )
                },
            )
        })
        .collect()
}

/// Hashes what is written through it.
pub struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    pub fn finish(self) -> (W, Checksum) {
        (self.inner, self.hasher.finalize().into())
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Hashes what is read through it.
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    /// The checksum of what was read, once it was read to the end.
    pub fn checksum(&self) -> Checksum {
        self.hasher.clone().finalize().into()
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        Ok(read)
    }
}

/// A file's checksum.
pub fn hash_file(path: &Path) -> io::Result<Checksum> {
    let mut reader = HashingReader::new(std::fs::File::open(path)?);
    io::copy(&mut reader, &mut io::sink())?;
    Ok(reader.checksum())
}

/// Copies a dump from `input` to `output`; whether it ends as a whole dump
/// does.
pub fn copy_dump(input: &mut impl Read, output: &mut impl Write) -> io::Result<bool> {
    let mut tail = Vec::with_capacity(TAIL * 2);
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        tail.extend_from_slice(&buffer[..read]);
        if tail.len() > TAIL {
            tail.drain(..tail.len() - TAIL);
        }
    }
    Ok(tail
        .windows(DUMP_END.len())
        .any(|window| window == DUMP_END))
}

/// Reads a compressed dump to its end: whether it is whole.
pub fn read_dump(compressed: impl Read) -> Result<(), String> {
    let mut decoder = flate2::read::GzDecoder::new(compressed);
    match copy_dump(&mut decoder, &mut io::sink()) {
        Ok(true) => Ok(()),
        Ok(false) => {
            Err("the dump is cut short: it does not end with \"-- Dump completed\"".to_owned())
        }
        Err(error) => Err(format!("the dump cannot be read: {error}")),
    }
}

/// Reads a configuration file back and checks it as a restore would.
pub fn config(bytes: &[u8], schema: i64) -> ApiResult<ConfigFile> {
    let file: ConfigFile = serde_json::from_slice(bytes).map_err(|error| {
        ApiError::Invalid(format!("this is not a Heron configuration file: {error}"))
    })?;
    file::check(&file, schema)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_are_written_as_sha256sum_does() {
        let mut writer = HashingWriter::new(Vec::new());
        writer.write_all(b"abc").unwrap();
        let (bytes, checksum) = writer.finish();
        assert_eq!(bytes, b"abc");
        // sha256sum of "abc"
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(hex(&checksum), expected);
        let text = checksums_text(&[("heron-config-x.json".to_owned(), checksum)]);
        assert_eq!(text, format!("{expected}  heron-config-x.json\n"));
        assert_eq!(
            parse_checksums(&text),
            [("heron-config-x.json".to_owned(), expected.to_owned())]
        );
        assert!(parse_checksums("not a checksum  file\n").is_empty());

        let mut reader = HashingReader::new(&b"abc"[..]);
        io::copy(&mut reader, &mut io::sink()).unwrap();
        assert_eq!(hex(&reader.checksum()), expected);
    }

    fn compressed(text: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(text).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn a_dump_is_whole_when_it_ends_as_mariadb_dump_ends_it() {
        let mut dump = b"-- MariaDB dump\nINSERT INTO t VALUES (1);\n".repeat(10_000);
        let mut copy = Vec::new();
        assert!(!copy_dump(&mut &dump[..], &mut copy).unwrap());
        assert_eq!(copy, dump);
        assert!(
            read_dump(&compressed(&dump)[..])
                .unwrap_err()
                .contains("cut short")
        );

        dump.extend_from_slice(b"\n-- Dump completed on 2026-11-03  3:30:00\n");
        assert!(copy_dump(&mut &dump[..], &mut Vec::new()).unwrap());
        read_dump(&compressed(&dump)[..]).unwrap();
        // Compressed, then cut.
        let whole = compressed(&dump);
        assert!(read_dump(&whole[..whole.len() / 2]).is_err());
    }
}
