use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use tokio::sync::Mutex;

const LOGIN_WINDOW: Duration = Duration::from_secs(10 * 60);
const MAX_LOGIN_FAILURES: usize = 8;

#[derive(Default)]
pub struct LoginRateLimiter {
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl LoginRateLimiter {
    pub async fn check(&self, username: &str) -> bool {
        let mut failures = self.failures.lock().await;
        let attempts = failures
            .entry(username.trim().to_ascii_lowercase())
            .or_default();
        prune(attempts);
        attempts.len() < MAX_LOGIN_FAILURES
    }

    pub async fn record_failure(&self, username: &str) {
        let mut failures = self.failures.lock().await;
        let attempts = failures
            .entry(username.trim().to_ascii_lowercase())
            .or_default();
        prune(attempts);
        attempts.push_back(Instant::now());
    }

    pub async fn clear(&self, username: &str) {
        self.failures
            .lock()
            .await
            .remove(&username.trim().to_ascii_lowercase());
    }
}

fn prune(attempts: &mut VecDeque<Instant>) {
    let cutoff = Instant::now() - LOGIN_WINDOW;
    while attempts.front().is_some_and(|attempt| *attempt < cutoff) {
        attempts.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocks_after_repeated_failures_and_resets() {
        let limiter = LoginRateLimiter::default();
        for _ in 0..MAX_LOGIN_FAILURES {
            assert!(limiter.check("Admin").await);
            limiter.record_failure("admin").await;
        }
        assert!(!limiter.check("ADMIN").await);
        limiter.clear("admin").await;
        assert!(limiter.check("admin").await);
    }
}
