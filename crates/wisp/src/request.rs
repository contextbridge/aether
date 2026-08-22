use std::sync::atomic::{AtomicU64, Ordering};

/// Correlates an in-flight request with its eventual result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    /// Returns an ID no other caller will hand out.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
