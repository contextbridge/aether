use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};
use std::collections::HashSet;
use tui_scrollview::{ScrollView, ScrollbarVisibility};

use crate::diff::SPLIT_VIEW_MIN_WIDTH;
use crate::git_diff::{FileDiff, FileStatus, PatchAnchor, PatchLine, PatchLineKind, StageState};
use crate::screens::{RenderContext, Screen};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::widgets::TextInput;
use crate::wrap::{fit_line, text_position_in_wrap, wrap_text_char};

use super::GitDiffScreen;
use super::state::{BottomBar, CursorRow, DiffView, DraftState, DrawerEntry, Focus, GitDiffLoadState};

type BuildResult = (Vec<Line<'static>>, Vec<CursorRow>, Option<(usize, u16)>);

/// Below this width the file drawer is hidden and the patch gets the full area.
pub(super) const DRAWER_MIN_WIDTH: u16 = 72;

impl GitDiffScreen {
    pub(super) fn render_screen(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut RenderContext<'_>,
    ) -> Option<Position> {
        let theme = cx.theme;
        self.last_area = area;
        Clear.render(area, buf);
        let block = Block::bordered()
            .title(format!(" Git Diff · {} ", self.scope.label()))
            .border_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD));
        let inner = block.inner(area);
        block.render(area, buf);

        let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        let cursor = match &self.state {
            GitDiffLoadState::Loading => {
                notice("Loading changes…", theme.muted).render(body, buf);
                None
            }
            GitDiffLoadState::Error(message) => {
                notice(format!("Git diff unavailable: {message}"), theme.error).render(body, buf);
                None
            }
            GitDiffLoadState::Ready(document) if document.files.is_empty() => {
                notice("No changes in working tree for this scope", theme.muted).render(body, buf);
                None
            }
            GitDiffLoadState::Ready(_) => self.render_document(body, buf, cx),
        };

        self.render_footer(footer, buf, theme).or(cursor)
    }

    fn render_document(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        if area.width < DRAWER_MIN_WIDTH {
            return self.render_patch(area, buf, cx);
        }
        let [drawer, separator, patch] = Layout::horizontal([
            Constraint::Length(drawer_width(area.width)),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(area);
        self.render_drawer(drawer, buf, cx.theme);
        Paragraph::new("│").style(Style::new().fg(cx.theme.muted)).render(separator, buf);
        self.render_patch(patch, buf, cx)
    }

    fn render_drawer(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let entries = self.drawer_entries();
        self.drawer_selection.ensure_visible(entries.len(), usize::from(area.height));
        let [list_area, track_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let items =
            entries.iter().map(|entry| ListItem::new(self.drawer_line(entry, usize::from(list_area.width), theme)));
        let list = List::new(items)
            .highlight_style(Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD));
        StatefulWidget::render(list, list_area, buf, self.drawer_selection.list_state_mut());

        let mut scrollbar_state = ScrollbarState::new(entries.len()).position(self.drawer_selection.offset());
        StatefulWidget::render(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            track_area,
            buf,
            &mut scrollbar_state,
        );
    }

    fn drawer_line(&self, entry: &DrawerEntry, width: usize, theme: &Theme) -> Line<'static> {
        let line = match entry {
            DrawerEntry::Directory { path, depth } => {
                let name = path.rsplit('/').next().unwrap_or(path);
                let marker = if self.collapsed.contains(path) { "▸" } else { "▾" };
                Line::from(vec![
                    Span::raw(format!("{}{} ", "  ".repeat(*depth), marker)),
                    Span::styled(format!("{name}/"), Style::new().fg(theme.info)),
                ])
            }
            DrawerEntry::File { index, depth } => {
                let Some(file) = self.file_at(*index) else {
                    return Line::default();
                };
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let stage = match file.staged {
                    StageState::Unstaged => "☐",
                    StageState::Staged => "☑",
                    StageState::PartiallyStaged => "◩",
                };
                Line::from(vec![
                    Span::raw(format!("{}{} ", "  ".repeat(*depth), stage)),
                    Span::styled(
                        file.status.marker().to_string(),
                        Style::new().fg(file_status_color(file.status, theme)),
                    ),
                    Span::raw(format!(" {name}")),
                    Span::styled(format!(" +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                ])
            }
        };
        fit_line(line, width, Style::new().fg(theme.text_primary))
    }

    fn render_patch(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        let file = self.selected_file().cloned()?;
        let [header_area, content_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(self.patch_header(&file, theme)).render(header_area, buf);
        self.last_patch_height = content_area.height;

        if let Some(message) = self.patch_placeholder(&file) {
            notice(message, theme.muted).render(content_area, buf);
            return None;
        }

        self.ensure_diff_view(&file, content_area.width, theme, cx.highlighter);

        let view = self.diff_view.as_ref()?;
        StatefulWidget::render(&view.scroll_view, content_area, buf, &mut self.patch_scroll_state);
        self.overlay_cursor_indicator(content_area, buf, theme);
        self.draft_cursor_position(content_area)
    }

    fn patch_header(&self, file: &FileDiff, theme: &Theme) -> Line<'static> {
        let header_style = if self.focus == Focus::Patch {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD)
        };
        let mut spans = vec![
            Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
            Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
        ];

        let comments = self.review_queue.comments_for_file(&file.path).count();
        if self.show_full_file {
            spans.push(Span::styled("  [full file]", Style::new().fg(theme.info)));
        } else if comments > 0 {
            spans.push(Span::styled(format!("  {}", plural(comments, "comment")), Style::new().fg(theme.info)));
        }
        Line::from(spans)
    }

    /// Message to show instead of a patch, when there is nothing to diff.
    fn patch_placeholder(&self, file: &FileDiff) -> Option<&'static str> {
        if self.show_full_file {
            if file.status == FileStatus::Deleted {
                return Some("File has been deleted");
            }
            if file.binary {
                return Some("Binary file — cannot display contents");
            }
            if self.full_file_content.is_none() {
                return Some("Loading file…");
            }
            return None;
        }
        file.binary.then_some("Binary file")
    }

    fn draft_cursor_position(&self, content_area: Rect) -> Option<Position> {
        let (draft_row, draft_col) = self.diff_view.as_ref()?.draft_cursor?;
        let offset = usize::from(self.patch_scroll_state.offset().y);
        if draft_row < offset || draft_row >= offset + usize::from(content_area.height) {
            return None;
        }
        let row = content_area.y + u16::try_from(draft_row - offset).unwrap_or(u16::MAX);
        let column = (content_area.x + draft_col).min(content_area.right().saturating_sub(1));
        Some(Position::new(column, row))
    }

    fn ensure_diff_view(
        &mut self,
        file: &FileDiff,
        content_width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) {
        let split = content_width >= SPLIT_VIEW_MIN_WIDTH && !self.show_full_file;
        let draft_sig = self.draft_signature();

        let signature_matches = |view: &DiffView| {
            view.file_path == file.path
                && view.content_width == content_width
                && view.split == split
                && view.full_file == self.show_full_file
                && view.document_revision == self.document_revision
                && view.comments_revision == self.comments_revision
                && view.draft_signature == draft_sig
        };

        if self.diff_view.as_ref().is_some_and(signature_matches) {
            return;
        }

        // Reuse a cached, matching snapshot for this file if one is available (these always
        // carry draft_signature == None, so they only match when there is no active draft).
        // Remove the hit before parking the active view so cache indices are not shifted.
        let cache_hit =
            self.diff_view_cache.iter().position(signature_matches).map(|pos| self.diff_view_cache.remove(pos));

        self.park_active_diff_view();
        self.diff_view =
            Some(cache_hit.unwrap_or_else(|| self.build_diff_view(file, content_width, split, theme, highlighter)));
    }

    fn build_diff_view(
        &self,
        file: &FileDiff,
        content_width: u16,
        split: bool,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> DiffView {
        let (lines, cursor_rows, draft_cursor) = if self.show_full_file {
            self.build_full_file_lines(file, content_width, theme, highlighter)
        } else if split {
            self.build_split_lines(file, content_width, theme, highlighter)
        } else {
            self.build_unified_lines(file, content_width, theme, highlighter)
        };

        let content_height = u16::try_from(lines.len().max(1)).unwrap_or(u16::MAX);
        let mut scroll_view = ScrollView::new(Size::new(content_width, content_height))
            .vertical_scrollbar_visibility(ScrollbarVisibility::Automatic);
        scroll_view.render_widget(Paragraph::new(Text::from(lines)), Rect::new(0, 0, content_width, content_height));

        DiffView {
            scroll_view,
            cursor_rows,
            draft_cursor,
            file_path: file.path.clone(),
            content_width,
            split,
            full_file: self.show_full_file,
            document_revision: self.document_revision,
            comments_revision: self.comments_revision,
            draft_signature: self.draft_signature(),
        }
    }

    fn overlay_cursor_indicator(&self, content_area: Rect, buf: &mut Buffer, theme: &Theme) {
        if self.focus != Focus::Patch || self.draft.is_some() {
            return;
        }
        let Some(cursor_row) = self.diff_view.as_ref().and_then(|view| self.cursor_row_in(view)) else {
            return;
        };
        let offset = usize::from(self.patch_scroll_state.offset().y);
        if cursor_row < offset || cursor_row >= offset + usize::from(content_area.height) {
            return;
        }
        let y = content_area.y + u16::try_from(cursor_row - offset).unwrap_or(u16::MAX);
        for x in content_area.x..content_area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(theme.accent);
                cell.set_fg(theme.background);
            }
        }
    }

    fn build_full_file_lines(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> BuildResult {
        let Some(content) = &self.full_file_content else {
            return (vec![Line::styled("Loading file…", Style::new().fg(theme.muted))], Vec::new(), None);
        };
        let language = file.language();
        // Mark lines that the diff reports as added so the full-file view highlights them
        // instead of rendering every line identically.
        let added_lines: HashSet<usize> = file
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .filter_map(|line| if line.kind == PatchLineKind::Added { line.new_line_no } else { None })
            .collect();
        let mut lines = Vec::new();
        let mut cursor_rows = Vec::new();
        for (index, text) in content.lines().enumerate() {
            let (foreground, background) = if added_lines.contains(&(index + 1)) {
                (theme.diff_added_fg, theme.diff_added_bg)
            } else {
                (theme.text_secondary, theme.background)
            };
            let style = Style::new().fg(foreground).bg(background);
            let mut spans = vec![Span::styled(format!("{:>4} ", index + 1), style)];
            spans.extend(highlighted_spans(text, language, background, theme, highlighter));
            cursor_rows.push((0, index, lines.len()));
            lines.push(fit_line(Line::from(spans).style(Style::new().bg(background)), usize::from(width), style));
        }
        (lines, cursor_rows, None)
    }

    fn build_unified_lines(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> BuildResult {
        let mut lines = Vec::new();
        let mut cursor_rows = Vec::new();
        let mut draft_cursor = None;
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            for (line_idx, patch_line) in hunk.lines.iter().enumerate() {
                cursor_rows.push((hunk_idx, line_idx, lines.len()));
                lines.push(render_unified_line(patch_line, file.language(), width, theme, highlighter));
                self.push_annotations(&mut lines, self.anchor(hunk_idx, line_idx), width, theme, &mut draft_cursor);
            }
        }
        (lines, cursor_rows, draft_cursor)
    }

    fn build_split_lines(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> BuildResult {
        let left_width = width.saturating_sub(1) / 2;
        let right_width = width.saturating_sub(left_width + 1);
        let mut lines = Vec::new();
        let mut cursor_rows = Vec::new();
        let mut draft_cursor = None;

        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            let mut index = 0;
            while index < hunk.lines.len() {
                let line = &hunk.lines[index];

                // A removed run followed by an added run is one changed block, aligned
                // side by side; everything else renders as a single row.
                if line.kind == PatchLineKind::Removed {
                    let removed_start = index;
                    while index < hunk.lines.len() && hunk.lines[index].kind == PatchLineKind::Removed {
                        index += 1;
                    }
                    let added_start = index;
                    while index < hunk.lines.len() && hunk.lines[index].kind == PatchLineKind::Added {
                        index += 1;
                    }
                    let removed: Vec<SplitSide> =
                        (removed_start..added_start).map(|i| SplitSide { line: &hunk.lines[i], idx: i }).collect();
                    let added: Vec<SplitSide> =
                        (added_start..index).map(|i| SplitSide { line: &hunk.lines[i], idx: i }).collect();

                    for (left, right) in pair_changed_block(&removed, &added) {
                        // Anchor comments on the added side when present, falling back to
                        // the removed side, so each line keeps its own comment slot.
                        let anchor_line = right.or(left).map_or(removed_start, |side| side.idx);
                        cursor_rows.push((hunk_idx, anchor_line, lines.len()));
                        lines.push(render_split_row(
                            left.map(|side| side.line),
                            right.map(|side| side.line),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        ));
                        self.push_annotations(
                            &mut lines,
                            self.anchor(hunk_idx, anchor_line),
                            width,
                            theme,
                            &mut draft_cursor,
                        );
                    }
                    continue;
                }

                match line.kind {
                    PatchLineKind::HunkHeader => {
                        cursor_rows.push((hunk_idx, index, lines.len()));
                        lines.push(styled_full_width(&line.text, width, theme.info));
                    }
                    PatchLineKind::Added | PatchLineKind::Context => {
                        let old = (line.kind == PatchLineKind::Context).then_some(line);
                        cursor_rows.push((hunk_idx, index, lines.len()));
                        lines.push(render_split_row(
                            old,
                            Some(line),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        ));
                    }
                    PatchLineKind::Meta | PatchLineKind::Removed => {
                        lines.push(styled_full_width(&line.text, width, theme.muted));
                    }
                }
                self.push_annotations(&mut lines, self.anchor(hunk_idx, index), width, theme, &mut draft_cursor);
                index += 1;
            }
        }
        (lines, cursor_rows, draft_cursor)
    }

    /// Appends the submitted comments and the in-progress draft anchored to the
    /// patch line just pushed, recording where the draft's cursor lands.
    fn push_annotations(
        &self,
        lines: &mut Vec<Line<'static>>,
        anchor: PatchAnchor,
        width: u16,
        theme: &Theme,
        draft_cursor: &mut Option<(usize, u16)>,
    ) {
        for comment in self.review_queue.comments().iter().filter(|comment| comment.anchor == anchor) {
            lines.extend(comment_box(
                "┌─ Comment ─",
                &wrap_text_char(&comment.body, comment_body_width(width)),
                theme.info,
                width,
                theme,
            ));
        }

        let Some(draft) = self.draft.as_ref().filter(|draft| draft.anchor == anchor) else {
            return;
        };
        let (body, cursor) = draft_body(draft, comment_body_width(width));
        *draft_cursor = Some((lines.len() + cursor.0, cursor.1));
        lines.extend(comment_box("┌ Draft ─", &body, theme.accent, width, theme));
    }

    fn anchor(&self, hunk: usize, line: usize) -> PatchAnchor {
        PatchAnchor { file_index: self.selected_file, hunk, line }
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        match &self.bottom_bar {
            BottomBar::CommitEditor { buffer } => {
                let input = TextInput::new(buffer)
                    .prefix("commit › ")
                    .prefix_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD))
                    .style(Style::new().fg(theme.text_primary));
                let cursor = input.cursor_position(area);
                input.render(area, buf);
                Some(Position::new(cursor.x.min(area.right().saturating_sub(1)), cursor.y))
            }
            BottomBar::DiscardConfirmation { path, status } => {
                Paragraph::new(Line::from(vec![
                    Span::styled("Discard changes to ", Style::new().fg(theme.warning)),
                    Span::styled(path.clone(), Style::new().fg(theme.warning).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" ({})?  ", status.label()), Style::new().fg(theme.warning)),
                    Span::styled("y", Style::new().fg(theme.accent)),
                    Span::styled(" confirm  ", Style::new().fg(theme.muted)),
                    Span::styled("n", Style::new().fg(theme.accent)),
                    Span::styled(" cancel", Style::new().fg(theme.muted)),
                ]))
                .render(area, buf);
                None
            }
            BottomBar::Error(error) => {
                notice(error.clone(), theme.error).render(area, buf);
                None
            }
            BottomBar::Help => {
                let queued = self.review_queue.len();
                let count = if queued > 0 { format!(" ({})", plural(queued, "comment")) } else { String::new() };
                let keys = if self.focus == Focus::Drawer {
                    "j/k move · h/l pane · space stage · a/A all · t scope · C commit · d discard · o full file · r refresh"
                } else {
                    "j/k scroll · c comment · s submit · u undo · h/l pane · space stage · C commit · d discard · o full file · r refresh"
                };
                notice(format!("{keys}{count} · Ctrl-G/Esc close"), theme.muted).render(area, buf);
                None
            }
        }
    }
}

