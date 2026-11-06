// SPDX-License-Identifier: AGPL-3.0-or-later

//! The play ratio: an exact quota of plays (`docs/rotation.md#play-ratio`).
//!
//! A track's ratio is in tenths, 1 to 10. Each station keeps a counter per
//! track, from 10 down to 1, then back to 10. Each time rotation offers the
//! track (it passes the restrictions and the separation rules), the counter
//! goes down one step; the track plays if the counter was in its first
//! `ratio` steps: at 2, it plays at 10 and 9 and is held back from 8 to 1.

use serde::{Deserialize, Serialize};

use crate::TrackId;

/// A full ratio, and the counter's first step.
pub const FULL: u8 = 10;

/// A track's ratio on a station: the station's own (`station`, 0 when it
/// disabled the track), else the track's `default`. `None`: the track does
/// not play on that station.
pub fn on_station(default: u8, station: Option<u8>) -> Option<u8> {
    match station {
        Some(0) => None,
        Some(ratio) => Some(ratio.min(FULL)),
        None => Some(default),
    }
}

/// Whether a track of `ratio` tenths plays with its counter at `counter`.
pub fn plays(ratio: u8, counter: u8) -> bool {
    counter > FULL - ratio.min(FULL)
}

/// The counter after the track was offered at `counter`.
pub fn after(counter: u8) -> u8 {
    if (2..=FULL).contains(&counter) {
        counter - 1
    } else {
        FULL
    }
}

/// A track rotation offered and its ratio held back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HeldByRatio {
    pub track_id: TrackId,
    /// In tenths, 1 to 9.
    pub ratio: u8,
    /// Where its counter was, 10 down to 1: it plays again in this many
    /// offers.
    pub counter: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_in_the_first_steps_of_the_counter() {
        let played: Vec<u8> = (1..=FULL).rev().filter(|c| plays(2, *c)).collect();
        assert_eq!(played, [10, 9]);
        assert!((1..=FULL).all(|c| plays(FULL, c)));
        let played: Vec<u8> = (1..=FULL).rev().filter(|c| plays(1, *c)).collect();
        assert_eq!(played, [10]);
    }

    #[test]
    fn a_station_ratio_replaces_the_default() {
        assert_eq!(on_station(10, None), Some(10));
        assert_eq!(on_station(2, None), Some(2));
        assert_eq!(on_station(10, Some(3)), Some(3));
        assert_eq!(on_station(3, Some(10)), Some(10));
        // 0: disabled on that station.
        assert_eq!(on_station(10, Some(0)), None);
        assert_eq!(on_station(42, Some(42)), Some(FULL));
    }

    #[test]
    fn the_counter_goes_down_then_back_to_ten() {
        let mut counter = FULL;
        let mut seen = Vec::new();
        for _ in 0..11 {
            seen.push(counter);
            counter = after(counter);
        }
        assert_eq!(seen, [10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 10]);
        // Out of range: starts over.
        assert_eq!(after(0), FULL);
        assert_eq!(after(42), FULL);
    }
}
