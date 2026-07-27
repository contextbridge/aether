use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};
use std::collections::HashSet;

use crate::annotation::{AnnotatedRows, Draft, draft_key};
use crate::diff::{DiffTone, SPLIT_VIEW_MIN_WIDTH, diff_line, join_split, split_side, split_widths};
use crate::git_diff::{FileDiff, FileStatus, PatchAnchor, PatchLine, PatchLineKind, StageState};
use crate::list_view::ListView;
use crate::render_context::RenderContext;
use crate::surface::{Action, MouseAction, Surface};
use crate::syntax::SyntaxHighlighter;
use crate::tasks::TaskResult;
use crate::theme::Theme;
use crate::widgets::{TextInput, key_hints, wrapped_with_cursor};
use crate::wrap::{fit_line, rows as rows_u16, wrap_text_char};

use super::GitDiffScreen;
use super::state::{
    BottomBar, DiffView, DiffViewKey, DrawerEntry, Focus, GitDiffLoadState, PatchCursor, PatchKey, PatchRow, PatchRows,
};

/// Below this width the file drawer is hidden and the patch gets the full area.
pub(super) const DRAWER_MIN_WIDTH: u16 = 72;

/// Narrowest the drawer can be resized to before it stops being a usable file list.
const DRAWER_MIN_COLUMNS: u16 = 16;

/// The patch as rendered rows, with review comments woven in.
type PatchRowSet = AnnotatedRows<PatchCursor>;

impl GitDiffScreen {
    pub(super) fn render_screen(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut RenderContext<'_>,
    ) -> Option<Position> {
        let theme = cx.theme;
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
            // No drawer at this width, so no rows for a click to land on.
            self.drawer_selection.set_rows_area(Rect::ZERO);
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
        ListView::new(rows, &mut self.drawer_selection, theme)
            .highlight_style(Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD))
            .scrollbar()
            .render(area, buf);
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
                        Style::new().fg(file_status_color(file.status, theme)),
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
    fn drawer_width(&mut self, total: u16) -> u16 {
        let natural = (total / 3).clamp(24, 36);
        let width = natural.saturating_add_signed(self.drawer_offset).clamp(DRAWER_MIN_COLUMNS, total / 2);
        self.drawer_offset = columns(width) - columns(natural);
        width
    }