impl Screen for GitDiffScreen {
    fn on_key(&mut self, key: crossterm::event::KeyEvent) -> crate::screens::ScreenOutcome {
        self.handle_key(key)
    }

    fn on_event(&mut self, event: crate::screens::ScreenEvent) -> Option<crate::screens::ScreenEffect> {
        self.handle_event(event)
    }

    fn on_mouse(&mut self, action: crate::screens::MouseAction, row: u16, column: u16) {
        self.handle_mouse(action, row, column);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        self.render_screen(area, buf, cx)
    }
}

/// Drawer width for a body of `total` columns: a third, within sane bounds.
pub(super) fn drawer_width(total: u16) -> u16 {
    (total / 3).clamp(24, 36)
}

fn notice(text: impl Into<String>, color: ratatui::style::Color) -> Paragraph<'static> {
    Paragraph::new(Line::styled(text.into(), Style::new().fg(color)))
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

fn styled_full_width(text: &str, width: u16, color: ratatui::style::Color) -> Line<'static> {
    let style = Style::new().fg(color);
    fit_line(Line::styled(text.to_string(), style), usize::from(width), style)
}

fn render_unified_line(
    line: &PatchLine,
    language: &str,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    match line.kind {
        PatchLineKind::HunkHeader => return styled_full_width(&line.text, width, theme.info),
        PatchLineKind::Meta => return styled_full_width(&line.text, width, theme.muted),
        _ => {}
    }
    let (marker, foreground, background) = match line.kind {
        PatchLineKind::Added => ('+', theme.diff_added_fg, theme.diff_added_bg),
        PatchLineKind::Removed => ('-', theme.diff_removed_fg, theme.diff_removed_bg),
        _ => (' ', theme.text_secondary, theme.background),
    };
    let old = line.old_line_no.map_or_else(|| "    ".to_string(), |number| format!("{number:>4}"));
    let new = line.new_line_no.map_or_else(|| "    ".to_string(), |number| format!("{number:>4}"));
    let style = Style::new().fg(foreground).bg(background);
    let mut spans = vec![Span::styled(format!("{old} {new} {marker} "), style)];
    spans.extend(highlighted_spans(&line.text, language, background, theme, highlighter));
    fit_line(Line::from(spans).style(Style::new().bg(background)), usize::from(width), style)
}

