//! Common patterns and utilities

use std::sync::Arc;
use tokio::sync::RwLock;

/// Lazy initialization pattern
pub struct Lazy<T> {
    value: RwLock<Option<T>>,
    init: Arc<dyn Fn() -> T + Send + Sync>,
}

impl<T: Send + Sync> Lazy<T> {
    /// Create new lazy value
    pub fn new<F>(init: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        Self {
            value: RwLock::new(None),
            init: Arc::new(init),
        }
    }

    /// Get or initialize value
    pub async fn get(&self) -> T
    where
        T: Clone,
    {
        let read = self.value.read().await;
        if let Some(ref value) = *read {
            return value.clone();
        }
        drop(read);

        let mut write = self.value.write().await;
        if let Some(ref value) = *write {
            return value.clone();
        }

        let value = (self.init)();
        *write = Some(value.clone());
        value
    }
}



/// Result extension trait
pub trait ResultExt<T, E> {
    /// Map error to anyhow
    fn anyhow(self) -> anyhow::Result<T>
    where
        E: std::fmt::Display;

    /// Log error but continue
    fn log_error(self, msg: &str) -> Option<T>;
}

impl<T, E: std::fmt::Display> ResultExt<T, E> for Result<T, E> {
    fn anyhow(self) -> anyhow::Result<T>
    where
        E: std::fmt::Display,
    {
        self.map_err(|e| anyhow::anyhow!("{}", e))
    }

    fn log_error(self, msg: &str) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!("{}: {}", msg, e);
                None
            }
        }
    }
}

/// Option extension trait
pub trait OptionExt<T> {
    /// Require value or return error
    fn required(self, msg: &str) -> anyhow::Result<T>;

    /// Map or default
    fn map_or_default<U, F>(self, f: F) -> U
    where
        F: FnOnce(T) -> U,
        U: Default;
}

impl<T> OptionExt<T> for Option<T> {
    fn required(self, msg: &str) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("{}", msg))
    }

    fn map_or_default<U, F>(self, f: F) -> U
    where
        F: FnOnce(T) -> U,
        U: Default,
    {
        self.map(f).unwrap_or_default()
    }
}

/// Memoize function results
pub fn memoize<A, B, F>(f: F) -> impl FnMut(A) -> B
where
    A: std::hash::Hash + Eq + Clone,
    B: Clone,
    F: Fn(A) -> B,
{
    use std::cell::RefCell;
    use std::collections::HashMap;
    
    let cache: RefCell<HashMap<A, B>> = RefCell::new(HashMap::new());
    
    move |arg: A| {
        let mut cache = cache.borrow_mut();
        cache
            .entry(arg.clone())
            .or_insert_with(|| f(arg))
            .clone()
    }
}

/// Retry operation with exponential backoff
pub async fn retry_with_backoff<T, E, F, Fut>(
    operation: F,
    max_retries: u32,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut retries = 0;
    
    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) if retries >= max_retries => return Err(e),
            Err(e) => {
                retries += 1;
                let delay = std::time::Duration::from_millis(100 * 2_u64.pow(retries));
                tracing::warn!("Retry {}/{} after {:?}: {:?}", retries, max_retries, delay, e);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Timeout wrapper
pub async fn with_timeout<T, Fut>(
    future: Fut,
    duration: std::time::Duration,
) -> anyhow::Result<T>
where
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| anyhow::anyhow!("Operation timed out"))?
}

/// Debounce pattern for async operations
pub struct Debouncer {
    delay: std::time::Duration,
    last_call: RwLock<Option<std::time::Instant>>,
}

impl Debouncer {
    /// Create debouncer
    pub fn new(delay: std::time::Duration) -> Self {
        Self {
            delay,
            last_call: RwLock::new(None),
        }
    }

    /// Check if should execute
    pub async fn should_execute(&self) -> bool {
        let mut last = self.last_call.write().await;
        let now = std::time::Instant::now();
        
        match *last {
            Some(instant) if now.duration_since(instant) < self.delay => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

/// Rate limiter (simple token bucket)
pub struct RateLimiter {
    rate: f64,
    tokens: RwLock<f64>,
    last_update: RwLock<std::time::Instant>,
}

impl RateLimiter {
    /// Create rate limiter
    pub fn new(rate: f64) -> Self {
        Self {
            rate,
            tokens: RwLock::new(rate),
            last_update: RwLock::new(std::time::Instant::now()),
        }
    }

    /// Try to acquire token
    pub async fn try_acquire(&self) -> bool {
        let now = std::time::Instant::now();
        let mut last = self.last_update.write().await;
        let mut tokens = self.tokens.write().await;
        
        // Add tokens based on time elapsed
        let elapsed = now.duration_since(*last).as_secs_f64();
        *tokens = (*tokens + elapsed * self.rate).min(self.rate);
        *last = now;
        
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Wait for token
    pub async fn acquire(&self) {
        while !self.try_acquire().await {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

/// Circuit breaker pattern
pub struct CircuitBreaker {
    failure_threshold: u32,
    reset_timeout: std::time::Duration,
    state: RwLock<CircuitState>,
    failures: RwLock<u32>,
    last_failure: RwLock<Option<std::time::Instant>>,
}

#[derive(Clone, Copy, PartialEq)]
enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreaker {
    /// Create circuit breaker
    pub fn new(failure_threshold: u32, reset_timeout: std::time::Duration) -> Self {
        Self {
            failure_threshold,
            reset_timeout,
            state: RwLock::new(CircuitState::Closed),
            failures: RwLock::new(0),
            last_failure: RwLock::new(None),
        }
    }

    /// Check if request should proceed
    pub async fn can_execute(&self) -> bool {
        let mut state = self.state.write().await;
        
        match *state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                let last = *self.last_failure.read().await;
                if let Some(instant) = last {
                    if instant.elapsed() >= self.reset_timeout {
                        *state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record success
    pub async fn record_success(&self) {
        let mut state = self.state.write().await;
        let mut failures = self.failures.write().await;
        
        *failures = 0;
        *state = CircuitState::Closed;
    }

    /// Record failure
    pub async fn record_failure(&self) {
        let mut state = self.state.write().await;
        let mut failures = self.failures.write().await;
        let mut last = self.last_failure.write().await;
        
        *failures += 1;
        *last = Some(std::time::Instant::now());
        
        if *failures >= self.failure_threshold {
            *state = CircuitState::Open;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_ext() {
        let ok: Result<i32, &str> = Ok(42);
        assert!(ok.anyhow().is_ok());

        let err: Result<(), String> = Err("error".to_string());
        assert!(err.anyhow().is_err());
    }

    #[test]
    fn test_option_ext() {
        let some: Option<i32> = Some(42);
        assert!(some.required("missing").is_ok());

        let none: Option<i32> = None;
        assert!(none.required("missing").is_err());
    }

    #[tokio::test]
    async fn test_debouncer() {
        let debouncer = Debouncer::new(std::time::Duration::from_millis(100));
        
        assert!(debouncer.should_execute().await);
        assert!(!debouncer.should_execute().await);
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(10.0); // 10 per second
        
        assert!(limiter.try_acquire().await);
        assert!(limiter.try_acquire().await);
    }
}
