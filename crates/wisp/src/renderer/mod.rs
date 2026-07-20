//! Frame rendering.
//!
//! [`Renderer`] is the facade: it owns the drawing services (theme, syntax
//! highlighter) and every cache, and sequences one draw. The rest of this
//! module layers below it:
//!
//! - `layout` measures the frame's bands once per draw;
//! - `history` moves overflow rows into the terminal's native scrollback;
//! - `transcript` turns conversation items into rows, through the `cache` of
//!   sealed items and the `streaming` incremental renderer;
//! - `frame` places the bands, composer, status line, and overlays.
//!
//! The pure item→rows builders live with their models in
//! `crate::conversation::{item_view, tool_view}`.

mod cache;
mod frame;
mod history;
mod layout;
mod stats;
mod streaming;
mod transcript;

use std::collections::HashMap;

use crate::app::App;
use crate::conversation::{ConversationId, ConversationItemId};
use crate::view::generation::Generation;
use crate::view::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use frame::draw_frame;
use ratatui::Terminal;
use ratatui::backend::Backend;

pub use stats::RenderStats;
use cache::RenderCache;
use history::{CommitPoint, NativeHistoryCursor};
use layout::FrameLayout;
use stats::Lap;
use streaming::StreamEntry;

/// Shared rendering services, borrowed for the duration of one draw.
///
/// `theme_generation` lets a route or overlay cache styled text across frames and
/// rebuild it only when the theme actually changes.
pub struct DrawContext<'a> {
    pub theme: &'a Theme,
    pub highlighter: &'a mut SyntaxHighlighter,
    pub theme_generation: Generation,
}

/// Owns everything drawing a frame needs: the theme, the syntax highlighter,
/// and the caches that keep streaming content stable between frames.
///
/// Methods destructure `self` rather than taking `&self.theme` through `&mut
/// self`, so the theme is borrowed alongside the highlighter instead of cloned
/// once per item per frame.
pub struct Renderer {
    theme: Theme,
    highlighter: SyntaxHighlighter,
    theme_generation: Option<Generation>,
    render_cache: RenderCache,
    native_history: NativeHistoryCursor,
    stream_cache: HashMap<ConversationItemId, StreamEntry>,
    stats: RenderStats,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            theme: Theme::default(),
            highlighter: SyntaxHighlighter::new(),
            theme_generation: None,
            render_cache: RenderCache::default(),
            native_history: NativeHistoryCursor::default(),
            stream_cache: HashMap::new(),
            stats: RenderStats::default(),
        }
    }

    pub fn take_stats(&mut self) -> RenderStats {
        let highlight = self.highlighter.take_stats();
        RenderStats { highlight, ..std::mem::take(&mut self.stats) }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The app owns the active theme; adopt it whenever its generation moves,
    /// dropping every cache styled with the old one.
    fn sync_theme(&mut self, app: &App) {
        if self.theme_generation != Some(app.theme_generation()) {
            self.theme = app.theme().clone();
            self.theme_generation = Some(app.theme_generation());
            self.highlighter.clear();
            self.render_cache.clear();
            self.stream_cache.clear();
        }
    }

    fn generation(&self) -> Generation {
        self.theme_generation.unwrap_or_default()
    }

    pub fn draw<B: Backend>(&mut self, terminal: &mut Terminal<B>, app: &mut App) -> Result<(), B::Error> {
        self.sync_theme(app);
        terminal.autoresize()?;
        let area = terminal.get_frame().area();
        self.sync_conversation(app.conversation_id());

        let lap = Lap::start();
        let layout = FrameLayout::new(area, app, self);
        let capacity =
            usize::from(layout.transcript_height).saturating_sub(usize::from(layout.progress_height));
        self.stats.ns_layout += lap.ns();

        let lap = Lap::start();
        let live = self.commit_overflow(terminal, app, area.width, capacity)?;
        self.stats.ns_live += lap.ns();

        let live = if app.full_screen_active() { Vec::new() } else { live };
        let lap = Lap::start();
        terminal.draw(|frame| draw_frame(frame, app, self, &layout, &live))?;
        self.stats.ns_draw += lap.ns();
        self.stats.frames += 1;
        self.render_cache.current = std::mem::take(&mut self.render_cache.frame);
        Ok(())
    }

    /// The rendering services a full-screen route or overlay draws with.
    fn context(&mut self) -> DrawContext<'_> {
        let theme_generation = self.generation();
        DrawContext { theme: &self.theme, highlighter: &mut self.highlighter, theme_generation }
    }

    fn sync_conversation(&mut self, conversation_id: ConversationId) {
        if self.native_history.conversation_id != Some(conversation_id) {
            self.native_history =
                NativeHistoryCursor { conversation_id: Some(conversation_id), commit: CommitPoint::default() };
            self.render_cache.clear();
            self.stream_cache.clear();
        }
    }
}