fn render_split_row(
    old: Option<&PatchLine>,
    new: Option<&PatchLine>,
    language: &str,
    left_width: u16,
    right_width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let mut spans = render_split_side(old, true, language, left_width, theme, highlighter).spans;
    spans.push(Span::styled("│", Style::new().fg(theme.muted).bg(theme.background)));
    spans.extend(render_split_side(new, false, language, right_width, theme, highlighter).spans);
    Line::from(spans)
}

fn render_split_side(
    line: Option<&PatchLine>,
    old_side: bool,
    language: &str,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    let (foreground, background) = match line.map(|line| line.kind) {
        Some(PatchLineKind::Removed) => (theme.diff_removed_fg, theme.diff_removed_bg),
        Some(PatchLineKind::Added) => (theme.diff_added_fg, theme.diff_added_bg),
        _ => (theme.text_secondary, theme.background),
    };
    let style = Style::new().fg(foreground).bg(background);
    let mut spans = match line {
        Some(line) => {
            let number = if old_side { line.old_line_no } else { line.new_line_no };
            vec![Span::styled(number.map_or_else(|| "     ".to_string(), |number| format!("{number:>4} ")), style)]
        }
        None => vec![Span::styled("     ", style)],
    };
    if let Some(line) = line {
        spans.extend(highlighted_spans(&line.text, language, background, theme, highlighter));
    }
    fit_line(Line::from(spans).style(Style::new().bg(background)), usize::from(width), style)
}

