// SPDX-License-Identifier: AGPL-3.0-or-later

//! Counters with a lifetime, for the limits per address, per device and
//! per second, and the event streams open per address.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Keys kept at most. A flood from more keys than this is refused rather
/// than allowed to fill the memory.
const MAX_KEYS: usize = 200_000;

/// Counts per key until each count expires.
pub struct Counters<K> {
    entries: Mutex<HashMap<K, (u32, Instant)>>,
}

impl<K: Eq + Hash + Clone> Default for Counters<K> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash + Clone> Counters<K> {
    /// Counts one for `key` if it has less than `limit` (its count lasting
    /// `lifetime` from its first); otherwise how long until it may again.
    pub fn hit(
        &self,
        key: &K,
        limit: u32,
        now: Instant,
        lifetime: Duration,
    ) -> Result<(), Duration> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if !entries.contains_key(key) && entries.len() >= MAX_KEYS {
            entries.retain(|_, (_, expires)| *expires > now);
            if entries.len() >= MAX_KEYS {
                return Err(Duration::from_secs(1));
            }
        }
        let entry = entries.entry(key.clone()).or_insert((0, now + lifetime));
        if entry.1 <= now {
            *entry = (0, now + lifetime);
        }
        if entry.0 >= limit {
            return Err(entry.1.saturating_duration_since(now));
        }
        entry.0 += 1;
        Ok(())
    }

    /// Forgets the counts that expired.
    pub fn prune(&self, now: Instant) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, (_, expires)| *expires > now);
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Requests per address per minute.
pub struct PerMinute {
    limit: u32,
    counters: Counters<IpAddr>,
}

impl PerMinute {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            counters: Counters::default(),
        }
    }

    pub fn hit(&self, address: IpAddr, now: Instant) -> Result<(), Duration> {
        self.counters
            .hit(&address, self.limit, now, Duration::from_secs(60))
    }

    pub fn prune(&self, now: Instant) {
        self.counters.prune(now);
    }
}

/// The event streams open, per address and in all.
pub struct Streams {
    per_address: u32,
    max: usize,
    open: Arc<Mutex<HashMap<IpAddr, u32>>>,
    total: Arc<AtomicUsize>,
}

/// One open stream: dropping it closes it.
pub struct StreamSlot {
    address: IpAddr,
    open: Arc<Mutex<HashMap<IpAddr, u32>>>,
    total: Arc<AtomicUsize>,
}

impl Streams {
    pub fn new(per_address: u32, max: usize) -> Self {
        Self {
            per_address,
            max,
            open: Arc::default(),
            total: Arc::default(),
        }
    }

    /// A slot for a new stream, or `None` over either limit.
    pub fn open(&self, address: IpAddr) -> Option<StreamSlot> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if self.total.load(Ordering::Relaxed) >= self.max {
            return None;
        }
        let count = open.entry(address).or_insert(0);
        if *count >= self.per_address {
            return None;
        }
        *count += 1;
        let total = self.total.fetch_add(1, Ordering::Relaxed) + 1;
        metrics::gauge!("heron_public_streams").set(total as f64);
        Some(StreamSlot {
            address,
            open: Arc::clone(&self.open),
            total: Arc::clone(&self.total),
        })
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.address) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.address);
            }
        }
        let total = self.total.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        metrics::gauge!("heron_public_streams").set(total as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(last: u8) -> IpAddr {
        IpAddr::from([192, 0, 2, last])
    }

    #[test]
    fn an_address_is_limited_per_minute_and_others_are_not() {
        let limit = PerMinute::new(3);
        let now = Instant::now();
        for _ in 0..3 {
            assert!(limit.hit(address(1), now).is_ok());
        }
        let wait = limit.hit(address(1), now).unwrap_err();
        assert_eq!(wait, Duration::from_secs(60));
        assert!(limit.hit(address(2), now).is_ok(), "another address");
        let later = now + Duration::from_secs(30);
        assert_eq!(
            limit.hit(address(1), later).unwrap_err(),
            Duration::from_secs(30)
        );
        // The minute is over: counted again from zero.
        assert!(limit.hit(address(1), now + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn expired_counts_are_forgotten() {
        let counters = Counters::default();
        let now = Instant::now();
        counters
            .hit(&"device", 1, now, Duration::from_secs(10))
            .unwrap();
        assert!(
            counters
                .hit(&"device", 1, now, Duration::from_secs(10))
                .is_err()
        );
        counters.prune(now + Duration::from_secs(5));
        assert_eq!(counters.len(), 1);
        counters.prune(now + Duration::from_secs(10));
        assert!(counters.is_empty());
    }

    #[test]
    fn a_flood_of_keys_does_not_fill_the_memory() {
        let counters = Counters::default();
        let now = Instant::now();
        for key in 0..MAX_KEYS {
            counters.hit(&key, 1, now, Duration::from_secs(60)).unwrap();
        }
        assert!(
            counters
                .hit(&MAX_KEYS, 1, now, Duration::from_secs(60))
                .is_err()
        );
        // Once the old ones expire, there is room again.
        let later = now + Duration::from_secs(61);
        assert!(
            counters
                .hit(&MAX_KEYS, 1, later, Duration::from_secs(60))
                .is_ok()
        );
        assert_eq!(counters.len(), 1);
    }

    #[test]
    fn streams_are_limited_per_address_and_in_all() {
        let streams = Streams::new(2, 3);
        let a = streams.open(address(1)).unwrap();
        let _b = streams.open(address(1)).unwrap();
        assert!(streams.open(address(1)).is_none(), "two per address");
        let _c = streams.open(address(2)).unwrap();
        assert!(streams.open(address(3)).is_none(), "three in all");
        drop(a);
        assert_eq!(streams.total(), 2);
        assert!(
            streams.open(address(1)).is_some(),
            "a closed stream frees its slot"
        );
    }
}
