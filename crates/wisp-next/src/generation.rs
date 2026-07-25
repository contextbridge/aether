//! A monotonic marker used to notice that something has moved on.
//!
//! The UI needs this in two shapes and they are the same thing: a cache holds
//! the marker it was built from and rebuilds when it no longer matches, and an
//! in-flight request carries the marker it was made at so a result that arrives
//! after it was superseded can be recognised and dropped.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Generation(u64);

impl Generation {
    /// A marker no other caller will hand out, for correlating a result with
    /// the request that asked for it.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Moves this marker on, invalidating everything built against the old one.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }

    /// The raw value, for the protocol fields that carry one over the wire.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for Generation {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
