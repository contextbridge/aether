use crate::filterable_list::{FilterableList, FilterableListView};
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, ListItem};
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
pub enum CompletionEntry {
    Command(CommandEntry),
    File(FileEntry),
}

/// The inline completion list the composer shows after a `/` or `@` trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionOverlay {
    trigger: char,
    entries: FilterableList<CompletionEntry>,
    empty_message: &'static str,
    /// The file-index request this overlay is waiting on, if any.
    pending_index: Option<u64>,
}

impl CompletionOverlay {
    pub fn command(entries: Vec<CommandEntry>) -> Self {
        Self::new('/', entries.into_iter().map(CompletionEntry::Command).collect(), "no matching commands")
    }

    /// An empty file list, waiting on the walk of the working tree. Entries
    /// arrive later via [`CompletionOverlay::set_files`].
    pub fn file(request_id: u64) -> Self {
        let mut overlay = Self::new('@', Vec::new(), "indexing files…");
        overlay.pending_index = Some(request_id);
        overlay
    }

    /// Fills in the results of the walk this overlay is waiting on, ignoring
    /// anything that arrives for a request it did not make.
    pub fn set_files(&mut self, request_id: u64, files: Vec<FileEntry>) {
        if self.pending_index != Some(request_id) {
            return;
        }
        self.pending_index = None;
        self.empty_message = "no matching files";
        let query = self.entries.query().to_string();
        self.entries =
            FilterableList::new(files.into_iter().map(CompletionEntry::File).collect(), CompletionEntry::match_key);
        self.entries.set_query(query);
    }

    /// The character that opened this overlay, and the one whose token the
    /// composer replaces on accept.
    pub fn trigger(&self) -> char {
        self.trigger
    }

    pub fn query(&self) -> &str {
        self.entries.query()
    }

    pub fn set_query(&mut self, query: String) {
        self.entries.set_query(query);
    }

    pub fn move_up(&mut self) {
        self.entries.select_previous();
    }

    pub fn move_down(&mut self) {
        self.entries.select_next();
    }

    /// Selects the entry drawn at terminal `row`, if one is there.
    pub fn select_at(&mut self, row: u16) {
        self.entries.select_at(row);
    }

    pub fn selected_command(&self) -> Option<CommandEntry> {
        match self.entries.selected_entry()? {
            CompletionEntry::Command(command) => Some(command.clone()),
            CompletionEntry::File(_) => None,
        }
    }

    pub fn selected_file(&self) -> Option<FileEntry> {
        match self.entries.selected_entry()? {
            CompletionEntry::File(file) => Some(file.clone()),
            CompletionEntry::Command(_) => None,
        }
    }

    /// Rows the overlay occupies above the composer: a rule plus either the
    /// visible matches or the single "no matches" placeholder.
    pub fn row_count(&self, max_rows: usize) -> usize {
        1 + self.entries.filtered_len().clamp(1, max_rows.max(1))
    }

    pub fn view<'a>(
        &'a mut self,
        theme: &'a Theme,
        width: u16,
    ) -> FilterableListView<'a, CompletionEntry, impl FnMut(&CompletionEntry) -> ListItem<'static> + 'a> {
        let content_width = usize::from(width).saturating_sub(2);
        self.entries
            .view(theme, self.empty_message, move |entry| {
                ListItem::new(format!("  {}", truncate_to_width(&entry.label(), content_width)))
                    .style(Style::new().fg(theme.text_secondary))
            })
            .block(Block::new().borders(Borders::TOP).border_style(Style::new().fg(theme.muted)))
    }

    fn new(trigger: char, entries: Vec<CompletionEntry>, empty_message: &'static str) -> Self {
        Self {
            trigger,
            entries: FilterableList::new(entries, CompletionEntry::match_key),
            empty_message,
            pending_index: None,
        }
    }
}

impl CompletionEntry {
    fn label(&self) -> String {
        match self {
            Self::Command(command) => {
                let hint = command.hint.as_deref().map_or_else(String::new, |hint| format!("  [{hint}]"));
                format!("/{:<16}  {}{hint}", command.name, command.description)
            }
            Self::File(file) => file.display_name.clone(),
        }
    }

    fn match_key(&self) -> String {
        match self {
            Self::Command(command) => format!("{} {}", command.name, command.description),
            Self::File(file) => file.display_name.clone(),
        }
    }
}

const MAX_INDEXED_FILES: usize = 50_000;

/// Walks `root` for every file the `@` picker can offer. Blocking: run it off
/// the event loop.
pub fn index_files(root: &Path) -> Vec<FileEntry> {
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

fn excluded(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component.as_os_str().to_string_lossy().as_ref(), ".git" | ".hg" | ".svn" | "node_modules" | "target")
    })
}
