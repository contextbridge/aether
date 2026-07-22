use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::diff::SPLIT_VIEW_MIN_WIDTH;
use crate::git_diff::{FileDiff, FileStatus, PatchAnchor, PatchLine, PatchLineKind, StageState};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::widgets::TextInput;
use crate::wrap::{fit_line, text_position_in_wrap, wrap_text_char};

use super::GitDiffScreen;
use super::state::{BottomBar, DraftState, DrawerEntry, Focus, GitDiffLoadState};

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
        let items = entries.iter().map(|entry| ListItem::new(self.drawer_line(entry, usize::from(area.width), theme)));
        let list = List::new(items)
            .highlight_style(Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(list, area, self.drawer_selection.list_state_mut());

        let mut scrollbar_state = ScrollbarState::new(entries.len()).position(self.drawer_selection.offset());
        frame.render_stateful_widget(Scrollbar::new(ScrollbarOrientation::VerticalRight), area, &mut scrollbar_state);
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

        let mut draft_cursor = None;
        let lines = if self.show_full_file {
            self.render_full_file(&file, content_area.width, theme, highlighter)
        } else if file.binary {
            vec![Line::styled("Binary file", Style::new().fg(theme.muted))]
        } else if area.width >= SPLIT_VIEW_MIN_WIDTH {
            self.render_split_with_comments(&file, area.width, theme, highlighter, &mut draft_cursor)
        } else {
            self.render_unified_with_comments(&file, area.width, theme, highlighter, &mut draft_cursor)
        };
        let offset_key = if self.show_full_file { format!("full:{}", file.path) } else { file.path.clone() };
        let scroll = self.scroll_offsets.entry(offset_key).or_default();
        scroll.vertical = scroll.vertical.min(lines.len().saturating_sub(1));
        let line_count = lines.len();
        let vertical = u16::try_from(scroll.vertical).unwrap_or(u16::MAX);
        frame.render_widget(Paragraph::new(Text::from(lines)).scroll((vertical, 0)), content_area);
        let mut scrollbar_state = ScrollbarState::new(line_count).position(scroll.vertical);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            content_area,
            &mut scrollbar_state,
        );

        if let Some((draft_line, draft_col)) = draft_cursor
            && draft_line >= scroll.vertical
            && draft_line < scroll.vertical + usize::from(content_area.height)
        {
            let row = content_area.y + u16::try_from(draft_line - scroll.vertical).unwrap_or(u16::MAX);
            let col = (content_area.x + draft_col).min(content_area.right().saturating_sub(1));
            frame.set_cursor_position((col, row));
        }
    }

    fn render_full_file(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> Vec<Line<'static>> {
        if file.status == FileStatus::Deleted {
            return vec![Line::styled("File has been deleted", Style::new().fg(theme.muted))];
        }
        if file.binary {
            return vec![Line::styled("Binary file — cannot display contents", Style::new().fg(theme.muted))];
        }
        match &self.full_file_content {
            None => {
                vec![Line::styled("Loading file…", Style::new().fg(theme.muted))]
            }
            Some(content) => {
                let language = file.language();
                let background = theme.background;
                content
                    .lines()
                    .enumerate()
                    .map(|(index, text)| {
                        let line_no = format!("{:>4} ", index + 1);
                        let style = Style::new().fg(theme.text_secondary).bg(background);
                        let mut spans = vec![Span::styled(line_no, style)];
                        spans.extend(highlighted_spans(text, language, background, theme, highlighter));
                        fit_line(
                            Line::from(spans).style(Style::new().bg(background)),
                            usize::from(width),
                            Style::new().fg(theme.text_secondary).bg(background),
                        )
                    })
                    .collect()
            }
        }
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
    fn render_unified_with_comments(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
        draft_cursor: &mut Option<(usize, u16)>,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
            for (line_idx, patch_line) in hunk.lines.iter().enumerate() {
                let anchor = PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: line_idx };
                let rendered = render_unified_line(patch_line, file.language(), width, theme, highlighter);
                let is_cursor = self.focus == Focus::Patch
                    && self.patch_cursor.hunk == hunk_idx
                    && self.patch_cursor.line == line_idx
                    && self.draft.is_none();
                if is_cursor {
                    let cursor_line = add_cursor_indicator(rendered, theme);
                    lines.push(cursor_line);
                } else {
                    lines.push(rendered);
                }
                lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                if let Some(draft) = &self.draft
                    && draft.anchor == anchor
                {
                    let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                    if let Some((line, col)) = cursor {
                        *draft_cursor = Some((lines.len() + line, col));
                    }
                    lines.extend(draft_lines);
                }
            }
        }
        lines
    }

    #[allow(clippy::too_many_lines)]
    fn render_split_with_comments(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
        draft_cursor: &mut Option<(usize, u16)>,
    ) -> Vec<Line<'static>> {
        let left_width = width.saturating_sub(1) / 2;
        let right_width = width.saturating_sub(left_width + 1);
        let mut lines = Vec::new();
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
                    let is_cursor = self.focus == Focus::Patch
                        && self.draft.is_none()
                        && self.patch_cursor.hunk == hunk_idx
                        && self.patch_cursor.line == index;
                    if is_cursor {
                        lines.push(add_cursor_indicator(row, theme));
                    } else {
                        lines.push(row);
                    }
                    lines.extend(self.render_comments_for_anchor(anchor, width, theme));
                    if let Some(draft) = &self.draft
                        && draft.anchor == anchor
                    {
                        let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                        if let Some((line, col)) = cursor {
                            *draft_cursor = Some((lines.len() + line, col));
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
                    let removed = &hunk.lines[removed_start..added_start];
                    let added = &hunk.lines[added_start..index];
                    let mut block_last_line_idx = removed_start;
                    for offset in 0..removed.len().max(added.len()) {
                        block_last_line_idx = removed_start + offset;
                        let row = render_split_row(
                            removed.get(offset),
                            added.get(offset),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        );
                        let is_cursor = self.focus == Focus::Patch
                            && self.draft.is_none()
                            && self.patch_cursor.hunk == hunk_idx
                            && self.patch_cursor.line == block_last_line_idx;
                        if is_cursor {
                            lines.push(add_cursor_indicator(row, theme));
                        } else {
                            lines.push(row);
                        }
                    }
                    let final_anchor =
                        PatchAnchor { file_index: self.selected_file, hunk: hunk_idx, line: block_last_line_idx };
                    lines.extend(self.render_comments_for_anchor(final_anchor, width, theme));
                    if let Some(draft) = &self.draft
                        && draft.anchor.hunk == hunk_idx
                        && draft.anchor.line >= removed_start
                        && draft.anchor.line < index
                    {
                        let (draft_lines, cursor) = render_draft_comment(draft, width, theme);
                        if let Some((line, col)) = cursor {
                            *draft_cursor = Some((lines.len() + line, col));
                        }
                        lines.extend(draft_lines);
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
                    let is_cursor = self.focus == Focus::Patch
                        && self.draft.is_none()
                        && self.patch_cursor.hunk == hunk_idx
                        && self.patch_cursor.line == index;
                    if is_cursor {
                        lines.push(add_cursor_indicator(row, theme));
                    } else {
                        lines.push(row);
                    }
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
                    let is_cursor = self.focus == Focus::Patch
                        && self.draft.is_none()
                        && self.patch_cursor.hunk == hunk_idx
                        && self.patch_cursor.line == index;
                    if is_cursor {
                        lines.push(add_cursor_indicator(row, theme));
                    } else {
                        lines.push(row);
                    }
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
                        *draft_cursor = Some((lines.len() + line, col));
                    }
                    lines.extend(draft_lines);
                }
                index += 1;
            }
        }
        lines
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

fn add_cursor_indicator(line: Line<'static>, theme: &Theme) -> Line<'static> {
    Line::from(line.spans).style(Style::new().bg(theme.accent).fg(theme.background))
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
