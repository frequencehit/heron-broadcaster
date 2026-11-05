// SPDX-License-Identifier: AGPL-3.0-or-later

//! Authenticator app codes: TOTP (RFC 6238) over HOTP (RFC 4226), with
//! HMAC-SHA-1, 6 digits and 30-second steps, as every app expects.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

/// Bytes of a new secret (160 bits, as RFC 4226 recommends).
pub const SECRET_LEN: usize = 20;
pub const STEP_SECONDS: u64 = 30;
const DIGITS: u32 = 6;
/// Steps of clock drift accepted either side.
const DRIFT: u64 = 1;

/// The HOTP code of `secret` for a counter.
pub fn code(secret: &[u8], counter: u64) -> u32 {
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret) else {
        return u32::MAX;
    };
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    binary % 10u32.pow(DIGITS)
}

/// The step of a Unix time, in seconds.
pub fn step_at(unix_seconds: u64) -> u64 {
    unix_seconds / STEP_SECONDS
}

/// The step `code` was made for, around `now` (a step), when it is after
/// `last_step`: each step is accepted once.
pub fn verify(secret: &[u8], code: &str, now: u64, last_step: Option<u64>) -> Option<u64> {
    let code = code.trim().replace(' ', "");
    if code.len() != DIGITS as usize || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let wanted: u32 = code.parse().ok()?;
    (now.saturating_sub(DRIFT)..=now + DRIFT)
        .filter(|step| last_step.is_none_or(|last| *step > last))
        .find(|step| self::code(secret, *step) == wanted)
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, without padding: how apps take secrets.
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The `otpauth://` URI apps read from the QR code.
pub fn uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
    let issuer = utf8_percent_encode(issuer, NON_ALPHANUMERIC);
    let account = utf8_percent_encode(account, NON_ALPHANUMERIC);
    format!(
        "otpauth://totp/{issuer}:{account}?secret={}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        base32(secret)
    )
}

/// The URI as a QR code, in SVG.
pub fn qr_svg(uri: &str) -> Option<String> {
    let code = qrcode::QrCode::new(uri.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238, appendix B (SHA-1, its 20-byte secret), to 6 digits.
    #[test]
    fn matches_the_rfc_test_vectors() {
        let secret = b"12345678901234567890";
        for (time, expected) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_111_111_111, 50_471),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
            (20_000_000_000, 353_130),
        ] {
            assert_eq!(code(secret, step_at(time)), expected, "{time}");
        }
    }

    #[test]
    fn accepts_one_step_either_side_and_each_step_once() {
        let secret = b"12345678901234567890";
        let now = step_at(1_111_111_111);
        let current = format!("{:06}", code(secret, now));
        let previous = format!("{:06}", code(secret, now - 1));
        let next = format!("{:06}", code(secret, now + 1));
        let too_old = format!("{:06}", code(secret, now - 2));
        assert_eq!(verify(secret, &current, now, None), Some(now));
        assert_eq!(verify(secret, &previous, now, None), Some(now - 1));
        assert_eq!(verify(secret, &next, now, None), Some(now + 1));
        assert_eq!(verify(secret, &too_old, now, None), None);
        // Used already, or older than the last one used.
        assert_eq!(verify(secret, &current, now, Some(now)), None);
        assert_eq!(verify(secret, &previous, now, Some(now)), None);
        assert_eq!(verify(secret, &next, now, Some(now)), Some(now + 1));
        assert_eq!(verify(secret, "12345", now, None), None);
        assert_eq!(verify(secret, "abcdef", now, None), None);
        let spaced = format!("{} {}", &current[..3], &current[3..]);
        assert_eq!(verify(secret, &spaced, now, None), Some(now));
    }

    #[test]
    fn encodes_secrets_in_base32() {
        // RFC 4648, section 10, without padding.
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"fo"), "MZXQ");
        assert_eq!(base32(b"foo"), "MZXW6");
        assert_eq!(base32(b"foob"), "MZXW6YQ");
        assert_eq!(base32(b"fooba"), "MZXW6YTB");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(&[0u8; SECRET_LEN]).len(), 32);
    }

    #[test]
    fn writes_the_uri_and_its_qr_code() {
        let uri = uri("Heron", "lea@studio", b"12345678901234567890");
        assert_eq!(
            uri,
            "otpauth://totp/Heron:lea%40studio?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\
             &issuer=Heron&algorithm=SHA1&digits=6&period=30"
        );
        let svg = qr_svg(&uri).unwrap();
        assert!(svg.contains("<svg") && svg.contains("</svg>"));
    }
}
