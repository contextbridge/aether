use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use std::collections::HashSet;
use tui_scrollview::{ScrollView, ScrollbarVisibility};

use crate::diff::SPLIT_VIEW_MIN_WIDTH;
use crate::git_diff::{FileDiff, FileStatus, PatchAnchor, PatchLine, PatchLineKind, StageState};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::widgets::TextInput;
use crate::wrap::{fit_line, text_position_in_wrap, wrap_text_char};

use super::GitDiffScreen;
use super::state::{BottomBar, CursorRow, DiffView, DraftState, DrawerEntry, Focus, GitDiffLoadState};

type BuildResult = (Vec<Line<'static>>, Vec<CursorRow>, Option<(usize, u16)>);

impl GitDiffScreen {
    pub fn render(&mut self, frame: &mut Frame, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        let area = frame.area();
        self.last_area = area;
        frame.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Git Diff · {} ", self.scope.label()))
            .border_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        match &self.state {
            GitDiffLoadState::Loading => {
                frame.render_widget(
                    Paragraph::new(Line::styled("Loading changes…", Style::new().fg(theme.muted))),
                    body,
                );
            }
            GitDiffLoadState::Error(message) => {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        format!("Git diff unavailable: {message}"),
                        Style::new().fg(theme.error),
                    )),
                    body,
                );
            }
            GitDiffLoadState::Ready(document) if document.files.is_empty() => {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        "No changes in working tree for this scope",
                        Style::new().fg(theme.muted),
                    )),
                    body,
                );
            }
            GitDiffLoadState::Ready(_) => self.render_document(frame, body, theme, highlighter),
        }

        self.render_footer(frame, footer, theme);
    }
    fn render_document(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        if area.width >= 72 {
            let drawer_width = (area.width / 3).clamp(24, 36);
            let [drawer, separator, patch] =
                Layout::horizontal([Constraint::Length(drawer_width), Constraint::Length(1), Constraint::Min(1)])
                    .areas(area);
            self.render_drawer(frame, drawer, theme);
            frame.render_widget(Paragraph::new("│").style(Style::new().fg(theme.muted)), separator);
            self.render_patch(frame, patch, theme, highlighter);
        } else {
            self.render_patch(frame, area, theme, highlighter);
        }
    }

    fn render_drawer(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let entries = self.drawer_entries();
        self.drawer_selection.ensure_visible(entries.len(), usize::from(area.height));
        let [list_area, track_area] = Layout::horizontal([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        let items =
            entries.iter().map(|entry| ListItem::new(self.drawer_line(entry, usize::from(list_area.width), theme)));
        let list = List::new(items)
            .highlight_style(Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(list, list_area, self.drawer_selection.list_state_mut());

        let mut scrollbar_state = ScrollbarState::new(entries.len()).position(self.drawer_selection.offset());
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            track_area,
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

    fn render_patch(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        let Some(file) = self.selected_file().cloned() else {
            return;
        };
        let header_style = if self.focus == Focus::Patch {
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text_primary).add_modifier(Modifier::BOLD)
        };
        let comment_count = self.review_queue.comments_for_file(&file.path).count();
        let header = if self.show_full_file {
            Line::from(vec![
                Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
                Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                Span::styled("  [full file]", Style::new().fg(theme.info)),
            ])
        } else if comment_count > 0 {
            Line::from(vec![
                Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
                Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                Span::styled(
                    format!("  {comment_count} comment{}", if comment_count == 1 { "" } else { "s" }),
                    Style::new().fg(theme.info),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
                Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
            ])
        };
        let [header_area, content_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(header), header_area);
        self.last_patch_height = content_area.height;

        if self.show_full_file
            && (file.status == FileStatus::Deleted || file.binary || self.full_file_content.is_none())
        {
            let message = if file.status == FileStatus::Deleted {
                "File has been deleted"
            } else if file.binary {
                "Binary file — cannot display contents"
            } else {
                "Loading file…"
            };
            frame.render_widget(Paragraph::new(Line::styled(message, Style::new().fg(theme.muted))), content_area);
            return;
        }
        if !self.show_full_file && file.binary {
            frame
                .render_widget(Paragraph::new(Line::styled("Binary file", Style::new().fg(theme.muted))), content_area);
            return;
        }

        self.ensure_diff_view(&file, content_area.width, theme, highlighter);

        if let Some(view) = self.diff_view.as_ref() {
            frame.render_stateful_widget(&view.scroll_view, content_area, &mut self.patch_scroll_state);
        }

        self.overlay_cursor_indicator(frame, content_area, theme);

        if let Some(view) = &self.diff_view
            && let Some((draft_row, draft_col)) = view.draft_cursor
        {
            let offset = self.patch_scroll_state.offset().y as usize;
            if draft_row >= offset && draft_row < offset + usize::from(content_area.height) {
                let row = content_area.y + u16::try_from(draft_row - offset).unwrap_or(u16::MAX);
                let col = (content_area.x + draft_col).min(content_area.right().saturating_sub(1));
                frame.set_cursor_position((col, row));
            }
        }
    }

    fn ensure_diff_view(
        &mut self,
        file: &FileDiff,
        content_width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) {
        let split = content_width >= SPLIT_VIEW_MIN_WIDTH && !self.show_full_file;
        let draft_sig =
            self.draft.as_ref().map(|d| (d.anchor.hunk, d.anchor.line, d.buffer.text().len(), d.buffer.cursor()));

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
        let cache_hit = self
            .diff_view_cache
            .iter()
            .position(signature_matches)
            .map(|pos| self.diff_view_cache.remove(pos));

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

        let line_count = lines.len();
        let content_height = u16::try_from(line_count.max(1)).unwrap_or(u16::MAX);
        let mut scroll_view = ScrollView::new(Size::new(content_width, content_height))
            .vertical_scrollbar_visibility(ScrollbarVisibility::Automatic);
        scroll_view.render_widget(Paragraph::new(Text::from(lines)), Rect::new(0, 0, content_width, content_height));

        let draft_sig =
            self.draft.as_ref().map(|d| (d.anchor.hunk, d.anchor.line, d.buffer.text().len(), d.buffer.cursor()));

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
            draft_signature: draft_sig,
        }
    }

    fn overlay_cursor_indicator(&self, frame: &mut Frame, content_area: Rect, theme: &Theme) {
        if self.focus != Focus::Patch || self.draft.is_some() {
            return;
        }
        let Some(view) = &self.diff_view else {
            return;
        };
        let cursor_row = view
            .cursor_rows
            .iter()
            .find_map(|(h, l, row)| (*h == self.patch_cursor.hunk && *l == self.patch_cursor.line).then_some(*row));
        let Some(cursor_row) = cursor_row else {
            return;
        };
        let offset = self.patch_scroll_state.offset().y as usize;
        let viewport = usize::from(content_area.height);
        if cursor_row < offset || cursor_row >= offset + viewport {
            return;
        }
        let y = content_area.y + u16::try_from(cursor_row - offset).unwrap_or(u16::MAX);
        let end_x = content_area.right().saturating_sub(1);
        let buf = frame.buffer_mut();
        for x in content_area.x..=end_x {
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
        let background = theme.background;
        let mut lines = Vec::new();
        let mut cursor_rows = Vec::new();
        for (index, text) in content.lines().enumerate() {
            let line_no = format!("{:>4} ", index + 1);
            let is_added = added_lines.contains(&(index + 1));
            let (foreground, background) =
                if is_added { (theme.diff_added_fg, theme.diff_added_bg) } else { (theme.text_secondary, background) };
            let style = Style::new().fg(foreground).bg(background);
            let mut spans = vec![Span::styled(line_no, style)];
            spans.extend(highlighted_spans(text, language, background, theme, highlighter));
            cursor_rows.push((0, index, lines.len()));
            lines.push(fit_line(
                Line::from(spans).style(Style::new().bg(background)),
                usize::from(width),
                Style::new().fg(foreground).bg(background),
            ));
        }
        (lines, cursor_rows, None)
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match &self.bottom_bar {
            BottomBar::CommitEditor { buffer } => {
                let input = TextInput::new(buffer)
                    .prefix("commit › ")
                    .prefix_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD))
                    .style(Style::new().fg(theme.text_primary));
                let cursor = input.cursor_position(area);
                frame.render_widget(input, area);
                frame.set_cursor_position((cursor.x.min(area.right().saturating_sub(1)), cursor.y));
            }
            BottomBar::DiscardConfirmation { path, status } => {
                let status_label = format!("({})", status.label());
                let line = Line::from(vec![
                    Span::styled("Discard changes to ", Style::new().fg(theme.warning)),
                    Span::styled(path.clone(), Style::new().fg(theme.warning).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {status_label}?  "), Style::new().fg(theme.warning)),
                    Span::styled("y", Style::new().fg(theme.accent)),
                    Span::styled(" confirm  ", Style::new().fg(theme.muted)),
                    Span::styled("n", Style::new().fg(theme.accent)),
                    Span::styled(" cancel", Style::new().fg(theme.muted)),
                ]);
                frame.render_widget(Paragraph::new(line), area);
            }
            BottomBar::Error(error) => {
                let line = Line::styled(error.clone(), Style::new().fg(theme.error));
                frame.render_widget(Paragraph::new(line), area);
            }
            BottomBar::Help => {
                let total = self.review_queue.len();
                let count_str = if total > 0 {
                    format!(" ({total} comment{})", if total == 1 { "" } else { "s" })
                } else {
                    String::new()
                };
                let hint = if self.focus == Focus::Drawer {
                    format!(
                        "j/k move · h/l pane · space stage · a/A all · t scope · C commit · d discard · o full file · r refresh{count_str} · Ctrl-G/Esc close"
                    )
                } else {
                    format!(
                        "j/k scroll · c comment · s submit · u undo · h/l pane · space stage · C commit · d discard · o full file · r refresh{count_str} · Ctrl-G/Esc close"
                    )
                };
                let line = Line::styled(hint, Style::new().fg(theme.muted));
                frame.render_widget(Paragraph::new(line), area);
            }
        }
    }
}

impl GitDiffScreen {
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
                let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: line_idx };
                let rendered = render_unified_line(patch_line, file.language(), width, theme, highlighter);
                cursor_rows.push((hunk_idx, line_idx, lines.len()));
                lines.push(rendered);
                lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                if let Some(draft) = &self.draft
                    && draft.anchor == anchor
                {
                    let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                    if let Some((line, col)) = cursor {
                        draft_cursor = Some((lines.len() + line, col));
                    }
                    lines.extend(draft_lines);
                }
            }
        }
        (lines, cursor_rows, draft_cursor)
    }

    #[allow(clippy::too_many_lines)]
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
                if line.kind == PatchLineKind::HunkHeader {
                    let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: index };
                    let row = fit_line(
                        Line::styled(line.text.clone(), Style::new().fg(theme.info)),
                        usize::from(width),
                        Style::new().fg(theme.info),
                    );
                    cursor_rows.push((hunk_idx, index, lines.len()));
                    lines.push(row);
                    lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                    if let Some(draft) = &self.draft
                        && draft.anchor == anchor
                    {
                        let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                        if let Some((line, col)) = cursor {
                            draft_cursor = Some((lines.len() + line, col));
                        }
                        lines.extend(draft_lines);
                    }
                    index += 1;
                    continue;
                }
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
                        let row = render_split_row(
                            left.map(|side| side.line),
                            right.map(|side| side.line),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        );
                        // Anchor comments on the added side when present, falling back to the
                        // removed side, so each line keeps its own comment slot.
                        let anchor_line = right.or(left).map_or(removed_start, |side| side.idx);
                        let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: anchor_line };
                        cursor_rows.push((hunk_idx, anchor_line, lines.len()));
                        lines.push(row);
                        lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                        if let Some(draft) = &self.draft
                            && draft.anchor == anchor
                        {
                            let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                            if let Some((line, col)) = cursor {
                                draft_cursor = Some((lines.len() + line, col));
                            }
                            lines.extend(draft_lines);
                        }
                    }
                    continue;
                }
                let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: index };
                if line.kind == PatchLineKind::Added {
                    let row = render_split_row(
                        None,
                        Some(line),
                        file.language(),
                        left_width,
                        right_width,
                        theme,
                        highlighter,
                    );
                    cursor_rows.push((hunk_idx, index, lines.len()));
                    lines.push(row);
                } else if line.kind == PatchLineKind::Context {
                    let row = render_split_row(
                        Some(line),
                        Some(line),
                        file.language(),
                        left_width,
                        right_width,
                        theme,
                        highlighter,
                    );
                    cursor_rows.push((hunk_idx, index, lines.len()));
                    lines.push(row);
                } else {
                    lines.push(fit_line(
                        Line::styled(line.text.clone(), Style::new().fg(theme.muted)),
                        usize::from(width),
                        Style::new().fg(theme.muted),
                    ));
                }
                lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                if let Some(draft) = &self.draft
                    && draft.anchor == anchor
                {
                    let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                    if let Some((line, col)) = cursor {
                        draft_cursor = Some((lines.len() + line, col));
                    }
                    lines.extend(draft_lines);
                }
                index += 1;
            }
        }
        (lines, cursor_rows, draft_cursor)
    }

    fn render_comments_for_anchor(&self, anchor: PatchAnchor, width: u16, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for comment in self.review_queue.comments() {
            if comment.anchor == anchor {
                lines.extend(render_submitted_comment(&comment.body, width, theme));
            }
        }
        lines
    }
}

