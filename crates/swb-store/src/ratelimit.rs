//! Per-sender send rate limit (ADR-0003, SWB-R66).
//!
//! One token bucket per sender agent id. A bucket holds at most `burst`
//! sends and refills at `refill_per_minute`. A send that finds its bucket
//! empty is refused with `rate_limited` (HTTP 429). Buckets live in memory
//! only: a broker restart refills every bucket, and nothing here touches the
//! store's tables. Time comes from the store's injected clock, in whole
//! seconds, so tests drive it with a manual clock.
//!
//! v0.5.1 (TIN-7577): the limiter counts refusals by the sender's harness
//! kind, never by agent id, and `/metrics` reports them with the limit
//! settings and the number of buckets held.
//!
//! The bucket is a guard against a looping or flooding client that keeps
//! one agent id. An unbound id is self-asserted, so a client that changes
//! its `from` on every send gets a fresh bucket each time; the tailnet ACL,
//! not this limit, bounds such a client (SECURITY.md). The map holds at most
//! [`MAX_BUCKETS`] buckets, so rotating ids cannot grow broker memory.

use std::collections::HashMap;

/// Default bucket size: a sender may send this many messages at once.
pub const SEND_BURST_DEFAULT: u32 = 20;
/// Default refill: one send every 2 seconds, sustained.
pub const SEND_REFILL_PER_MINUTE_DEFAULT: u32 = 30;
/// The most buckets the limiter keeps. A new sender that finds the map full
/// first drops every bucket that has refilled to full, which is the same as
/// never having sent. If none has, it drops the fullest one, so a client
/// that exhausted its own bucket keeps it while ids rotate around it.
pub(crate) const MAX_BUCKETS: usize = 4096;

/// The limit's settings. `burst == 0` turns the limit off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendLimit {
    pub burst: u32,
    pub refill_per_minute: u32,
}

impl Default for SendLimit {
    fn default() -> Self {
        Self {
            burst: SEND_BURST_DEFAULT,
            refill_per_minute: SEND_REFILL_PER_MINUTE_DEFAULT,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: i64,
}

#[derive(Debug, Default)]
pub(crate) struct Limiter {
    pub(crate) limit: SendLimit,
    buckets: HashMap<String, Bucket>,
    /// Refused sends since broker start, by the sender's harness kind in
    /// [`crate::HARNESS_KINDS`] order, never by agent id (TIN-7577).
    pub(crate) refused: [u64; crate::HARNESS_KINDS.len()],
}

fn refilled(limit: SendLimit, bucket: Bucket, now: i64) -> f64 {
    let elapsed = (now - bucket.at).max(0) as f64;
    let per_second = f64::from(limit.refill_per_minute) / 60.0;
    (bucket.tokens + elapsed * per_second).min(f64::from(limit.burst))
}

impl Limiter {
    fn refilled(&self, bucket: Bucket, now: i64) -> f64 {
        refilled(self.limit, bucket, now)
    }

    /// How many buckets the limiter holds now, at most [`MAX_BUCKETS`].
    pub(crate) fn buckets_held(&self) -> usize {
        self.buckets.len()
    }

    /// Make room for one new bucket when the map is at [`MAX_BUCKETS`].
    fn make_room(&mut self, now: i64) {
        if self.buckets.len() < MAX_BUCKETS {
            return;
        }
        let limit = self.limit;
        let full = f64::from(limit.burst);
        self.buckets
            .retain(|_, bucket| refilled(limit, *bucket, now) < full);
        if self.buckets.len() < MAX_BUCKETS {
            return;
        }
        let fullest = self
            .buckets
            .iter()
            .max_by(|a, b| refilled(limit, *a.1, now).total_cmp(&refilled(limit, *b.1, now)))
            .map(|(key, _)| key.clone());
        if let Some(key) = fullest {
            self.buckets.remove(&key);
        }
    }

    /// Take one send from `sender`'s bucket. `Err(seconds)` is how long
    /// until the bucket holds one again (at least 1).
    pub(crate) fn admit(&mut self, sender: &str, now: i64) -> Result<(), u64> {
        if self.limit.burst == 0 {
            return Ok(());
        }
        let current = self
            .buckets
            .get(sender)
            .map_or(f64::from(self.limit.burst), |bucket| {
                self.refilled(*bucket, now)
            });
        if current >= 1.0 {
            if !self.buckets.contains_key(sender) {
                self.make_room(now);
            }
            self.buckets.insert(
                sender.to_owned(),
                Bucket {
                    tokens: current - 1.0,
                    at: now,
                },
            );
            return Ok(());
        }
        if self.limit.refill_per_minute == 0 {
            return Err(u64::MAX);
        }
        let per_second = f64::from(self.limit.refill_per_minute) / 60.0;
        Err(((1.0 - current) / per_second).ceil().max(1.0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(burst: u32, refill_per_minute: u32) -> Limiter {
        Limiter {
            limit: SendLimit {
                burst,
                refill_per_minute,
            },
            ..Limiter::default()
        }
    }

    #[test]
    fn burst_then_refill_at_the_configured_rate() {
        let mut l = limiter(3, 30);
        for _ in 0..3 {
            assert_eq!(l.admit("a", 100), Ok(()));
        }
        assert_eq!(l.admit("a", 100), Err(2));
        // Another sender has its own bucket.
        assert_eq!(l.admit("b", 100), Ok(()));
        assert_eq!(l.admit("a", 101), Err(1));
        assert_eq!(l.admit("a", 102), Ok(()));
        assert_eq!(l.admit("a", 102), Err(2));
        // A long pause refills to the burst, never past it.
        for _ in 0..3 {
            assert_eq!(l.admit("a", 10_000), Ok(()));
        }
        assert!(l.admit("a", 10_000).is_err());
    }

    #[test]
    fn zero_burst_turns_the_limit_off() {
        let mut l = limiter(0, 0);
        for _ in 0..1000 {
            assert_eq!(l.admit("a", 1), Ok(()));
        }
        assert!(l.buckets.is_empty());
    }

    #[test]
    fn full_buckets_are_swept_when_the_map_is_full() {
        let mut l = limiter(2, 60);
        for n in 0..MAX_BUCKETS {
            l.admit(&format!("s{n}"), 0).unwrap();
        }
        assert_eq!(l.buckets.len(), MAX_BUCKETS);
        // Two seconds later every bucket is full again and is dropped.
        l.admit("late", 2).unwrap();
        assert_eq!(l.buckets.len(), 1);
    }

    #[test]
    fn rotating_ids_never_grow_the_map_or_free_a_looping_sender() {
        let mut l = limiter(2, 1);
        // A looping client with one id empties its bucket.
        assert_eq!(l.admit("loop", 0), Ok(()));
        assert_eq!(l.admit("loop", 0), Ok(()));
        assert!(l.admit("loop", 0).is_err());
        // A flood that changes its id on every send never refills a bucket
        // within the second, so the map stays at the cap by dropping the
        // fullest bucket, never the exhausted one.
        for n in 0..(MAX_BUCKETS + 64) {
            l.admit(&format!("flood{n}"), 0).unwrap();
            assert!(l.buckets.len() <= MAX_BUCKETS);
        }
        assert_eq!(l.buckets.len(), MAX_BUCKETS);
        assert!(l.admit("loop", 0).is_err());
    }
}
