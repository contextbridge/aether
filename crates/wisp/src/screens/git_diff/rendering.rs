use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, StatefulWidget, Widget};
use std::collections::HashSet;

use crate::renderer::DrawContext;
use crate::screens::annotation::{AnnotatedRows, Row, comment_body_width, comment_box, draft_body};
use crate::git_review::{FileDiff, FileStatus, GitDiffEvent, PatchAnchor, PatchLineKind, StageState};
use crate::screens::review::{
    Pane, ShortcutGroup, body_and_footer, focused_title, render_command_bar, render_shortcut_help,
};
use crate::surfaces::input::{GitReviewOutput, MouseAction, UiEvent, is_press};
use crate::view::diff::{DiffRowKind, DiffTone, diff_line, diff_rows};
use crate::view::list_view::ListView;
use crate::view::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::view::widgets::TextInput;
use crate::view::wrap::{fit_line, wrap_text_char};

use super::GitDiffScreen;
use super::state::{BottomBar, DrawerEntry, FullFileView, GitDiffLoadState, PatchCursor, PatchRow};

pub(super) const DRAWER_MIN_WIDTH: u16 = 72;

const GIT_SHORTCUTS: &[ShortcutGroup] = &[
    ShortcutGroup {
        title: "Navigation",
        hints: &[
            ("j / k", "move or scroll"),
            ("h / l", "change pane"),
            ("Enter", "open file"),
            ("PgUp / PgDn", "scroll a page"),
        ],
    },
    ShortcutGroup {
        title: "Review",
        hints: &[("c", "add comment"), ("s", "submit review"), ("u", "undo comment")],
    },
    ShortcutGroup {
        title: "Git",
        hints: &[
            ("Space", "stage file"),
            ("a / A", "stage / unstage all"),
            ("C", "Commit"),
            ("d", "discard file"),
        ],
    },
    ShortcutGroup {
        title: "View",
        hints: &[
            ("t", "change scope"),
            ("o", "show full file"),
            ("r", "refresh"),
            ("< / >", "resize file list"),
            ("Esc / Ctrl-G", "close review"),
        ],
    },
];

/// Narrowest the drawer can be resized to before it stops being a usable file list.
const DRAWER_MIN_COLUMNS: u16 = 16;

/// The patch as rendered rows, with review comments woven in.
type PatchRowSet = AnnotatedRows<PatchCursor>;

