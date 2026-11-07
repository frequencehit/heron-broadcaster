// SPDX-License-Identifier: AGPL-3.0-or-later

//! The anti-robot proof of work, and the device's hash.
//!
//! A challenge is `<expires_unix_ms>.<random>.<mac>`, signed with a key the
//! gateway draws when it starts: it keeps nothing until one is used. The
//! browser finds a nonce such that `SHA-256(challenge + ":" + nonce)`
//! starts with `bits` zero bits, and sends `X-Heron-Proof:
//! <challenge>:<nonce>`. Each challenge is accepted once, until it expires.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// How long a challenge can be used.
pub const CHALLENGE_MS: i64 = 5 * 60 * 1_000;
/// The longest nonce read.
const MAX_NONCE: usize = 64;
/// Challenges remembered as used at most.
const MAX_USED: usize = 500_000;

/// A challenge for the browser.
#[derive(Clone, Debug, Serialize)]
pub struct Challenge {
    pub challenge: String,
    /// Zero bits the hash must start with.
    pub bits: u8,
    pub expires_at_unix_ms: i64,
}

/// Why a proof is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Missing,
    Malformed,
    Forged,
    Expired,
    TooEasy,
    Used,
    /// Too many challenges in use to remember another.
    Busy,
}

pub struct Proofs {
    key: [u8; 32],
    bits: u8,
    used: Mutex<HashMap<String, i64>>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Proofs {
    /// With a key drawn now. `bits` is capped at 24; 0 turns the proof off.
    pub fn new(bits: u8) -> Result<Self, getrandom::Error> {
        let mut key = [0; 32];
        getrandom::fill(&mut key)?;
        Ok(Self::with_key(key, bits))
    }

    pub fn with_key(key: [u8; 32], bits: u8) -> Self {
        Self {
            key,
            bits: bits.min(24),
            used: Mutex::new(HashMap::new()),
        }
    }

    pub fn bits(&self) -> u8 {
        self.bits
    }

    fn mac(&self, data: &[u8]) -> Hmac<Sha256> {
        // HMAC takes keys of any length.
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key)
            .unwrap_or_else(|_| unreachable!("HMAC takes any key"));
        mac.update(data);
        mac
    }

    pub fn challenge(&self, now_ms: i64) -> Result<Challenge, getrandom::Error> {
        let mut random = [0; 16];
        getrandom::fill(&mut random)?;
        let expires = now_ms + CHALLENGE_MS;
        let body = format!("{expires}.{}", hex(&random));
        let mac = hex(&self.mac(body.as_bytes()).finalize().into_bytes());
        Ok(Challenge {
            challenge: format!("{body}.{mac}"),
            bits: self.bits,
            expires_at_unix_ms: expires,
        })
    }

    /// Checks a proof (`<challenge>:<nonce>`) and marks its challenge used.
    pub fn verify(&self, proof: Option<&str>, now_ms: i64) -> Result<(), Refused> {
        if self.bits == 0 {
            return Ok(());
        }
        let proof = proof.ok_or(Refused::Missing)?;
        let (challenge, nonce) = proof.rsplit_once(':').ok_or(Refused::Malformed)?;
        if nonce.is_empty() || nonce.len() > MAX_NONCE {
            return Err(Refused::Malformed);
        }
        let (body, mac) = challenge.rsplit_once('.').ok_or(Refused::Malformed)?;
        let expires: i64 = body
            .split_once('.')
            .and_then(|(expires, _)| expires.parse().ok())
            .ok_or(Refused::Malformed)?;
        let mac = (0..mac.len())
            .step_by(2)
            .map(|at| {
                mac.get(at..at + 2)
                    .and_then(|byte| u8::from_str_radix(byte, 16).ok())
            })
            .collect::<Option<Vec<u8>>>()
            .ok_or(Refused::Malformed)?;
        // In constant time.
        self.mac(body.as_bytes())
            .verify_slice(&mac)
            .map_err(|_| Refused::Forged)?;
        if expires <= now_ms {
            return Err(Refused::Expired);
        }
        if leading_zero_bits(&Sha256::digest(proof.as_bytes())) < u32::from(self.bits) {
            return Err(Refused::TooEasy);
        }
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        if used.len() >= MAX_USED {
            used.retain(|_, expires| *expires > now_ms);
            if used.len() >= MAX_USED {
                return Err(Refused::Busy);
            }
        }
        if used.insert(challenge.to_owned(), expires).is_some() {
            return Err(Refused::Used);
        }
        Ok(())
    }

