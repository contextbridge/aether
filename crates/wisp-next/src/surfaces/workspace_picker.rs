use crate::components::edit_buffer::{EditBuffer, apply_edit_key};
use crate::components::filterable_list::FilterableList;
use crate::components::theme::Theme;
use crate::components::widgets::TextInput;
use crate::renderer::DrawContext;
use crate::session::workspace_status::home_relative_path;
use crate::surfaces::surface::{Action, Surface, SurfaceList};
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, StatefulWidget, Widget};
use std::path::{Path, PathBuf};

/// Picker for moving the session to another workspace, or naming a new one.
pub struct WorkspacePicker {
    rows: FilterableList<WorkspaceRow>,
    parent_dir: Option<PathBuf>,
    mode: Mode,
}

pub(super) struct WorkspacePickerView<'a> {
    theme: &'a Theme,
}

impl<'a> WorkspacePickerView<'a> {
    pub(super) fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
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

    fn on_naming_key(&mut self, key: KeyEvent) -> Vec<Action> {
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
                vec![Action::MoveWorkspace { target: WorkspaceMoveTarget::New { name: trimmed } }]
            }
            _ => {
                apply_edit_key(name, key);
                Vec::new()
            }
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let title = self.rows.search_title("Workspaces");
        let (view, selection) = self.rows.view(theme, |row| match row {
            WorkspaceRow::Existing(entry) => {
                Line::styled(format!("  {}", home_relative_path(&entry.path)), Style::new().fg(theme.text_secondary))
            }
            WorkspaceRow::CreateNew => Line::styled(format!("  {CREATE_NEW_LABEL}"), Style::new().fg(theme.info)),
        });
        let view = view.empty_message("  (no matching workspaces)").bordered(title).scrollbar();
        StatefulWidget::render(view, area, buf, selection);
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

impl StatefulWidget for WorkspacePickerView<'_> {
    type State = WorkspacePicker;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        match &state.mode {
            Mode::List => state.render_list(area, buf, self.theme),
            Mode::NamingNew { name } => {
                WorkspacePicker::render_name_input(state.parent_dir.as_deref(), name, area, buf, self.theme);
            }
        }
    }
}
impl Surface for WorkspacePicker {
    /// Acts on the focused row: existing workspaces move immediately, while
    /// "create new" switches to the name prompt.
    fn activate(&mut self) -> Vec<Action> {
        match self.rows.selected_entry().cloned() {
            Some(WorkspaceRow::Existing(entry)) => {
                vec![Action::MoveWorkspace { target: WorkspaceMoveTarget::Existing { path: entry.path } }]
            }
            Some(WorkspaceRow::CreateNew) => {
                self.mode = Mode::NamingNew { name: EditBuffer::default() };
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// While naming a new workspace the prompt owns every key, so nothing falls
    /// through to the list's navigation and filter keys.
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        if matches!(self.mode, Mode::NamingNew { .. }) {
            return Some(self.on_naming_key(key));
        }
        matches!(key.code, KeyCode::Enter | KeyCode::Tab).then(|| self.activate())
    }

    fn on_paste(&mut self, text: &str) -> Vec<Action> {
        if let Mode::NamingNew { name } = &mut self.mode {
            name.insert_paste(text);
        }
        Vec::new()
    }

    /// Only the list has rows to filter, navigate, and click; while a new
    /// workspace is being named the prompt owns input instead.
    fn list(&mut self) -> Option<&mut dyn SurfaceList> {
        matches!(self.mode, Mode::List).then(|| &mut self.rows as &mut dyn SurfaceList)
    }
}

impl WorkspacePicker {
    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        StatefulWidget::render(WorkspacePickerView::new(cx.theme), area, buf, self);
        None
    }
}