impl GitDiffScreen {
    pub(super) fn render_screen(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        Clear.render(area, buf);
        let block = Block::bordered()
            .title(format!(" Git Diff · {} ", self.scope.label()))
            .border_style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD));
        let inner = block.inner(area);
        block.render(area, buf);

        let (body, footer) = body_and_footer(inner);
        let cursor = match &self.state {
            GitDiffLoadState::Loading { .. } => {
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

        let footer_cursor = self.render_footer(footer, buf, theme);
        if self.shortcuts_open {
            render_shortcut_help(area, buf, theme, GIT_SHORTCUTS);
            None
        } else {
            footer_cursor.or(cursor)
        }
    }

    fn render_document(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        if area.width < DRAWER_MIN_WIDTH {
            return self.render_patch(area, buf, cx);
        }
        let [drawer, separator, patch] = Layout::horizontal([
            Constraint::Length(self.drawer_width(area.width)),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(area);
        self.render_drawer(drawer, buf, cx.theme);
        Paragraph::new("│").style(Style::new().fg(cx.theme.muted)).render(separator, buf);
        self.render_patch(patch, buf, cx)
    }

    fn render_drawer(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let rows: Vec<Line<'static>> =
            self.drawer_entries().iter().map(|entry| self.drawer_line(entry, theme)).collect();
        let view = ListView::new(rows, theme)
            .highlight_style(Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD))
            .scrollbar();
        StatefulWidget::render(view, area, buf, &mut self.drawer_selection);
    }

    fn drawer_line(&self, entry: &DrawerEntry, theme: &Theme) -> Line<'static> {
        match entry {
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
                        Style::new().fg(match file.status {
                            FileStatus::Modified => theme.warning,
                            FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg,
                            FileStatus::Deleted => theme.diff_removed_fg,
                            FileStatus::Renamed => theme.info,
                        }),
                    ),
                    Span::raw(format!(" {name}")),
                    Span::styled(format!(" +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
                ])
            }
        }
    }

    /// Drawer width for a body of `total` columns: a third of it, moved by
    /// however far the reviewer has resized the drawer. The offset is re-anchored
    /// to the width actually granted, so the resize keys bite on the first press
    /// after the drawer has hit an edge. Only valid for a body at least
    /// [`DRAWER_MIN_WIDTH`] wide, which is the only width that draws a drawer.
    fn drawer_width(&self, total: u16) -> u16 {
        let natural = (total / 3).clamp(24, 36);
        natural.saturating_add_signed(self.drawer_offset).clamp(DRAWER_MIN_COLUMNS, total / 2)
    }

    fn render_patch(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        let file = self.selected_file().cloned()?;
        let [header_area, content_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(self.patch_header(&file, theme)).render(header_area, buf);

        if let Some(message) = self.patch_placeholder(&file) {
            notice(message, theme.muted).render(content_area, buf);
            return None;
        }

        let content_width = PatchRowSet::content_width(content_area.width);
        let source = if self.full_file.is_on() {
            self.build_full_file_rows(&file, content_width, theme, cx.highlighter)
        } else {
            build_patch_rows(&file, content_width, theme, cx.highlighter)
        };
        let mut rows = AnnotatedRows::default();
        for row in &source {
            rows.push_row(row);
            if let Some(cursor) = row.anchor() {
                self.push_annotations(
                    &mut rows,
                    PatchAnchor { file_index: self.selected_file, hunk: cursor.hunk, line: cursor.line },
                    content_width,
                    theme,
                );
            }
        }

        let mark_cursor = self.focus == Pane::Document && self.review.draft.is_none();
        self.patch.document.render_rows(rows, content_area, buf, theme, mark_cursor);
        self.patch.document.draft_cursor_position()
    }

    fn patch_header(&self, file: &FileDiff, theme: &Theme) -> Line<'static> {
        let header_style = focused_title(self.focus == Pane::Document, theme);
        let mut spans = vec![
            Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
            Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
        ];

        let comments = self.review.queue.comments_for_file(&file.path).count();
        if self.full_file.is_on() {
            spans.push(Span::styled("  [full file]", Style::new().fg(theme.info)));
        } else if comments > 0 {
            spans.push(Span::styled(format!("  {}", plural(comments, "comment")), Style::new().fg(theme.info)));
        }
        Line::from(spans)
    }

    /// Message to show instead of a patch, when there is nothing to diff.
    fn patch_placeholder(&self, file: &FileDiff) -> Option<&'static str> {
        match &self.full_file {
            FullFileView::Off => file.binary.then_some("Binary file"),
            _ if file.status == FileStatus::Deleted => Some("File has been deleted"),
            _ if file.binary => Some("Binary file — cannot display contents"),
            FullFileView::Loading => Some("Loading file…"),
            FullFileView::Loaded(_) => None,
        }
    }

    fn build_full_file_rows(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> Vec<PatchRow> {
        let FullFileView::Loaded(content) = &self.full_file else {
            return vec![Row::inert(Line::styled("Loading file…", Style::new().fg(theme.muted)))];
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
        content
            .lines()
            .enumerate()
            .map(|(index, text)| {
                let line_no = index + 1;
                let tone = if added_lines.contains(&line_no) { DiffTone::Added } else { DiffTone::Context };
                let rendered = diff_line(&format!("{line_no:>4} "), text, language, tone, theme, highlighter);
                Row::at(
                    fit_line(rendered.line, usize::from(width), rendered.fill),
                    PatchCursor { hunk: 0, line: index },
                )
            })
            .collect()
    }

    /// Appends the submitted comments and the in-progress draft anchored to the
    /// patch line just pushed.
    fn push_annotations(&self, rows: &mut PatchRowSet, anchor: PatchAnchor, width: u16, theme: &Theme) {
        for comment in self.review.queue.comments().iter().filter(|comment| comment.anchor == anchor) {
            rows.push_annotation(comment_box(
                "┌─ Comment ─",
                &wrap_text_char(&comment.body, comment_body_width(width)),
                theme.info,
                width,
                theme,
            ));
        }

        let Some(draft) = self.review.draft.as_ref().filter(|draft| draft.anchor == anchor) else {
            return;
        };
        let (body, cursor) = draft_body(draft, comment_body_width(width));
        rows.push_draft(comment_box("┌ Draft ─", &body, theme.accent, width, theme), cursor);
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
                if self.review.draft.is_some() {
                    render_command_bar(
                        area,
                        buf,
                        theme,
                        &[("Enter", "add comment"), ("Esc", "cancel")],
                        None,
                        false,
                    );
                    return None;
                }

                let actions = if self.focus == Pane::Nav {
                    &[("Enter", "open"), ("Space", "stage"), ("l", "diff")][..]
                } else {
                    &[("c", "comment"), ("s", "submit"), ("h", "files")][..]
                };
                let queued = self.review.queue.len();
                let status = (queued > 0).then(|| plural(queued, "comment"));
                render_command_bar(area, buf, theme, actions, status.as_deref(), true);
                None
            }
        }
    }
}

impl GitDiffScreen {
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<GitReviewOutput> {
        match event {
            UiEvent::Key(key) if is_press(key) => self.handle_key(key),
            UiEvent::Key(_) => Vec::new(),
            UiEvent::Paste(text) => {
                self.handle_paste(&text);
                Vec::new()
            }
            UiEvent::Mouse(action, (column, row)) => {
                self.handle_mouse(action, row, column);
                Vec::new()
            }
        }
    }

    pub fn on_event(&mut self, event: GitDiffEvent) -> Vec<GitReviewOutput> {
        self.handle_event(event)
    }

    pub fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<GitReviewOutput> {
        self.handle_mouse(action, row, column);
        Vec::new()
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        self.render_screen(area, buf, cx)
    }
}

/// The full-screen patch: every canonical diff row, cursor-navigable except for
/// metadata rows, which anchor without taking the cursor.
fn build_patch_rows(file: &FileDiff, width: u16, theme: &Theme, highlighter: &mut SyntaxHighlighter) -> Vec<PatchRow> {
    diff_rows(file, width, theme, highlighter)
        .into_iter()
        .map(|row| {
            let cursor = PatchCursor { hunk: row.hunk, line: row.index };
            match row.kind {
                DiffRowKind::Content | DiffRowKind::HunkHeader => Row::at(row.line, cursor),
                DiffRowKind::Meta => Row::anchored(row.line, cursor),
            }
        })
        .collect()
}

fn notice(text: impl Into<String>, color: ratatui::style::Color) -> Paragraph<'static> {
    Paragraph::new(Line::styled(text.into(), Style::new().fg(color)))
}

pub(super) fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}
