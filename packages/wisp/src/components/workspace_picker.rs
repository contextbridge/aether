use crate::components::picker_rendering::{boxed_search_field, render_two_column_items};
use acp_utils::notifications::{WorkspaceDestination, WorkspaceOption, validate_workspace_name};
use std::path::{Path, PathBuf};
use tui::{
    Combobox, Component, Cursor, Event, Frame, KeyCode, Line, MouseEventKind, PickerMessage, Searchable, Spinner,
    Style, TextField, ViewContext,
};
use utils::paths::home_relative_path;

pub struct WorkspacePicker {
    mode: PickerMode,
    entries: Vec<WorkspaceEntry>,
    source_dir: PathBuf,
}

#[derive(Clone)]
pub enum WorkspaceEntry {
    Existing(WorkspaceOption),
    CreateNew,
}

pub enum WorkspacePickerMessage {
    Close,
    Fork(WorkspaceDestination),
}

impl WorkspacePicker {
    pub fn new(workspaces: Vec<WorkspaceOption>, source_dir: PathBuf) -> Self {
        let entries: Vec<WorkspaceEntry> =
            workspaces.into_iter().map(WorkspaceEntry::Existing).chain([WorkspaceEntry::CreateNew]).collect();
        Self { mode: Self::list_mode(&entries), entries, source_dir }
    }

    fn list_mode(entries: &[WorkspaceEntry]) -> PickerMode {
        PickerMode::List { combobox: Combobox::new(entries.to_vec()).close_on_whitespace(false), error: None }
    }

    pub fn on_tick(&mut self) {
        if let PickerMode::Busy { spinner, .. } = &mut self.mode {
            spinner.on_tick();
        }
    }

    pub fn fail(&mut self, message: &str) {
        match &self.mode {
            PickerMode::Busy { origin: BusyOrigin::NameEntry { name }, .. } => {
                self.mode =
                    PickerMode::NameEntry { field: TextField::new(name.clone()), error: Some(message.to_string()) };
            }
            PickerMode::Busy { origin: BusyOrigin::List, .. } => {
                self.mode = PickerMode::List {
                    combobox: Combobox::new(self.entries.clone()).close_on_whitespace(false),
                    error: Some(message.to_string()),
                };
            }
            _ => {}
        }
    }
}

impl Searchable for WorkspaceEntry {
    fn search_text(&self) -> String {
        match self {
            Self::Existing(workspace) => {
                format!("{} {} {}", workspace.name, workspace.subtitle, workspace.path.display())
            }
            Self::CreateNew => "create new workspace".to_string(),
        }
    }
}

impl Component for WorkspacePicker {
    type Message = WorkspacePickerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match &mut self.mode {
            PickerMode::List { combobox, .. } => {
                if let Event::Mouse(mouse) = event {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => combobox.move_up(),
                        MouseEventKind::ScrollDown => combobox.move_down(),
                        _ => {}
                    }
                    return Some(vec![]);
                }

                let msgs = combobox.handle_picker_event(event)?;
                let mut out = Vec::new();
                for msg in msgs {
                    match msg {
                        PickerMessage::Close | PickerMessage::CloseAndPopChar => {
                            out.push(WorkspacePickerMessage::Close);
                        }
                        PickerMessage::Confirm(WorkspaceEntry::Existing(workspace)) => {
                            let label = format!("Moving changes into {}…", workspace.name);
                            self.mode = PickerMode::Busy { label, spinner: reset_spinner(), origin: BusyOrigin::List };
                            out.push(WorkspacePickerMessage::Fork(WorkspaceDestination::Existing {
                                path: workspace.path,
                            }));
                        }
                        PickerMessage::Confirm(WorkspaceEntry::CreateNew) => {
                            self.mode = PickerMode::NameEntry { field: TextField::new(String::new()), error: None };
                        }
                        _ => {}
                    }
                }
                Some(out)
            }
            PickerMode::NameEntry { field, error } => {
                if let Event::Key(key) = event {
                    match key.code {
                        KeyCode::Esc => {
                            self.mode = Self::list_mode(&self.entries);
                            return Some(vec![]);
                        }
                        KeyCode::Enter => {
                            let name = field.value.trim().to_string();
                            return match validate_workspace_name(&name) {
                                Ok(()) => {
                                    self.mode = PickerMode::Busy {
                                        label: format!(
                                            "Cloning {} into sibling workspace '{name}'…",
                                            home_relative_path(&self.source_dir)
                                        ),
                                        spinner: reset_spinner(),
                                        origin: BusyOrigin::NameEntry { name: name.clone() },
                                    };
                                    Some(vec![WorkspacePickerMessage::Fork(WorkspaceDestination::NewSibling { name })])
                                }
                                Err(e) => {
                                    *error = Some(e.to_string());
                                    Some(vec![])
                                }
                            };
                        }
                        _ => {}
                    }
                }
                if field.on_event(event).await.is_some() {
                    *error = None;
                }
                Some(vec![])
            }
            PickerMode::Busy { .. } => Some(vec![]),
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        match &mut self.mode {
            PickerMode::List { combobox, error } => render_list(combobox, error.as_deref(), context),
            PickerMode::NameEntry { field, error } => {
                render_name_entry(field, error.as_deref(), &self.source_dir, context)
            }
            PickerMode::Busy { label, spinner, .. } => render_busy(label, spinner, context),
        }
    }
}