    /// Forgets the challenges that expired.
    pub fn prune(&self, now_ms: i64) {
        self.used
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, expires| *expires > now_ms);
    }

    /// The device's hash: a keyed hash of its address and browser, never
    /// stored, so that the address cannot be read back from it.
    pub fn device(&self, address: IpAddr, user_agent: &str, language: &str) -> String {
        let data = format!("device\n{address}\n{user_agent}\n{language}");
        hex(&self.mac(data.as_bytes()).finalize().into_bytes()[..16])
    }
}

fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

/// Finds a nonce for a challenge, as the browser does. For tests and tools.
pub fn solve(challenge: &str, bits: u8) -> String {
    (0u64..)
        .map(|nonce| nonce.to_string())
        .find(|nonce| {
            let hash = Sha256::digest(format!("{challenge}:{nonce}").as_bytes());
            leading_zero_bits(&hash) >= u32::from(bits)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn a_solved_challenge_is_accepted_once() {
        let proofs = Proofs::with_key([7; 32], 8);
        let challenge = proofs.challenge(NOW).unwrap();
        assert_eq!(challenge.bits, 8);
        let nonce = solve(&challenge.challenge, 8);
        let proof = format!("{}:{nonce}", challenge.challenge);
        assert_eq!(proofs.verify(Some(&proof), NOW + 1_000), Ok(()));
        assert_eq!(proofs.verify(Some(&proof), NOW + 2_000), Err(Refused::Used));
    }

    #[test]
    fn wrong_proofs_are_refused() {
        let proofs = Proofs::with_key([7; 32], 8);
        let challenge = proofs.challenge(NOW).unwrap().challenge;
        let nonce = solve(&challenge, 8);
        assert_eq!(proofs.verify(None, NOW), Err(Refused::Missing));
        assert_eq!(
            proofs.verify(Some("nonsense"), NOW),
            Err(Refused::Malformed)
        );
        // Another gateway's (or a restarted one's) challenge.
        let other = Proofs::with_key([8; 32], 8);
        let theirs = other.challenge(NOW).unwrap().challenge;
        let their_nonce = solve(&theirs, 8);
        assert_eq!(
            proofs.verify(Some(&format!("{theirs}:{their_nonce}")), NOW),
            Err(Refused::Forged)
        );
        // A later expiry written in: the signature no longer matches.
        let (_, rest) = challenge.split_once('.').unwrap();
        let forged = format!("{}.{rest}", NOW + 10 * CHALLENGE_MS);
        assert_eq!(
            proofs.verify(Some(&format!("{forged}:{nonce}")), NOW),
            Err(Refused::Forged)
        );
        assert_eq!(
            proofs.verify(Some(&format!("{challenge}:{nonce}")), NOW + CHALLENGE_MS),
            Err(Refused::Expired)
        );
        // A nonce that does not solve it (most do not, at 20 bits).
        let hard = Proofs::with_key([7; 32], 20);
        let challenge = hard.challenge(NOW).unwrap().challenge;
        let unsolved = (0..)
            .map(|n: u32| n.to_string())
            .find(|nonce| {
                let hash = Sha256::digest(format!("{challenge}:{nonce}").as_bytes());
                leading_zero_bits(&hash) < 20
            })
            .unwrap();
        assert_eq!(
            hard.verify(Some(&format!("{challenge}:{unsolved}")), NOW),
            Err(Refused::TooEasy)
        );
    }

    #[test]
    fn no_proof_is_asked_at_zero_bits() {
        let proofs = Proofs::with_key([7; 32], 0);
        assert_eq!(proofs.verify(None, NOW), Ok(()));
    }

    #[test]
    fn devices_are_hashed_and_cannot_be_read_back() {
        let proofs = Proofs::with_key([7; 32], 0);
        let address = IpAddr::from([192, 0, 2, 1]);
        let one = proofs.device(address, "Firefox", "fr");
        assert_eq!(one.len(), 32);
        assert!(!one.contains("192"));
        assert_eq!(one, proofs.device(address, "Firefox", "fr"));
        assert_ne!(one, proofs.device(address, "Chrome", "fr"));
        assert_ne!(
            one,
            Proofs::with_key([9; 32], 0).device(address, "Firefox", "fr")
        );
    }

    #[test]
    fn zero_bits_are_counted() {
        assert_eq!(leading_zero_bits(&[0, 0b0001_0000, 0xff]), 11);
        assert_eq!(leading_zero_bits(&[0xff]), 0);
    }
}
