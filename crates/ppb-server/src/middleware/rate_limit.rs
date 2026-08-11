//! Simple in-memory sliding-window rate limiter (per key per minute).
//!
//! Applied at sensitive endpoints (login/reauth/github callback/chat send/raw
//! CLI). Returns 429 + `Retry-After`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Default)]
pub struct RateLimiter {
    buckets: Arc<DashMap<String, VecDeque<i64>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check + record an attempt. Returns `RATE_LIMIT` when over `limit_per_minute`.
    pub fn check(&self, key: &str, limit_per_minute: u32) -> Result<(), ApiError> {
        if limit_per_minute == 0 {
            return Err(ApiError::new(ErrorCode::RateLimit, "rate limit disabled by policy"));
        }
        let now = now_secs();
        let mut bucket = self.buckets.entry(key.to_string()).or_default();
        bucket.retain(|t| now - *t < 60);
        if bucket.len() >= limit_per_minute as usize {
            let retry_after = bucket
                .front()
                .map(|t| (60 - (now - *t)).max(1))
                .unwrap_or(60);
            return Err(ApiError {
                code: ErrorCode::RateLimit,
                message: "rate limit exceeded".to_string(),
                request_id: String::new(),
                details: serde_json::json!({ "retry_after": retry_after }),
                retry_after_secs: Some(retry_after as u64),
            });
        }
        bucket.push_back(now);
        Ok(())
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_within_limit() {
        let rl = RateLimiter::new();
        for _ in 0..5 {
            assert!(rl.check("login:1.2.3.4", 10).is_ok());
        }
    }

    #[test]
    fn blocks_over_limit() {
        let rl = RateLimiter::new();
        for _ in 0..3 {
            let _ = rl.check("cli:root", 3);
        }
        let err = rl.check("cli:root", 3).unwrap_err();
        assert_eq!(err.code, ErrorCode::RateLimit);
        assert!(err.retry_after_secs.is_some());
    }

    #[test]
    fn keys_are_independent() {
        let rl = RateLimiter::new();
        for _ in 0..3 {
            let _ = rl.check("a", 3);
        }
        assert!(rl.check("a", 3).is_err());
        assert!(rl.check("b", 3).is_ok());
    }
}
