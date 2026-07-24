use crate::filterable_list::FilterableList;
use crate::theme::Theme;
use ratatui::text::Line;
use std::path::{Path, PathBuf};

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
    Command(FilterableList<CommandEntry>),
    File(FilterableList<FileEntry>),
}

impl Overlay {
    pub fn command(entries: Vec<CommandEntry>) -> Self {
        Self::Command(FilterableList::new(entries, command_search_text))
    }

    pub fn file(root: &Path) -> Self {
        Self::File(FilterableList::new(index_files(root), |entry| entry.display_name.clone()))
    }

    pub fn query(&self) -> &str {
        match self {
            Self::Command(list) => list.query(),
            Self::File(list) => list.query(),
        }
    }

    pub fn set_query(&mut self, query: String) {
        match self {
            Self::Command(list) => list.set_query(query),
            Self::File(list) => list.set_query(query),
        }
    }

    pub fn move_up(&mut self) {
        match self {
            Self::Command(list) => list.select_previous(),
            Self::File(list) => list.select_previous(),
        }
    }

    pub fn move_down(&mut self) {
        match self {
            Self::Command(list) => list.select_next(),
            Self::File(list) => list.select_next(),
        }
    }

    pub fn select_row(&mut self, row: usize) {
        match self {
            Self::Command(list) => list.select_row(row),
            Self::File(list) => list.select_row(row),
        }
    }

    pub fn selected_command(&self) -> Option<CommandEntry> {
        match self {
            Self::Command(list) => list.selected_entry().cloned(),
            Self::File(_) => None,
        }
    }

    pub fn selected_file(&self) -> Option<FileEntry> {
        match self {
            Self::File(list) => list.selected_entry().cloned(),
            Self::Command(_) => None,
        }
    }

    pub fn lines(&self, width: u16, max_rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        match self {
            Self::Command(list) => list.inline_lines(width, max_rows, theme, "no matching commands", |entry| {
                let hint = entry.hint.as_deref().map_or_else(String::new, |hint| format!("  [{hint}]"));
                format!("/{:<16}  {}{hint}", entry.name, entry.description)
            }),
            Self::File(list) => {
                list.inline_lines(width, max_rows, theme, "no matching files", |entry| entry.display_name.clone())
            }
        }
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
