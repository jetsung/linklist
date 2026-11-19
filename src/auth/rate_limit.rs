use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 登录失败滑动窗口限速器（进程内）。
///
/// 语义：同一 key（IP+用户名）在 `window` 内失败次数达到 `max_failures`
/// 后，后续尝试（含成功尝试前的校验）一律拒绝，直到窗口滑出。
#[derive(Clone)]
pub struct LoginLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    max_failures: u32,
    window: Duration,
}

impl LoginLimiter {
    pub fn new(max_failures: u32, window_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max_failures,
            window: Duration::from_secs(window_secs),
        }
    }

    /// 是否允许本次尝试（未被限速）。
    pub fn check(&self, key: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        if let Some(stamps) = map.get_mut(key) {
            stamps.retain(|t| now.duration_since(*t) < self.window);
            if stamps.len() >= self.max_failures as usize {
                return false;
            }
        }
        true
    }

    /// 记录一次失败。
    pub fn record_failure(&self, key: &str) {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        let stamps = map.entry(key.to_string()).or_default();
        stamps.retain(|t| now.duration_since(*t) < self.window);
        stamps.push(now);
    }

    /// 成功后清除失败记录。
    pub fn reset(&self, key: &str) {
        self.inner.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_under_threshold() {
        let limiter = LoginLimiter::new(3, 60);
        assert!(limiter.check("k"));
        limiter.record_failure("k");
        limiter.record_failure("k");
        assert!(limiter.check("k"));
    }

    #[test]
    fn blocks_at_threshold() {
        let limiter = LoginLimiter::new(3, 60);
        for _ in 0..3 {
            limiter.record_failure("k");
        }
        assert!(!limiter.check("k"));
    }

    #[test]
    fn reset_clears_failures() {
        let limiter = LoginLimiter::new(2, 60);
        limiter.record_failure("k");
        limiter.record_failure("k");
        assert!(!limiter.check("k"));
        limiter.reset("k");
        assert!(limiter.check("k"));
    }

    #[test]
    fn keys_are_isolated() {
        let limiter = LoginLimiter::new(1, 60);
        limiter.record_failure("a");
        assert!(!limiter.check("a"));
        assert!(limiter.check("b"));
    }
}