fn highlighted_spans(
    source: &str,
    language: &str,
    background: ratatui::style::Color,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Span<'static>> {
    highlighter
        .highlight(source, language, theme)
        .into_iter()
        .next()
        .unwrap_or_else(|| Line::raw(source.to_string()))
        .spans
        .into_iter()
        .map(|mut span| {
            span.style = span.style.patch(Style::new().bg(background));
            span
        })
        .collect()
}

/// A patch line paired with its index into the owning hunk's `lines` vector, used to
/// preserve per-line comment anchors in the split view.
struct SplitSide<'a> {
    line: &'a PatchLine,
    idx: usize,
}

/// Aligns a contiguous removed/added block using a real line-level diff so that unchanged
/// lines within the block stay paired, instead of naïvely pairing removed[i] with added[i].
fn pair_changed_block<'a>(
    removed: &'a [SplitSide<'a>],
    added: &'a [SplitSide<'a>],
) -> Vec<(Option<&'a SplitSide<'a>>, Option<&'a SplitSide<'a>>)> {
    let old: Vec<&str> = removed.iter().map(|side| side.line.text.as_str()).collect();
    let new: Vec<&str> = added.iter().map(|side| side.line.text.as_str()).collect();
    let diff = similar::TextDiff::from_slices(&old, &new);
    let mut rows = Vec::new();

    for op in diff.ops() {
        match *op {
            similar::DiffOp::Equal { old_index, new_index, len } => {
                for offset in 0..len {
                    rows.push((Some(&removed[old_index + offset]), Some(&added[new_index + offset])));
                }
            }
            similar::DiffOp::Delete { old_index, old_len, .. } => {
                for side in &removed[old_index..old_index + old_len] {
                    rows.push((Some(side), None));
                }
            }
            similar::DiffOp::Insert { new_index, new_len, .. } => {
                for side in &added[new_index..new_index + new_len] {
                    rows.push((None, Some(side)));
                }
            }
            similar::DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let pair_len = old_len.min(new_len);
                for offset in 0..pair_len {
                    rows.push((Some(&removed[old_index + offset]), Some(&added[new_index + offset])));
                }
                for side in &removed[old_index + pair_len..old_index + old_len] {
                    rows.push((Some(side), None));
                }
                for side in &added[new_index + pair_len..new_index + new_len] {
                    rows.push((None, Some(side)));
                }
            }
        }
    }

    rows
}

