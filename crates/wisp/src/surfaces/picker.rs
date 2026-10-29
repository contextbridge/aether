use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::list_view::ListView;
use crate::view::selection::SelectionState;
use acp_utils::notifications::AetherCapabilities;
use client_core::composer::attachment::PromptAttachment;
use client_core::session_core::CommandEntry;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionEntry {
    Command(SlashCommand),
    File(PromptAttachment),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Builtin(Builtin),
    Agent(CommandEntry),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Clear,
    Resume,
    Settings,
    Move,
}

/// The inline completion list the composer shows after a `/` or `@` trigger.
#[derive(Debug, Clone)]
pub struct CompletionOverlay {
    trigger: char,
    entries: FilterableList<CompletionEntry>,
    empty_message: &'static str,
    indexing: bool,
}

impl CompletionOverlay {
    pub fn command(entries: Vec<SlashCommand>) -> Self {
        Self::new('/', entries.into_iter().map(CompletionEntry::Command).collect(), "no matching commands")
    }

    /// An empty file list, waiting on the walk of the working tree. Entries
    /// arrive later via [`CompletionOverlay::set_files`].
    pub fn file() -> Self {
        Self { indexing: true, ..Self::new('@', Vec::new(), "indexing files…") }
    }

    /// Fills in the results of the walk this overlay is waiting on, ignoring
    pub fn set_files(&mut self, files: Vec<PromptAttachment>) {
        if !self.indexing {
            return;
        }
        self.indexing = false;
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

    pub fn selected_command(&self) -> Option<SlashCommand> {
        match self.entries.selected_entry()? {
            CompletionEntry::Command(command) => Some(command.clone()),
            CompletionEntry::File(_) => None,
        }
    }

    pub fn selected_file(&self) -> Option<PromptAttachment> {
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
        let (view, selection) =
            self.entries.view(theme, |entry| Line::styled(format!("  {}", entry.label()), theme.text_secondary));
        let view =
            view.empty_message(empty_message).block(Block::new().borders(Borders::TOP).border_style(theme.muted));
        (view, selection)
    }

    fn new(trigger: char, entries: Vec<CompletionEntry>, empty_message: &'static str) -> Self {
        Self {
            trigger,
            entries: FilterableList::new(entries, CompletionEntry::match_key),
            empty_message,
            indexing: false,
        }
    }
}

impl SlashCommand {
    pub fn name(&self) -> &str {
        match self {
            Self::Builtin(builtin) => builtin.name(),
            Self::Agent(command) => &command.name,
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::Builtin(builtin) => builtin.description(),
            Self::Agent(command) => &command.description,
        }
    }

    fn hint(&self) -> Option<&str> {
        match self {
            Self::Builtin(_) => None,
            Self::Agent(command) => command.hint.as_deref(),
        }
    }
}

impl Builtin {
    pub fn offered(capabilities: &AetherCapabilities) -> Vec<Self> {
        let mut builtins = vec![Self::Clear, Self::Resume, Self::Settings];
        if capabilities.workspace_move {
            builtins.push(Self::Move);
        }
        builtins
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Resume => "resume",
            Self::Settings => "settings",
            Self::Move => "move",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Clear => "Start a new session",
            Self::Resume => "Resume a previous session",
            Self::Settings => "Configure agent options",
            Self::Move => "Move session to another workspace",
        }
    }
}

impl CompletionEntry {
    fn label(&self) -> String {
        match self {
            Self::Command(command) => {
                let hint = command.hint().map_or_else(String::new, |hint| format!("  [{hint}]"));
                format!("/{:<16}  {}{hint}", command.name(), command.description())
            }
            Self::File(file) => file.display_name.clone(),
        }
    }

    fn match_key(&self) -> String {
        match self {
            Self::Command(command) => format!("{} {}", command.name(), command.description()),
            Self::File(file) => file.display_name.clone(),
        }
    }
}
