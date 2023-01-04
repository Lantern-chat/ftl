use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimiter {
    pub count: f32,
    pub last: Instant,
}

#[derive(Debug)]
pub struct RateLimitError {
    pub limit: f32,
}

impl Default for RateLimiter {
    fn default() -> Self {
        RateLimiter {
            count: 0.0,
            last: Instant::now(),
        }
    }
}

use crate::Route;

impl RateLimiter {
    /// Update this limiter. Will return true if within limits.
    pub fn update<S>(&mut self, route: &Route<S>, req_per_sec: f32) -> Result<(), RateLimitError> {
        // get the number of "decayed" requests since the last request
        let decayed = route.start.duration_since(self.last).as_millis() as f32 * (req_per_sec * 0.001);
        // compute the effective number of requests performed
        let req_count = self.count - decayed;

        if req_count < req_per_sec {
            // update with new request
            self.count = req_count.max(0.0) + 1.0;
            self.last = route.start;

            Ok(())
        } else {
            Err(RateLimitError { limit: req_per_sec })
        }
    }
}
