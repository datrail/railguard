use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rustix::time::{ClockId, clock_gettime};

type Read<T> = Arc<dyn Fn() -> T + Send + Sync>;

/// The two clocks a held value is aged on; tests pass their own.
#[derive(Clone)]
pub struct Clock {
    wall: Read<SystemTime>,
    since_boot: Read<Duration>,
}

impl Clock {
    /// The system's wall clock, and `CLOCK_BOOTTIME`, which counts time the host was suspended.
    pub fn system() -> Self {
        Self::new(SystemTime::now, read_since_boot)
    }

    pub fn new(
        wall: impl Fn() -> SystemTime + Send + Sync + 'static,
        since_boot: impl Fn() -> Duration + Send + Sync + 'static,
    ) -> Self {
        Self {
            wall: Arc::new(wall),
            since_boot: Arc::new(since_boot),
        }
    }

    pub fn now(&self) -> Stamp {
        Stamp {
            wall: (self.wall)(),
            since_boot: (self.since_boot)(),
        }
    }
}

/// One reading of both clocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    wall: SystemTime,
    since_boot: Duration,
}

impl Stamp {
    /// The larger of the two clocks' time since `earlier`: a frozen period shows in at least one,
    /// and a wall clock stepped back can't make a value look fresher.
    pub fn duration_since(&self, earlier: Stamp) -> Duration {
        let wall = self.wall.duration_since(earlier.wall).unwrap_or_default();
        let boot = self.since_boot.saturating_sub(earlier.since_boot);
        wall.max(boot)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_since_boot() -> Duration {
    to_duration(clock_gettime(ClockId::Boottime))
}

// Elsewhere only the monotonic clock is offered, which may stop while the host is suspended.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn read_since_boot() -> Duration {
    to_duration(clock_gettime(ClockId::Monotonic))
}

fn to_duration(time: rustix::time::Timespec) -> Duration {
    // Both fields are non-negative for a clock counting from boot.
    Duration::new(
        u64::try_from(time.tv_sec).unwrap_or_default(),
        u32::try_from(time.tv_nsec).unwrap_or_default(),
    )
}
