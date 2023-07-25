use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash},
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use scc::hash_map::{Entry, HashMap};

pub struct RateLimiter<K: Eq + Hash, H: BuildHasher = std::collections::hash_map::RandomState> {
    start: Instant,
    limits: HashMap<K, Gcra, H>,
}

impl<K: Eq + Hash, H: BuildHasher> RateLimiter<K, H> {
    fn relative(&self, ts: Instant) -> u64 {
        ts.saturating_duration_since(self.start).as_nanos() as u64
    }

    pub async fn clean(&self, before: Instant) {
        let before = self.relative(before);
        self.limits
            .retain_async(move |_, v| *AtomicU64::get_mut(&mut v.0) >= before)
            .await;
    }

    pub async fn req(&self, key: K, quota: Quota, now: Instant) -> Result<(), RateLimitError> {
        let now = self.relative(now);

        let Some(res) = self.limits.read_async(&key, |_, gcra| gcra.req(quota, now)).await else {
            return match self.limits.entry_async(key).await {
                Entry::Occupied(gcra) => gcra.get().req(quota, now),
                Entry::Vacant(gcra) => {
                    gcra.insert_entry(Gcra::first(quota, now));
                    Ok(())
                }
            };
        };

        res
    }
}

impl<K: Eq + Hash, H: BuildHasher> Default for RateLimiter<K, H>
where
    H: Default,
{
    fn default() -> Self {
        RateLimiter {
            start: Instant::now(),
            limits: HashMap::default(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct RateLimitError(NonZeroU64);

impl RateLimitError {
    pub const fn as_duration(self) -> Duration {
        Duration::from_nanos(self.0.get())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Quota {
    tau: u64,
    t: u64,
}

impl Quota {
    /// Constructs a new quota with the given number of burst requests and
    /// an `emission_interval` parameter, which is the amount of time it takes
    /// for the limit to release.
    ///
    /// For example, 100reqs/second would have an emission_interval of 10ms
    ///
    /// Burst requests ignore the individual emission interval in favor of
    /// delivering all at once or in quick succession, up until the provided limit.
    #[rustfmt::skip]
    pub const fn new(emission_interval: Duration, burst: NonZeroU64) -> Quota {
        let t = emission_interval.as_nanos() as u64;
        Quota { t, tau: t * burst.get() }
    }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct Gcra(AtomicU64);

impl Gcra {
    #[inline]
    pub const fn first(Quota { t, .. }: Quota, now: u64) -> Gcra {
        // Equivalent to `Gcra(now + t).req()` to calculate the first request
        Gcra(AtomicU64::new(now + t + t))
    }

    fn decide(prev: u64, now: u64, Quota { tau, t }: Quota) -> Result<u64, RateLimitError> {
        let next = prev.saturating_sub(tau);
        if now < next {
            // SAFETY: next > now, so next - now is non-zero
            Err(RateLimitError(unsafe { NonZeroU64::new_unchecked(next - now) }))
        } else {
            Ok(now.max(prev) + t)
        }
    }

    #[rustfmt::skip]
    pub fn req(&self, quota: Quota, now: u64) -> Result<(), RateLimitError> {
        let mut prev = self.0.load(Ordering::Acquire);
        let mut decision = Self::decide(prev, now, quota);

        while let Ok(next) = decision {
            match self.0.compare_exchange_weak(prev, next, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(next_prev) => prev = next_prev,
            }

            decision = Self::decide(prev, now, quota);
        }

        decision.map(|_| ())
    }
}
