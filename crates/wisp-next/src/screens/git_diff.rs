use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::git_diff::{
    DiffScope, FileDiff, FileStatus, GitDiffDocument, GitDiffError, PatchLine, PatchLineKind, StageState, stage_all,
    stage_files, unstage_all, unstage_files,
};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::wrap::wrap_line;

pub struct GitDiffScreen {
    working_dir: PathBuf,
    repo_root: Option<PathBuf>,
    scope: DiffScope,
    state: GitDiffLoadState,
    selected_file: usize,
    selected_path: Option<String>,
    selected_drawer_row: usize,
    focus: Focus,
    collapsed: HashSet<String>,
    scroll_offsets: HashMap<String, usize>,
    request_id: u64,
    operation_in_flight: bool,
    footer_error: Option<String>,
}

pub enum GitDiffEffect {
    Load { request_id: u64, working_dir: PathBuf, repo_root: Option<PathBuf>, scope: DiffScope },
    StageFiles { request_id: u64, repo_root: PathBuf, paths: Vec<String> },
    UnstageFiles { request_id: u64, repo_root: PathBuf, paths: Vec<String> },
    StageAll { request_id: u64, repo_root: PathBuf },
    UnstageAll { request_id: u64, repo_root: PathBuf },
}

pub enum GitDiffEvent {
    Loaded { request_id: u64, result: Result<GitDiffDocument, GitDiffError> },
    ActionFinished { request_id: u64, result: Result<(), GitDiffError> },
}

pub enum GitDiffOutcome {
    None,
    Close,
    Effect(GitDiffEffect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Drawer,
    Patch,
}

enum GitDiffLoadState {
    Loading,
    Ready(GitDiffDocument),
    Error(String),
}

#[derive(Clone)]
enum DrawerEntry {
    Directory { path: String, depth: usize },
    File { index: usize, depth: usize },
}

impl GitDiffScreen {
    pub fn new(working_dir: PathBuf) -> (Self, GitDiffEffect) {
        let mut screen = Self {
            working_dir,
            repo_root: None,
            scope: DiffScope::default(),
            state: GitDiffLoadState::Loading,
            selected_file: 0,
            selected_path: None,
            selected_drawer_row: 0,
            focus: Focus::Drawer,
            collapsed: HashSet::new(),
            scroll_offsets: HashMap::new(),
            request_id: 0,
            operation_in_flight: false,
            footer_error: None,
        };
        let effect = screen.begin_load();
        (screen, effect)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> GitDiffOutcome {
        if matches!(key.code, KeyCode::Esc)
            || key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return GitDiffOutcome::Close;
        }
        if self.operation_in_flight {
            return GitDiffOutcome::None;
        }

        match key.code {
            KeyCode::Char('t') | KeyCode::Tab => {
                self.scope = self.scope.next();
                GitDiffOutcome::Effect(self.begin_load())
            }
            KeyCode::Char('r') => GitDiffOutcome::Effect(self.begin_load()),
            KeyCode::Char('a') => self.stage_all(),
            KeyCode::Char('A') => self.unstage_all(),
            KeyCode::Char(' ') => self.toggle_stage(),
            KeyCode::Left | KeyCode::Char('h') => {
                if self.focus == Focus::Patch {
                    self.focus = Focus::Drawer;
                } else {
                    self.collapse_selected();
                }
                GitDiffOutcome::None
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                if self.focus == Focus::Drawer && !self.expand_or_open_selected() {
                    self.focus = Focus::Patch;
                }
                GitDiffOutcome::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_vertical(-1);
                GitDiffOutcome::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_vertical(1);
                GitDiffOutcome::None
            }
            KeyCode::PageUp => {
                self.move_patch_scroll(-10);
                GitDiffOutcome::None
            }
            KeyCode::PageDown => {
                self.move_patch_scroll(10);
                GitDiffOutcome::None
            }
            _ => GitDiffOutcome::None,
        }
    }