/// Width available inside a comment box after its border and quote marker.
fn comment_body_width(width: u16) -> usize {
    usize::from(width).saturating_sub(4).max(10).saturating_sub(3)
}

/// Draws a boxed annotation beneath a patch line, used for both submitted
/// comments and the draft being typed.
fn comment_box(
    title: &str,
    body: &[String],
    border_color: ratatui::style::Color,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let border = Style::new().fg(border_color);
    let body_style = Style::new().fg(theme.text_primary);
    let width = usize::from(width);

    let mut lines = vec![fit_line(Line::styled(title.to_string(), border), width, border)];
    lines.extend(body.iter().map(|text| {
        fit_line(
            Line::styled(format!("│ > {text}"), body_style),
            width,
            body_style.patch(Style::new().bg(theme.background)),
        )
    }));
    lines.push(fit_line(Line::styled("└", border), width, border));
    lines
}

/// Wrapped draft text plus the cursor's (row, column) within the rendered box.
fn draft_body(draft: &DraftState, body_width: usize) -> (Vec<String>, (usize, u16)) {
    if draft.buffer.is_empty() {
        return (vec!["█".to_string()], (1, 3));
    }
    let text = draft.buffer.text();
    let (row, column) = text_position_in_wrap(&text[..draft.buffer.cursor()], body_width);
    (wrap_text_char(text, body_width), (1 + row, 3 + column))
}

fn file_status_color(status: FileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        FileStatus::Modified => theme.warning,
        FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg,
        FileStatus::Deleted => theme.diff_removed_fg,
        FileStatus::Renamed => theme.info,
    }
}
