// SPDX-License-Identifier: AGPL-3.0-or-later

//! Backup encryption with age (docs/backups.md#encryption): X25519
//! recipients. Heron keeps the public key only; the secret key is given for
//! a check or a restore, used, then dropped.

use std::io::{self, BufRead, Read, Write};
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};

use crate::error::{ApiError, ApiResult};

/// How every age file starts.
const MAGIC: &[u8] = b"age-encryption.org/v1\n";

/// Whether `bytes` start an age file.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// A new key: the secret key, shown once, and its public key.
pub fn generate() -> (String, String) {
    let identity = Identity::generate();
    let secret = identity.to_string().expose_secret().to_owned();
    (secret, identity.to_public().to_string())
}

/// The contents of `heron-backup-key.txt`: an age identity file, which
/// `age -i` reads.
pub fn key_file(secret: &str, recipient: &str, created_at: &str) -> String {
    format!("# Heron backup key\n# created: {created_at}\n# public key: {recipient}\n{secret}\n")
}

/// A public key, `age1…`.
pub fn parse_recipient(text: &str) -> ApiResult<Recipient> {
    Recipient::from_str(text.trim())
        .map_err(|_| ApiError::Invalid("this is not an age public key (age1…)".to_owned()))
}

/// A secret key, `AGE-SECRET-KEY-1…`, alone or in an identity file
/// (comments and blank lines around it).
pub fn parse_identity(text: &str) -> ApiResult<Identity> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default();
    // Password managers and copies may change the letter case.
    Identity::from_str(&line.to_ascii_uppercase())
        .map_err(|_| ApiError::Invalid("this is not a backup key (AGE-SECRET-KEY-1…)".to_owned()))
}

/// The public key of a secret key.
pub fn recipient_of(identity: &Identity) -> String {
    identity.to_public().to_string()
}

/// Writes what is written to it encrypted to `output`; `finish` ends the
/// file.
pub fn encrypt_to<W: Write>(
    recipient: &Recipient,
    output: W,
) -> io::Result<age::stream::StreamWriter<W>> {
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
            .map_err(io::Error::other)?;
    encryptor.wrap_output(output)
}

/// Encrypts `bytes` whole.
pub fn encrypt(recipient: &Recipient, bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut writer = encrypt_to(recipient, Vec::with_capacity(bytes.len() + 256))?;
    writer.write_all(bytes)?;
    writer.finish()
}

/// Why an age file could not be read, in words.
fn decrypt_error(error: &age::DecryptError) -> ApiError {
    match error {
        age::DecryptError::NoMatchingKeys => ApiError::Invalid(
            "this key does not open this backup: it was encrypted with another".to_owned(),
        ),
        age::DecryptError::Io(error) => {
            ApiError::Invalid(format!("cannot read the backup: {error}"))
        }
        _ => ApiError::Invalid("this backup is damaged: it cannot be decrypted".to_owned()),
    }
}

/// Reads an age file with `identity`: the plaintext as it is read. A
/// changed or cut file fails while it is read.
pub fn decrypt_from<'a, R: BufRead + 'a>(
    identity: &Identity,
    input: R,
) -> ApiResult<impl Read + 'a> {
    let decryptor = age::Decryptor::new_buffered(input).map_err(|error| decrypt_error(&error))?;
    decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|error| decrypt_error(&error))
}

/// Decrypts `bytes` whole.
pub fn decrypt(identity: &Identity, bytes: &[u8]) -> ApiResult<Vec<u8>> {
    let mut reader = decrypt_from(identity, bytes)?;
    let mut plain = Vec::new();
    reader.read_to_end(&mut plain).map_err(|error| {
        ApiError::Invalid(format!(
            "this backup is damaged: it cannot be decrypted ({error})"
        ))
    })?;
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_key_opens_what_its_public_key_encrypts() {
        let (secret, public) = generate();
        // Never printed, even by a failing test.
        assert!(secret.starts_with("AGE-SECRET-KEY-1"));
        assert_eq!(secret.len(), 74);
        assert!(public.starts_with("age1"), "{public}");
        let identity = parse_identity(&secret).unwrap();
        assert_eq!(recipient_of(&identity), public);
        let encrypted = encrypt(&parse_recipient(&public).unwrap(), b"{\"format\": 1}").unwrap();
        assert!(is_encrypted(&encrypted));
        assert!(!is_encrypted(b"{\"format\": 1}"));
        assert_eq!(decrypt(&identity, &encrypted).unwrap(), b"{\"format\": 1}");
    }

    #[test]
    fn a_key_file_and_a_key_in_lower_case_are_read() {
        let (secret, public) = generate();
        let file = key_file(&secret, &public, "2026-11-03T03:30:00Z");
        assert_eq!(recipient_of(&parse_identity(&file).unwrap()), public);
        let lower = format!("  {}\n", secret.to_lowercase());
        assert_eq!(recipient_of(&parse_identity(&lower).unwrap()), public);
        assert!(parse_identity("").is_err());
        assert!(parse_identity(&public).is_err());
        assert!(parse_recipient(&secret).is_err());
    }

    #[test]
    fn another_key_or_a_changed_file_is_refused() {
        let (_, public) = generate();
        let (other, _) = generate();
        let mut encrypted = encrypt(&parse_recipient(&public).unwrap(), &[7; 100_000]).unwrap();
        let message = decrypt(&parse_identity(&other).unwrap(), &encrypted)
            .unwrap_err()
            .to_string();
        assert!(message.contains("another"), "{message}");

        let (secret, public) = generate();
        encrypted = encrypt(&parse_recipient(&public).unwrap(), &[7; 100_000]).unwrap();
        let identity = parse_identity(&secret).unwrap();
        let last = encrypted.len() - 10;
        encrypted[last] ^= 1;
        assert!(decrypt(&identity, &encrypted).is_err(), "changed");
        encrypted.truncate(encrypted.len() / 2);
        assert!(decrypt(&identity, &encrypted).is_err(), "cut short");
    }
}
