//! Aggregate frame cadence with fixed-size storage, including soak runs.

use std::time::Duration;

const BUCKETS: usize = 5_001;
const BUCKET_MICROS: u128 = 100;

pub struct FrameIntervals {
    counts: Box<[u64]>,
    pub count: u64,
    pub over_20ms: u64,
    pub over_33ms: u64,
    pub max: Duration,
}

impl Default for FrameIntervals {
    fn default() -> Self {
        Self {
            counts: vec![0; BUCKETS].into_boxed_slice(),
            count: 0,
            over_20ms: 0,
            over_33ms: 0,
            max: Duration::ZERO,
        }
    }
}

impl FrameIntervals {
    pub fn add(&mut self, elapsed: Duration) {
        let bucket = elapsed.as_micros() / BUCKET_MICROS;
        let index = bucket.min((BUCKETS - 1) as u128) as usize;
        self.counts[index] += 1;
        self.count += 1;
        self.over_20ms += u64::from(elapsed > Duration::from_millis(20));
        self.over_33ms += u64::from(elapsed > Duration::from_millis(33));
        self.max = self.max.max(elapsed);
    }

    pub fn percentile_ms(&self, percentile: u64) -> f64 {
        let rank = (self.count * percentile).div_ceil(100);
        let mut seen = 0;
        self.counts
            .iter()
            .position(|count| {
                seen += count;
                seen >= rank
            })
            .map_or(0., |bucket| bucket as f64 * BUCKET_MICROS as f64 / 1000.)
    }
}