    pub fn on_event(&mut self, event: GitDiffEvent) -> Option<GitDiffEffect> {
        match event {
            GitDiffEvent::Loaded { request_id, result } if request_id == self.request_id => {
                self.operation_in_flight = false;
                match result {
                    Ok(document) => self.apply_document(document),
                    Err(error) => self.state = GitDiffLoadState::Error(error.to_string()),
                }
                None
            }
            GitDiffEvent::ActionFinished { request_id, result } if request_id == self.request_id => {
                self.operation_in_flight = false;
                match result {
                    Ok(()) => Some(self.begin_load()),
                    Err(error) => {
                        self.footer_error = Some(error.to_string());
                        None
                    }
                }
            }
            _ => None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        let area = frame.area();
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

        let footer_text = self.footer_error.as_deref().unwrap_or(if self.operation_in_flight {
            "Updating repository…"
        } else {
            "j/k move · h/l pane · space stage · a/A all · t scope · r refresh · Ctrl-G/Esc close"
        });
        let footer_style =
            if self.footer_error.is_some() { Style::new().fg(theme.error) } else { Style::new().fg(theme.muted) };
        frame.render_widget(Paragraph::new(Line::styled(footer_text.to_string(), footer_style)), footer);
    }

    pub fn cancel(&mut self) {}

    fn begin_load(&mut self) -> GitDiffEffect {
        if let Some(path) = self.selected_file().map(|file| file.path.clone()) {
            self.selected_path = Some(path);
        }
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        self.footer_error = None;
        self.state = GitDiffLoadState::Loading;
        GitDiffEffect::Load {
            request_id: self.request_id,
            working_dir: self.working_dir.clone(),
            repo_root: self.repo_root.clone(),
            scope: self.scope,
        }
    }

    fn apply_document(&mut self, document: GitDiffDocument) {
        self.repo_root = Some(document.repo_root.clone());
        self.selected_file = self
            .selected_path
            .as_deref()
            .and_then(|path| document.files.iter().position(|file| file.path == path))
            .unwrap_or(0)
            .min(document.files.len().saturating_sub(1));
        self.state = GitDiffLoadState::Ready(document);
        self.sync_drawer_selection();
    }

    fn stage_all(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        self.footer_error = None;
        GitDiffOutcome::Effect(GitDiffEffect::StageAll { request_id: self.request_id, repo_root })
    }

    fn unstage_all(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        self.footer_error = None;
        GitDiffOutcome::Effect(GitDiffEffect::UnstageAll { request_id: self.request_id, repo_root })
    }

    fn toggle_stage(&mut self) -> GitDiffOutcome {
        let Some(repo_root) = self.repo_root.clone() else {
            return GitDiffOutcome::None;
        };
        let entries = self.drawer_entries();
        let Some(entry) = entries.get(self.selected_drawer_row) else {
            return GitDiffOutcome::None;
        };
        let files = self.files_for_entry(entry);
        if files.is_empty() {
            return GitDiffOutcome::None;
        }
        let all_staged = files.iter().all(|file| file.staged == StageState::Staged);
        let paths = files.iter().map(|file| file.path.clone()).collect();
        self.request_id = next_request_id();
        self.operation_in_flight = true;
        self.footer_error = None;
        let effect = if all_staged {
            GitDiffEffect::UnstageFiles { request_id: self.request_id, repo_root, paths }
        } else {
            GitDiffEffect::StageFiles { request_id: self.request_id, repo_root, paths }
        };
        GitDiffOutcome::Effect(effect)
    }

    fn move_vertical(&mut self, amount: isize) {
        if self.focus == Focus::Patch {
            self.move_patch_scroll(amount);
            return;
        }
        let entries = self.drawer_entries();
        if entries.is_empty() {
            return;
        }
        self.selected_drawer_row = self.selected_drawer_row.saturating_add_signed(amount).min(entries.len() - 1);
        if let Some(DrawerEntry::File { index, .. }) = entries.get(self.selected_drawer_row) {
            self.selected_file = *index;
        }
    }

    fn move_patch_scroll(&mut self, amount: isize) {
        let Some(path) = self.selected_file().map(|file| file.path.clone()) else {
            return;
        };
        let offset = self.scroll_offsets.entry(path).or_default();
        *offset = offset.saturating_add_signed(amount);
    }

    fn collapse_selected(&mut self) {
        let entries = self.drawer_entries();
        if let Some(DrawerEntry::Directory { path, .. }) = entries.get(self.selected_drawer_row) {
            self.collapsed.insert(path.clone());
            self.sync_drawer_selection();
        }
    }

    fn expand_or_open_selected(&mut self) -> bool {
        let entries = self.drawer_entries();
        match entries.get(self.selected_drawer_row) {
            Some(DrawerEntry::Directory { path, .. }) => {
                self.collapsed.remove(path);
                true
            }
            Some(DrawerEntry::File { index, .. }) => {
                self.selected_file = *index;
                false
            }
            None => false,
        }
    }

    fn sync_drawer_selection(&mut self) {
        let entries = self.drawer_entries();
        self.selected_drawer_row = entries
            .iter()
            .position(|entry| matches!(entry, DrawerEntry::File { index, .. } if *index == self.selected_file))
            .unwrap_or(0);
    }

    fn selected_file(&self) -> Option<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return None;
        };
        document.files.get(self.selected_file)
    }

