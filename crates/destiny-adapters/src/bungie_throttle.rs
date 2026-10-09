//! Process-wide Bungie pacing; OAuth is deliberately excluded.
//!
//! Waits above 30 seconds fail before dispatch. After the computed cooldown
//! (or a longer positive ThrottleSeconds hint) elapses, a caller may send a probe
//! without sleeping. Its success halves the counter; another throttle increments
//! it and starts a new cooldown. No probe or failed request is automatically retried.
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Injectable monotonic clock and sleeper. No wall-clock or real wait in tests.
pub trait ThrottleTime: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}
struct SystemTime(Instant);
impl ThrottleTime for SystemTime {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
#[derive(Default)]
struct State {
    count: u32,
    retry_seconds: Option<u64>,
    observed: Duration,
}
/// Shared by every production Bungie read/action transport in this process.
pub struct BungieThrottle {
    state: Mutex<State>,
    time: Arc<dyn ThrottleTime>,
}
impl BungieThrottle {
    pub fn new(time: Arc<dyn ThrottleTime>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            time,
        }
    }
    /// Sleeps once, never retries a request. The mutex is not held while sleeping.
    pub(crate) fn before_request(&self) -> Result<(), u64> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.count == 0 {
            return Ok(());
        }
        let millis = 500u64.saturating_mul(1u64.checked_shl(state.count).unwrap_or(u64::MAX));
        // DIM caps the backoff at five minutes; without it the cooldown grows unbounded.
        let delay = Duration::from_millis(millis).min(Duration::from_secs(300));
        if delay > Duration::from_secs(30) {
            // Refuse long waits, but allow a probe once the advertised cooldown
            // has elapsed; otherwise a process could never recover after six throttles.
            let cooldown = delay.max(Duration::from_secs(state.retry_seconds.unwrap_or(0)));
            let remaining = cooldown.saturating_sub(self.time.now().saturating_sub(state.observed));
            if !remaining.is_zero() {
                return Err(remaining
                    .as_secs()
                    .saturating_add(u64::from(remaining.subsec_nanos() > 0)));
            }
            return Ok(());
        }
        drop(state);
        self.time.sleep(delay);
        Ok(())
    }
    pub(crate) fn observe(&self, value: &Value) {
        let Some(root) = value.as_object() else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match root.get("ErrorCode").and_then(Value::as_u64) {
            Some(1) => {
                state.count /= 2;
                state.retry_seconds = None;
            }
            Some(code) if crate::bungie_errors::is_throttle(code) => {
                state.count = state.count.saturating_add(1);
                state.retry_seconds = crate::bungie_errors::throttle_seconds(root);
                state.observed = self.time.now();
            }
            _ => {}
        }
    }
    pub(crate) fn observe_bytes(&self, bytes: &[u8]) {
        if let Ok(value) = serde_json::from_slice(bytes) {
            self.observe(&value);
        }
    }
}
pub(crate) fn shared() -> Arc<BungieThrottle> {
    static SHARED: OnceLock<Arc<BungieThrottle>> = OnceLock::new();
    SHARED
        .get_or_init(|| Arc::new(BungieThrottle::new(Arc::new(SystemTime(Instant::now())))))
        .clone()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[derive(Default)]
    struct Time {
        now: Mutex<Duration>,
        sleeps: Mutex<Vec<Duration>>,
    }
    impl ThrottleTime for Time {
        fn now(&self) -> Duration {
            *self.now.lock().unwrap()
        }
        fn sleep(&self, d: Duration) {
            self.sleeps.lock().unwrap().push(d);
            *self.now.lock().unwrap() += d;
        }
    }
    #[test]
    fn counter_math_and_cap() {
        let time = Arc::new(Time::default());
        let throttle = BungieThrottle::new(time.clone());
        throttle.before_request().unwrap();
        assert!(time.sleeps.lock().unwrap().is_empty());
        for n in 1..=5 {
            throttle.observe(&json!({"ErrorCode":36}));
            throttle.before_request().unwrap();
            assert_eq!(
                time.sleeps.lock().unwrap().last(),
                Some(&Duration::from_millis(500 * (1 << n)))
            );
        }
        throttle.observe(&json!({"ErrorCode":1672,"ThrottleSeconds":45}));
        let count = time.sleeps.lock().unwrap().len();
        assert_eq!(throttle.before_request(), Err(45));
        assert_eq!(time.sleeps.lock().unwrap().len(), count);
        throttle.observe(&json!({"ErrorCode":1}));
        throttle.before_request().unwrap();
        assert_eq!(
            time.sleeps.lock().unwrap().last(),
            Some(&Duration::from_secs(4))
        );
    }

    #[test]
    fn cap_hint_counts_down_then_allows_a_probe_without_sleeping() {
        let time = Arc::new(Time::default());
        let throttle = BungieThrottle::new(time.clone());
        for _ in 0..6 {
            throttle.observe(&json!({"ErrorCode":36}));
        }
        assert_eq!(throttle.before_request(), Err(32));
        *time.now.lock().unwrap() = Duration::from_millis(31_001);
        assert_eq!(throttle.before_request(), Err(1));
        *time.now.lock().unwrap() = Duration::from_secs(32);
        throttle.before_request().unwrap();
        assert!(time.sleeps.lock().unwrap().is_empty());
        throttle.observe(&json!({"ErrorCode":1}));
        throttle.before_request().unwrap();
        assert_eq!(
            time.sleeps.lock().unwrap().last(),
            Some(&Duration::from_secs(4))
        );
    }

    #[test]
    fn a_throttled_probe_increments_the_counter_and_restarts_cooldown() {
        let time = Arc::new(Time::default());
        let throttle = BungieThrottle::new(time.clone());
        for _ in 0..6 {
            throttle.observe(&json!({"ErrorCode":36,"ThrottleSeconds":1}));
        }
        assert_eq!(throttle.before_request(), Err(32));
        *time.now.lock().unwrap() = Duration::from_secs(32);
        throttle.before_request().unwrap();
        throttle.observe(&json!({"ErrorCode":36,"ThrottleSeconds":45}));
        assert_eq!(throttle.before_request(), Err(64));
        *time.now.lock().unwrap() = Duration::from_secs(95);
        assert_eq!(throttle.before_request(), Err(1));
        *time.now.lock().unwrap() = Duration::from_secs(96);
        throttle.before_request().unwrap();
        assert!(time.sleeps.lock().unwrap().is_empty());
    }

    #[test]
    fn cooldown_is_capped_at_five_minutes_like_dim() {
        let time = Arc::new(Time::default());
        let throttle = BungieThrottle::new(time.clone());
        for _ in 0..20 {
            throttle.observe(&json!({"ErrorCode":36}));
        }
        assert_eq!(throttle.before_request(), Err(300));
        *time.now.lock().unwrap() = Duration::from_secs(300);
        throttle.before_request().unwrap();
    }

    #[test]
    fn unknown_codes_and_status_text_do_not_change_the_counter() {
        let time = Arc::new(Time::default());
        let throttle = BungieThrottle::new(time.clone());
        throttle.observe(&json!({"ErrorCode":51,"ErrorStatus":"DestinyThrottledByGameServer"}));
        throttle.before_request().unwrap();
        assert!(time.sleeps.lock().unwrap().is_empty());
    }
}
