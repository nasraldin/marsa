use parking_lot::Mutex;
use std::time::Duration;
use time::{OffsetDateTime, macros::datetime};

pub use web_time::Instant;

pub trait Clock {
    fn utc_now(&self) -> OffsetDateTime;
    fn now(&self) -> Instant;
}

pub struct TestClock(Mutex<TestClockState>);

struct TestClockState {
    now: Instant,
    utc_now: OffsetDateTime,
}

impl TestClock {
    pub fn new() -> Self {
        Self(Mutex::new(TestClockState {
            now: Instant::now(),
            utc_now: datetime!(2025-07-01 23:59:58 UTC),
        }))
    }

    pub fn set_utc_now(&self, now: OffsetDateTime) {
        let mut state = self.0.lock();
        state.utc_now = now;
    }

    pub fn advance(&self, duration: Duration) {
        let mut state = self.0.lock();
        state.now += duration;
        state.utc_now += duration;
    }
}

impl Clock for TestClock {
    fn utc_now(&self) -> OffsetDateTime {
        self.0.lock().utc_now
    }

    fn now(&self) -> Instant {
        self.0.lock().now
    }
}