enum PickerMode {
    List { combobox: Combobox<WorkspaceEntry>, error: Option<String> },
    NameEntry { field: TextField, error: Option<String> },
    Busy { label: String, spinner: Spinner, origin: BusyOrigin },
}

enum BusyOrigin {
    NameEntry { name: String },
    List,
}

const MAX_NAME_WIDTH: usize = 32;
const CREATE_NEW_LABEL: &str = "+ Create new workspace…";

fn render_list(combobox: &mut Combobox<WorkspaceEntry>, error: Option<&str>, context: &ViewContext) -> Frame {
    let search = boxed_search_field("🔍 Search", combobox.query(), "type to filter workspaces", context);

    let mut lines = vec![Line::new(String::new())];
    if let Some(error) = error {
        let mut line = Line::new("  ");
        line.push_with_style(error, Style::fg(context.theme.error()));
        lines.push(line);
    }
    if combobox.is_empty() {
        lines.push(Line::new("  (no matching workspaces)"));
        return Frame::vstack([search, Frame::new(lines)]);
    }

    let item_lines = render_two_column_items(combobox, context, MAX_NAME_WIDTH, entry_label, entry_metadata);
    lines.extend(item_lines);
    Frame::vstack([search, Frame::new(lines)])
}

fn render_name_entry(field: &TextField, error: Option<&str>, source_dir: &Path, context: &ViewContext) -> Frame {
    let input = boxed_search_field("New workspace name", &field.value, "e.g. big-refactor", context);

    let mut lines = vec![Line::new(String::new())];
    let mut hint = Line::new("  ");
    hint.push_with_style(
        format!("Clones {} into a sibling directory (copy-on-write).", home_relative_path(source_dir)),
        Style::fg(context.theme.muted()),
    );
    lines.push(hint);
    if let Some(error) = error {
        let mut line = Line::new("  ");
        line.push_with_style(error, Style::fg(context.theme.error()));
        lines.push(line);
    }
    let mut help = Line::new("  ");
    help.push_with_style("enter to create · esc to go back", Style::fg(context.theme.muted()));
    lines.push(help);
    Frame::vstack([input, Frame::new(lines)])
}

fn render_busy(label: &str, spinner: &Spinner, context: &ViewContext) -> Frame {
    let mut lines = vec![Line::new(String::new())];
    let mut line = Line::new("  ");
    line.push_styled(spinner.current_frame().to_string(), context.theme.info());
    line.push_text(format!(" {label}"));
    lines.push(line);
    let mut hint = Line::new("    ");
    hint.push_with_style("workspace operation in progress", Style::fg(context.theme.muted()));
    lines.push(hint);
    Frame::new(lines).with_cursor(Cursor::hidden())
}

fn reset_spinner() -> Spinner {
    let mut spinner = Spinner::braille();
    spinner.reset();
    spinner
}

fn entry_label(entry: &WorkspaceEntry) -> String {
    match entry {
        WorkspaceEntry::Existing(workspace) => workspace.name.clone(),
        WorkspaceEntry::CreateNew => CREATE_NEW_LABEL.to_string(),
    }
}

