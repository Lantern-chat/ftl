use std::{
    borrow::Borrow,
    hash::Hash,
    num::NonZeroU64,
    time::{Duration, Instant},
};

use scc::hash_map::{Entry, HashMap};

pub struct RateLimiter<K: Eq + Hash> {
    start: Instant,
    limits: HashMap<K, Gcra>,
}

impl<K: Eq + Hash> RateLimiter<K> {
    fn relative(&self, ts: Instant) -> u64 {
        ts.checked_duration_since(self.start)
            .unwrap_or_default()
            .as_nanos() as u64
    }

    pub async fn clean(&self, before: Instant) {
        let before = self.relative(before);
        self.limits.retain_async(move |_, v| v.0 >= before).await;
    }

    pub async fn req(&self, key: K, quota: Quota, now: Instant) -> Result<(), RateLimitError> {
        let now = self.relative(now);

        match self.limits.entry_async(key).await {
            Entry::Occupied(mut gcra) => gcra.get_mut().req(quota, now),
            Entry::Vacant(gcra) => {
                gcra.insert_entry(Gcra::first(quota, now));
                Ok(())
            }
        }
    }
}

impl<K: Eq + Hash> Default for RateLimiter<K> {
    fn default() -> Self {
        RateLimiter {
            start: Instant::now(),
            limits: HashMap::new(),
        }
    }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct RateLimitError(NonZeroU64);

impl RateLimitError {
    pub const fn as_duration(&self) -> Duration {
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

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Gcra(u64);

impl Gcra {
    #[inline]
    pub const fn first(Quota { t, .. }: Quota, now: u64) -> Gcra {
        // Equivalent to `Gcra(now + t).req()` to calculate the first request
        Gcra(now + t + t)
    }

    #[inline]
    pub fn req(&mut self, Quota { tau, t }: Quota, now: u64) -> Result<(), RateLimitError> {
        let next = self.0.saturating_sub(tau);
        if now < next {
            // SAFETY: next > now, so next - now is non-zero
            return Err(RateLimitError(unsafe { NonZeroU64::new_unchecked(next - now) }));
        }

        self.0 = now.max(self.0) + t;

        Ok(())
    }
}