fn render_unified_line(
    line: &PatchLine,
    language: &str,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Line<'static> {
    if line.kind == PatchLineKind::HunkHeader {
        return fit_line(
            Line::styled(line.text.clone(), Style::new().fg(theme.info)),
            usize::from(width),
            Style::new().fg(theme.info),
        );
    }
    if line.kind == PatchLineKind::Meta {
        return fit_line(
            Line::styled(line.text.clone(), Style::new().fg(theme.muted)),
            usize::from(width),
            Style::new().fg(theme.muted),
        );
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
    let mut spans = if let Some(line) = line {
        let number = if old_side { line.old_line_no } else { line.new_line_no };
        vec![Span::styled(number.map_or_else(|| "     ".to_string(), |number| format!("{number:>4} ")), style)]
    } else {
        vec![Span::styled("     ", style)]
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
/// Mirrors the original wisp `split_patch_renderer::pair_changed_block`.
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

fn render_submitted_comment(body: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let border_style = Style::new().fg(theme.info);
    let body_style = Style::new().fg(theme.text_primary);
    let inner_width = usize::from(width).saturating_sub(4).max(10);
    let body_width = inner_width.saturating_sub(3);

    let body_lines = wrap_text_char(body, body_width);

    let mut lines = Vec::new();
    let top = fit_line(Line::from(vec![Span::styled("┌─ Comment ─", border_style)]), usize::from(width), border_style);
    lines.push(top);
    for body_line in &body_lines {
        let content = format!("│ > {body_line}");
        let line = fit_line(
            Line::styled(content, body_style),
            usize::from(width),
            body_style.patch(Style::new().bg(theme.background)),
        );
        lines.push(line);
    }
    let bottom = fit_line(Line::from(vec![Span::styled("└", border_style)]), usize::from(width), border_style);
    lines.push(bottom);
    lines
}

fn render_draft_comment(draft: &DraftState, width: u16, theme: &Theme) -> (Vec<Line<'static>>, Option<(usize, u16)>) {
    let border_style = Style::new().fg(theme.accent);
    let body_style = Style::new().fg(theme.text_primary);
    let inner_width = usize::from(width).saturating_sub(4).max(10);
    let body_width = inner_width.saturating_sub(3);

    let body_lines: Vec<String>;
    let cursor: Option<(usize, u16)>;
    if draft.buffer.is_empty() {
        body_lines = vec!["█".to_string()];
        cursor = Some((1, 3));
    } else {
        let display = draft.buffer.text();
        body_lines = wrap_text_char(display, body_width);
        let text_before = &display[..draft.buffer.cursor()];
        let (cursor_line, cursor_col) = text_position_in_wrap(text_before, body_width);
        cursor = Some((1 + cursor_line, 3u16 + cursor_col));
    }

    let mut lines = Vec::new();
    let top = fit_line(Line::from(vec![Span::styled("┌ Draft ─", border_style)]), usize::from(width), border_style);
    lines.push(top);
    for body_line in &body_lines {
        let content = format!("│ > {body_line}");
        let line = fit_line(
            Line::styled(content, body_style),
            usize::from(width),
            body_style.patch(Style::new().bg(theme.background)),
        );
        lines.push(line);
    }
    let bottom = fit_line(Line::from(vec![Span::styled("└", border_style)]), usize::from(width), border_style);
    lines.push(bottom);
    (lines, cursor)
}

fn file_status_color(status: FileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        FileStatus::Modified => theme.warning,
        FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg,
        FileStatus::Deleted => theme.diff_removed_fg,
        FileStatus::Renamed => theme.info,
    }
}
