//! A monotonic marker for cache invalidation.

/// A cache-invalidation marker: bumped when the underlying data changes,
/// compared by caches to notice they are stale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Generation(u64);

impl Generation {
    /// Moves this marker on, invalidating everything built against the old one.
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}
