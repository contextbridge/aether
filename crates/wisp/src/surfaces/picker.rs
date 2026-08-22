use crate::file_index::FileEntry;
use crate::view::filterable_list::FilterableList;
use crate::request::RequestId;
use crate::view::list_view::ListView;
use crate::view::selection::SelectionState;
use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub has_input: bool,
    pub hint: Option<String>,
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionEntry {
    Command(CommandEntry),
    File(FileEntry),
}

/// The inline completion list the composer shows after a `/` or `@` trigger.
#[derive(Debug, Clone)]
pub struct CompletionOverlay {
    trigger: char,
    entries: FilterableList<CompletionEntry>,
    empty_message: &'static str,
    /// The file-index request this overlay is waiting on, if any.
    pending_index: Option<RequestId>,
}

impl CompletionOverlay {
    pub fn command(entries: Vec<CommandEntry>) -> Self {
        Self::new('/', entries.into_iter().map(CompletionEntry::Command).collect(), "no matching commands")
    }

    /// An empty file list, waiting on the walk of the working tree. Entries
    /// arrive later via [`CompletionOverlay::set_files`].
    pub fn file(request_id: RequestId) -> Self {
        let mut overlay = Self::new('@', Vec::new(), "indexing files…");
        overlay.pending_index = Some(request_id);
        overlay
    }

    /// Fills in the results of the walk this overlay is waiting on, ignoring
    /// anything that arrives for a request it did not make.
    pub fn set_files(&mut self, request_id: RequestId, files: Vec<FileEntry>) {
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

    pub fn entries_mut(&mut self) -> &mut FilterableList<CompletionEntry> {
        &mut self.entries
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

    pub fn view<'a>(&'a mut self, theme: &'a Theme) -> (ListView<'a>, &'a mut SelectionState) {
        let empty_message = self.empty_message;
        let (view, selection) = self
            .entries
            .view(theme, |entry| Line::styled(format!("  {}", entry.label()), Style::new().fg(theme.text_secondary)));
        let view = view
            .empty_message(empty_message)
            .block(Block::new().borders(Borders::TOP).border_style(Style::new().fg(theme.muted)));
        (view, selection)
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