    fn drawer_entries(&self) -> Vec<DrawerEntry> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        let mut emitted = HashSet::new();
        for (index, file) in document.files.iter().enumerate() {
            let parts: Vec<&str> = file.path.split('/').collect();
            let mut parent = String::new();
            let mut hidden = false;
            for (depth, part) in parts.iter().take(parts.len().saturating_sub(1)).enumerate() {
                if !parent.is_empty() {
                    parent.push('/');
                }
                parent.push_str(part);
                if hidden {
                    continue;
                }
                if emitted.insert(parent.clone()) {
                    entries.push(DrawerEntry::Directory { path: parent.clone(), depth });
                }
                if self.collapsed.contains(&parent) {
                    hidden = true;
                }
            }
            if !hidden {
                entries.push(DrawerEntry::File { index, depth: parts.len().saturating_sub(1) });
            }
        }
        entries
    }

    fn files_for_entry(&self, entry: &DrawerEntry) -> Vec<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return Vec::new();
        };
        match entry {
            DrawerEntry::Directory { path, .. } => {
                let prefix = format!("{path}/");
                document.files.iter().filter(|file| file.path.starts_with(&prefix)).collect()
            }
            DrawerEntry::File { index, .. } => document.files.get(*index).into_iter().collect(),
        }
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

    fn render_drawer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let entries = self.drawer_entries();
        let selected = self.selected_drawer_row.min(entries.len().saturating_sub(1));
        let visible = usize::from(area.height);
        let start = selected.saturating_sub(visible.saturating_sub(1));
        let lines = entries
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(row, entry)| self.drawer_line(entry, row == selected, usize::from(area.width), theme))
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(lines)), area);
    }

    fn drawer_line(&self, entry: &DrawerEntry, selected: bool, width: usize, theme: &Theme) -> Line<'static> {
        let selection = selected && self.focus == Focus::Drawer;
        let selected_style = Style::new().fg(theme.background).bg(theme.accent).add_modifier(Modifier::BOLD);
        let mut line = match entry {
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
        if selection {
            line = line.style(selected_style);
        }
        fit_line(line, width, if selection { selected_style } else { Style::new().fg(theme.text_primary) })
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
        let header = Line::from(vec![
            Span::styled(format!(" {}  {}", file.path, file.status.label()), header_style),
            Span::styled(format!("  +{} -{}", file.additions(), file.deletions()), Style::new().fg(theme.muted)),
        ]);
        let [header_area, content_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(Paragraph::new(header), header_area);

        let lines = if file.binary {
            vec![Line::styled("Binary file", Style::new().fg(theme.muted))]
        } else if area.width >= 96 {
            render_split_patch(&file, area.width, theme, highlighter)
        } else {
            render_unified_patch(&file, area.width, theme, highlighter)
        };
        let offset = self.scroll_offsets.entry(file.path.clone()).or_default();
        *offset = (*offset).min(lines.len().saturating_sub(1));
        let visible = lines.into_iter().skip(*offset).take(usize::from(content_area.height)).collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Text::from(visible)), content_area);
    }

    fn file_at(&self, index: usize) -> Option<&FileDiff> {
        let GitDiffLoadState::Ready(document) = &self.state else {
            return None;
        };
        document.files.get(index)
    }
}

