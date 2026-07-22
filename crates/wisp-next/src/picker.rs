use crate::selection::SelectionState;
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub has_input: bool,
    pub hint: Option<String>,
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Command(Picker<CommandEntry>),
    File(Picker<FileEntry>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker<T> {
    entries: Vec<T>,
    matches: Vec<usize>,
    query: String,
    selection: SelectionState,
}

impl Overlay {
    pub fn command(entries: Vec<CommandEntry>) -> Self {
        Self::Command(Picker::new(entries, command_search_text))
    }

    pub fn file(root: &Path) -> Self {
        Self::File(Picker::new(index_files(root), |entry| entry.display_name.clone()))
    }

    pub fn query(&self) -> &str {
        match self {
            Self::Command(picker) => picker.query(),
            Self::File(picker) => picker.query(),
        }
    }

    pub fn set_query(&mut self, query: String) {
        match self {
            Self::Command(picker) => picker.set_query(query, command_search_text),
            Self::File(picker) => picker.set_query(query, |entry| entry.display_name.clone()),
        }
    }

    pub fn move_up(&mut self) {
        match self {
            Self::Command(picker) => picker.move_up(),
            Self::File(picker) => picker.move_up(),
        }
    }

    pub fn move_down(&mut self) {
        match self {
            Self::Command(picker) => picker.move_down(),
            Self::File(picker) => picker.move_down(),
        }
    }

    pub fn select_row(&mut self, row: usize) {
        match self {
            Self::Command(picker) => picker.select_row(row),
            Self::File(picker) => picker.select_row(row),
        }
    }

    pub fn selected_command(&self) -> Option<CommandEntry> {
        match self {
            Self::Command(picker) => picker.selected().cloned(),
            Self::File(_) => None,
        }
    }

    pub fn selected_file(&self) -> Option<FileEntry> {
        match self {
            Self::File(picker) => picker.selected().cloned(),
            Self::Command(_) => None,
        }
    }

    pub fn lines(&self, width: u16, max_rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        match self {
            Self::Command(picker) => picker.lines(width, max_rows, theme, "no matching commands", |entry| {
                let hint = entry.hint.as_deref().map_or_else(String::new, |hint| format!("  [{hint}]"));
                format!("/{:<16}  {}{hint}", entry.name, entry.description)
            }),
            Self::File(picker) => {
                picker.lines(width, max_rows, theme, "no matching files", |entry| entry.display_name.clone())
            }
        }
    }
}

impl<T> Picker<T> {
    pub fn new(entries: Vec<T>, search_text: impl Fn(&T) -> String) -> Self {
        let mut picker =
            Self { entries, matches: Vec::new(), query: String::new(), selection: SelectionState::default() };
        picker.rebuild(search_text);
        picker
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: String, search_text: impl Fn(&T) -> String) {
        self.query = query;
        self.rebuild(search_text);
    }

    pub fn move_up(&mut self) {
        self.selection.previous(self.matches.len());
    }

    pub fn move_down(&mut self) {
        self.selection.next(self.matches.len());
    }

    pub fn select_row(&mut self, row: usize) {
        self.selection.select_row(row, self.matches.len());
    }

    pub fn selected(&self) -> Option<&T> {
        self.selection
            .selected()
            .and_then(|selected| self.matches.get(selected))
            .and_then(|index| self.entries.get(*index))
    }

    fn rebuild(&mut self, search_text: impl Fn(&T) -> String) {
        self.matches = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| fuzzy_matches(&search_text(entry), &self.query).then_some(index))
            .collect();
        self.selection.select_first(self.matches.len());
    }

    fn lines(
        &self,
        width: u16,
        max_rows: usize,
        theme: &Theme,
        empty: &str,
        label: impl Fn(&T) -> String,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        let mut lines = vec![Line::styled("─".repeat(width), Style::new().fg(theme.muted))];
        if self.matches.is_empty() {
            lines.push(Line::styled(format!("  ({empty})"), Style::new().fg(theme.muted)));
        } else {
            for (row, index) in self.matches.iter().take(max_rows).enumerate() {
                let value = truncate_to_width(&label(&self.entries[*index]), width.saturating_sub(2));
                let selected = self.selection.selected() == Some(row);
                let style = if selected {
                    Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)
                } else {
                    Style::new().fg(theme.text_secondary)
                };
                let text = format!("  {value}");
                let padding = " ".repeat(width.saturating_sub(text.width()));
                lines.push(Line::from(vec![Span::styled(text, style), Span::styled(padding, style)]));
            }
        }
        lines
    }
}

const MAX_INDEXED_FILES: usize = 50_000;

fn index_files(root: &Path) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .hidden(false)
        .parents(true)
        .build()
        .flatten()
        .take(MAX_INDEXED_FILES)
    {
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) || excluded(path) {
            continue;
        }
        entries.push(FileEntry {
            path: path.to_path_buf(),
            display_name: path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/"),
        });
    }
    entries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    entries
}

fn command_search_text(entry: &CommandEntry) -> String {
    format!("{} {}", entry.name, entry.description)
}

fn excluded(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component.as_os_str().to_string_lossy().as_ref(), ".git" | ".hg" | ".svn" | "node_modules" | "target")
    })
}

fn fuzzy_matches(value: &str, query: &str) -> bool {
    let mut query = query.chars().map(|character| character.to_ascii_lowercase());
    let mut wanted = query.next();
    for character in value.chars().map(|character| character.to_ascii_lowercase()) {
        if Some(character) == wanted {
            wanted = query.next();
        }
    }
    wanted.is_none()
}
