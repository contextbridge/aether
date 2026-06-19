pub(crate) struct CachedLayer<K, V> {
    entry: Option<(K, V)>,
}

impl<K: PartialEq, V> CachedLayer<K, V> {
    pub(crate) fn new() -> Self {
        Self { entry: None }
    }

    pub(crate) fn reset(&mut self) {
        self.entry = None;
    }

    pub(crate) fn get(&self) -> Option<&V> {
        self.entry.as_ref().map(|(_, value)| value)
    }

    pub(crate) fn ensure(&mut self, key: K, build: impl FnOnce() -> V) -> &V {
        self.ensure_rebuilt(key, build);
        &self.entry.as_ref().expect("entry populated").1
    }

    /// Populate the entry if missing or keyed differently, and return `true` when a rebuild occurred.
    pub(crate) fn ensure_rebuilt(&mut self, key: K, build: impl FnOnce() -> V) -> bool {
        if self.entry.as_ref().is_some_and(|(current, _)| current == &key) {
            return false;
        }
        self.entry = Some((key, build()));
        true
    }
}

impl<K: PartialEq, V> Default for CachedLayer<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_builds_on_first_call() {
        let mut layer: CachedLayer<u32, String> = CachedLayer::new();
        let mut calls = 0;

        let value = layer.ensure(1, || {
            calls += 1;
            "built".to_string()
        });
        assert_eq!(value, "built");
        assert_eq!(calls, 1);
    }

    #[test]
    fn ensure_skips_rebuild_on_matching_key() {
        let mut layer: CachedLayer<u32, String> = CachedLayer::new();
        let mut calls = 0;

        layer.ensure(1, || {
            calls += 1;
            "first".to_string()
        });
        layer.ensure(1, || {
            calls += 1;
            "second".to_string()
        });

        assert_eq!(calls, 1);
        assert_eq!(layer.get(), Some(&"first".to_string()));
    }

    #[test]
    fn ensure_rebuilds_on_key_change() {
        let mut layer: CachedLayer<u32, String> = CachedLayer::new();
        layer.ensure(1, || "one".to_string());
        layer.ensure(2, || "two".to_string());

        assert_eq!(layer.get(), Some(&"two".to_string()));
    }

    #[test]
    fn reset_clears_entry() {
        let mut layer: CachedLayer<u32, String> = CachedLayer::new();
        layer.ensure(1, || "one".to_string());
        layer.reset();

        assert!(layer.get().is_none());
    }

    #[test]
    fn ensure_rebuilt_reports_whether_build_ran() {
        let mut layer: CachedLayer<u32, String> = CachedLayer::new();
        assert!(layer.ensure_rebuilt(1, || "one".to_string()));
        assert!(!layer.ensure_rebuilt(1, || "still one".to_string()));
        assert!(layer.ensure_rebuilt(2, || "two".to_string()));
    }
}
