//! A small fixed-window rate limiter keyed by client IP.
//!
//! Per process: with several API replicas each enforces its own window.
//! Account lockout (in the database) is the cross-replica control; this limiter
//! blunts password spraying across many accounts from one address. A shared
//! Valkey-backed limiter replaces it when Valkey becomes part of the stack.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RateLimiter {
    limit: u32,
    window: Duration,
    hits: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

/// Pruning threshold so the map can't grow without bound.
const MAX_TRACKED: usize = 10_000;

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Records a hit. Returns `false` if the address is over the limit.
    pub fn check(&self, ip: IpAddr) -> bool {
        self.check_at(ip, Instant::now())
    }

    fn check_at(&self, ip: IpAddr, now: Instant) -> bool {
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if hits.len() >= MAX_TRACKED {
            hits.retain(|_, (start, _)| now.duration_since(*start) < self.window);
        }
        let entry = hits.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_within_a_window_and_resets_after() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();
        let t0 = Instant::now();
        assert!(limiter.check_at(ip, t0));
        assert!(limiter.check_at(ip, t0));
        assert!(!limiter.check_at(ip, t0));
        assert!(limiter.check_at(other, t0), "addresses are independent");
        assert!(limiter.check_at(ip, t0 + Duration::from_secs(61)));
    }
}