impl GitDiffEffect {
    pub async fn execute(self) -> GitDiffEvent {
        match self {
            Self::Load { request_id, working_dir, repo_root, scope } => GitDiffEvent::Loaded {
                request_id,
                result: GitDiffDocument::load(&working_dir, repo_root.as_deref(), scope).await,
            },
            Self::StageFiles { request_id, repo_root, paths } => {
                GitDiffEvent::ActionFinished { request_id, result: stage_files(&repo_root, &paths).await }
            }
            Self::UnstageFiles { request_id, repo_root, paths } => {
                GitDiffEvent::ActionFinished { request_id, result: unstage_files(&repo_root, &paths).await }
            }
            Self::StageAll { request_id, repo_root } => {
                GitDiffEvent::ActionFinished { request_id, result: stage_all(&repo_root).await }
            }
            Self::UnstageAll { request_id, repo_root } => {
                GitDiffEvent::ActionFinished { request_id, result: unstage_all(&repo_root).await }
            }
        }
    }
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn render_unified_patch(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    file.hunks
        .iter()
        .flat_map(|hunk| {
            hunk.lines
                .iter()
                .map(|line| render_unified_line(line, file.language(), width, theme, highlighter))
                .collect::<Vec<_>>()
        })
        .collect()
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

fn render_split_patch(
    file: &FileDiff,
    width: u16,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let left_width = width.saturating_sub(1) / 2;
    let right_width = width.saturating_sub(left_width + 1);
    let mut rendered = Vec::new();
    for hunk in &file.hunks {
        let mut index = 0;
        while index < hunk.lines.len() {
            let line = &hunk.lines[index];
            if line.kind == PatchLineKind::HunkHeader {
                rendered.push(fit_line(
                    Line::styled(line.text.clone(), Style::new().fg(theme.info)),
                    usize::from(width),
                    Style::new().fg(theme.info),
                ));
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
                for offset in 0..removed.len().max(added.len()) {
                    rendered.push(render_split_row(
                        removed.get(offset),
                        added.get(offset),
                        file.language(),
                        left_width,
                        right_width,
                        theme,
                        highlighter,
                    ));
                }
                continue;
            }
            if line.kind == PatchLineKind::Added {
                rendered.push(render_split_row(
                    None,
                    Some(line),
                    file.language(),
                    left_width,
                    right_width,
                    theme,
                    highlighter,
                ));
            } else if line.kind == PatchLineKind::Context {
                rendered.push(render_split_row(
                    Some(line),
                    Some(line),
                    file.language(),
                    left_width,
                    right_width,
                    theme,
                    highlighter,
                ));
            } else {
                rendered.push(fit_line(
                    Line::styled(line.text.clone(), Style::new().fg(theme.muted)),
                    usize::from(width),
                    Style::new().fg(theme.muted),
                ));
            }
            index += 1;
        }
    }
    rendered
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

fn fit_line(mut line: Line<'static>, width: usize, fill_style: Style) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    if line.width() > width {
        let content_width = width.saturating_sub(1).max(1);
        line = wrap_line(line, u16::try_from(content_width).unwrap_or(u16::MAX)).into_iter().next().unwrap_or_default();
        line.spans.push(Span::styled("…", fill_style));
    }
    if line.width() < width {
        line.spans.push(Span::styled(" ".repeat(width - line.width()), fill_style));
    }
    line
}

fn file_status_color(status: FileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        FileStatus::Modified => theme.warning,
        FileStatus::Added | FileStatus::Untracked => theme.diff_added_fg,
        FileStatus::Deleted => theme.diff_removed_fg,
        FileStatus::Renamed => theme.info,
    }
}
