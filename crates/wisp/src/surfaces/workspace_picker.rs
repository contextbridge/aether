use crate::renderer::DrawContext;
use crate::session::workspace_status::home_relative_path;
use crate::surfaces::input::{Nav, UiEvent, WorkspacePickerOutput, is_press};
use crate::view::edit_buffer::{EditBuffer, apply_edit_key};
use crate::view::filterable_list::FilterableList;
use crate::theme::Theme;
use crate::view::widgets::TextInput;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use std::path::{Path, PathBuf};

/// Picker for moving the session to another workspace, or naming a new one.
pub struct WorkspacePicker {
    rows: FilterableList<WorkspaceRow>,
    parent_dir: Option<PathBuf>,
    mode: Mode,
}

enum Mode {
    List,
    NamingNew { name: EditBuffer },
}

#[derive(Clone)]
enum WorkspaceRow {
    Existing(WorkspaceEntry),
    CreateNew,
}

const CREATE_NEW_LABEL: &str = "Create new workspace…";

impl WorkspacePicker {
    pub fn new(workspaces: Vec<WorkspaceEntry>) -> Self {
        let parent_dir = workspaces.iter().find(|w| w.is_current).and_then(|w| w.path.parent().map(Path::to_path_buf));
        let mut rows: Vec<WorkspaceRow> =
            workspaces.into_iter().filter(|w| !w.is_current).map(WorkspaceRow::Existing).collect();
        rows.push(WorkspaceRow::CreateNew);
        Self {
            rows: FilterableList::new(rows, |row| match row {
                WorkspaceRow::Existing(entry) => home_relative_path(&entry.path),
                WorkspaceRow::CreateNew => CREATE_NEW_LABEL.to_string(),
            }),
            parent_dir,
            mode: Mode::List,
        }
    }

    fn on_naming_key(&mut self, key: KeyEvent) -> Vec<WorkspacePickerOutput> {
        let Mode::NamingNew { name } = &mut self.mode else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::List;
                Vec::new()
            }
            KeyCode::Enter => {
                let trimmed = name.text().trim().to_string();
                if trimmed.is_empty() {
                    return Vec::new();
                }
                vec![WorkspacePickerOutput::Move { target: WorkspaceMoveTarget::New { name: trimmed } }]
            }
            _ => {
                apply_edit_key(name, key);
                Vec::new()
            }
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.rows.render_pane("Workspaces", "  (no matching workspaces)", area, buf, theme, |row| match row {
            WorkspaceRow::Existing(entry) => {
                Line::styled(format!("  {}", home_relative_path(&entry.path)), Style::new().fg(theme.text_secondary))
            }
            WorkspaceRow::CreateNew => Line::styled(format!("  {CREATE_NEW_LABEL}"), Style::new().fg(theme.info)),
        });
    }

    fn render_name_input(parent_dir: Option<&Path>, name: &EditBuffer, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let block = Block::bordered().title(" New workspace ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        if let Some(parent) = parent_dir {
            let hint = Line::from(Span::styled(
                format!("  will be created in {}/", home_relative_path(parent)),
                Style::new().fg(theme.muted),
            ));
            Paragraph::new(vec![Line::raw(""), hint]).render(inner, buf);
        }

        let field = Style::new().fg(theme.text_primary).bg(theme.sidebar_bg);
        TextInput::new(name)
            .prefix("  Name: ")
            .prefix_style(field)
            .style(field)
            .render(Rect { height: 1, ..inner }, buf);
    }
}

impl WorkspacePicker {
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<WorkspacePickerOutput> {
        if matches!(self.mode, Mode::NamingNew { .. }) {
            return match event {
                UiEvent::Key(key) if is_press(key) => self.on_naming_key(key),
                UiEvent::Paste(text) => self.on_paste(&text),
                UiEvent::Key(_) | UiEvent::Mouse(..) => Vec::new(),
            };
        }
        if let UiEvent::Key(key) = &event
            && is_press(*key)
            && key.code == KeyCode::Tab
        {
            return self.activate();
        }
        match self.rows.on_nav_event(&event) {
            Nav::Close => vec![WorkspacePickerOutput::Close],
            Nav::Activate | Nav::Clicked => self.activate(),
            Nav::Moved | Nav::Unhandled => Vec::new(),
        }
    }

    fn on_paste(&mut self, text: &str) -> Vec<WorkspacePickerOutput> {
        if let Mode::NamingNew { name } = &mut self.mode {
            name.insert_paste(text);
        }
        Vec::new()
    }

    /// Acts on the focused row: existing workspaces move immediately, while
    /// "create new" switches to the name prompt.
    fn activate(&mut self) -> Vec<WorkspacePickerOutput> {
        match self.rows.selected_entry().cloned() {
            Some(WorkspaceRow::Existing(entry)) => {
                vec![WorkspacePickerOutput::Move { target: WorkspaceMoveTarget::Existing { path: entry.path } }]
            }
            Some(WorkspaceRow::CreateNew) => {
                self.mode = Mode::NamingNew { name: EditBuffer::default() };
                Vec::new()
            }
            None => Vec::new(),
        }
    }
}

impl WorkspacePicker {
    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        match &mut self.mode {
            Mode::NamingNew { name } => Self::render_name_input(self.parent_dir.as_deref(), name, area, buf, cx.theme),
            Mode::List => self.render_list(area, buf, cx.theme),
        }
        None
    }
}
