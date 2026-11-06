//! Local -> server mirror and the "this PC is on" signal.
//!
//! The local SQLite database is the source of truth and the desktop is the only writer. This
//! module copies plans (expanded 14 days ahead), their change history, and the notification
//! channel to the server, and sends a presence beat so the server can tell when a planned
//! session did not start. Nothing here can weaken locking, timing, or blocking: every failure is
//! a status code in `SyncStatus`, never an error for the engine, and workers run on their own
//! threads without holding the database lock across a network call.

pub mod app;
pub mod ports;
pub mod presence;
pub mod push;
pub mod snapshot;
pub mod status;
pub mod worker;

use std::time::Instant;

/// Monotonic milliseconds. Interval logic uses this instead of the wall clock, so a clock set
/// back (or the `HighWater` floor catching up with it) can neither stall nor flood the beats.
pub trait Mono: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemMono {
    start: Instant,
}

impl SystemMono {
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for SystemMono {
    fn default() -> Self {
        Self::new()
    }
}

impl Mono for SystemMono {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// Spread for retry delays (0..=1000 permille) so workers do not retry in lockstep.
pub trait Jitter: Send + Sync {
    fn permille(&self) -> u32;
}

pub struct RandomJitter;

impl Jitter for RandomJitter {
    fn permille(&self) -> u32 {
        u32::try_from(uuid::Uuid::new_v4().as_u128() % 1001).unwrap_or(0)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Jitter, Mono};

    pub struct ManualMono(AtomicU64);

    impl ManualMono {
        pub fn new(start: u64) -> Self {
            Self(AtomicU64::new(start))
        }

        pub fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Mono for ManualMono {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    pub struct FixedJitter(pub u32);

    impl Jitter for FixedJitter {
        fn permille(&self) -> u32 {
            self.0
        }
    }
}
