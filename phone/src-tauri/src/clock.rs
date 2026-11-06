//! The phone's clocks. Lease expiry is measured on [`BootClock::mono_ms`], which must count
//! device sleep (Rust's `Instant` on Android is `CLOCK_MONOTONIC`, which does not) and must not be
//! user-settable: `CLOCK_BOOTTIME`, the clock behind `SystemClock.elapsedRealtime()` that the
//! Kotlin plugin uses for its own deadline. On a build that is neither Linux nor Android (desktop
//! development only) it falls back to `Instant`; that fallback is not used on a phone.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use study_phone_core::Clock;

pub struct BootClock {
    started: Instant,
}

impl BootClock {
    pub fn new() -> Self {
        Self { started: Instant::now() }
    }
}

impl Default for BootClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn boottime_ms() -> Option<i64> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    (rc == 0).then(|| i64::from(ts.tv_sec).saturating_mul(1000).saturating_add(i64::from(ts.tv_nsec) / 1_000_000))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn boottime_ms() -> Option<i64> {
    None
}

impl Clock for BootClock {
    fn mono_ms(&self) -> i64 {
        boottime_ms().unwrap_or_else(|| i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX))
    }

    fn wall_ms(&self) -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_millis()).ok()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_monotonic_reading_never_goes_backwards() {
        let clock = BootClock::new();
        let a = clock.mono_ms();
        let b = clock.mono_ms();
        assert!(b >= a);
        assert!(clock.wall_ms() > 1_700_000_000_000);
    }
}