fn entry_metadata(entry: &WorkspaceEntry) -> String {
    match entry {
        WorkspaceEntry::Existing(workspace) => workspace.subtitle.clone(),
        WorkspaceEntry::CreateNew => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn workspace(name: &str, path: &str) -> WorkspaceOption {
        WorkspaceOption { name: name.to_string(), path: PathBuf::from(path), subtitle: path.to_string() }
    }

    fn picker_with(workspaces: Vec<WorkspaceOption>) -> WorkspacePicker {
        WorkspacePicker::new(workspaces, PathBuf::from("/home/dev/code/aether"))
    }

    async fn type_chars(picker: &mut WorkspacePicker, chars: &str) {
        for c in chars.chars() {
            picker.on_event(&key(KeyCode::Char(c))).await;
        }
    }

    fn rendered_text(picker: &mut WorkspacePicker) -> String {
        let context = ViewContext::new((100, 30));
        picker.render(&context).lines().iter().map(tui::Line::plain_text).collect::<Vec<_>>().join("\n")
    }

    #[tokio::test]
    async fn confirm_existing_workspace_emits_move() {
        let mut picker = picker_with(vec![workspace("other", "/home/dev/code/other")]);

        let msgs = picker.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            msgs.as_slice(),
            [WorkspacePickerMessage::Fork(WorkspaceDestination::Existing { path })] if path == &PathBuf::from("/home/dev/code/other")
        ));
        let text = rendered_text(&mut picker);
        assert!(text.contains("Moving changes into other"), "expected move busy indicator, got: {text}");
    }

    #[tokio::test]
    async fn confirm_create_new_switches_to_name_entry() {
        let mut picker = picker_with(vec![]);

        let msgs = picker.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(msgs.is_empty());
        assert!(rendered_text(&mut picker).contains("New workspace name"));
    }

    #[tokio::test]
    async fn name_entry_enter_emits_create_workspace() {
        let mut picker = picker_with(vec![]);
        picker.on_event(&key(KeyCode::Enter)).await;
        type_chars(&mut picker, "  big-refactor ").await;

        let msgs = picker.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(
            msgs.as_slice(),
            [WorkspacePickerMessage::Fork(WorkspaceDestination::NewSibling { name })] if name == "big-refactor"
        ));
    }

    #[tokio::test]
    async fn invalid_name_shows_error_and_stays_open() {
        let mut picker = picker_with(vec![]);
        picker.on_event(&key(KeyCode::Enter)).await;
        type_chars(&mut picker, "a/b").await;

        let msgs = picker.on_event(&key(KeyCode::Enter)).await.unwrap();
        assert!(msgs.is_empty());
        assert!(rendered_text(&mut picker).contains("New workspace name"));
        assert!(rendered_text(&mut picker).contains("invalid workspace name"));

        type_chars(&mut picker, "x").await;
        assert!(!rendered_text(&mut picker).contains("invalid workspace name"), "editing should clear the error");
    }

    #[tokio::test]
    async fn esc_in_name_entry_returns_to_list_and_esc_closes() {
        let mut picker = picker_with(vec![workspace("other", "/home/dev/code/other")]);
        type_chars(&mut picker, "create").await;
        picker.on_event(&key(KeyCode::Enter)).await;
        assert!(rendered_text(&mut picker).contains("New workspace name"));

        let msgs = picker.on_event(&key(KeyCode::Esc)).await.unwrap();
        assert!(msgs.is_empty());
        assert!(rendered_text(&mut picker).contains("other"));

        let msgs = picker.on_event(&key(KeyCode::Esc)).await.unwrap();
        assert!(matches!(msgs.as_slice(), [WorkspacePickerMessage::Close]));
    }

    #[tokio::test]
    async fn confirmed_name_shows_cloning_indicator_and_swallows_input() {
        let mut picker = picker_with(vec![]);
        picker.on_event(&key(KeyCode::Enter)).await;
        type_chars(&mut picker, "big-refactor").await;
        picker.on_event(&key(KeyCode::Enter)).await;

        let text = rendered_text(&mut picker);
        assert!(text.contains("Cloning"), "expected cloning indicator, got: {text}");
        assert!(text.contains("big-refactor"));

        let before = rendered_text(&mut picker);
        picker.on_tick();
        assert_ne!(rendered_text(&mut picker), before, "tick should advance the spinner");

        for event in [key(KeyCode::Esc), key(KeyCode::Enter), key(KeyCode::Char('x'))] {
            let msgs = picker.on_event(&event).await.unwrap();
            assert!(msgs.is_empty(), "cloning state must swallow input");
        }
        assert!(rendered_text(&mut picker).contains("Cloning"));
    }

    #[tokio::test]
    async fn fail_clone_returns_to_name_entry_with_error_and_name() {
        let mut picker = picker_with(vec![]);
        picker.on_event(&key(KeyCode::Enter)).await;
        type_chars(&mut picker, "taken").await;
        picker.on_event(&key(KeyCode::Enter)).await;

        picker.fail("destination already exists: /home/dev/code/taken");

        let text = rendered_text(&mut picker);
        assert!(text.contains("New workspace name"));
        assert!(text.contains("taken"));
        assert!(text.contains("destination already exists"));
    }

    #[tokio::test]
    async fn fail_from_list_busy_returns_to_list_with_error() {
        let mut picker = picker_with(vec![workspace("other", "/home/dev/code/other")]);
        picker.on_event(&key(KeyCode::Enter)).await;

        picker.fail("destination has uncommitted changes");

        let text = rendered_text(&mut picker);
        assert!(text.contains("other"));
        assert!(text.contains("Create new workspace"));
        assert!(text.contains("destination has uncommitted changes"));
    }

    #[tokio::test]
    async fn list_shows_workspaces_and_create_entry() {
        let mut picker = picker_with(vec![workspace("other", "/home/dev/code/other")]);
        let text = rendered_text(&mut picker);
        assert!(text.contains("other"));
        assert!(text.contains("Create new workspace"));
    }
}
