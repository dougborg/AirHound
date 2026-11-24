//! Injectable time for driver deadlines and hardware waits. Production uses the
//! monotonic clock and OS sleep; scripted transports advance logical time only.
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub trait TimeSource: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
    fn unix_time(&self) -> anyhow::Result<Duration>;
}

#[derive(Clone)]
pub struct Clock(Arc<dyn TimeSource>);
impl Clock {
    pub fn new(source: impl TimeSource + 'static) -> Self {
        Self(Arc::new(source))
    }
    pub fn now(&self) -> Duration {
        self.0.now()
    }
    pub fn elapsed(&self, since: Duration) -> Duration {
        self.now().saturating_sub(since)
    }
    pub fn sleep(&self, duration: Duration) {
        self.0.sleep(duration)
    }
    pub fn unix_time(&self) -> anyhow::Result<Duration> {
        self.0.unix_time()
    }
}
impl Default for Clock {
    fn default() -> Self {
        Self::new(SystemClock(Instant::now()))
    }
}
struct SystemClock(Instant);
impl TimeSource for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration)
    }
    fn unix_time(&self) -> anyhow::Result<Duration> {
        Ok(SystemTime::now().duration_since(UNIX_EPOCH)?)
    }
}