    fn render_patch(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        let file = self.selected_file().cloned()?;
        let [header_area, content_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(self.patch_header(&file, theme)).render(header_area, buf);
        self.patch.height = content_area.height;

        if let Some(message) = self.patch_placeholder(&file) {
            notice(message, theme.muted).render(content_area, buf);
            return None;
        }

        self.ensure_diff_view(&file, PatchRowSet::content_width(content_area.width), theme, cx.highlighter);

        // The cursor is only marked while the reviewer is browsing: a draft
        // already has the terminal cursor sitting in its box.
        let cursor_row =
            (self.focus == Focus::Patch && self.review.draft.is_none()).then(|| self.cursor_row()).flatten();
        let view = self.patch.view.as_ref()?;
        view.rows.render(content_area, buf, self.patch.scroll, cursor_row, theme);
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

        let comments = self.review.queue.comments_for_file(&file.path).count();
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
        let (draft_row, draft_col) = self.patch.view.as_ref()?.rows.draft_cursor()?;
        let offset = self.patch.scroll;
        if draft_row < offset || draft_row >= offset + usize::from(content_area.height) {
            return None;
        }
        let row = content_area.y + rows_u16(draft_row - offset);
        let column = (content_area.x + draft_col).min(content_area.right().saturating_sub(1));
        Some(Position::new(column, row))
    }

    /// Makes `self.patch.view` a render of `file` at the current width, reusing
    /// the active view when nothing that affects it has changed.
    fn ensure_diff_view(
        &mut self,
        file: &FileDiff,
        content_width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) {
        let key = DiffViewKey {
            patch: PatchKey {
                file_path: file.path.clone(),
                content_width,
                split: content_width >= SPLIT_VIEW_MIN_WIDTH && !self.show_full_file,
                full_file: self.show_full_file,
                document_revision: self.document_revision,
            },
            comments_revision: self.review.revision,
            draft: draft_key(self.review.draft.as_ref()),
        };

        if self.patch.view.as_ref().is_some_and(|view| view.key == key) {
            return;
        }

        self.ensure_patch_rows(file, &key.patch, theme, highlighter);
        self.patch.view = Some(self.build_diff_view(&key, theme));
    }

    /// Makes `self.patch.rows` the syntax-highlighted patch for `key`.
    ///
    /// This is the expensive half, and nothing a reviewer types changes it, so
    /// it survives every keystroke of a draft.
    fn ensure_patch_rows(
        &mut self,
        file: &FileDiff,
        key: &PatchKey,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) {
        if self.patch.rows.as_ref().is_some_and(|rows| rows.key == *key) {
            return;
        }
        let width = key.content_width;
        let rows = if key.full_file {
            self.build_full_file_rows(file, width, theme, highlighter)
        } else if key.split {
            build_split_rows(file, width, theme, highlighter)
        } else {
            build_unified_rows(file, width, theme, highlighter)
        };
        self.patch.rows = Some(PatchRows { rows, key: key.clone() });
    }

    /// Weaves the review comments into the cached patch rows.
    fn build_diff_view(&self, key: &DiffViewKey, theme: &Theme) -> DiffView {
        let width = key.patch.content_width;
        let empty = Vec::new();
        let source = self.patch.rows.as_ref().map_or(&empty, |cached| &cached.rows);

        let mut rows = AnnotatedRows::default();
        for row in source {
            match row.anchor {
                Some(cursor) if row.selectable => rows.push(row.line.clone(), cursor),
                Some(cursor) => rows.push_anchored(row.line.clone(), cursor),
                None => rows.push_inert(row.line.clone()),
            }
            if let Some(cursor) = row.anchor {
                self.push_annotations(&mut rows, self.anchor(cursor.hunk, cursor.line), width, theme);
            }
        }

        DiffView { rows, key: key.clone() }
    }

    fn build_full_file_rows(
        &self,
        file: &FileDiff,
        width: u16,
        theme: &Theme,
        highlighter: &mut SyntaxHighlighter,
    ) -> Vec<PatchRow> {
        let Some(content) = &self.full_file_content else {
            return vec![PatchRow::inert(Line::styled("Loading file…", Style::new().fg(theme.muted)))];
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
                PatchRow::at(
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
                let mut hints = if self.focus == Focus::Drawer {
                    vec![("j/k", "move"), ("h/l", "pane"), ("space", "stage"), ("a/A", "all"), ("t", "scope")]
                } else {
                    vec![
                        ("j/k", "scroll"),
                        ("c", "comment"),
                        ("s", "submit"),
                        ("u", "undo"),
                        ("h/l", "pane"),
                        ("space", "stage"),
                    ]
                };
                hints.extend([
                    ("C", "commit"),
                    ("d", "discard"),
                    ("o", "full file"),
                    ("r", "refresh"),
                    ("</>", "width"),
                ]);
                hints.push(("Ctrl-G/Esc", "close"));

                let mut line = key_hints(&hints, theme);
                let queued = self.review.queue.len();
                if queued > 0 {
                    line.push_span(Span::styled(
                        format!("  ({})", plural(queued, "comment")),
                        Style::new().fg(theme.info),
                    ));
                }
                Paragraph::new(line).render(area, buf);
                None
            }
        }
    }
}

impl Surface for GitDiffScreen {
    /// The screen owns every key, so nothing falls through to the shared list
    /// navigation.
    fn on_surface_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Vec<Action>> {
        Some(self.handle_key(key))
    }

    fn on_task_result(&mut self, result: TaskResult) -> Vec<Action> {
        match result {
            TaskResult::GitDiff(event) => self.handle_event(event),
            _ => Vec::new(),
        }
    }

    fn on_paste(&mut self, text: &str) -> Vec<Action> {
        self.handle_paste(text);
        Vec::new()
    }

    fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<Action> {
        self.handle_mouse(action, row, column);
        Vec::new()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        self.render_screen(area, buf, cx)
    }
}

fn build_unified_rows(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<PatchRow> {
    file.hunks
        .iter()
        .enumerate()
        .flat_map(|(hunk, entry)| {
            entry.lines.iter().enumerate().map(move |(line, patch_line)| (hunk, line, patch_line))
        })
        .map(|(hunk, line, patch_line)| {
            PatchRow::at(
                render_unified_line(patch_line, file.language(), width, theme, highlighter),
                PatchCursor { hunk, line },
            )
        })
        .collect()
}

fn build_split_rows(file: &FileDiff, width: u16, theme: &Theme, highlighter: &mut SyntaxHighlighter) -> Vec<PatchRow> {
    let (left_width, right_width) = split_widths(width);
    let mut rows = Vec::new();
    for (hunk, entry) in file.hunks.iter().enumerate() {
        for group in split_groups(&entry.lines) {
            match group {
                SplitGroup::Changed { removed, added } => {
                    for (left, right) in pair_changed_block(&removed, &added) {
                        // Anchor comments on the added side when present, falling back to
                        // the removed side, so each line keeps its own comment slot.
                        let line = right.or(left).map_or(0, |side| side.idx);
                        let rendered = render_split_row(
                            left.map(|side| side.line),
                            right.map(|side| side.line),
                            file.language(),
                            left_width,
                            right_width,
                            theme,
                            highlighter,
                        );
                        rows.push(PatchRow::at(rendered, PatchCursor { hunk, line }));
                    }
                }
                SplitGroup::Single { line: patch_line, index } => {
                    let cursor = PatchCursor { hunk, line: index };
                    rows.push(match patch_line.kind {
                        PatchLineKind::HunkHeader => {
                            PatchRow::at(styled_full_width(&patch_line.text, width, theme.info), cursor)
                        }
                        PatchLineKind::Added | PatchLineKind::Context => {
                            let old = (patch_line.kind == PatchLineKind::Context).then_some(patch_line);
                            let rendered = render_split_row(
                                old,
                                Some(patch_line),
                                file.language(),
                                left_width,
                                right_width,
                                theme,
                                highlighter,
                            );
                            PatchRow::at(rendered, cursor)
                        }
                        // A leftover removed line has no right-hand side to pair with, and a
                        // meta line is not part of the file, so neither takes the cursor.
                        PatchLineKind::Meta | PatchLineKind::Removed => {
                            PatchRow::anchored(styled_full_width(&patch_line.text, width, theme.muted), cursor)
                        }
                    });
                }
            }
        }
    }
    rows
}

/// Splits a hunk into the units a split view draws: a removed run followed by
/// an added run is one changed block, and everything else stands alone.
fn split_groups(lines: &[PatchLine]) -> Vec<SplitGroup<'_>> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].kind != PatchLineKind::Removed {
            groups.push(SplitGroup::Single { line: &lines[index], index });
            index += 1;
            continue;
        }
        let sides =
            |range: std::ops::Range<usize>| range.map(|idx| SplitSide { line: &lines[idx], idx }).collect::<Vec<_>>();
        let removed_start = index;
        index += lines[index..].iter().take_while(|line| line.kind == PatchLineKind::Removed).count();
        let added_start = index;
        index += lines[index..].iter().take_while(|line| line.kind == PatchLineKind::Added).count();
        groups
            .push(SplitGroup::Changed { removed: sides(removed_start..added_start), added: sides(added_start..index) });
    }
    groups
}

/// One unit of a split view: a standalone line, or a removed/added run drawn
/// side by side.
enum SplitGroup<'a> {
    Single { line: &'a PatchLine, index: usize },
    Changed { removed: Vec<SplitSide<'a>>, added: Vec<SplitSide<'a>> },
}

/// A column count as a signed offset, for arithmetic between the two.
fn columns(count: u16) -> i16 {
    i16::try_from(count).unwrap_or(i16::MAX)
}

fn notice(text: impl Into<String>, color: ratatui::style::Color) -> Paragraph<'static> {
    Paragraph::new(Line::styled(text.into(), Style::new().fg(color)))
}

pub(super) fn plural(count: usize, noun: &str) -> String {
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
    let marker = match line.kind {
        PatchLineKind::Added => '+',
        PatchLineKind::Removed => '-',
        _ => ' ',
    };
    let gutter = format!("{} {} {marker} ", line_number(line.old_line_no), line_number(line.new_line_no));
    let rendered = diff_line(&gutter, &line.text, language, tone_of(line.kind), theme, highlighter);
    fit_line(rendered.line, usize::from(width), rendered.fill)
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
    let side = |line: Option<&PatchLine>, number: fn(&PatchLine) -> Option<usize>, width, highlighter: &mut _| {
        split_side(
            line.and_then(number),
            line.map(|line| line.text.as_str()),
            language,
            line.map_or(DiffTone::Context, |line| tone_of(line.kind)),
            width,
            theme,
            highlighter,
        )
    };
    let left = side(old, |line| line.old_line_no, left_width, highlighter);
    let right = side(new, |line| line.new_line_no, right_width, highlighter);
    join_split(left, right, theme)
}

fn tone_of(kind: PatchLineKind) -> DiffTone {
    match kind {
        PatchLineKind::Added => DiffTone::Added,
        PatchLineKind::Removed => DiffTone::Removed,
        _ => DiffTone::Context,
    }
}

fn line_number(number: Option<usize>) -> String {
    number.map_or_else(|| "    ".to_string(), |number| format!("{number:>4}"))
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

/// Wrapped draft text plus the cursor's (row, column) within the rendered box,
/// offset past the box's title row and its `│ > ` quote marker.
fn draft_body(draft: &Draft<PatchAnchor>, body_width: usize) -> (Vec<String>, (usize, u16)) {
    if draft.buffer.is_empty() {
        return (vec!["█".to_string()], (1, 3));
    }
    let (lines, (row, column)) = wrapped_with_cursor(&draft.buffer, body_width);
    (lines, (1 + row, 3 + column))
}

fn file_status_color(status: FileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        FileStatus::Modified => theme.warning,
        FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg,
        FileStatus::Deleted => theme.diff_removed_fg,
        FileStatus::Renamed => theme.info,
    }
}
