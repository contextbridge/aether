use crate::view::syntax::HighlightStats;
use std::time::Instant;

/// Work one [`Renderer`](super::Renderer) did since the last
/// [`Renderer::take_stats`](super::Renderer::take_stats), so tests can bound
/// per-frame rendering work by counting it instead of timing it.
///
/// Byte counters measure input re-processed, not output produced: an item whose
/// rendering is O(content) every frame shows up as its content size again and
/// again, whatever the drawn output looks like.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    pub frames: u64,
    /// Item renders that ran rather than being served from a cache. A streaming
    /// item rebuilds as its content grows and an open tool call as its spinner
    /// moves; anything else rebuilding is wasted work.
    pub item_rebuilds: u64,
    pub markdown_bytes_parsed: u64,
    /// Largest live region any single render produced. Native scrollback commits
    /// are supposed to bound this to roughly the viewport height.
    pub max_live_rows: usize,
    pub history_rows_inserted: u64,
    pub highlight: HighlightStats,
    pub ns_layout: u64,
    pub ns_live: u64,
    pub ns_draw: u64,
    pub ns_item_rebuild: u64,
}

/// One measured stretch of a draw. Compiled to nothing outside the `testing`
/// feature, so production draws pay only the zero-sized struct.
pub(super) struct Lap {
    #[cfg(feature = "testing")]
    start: Instant,
}

impl Lap {
    pub(super) fn start() -> Self {
        #[cfg(feature = "testing")]
        return Self { start: Instant::now() };
        #[cfg(not(feature = "testing"))]
        Self {}
    }

    #[cfg_attr(not(feature = "testing"), expect(clippy::unused_self))]
    pub(super) fn ns(self) -> u64 {
        #[cfg(feature = "testing")]
        return u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        #[cfg(not(feature = "testing"))]
        0
    }
}
