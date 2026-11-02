use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::list_view::ListView;
use crate::view::selection::SelectionState;
use acp_utils::notifications::AetherCapabilities;
use client_core::composer::attachment::PromptAttachment;
use client_core::composer::completion_token::Trigger;
use client_core::composer::file_listing::FileListing;
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
    trigger: Trigger,
    entries: FilterableList<CompletionEntry>,
    empty_message: String,
    loading: bool,
}

impl CompletionOverlay {
    pub fn command(entries: Vec<SlashCommand>) -> Self {
        Self::new(Trigger::Command, entries.into_iter().map(CompletionEntry::Command).collect(), "no matching commands")
    }

    pub fn file() -> Self {
        Self { loading: true, ..Self::new(Trigger::File, Vec::new(), "loading files…") }
    }

    pub fn set_files(&mut self, listing: FileListing) {
        if !self.loading || listing == FileListing::Pending {
            return;
        }
        self.loading = false;
        let (entries, empty_message) = match listing.into_result() {
            Ok(files) => (
                files.paths.iter().map(|path| CompletionEntry::File(files.attachment(path))).collect(),
                "no matching files".to_owned(),
            ),
            Err(reason) => (Vec::new(), reason),
        };
        self.empty_message = empty_message;
        let query = self.entries.query().to_string();
        self.entries = FilterableList::new(entries, CompletionEntry::match_key);
        self.entries.set_query(query);
    }

    /// The trigger that opened this overlay, and the one whose token the
    /// composer replaces on accept.
    pub fn trigger(&self) -> Trigger {
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
        let empty_message = &self.empty_message;
        let (view, selection) =
            self.entries.view(theme, |entry| Line::styled(format!("  {}", entry.label()), theme.text_secondary));
        let view =
            view.empty_message(empty_message).block(Block::new().borders(Borders::TOP).border_style(theme.muted));
        (view, selection)
    }

    fn new(trigger: Trigger, entries: Vec<CompletionEntry>, empty_message: &'static str) -> Self {
        Self {
            trigger,
            entries: FilterableList::new(entries, CompletionEntry::match_key),
            empty_message: empty_message.to_owned(),
            loading: false,
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
