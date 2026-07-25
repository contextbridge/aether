use crate::edit_buffer::{EditBuffer, apply_edit_key};
use crate::filterable_list::FilterableList;
use crate::overlay::{Overlay, OverlayMessage};
use crate::selection::Direction;
use crate::widgets::TextInput;
use crate::workspace_status::home_relative_path;
use crate::wrap::truncate_to_width;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, ListItem, Paragraph, Widget};
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

    pub fn has_workspaces(&self) -> bool {
        self.rows.entries().iter().any(|row| matches!(row, WorkspaceRow::Existing(_)))
    }

    /// Acts on the focused row: existing workspaces move immediately, while
    /// "create new" switches to the name prompt.
    fn confirm_row(&mut self) -> Vec<OverlayMessage> {
        match self.rows.selected_entry().cloned() {
            Some(WorkspaceRow::Existing(entry)) => {
                vec![OverlayMessage::MoveWorkspace { target: WorkspaceMoveTarget::Existing { path: entry.path } }]
            }
            Some(WorkspaceRow::CreateNew) => {
                self.mode = Mode::NamingNew { name: EditBuffer::default() };
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn on_naming_key(&mut self, key: KeyEvent) -> Vec<OverlayMessage> {
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
                vec![OverlayMessage::MoveWorkspace { target: WorkspaceMoveTarget::New { name: trimmed } }]
            }
            _ => {
                apply_edit_key(name, key);
                Vec::new()
            }
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let title = self.rows.search_title("Workspaces");
        let item_width = usize::from(area.width.saturating_sub(2));
        self.rows
            .view(theme, "  (no matching workspaces)", |row| {
                let (text, style) = match row {
                    WorkspaceRow::Existing(entry) => {
                        (format!("  {}", home_relative_path(&entry.path)), Style::new().fg(theme.text_secondary))
                    }
                    WorkspaceRow::CreateNew => (format!("  {CREATE_NEW_LABEL}"), Style::new().fg(theme.info)),
                };
                ListItem::new(truncate_to_width(&text, item_width)).style(style)
            })
            .bordered(title)
            .render(area, buf);
    }

    fn render_name_input(&self, name: &EditBuffer, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block = Block::bordered().title(" New workspace ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        if let Some(parent) = &self.parent_dir {
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

impl Overlay for WorkspacePicker {
    fn on_key(&mut self, key: KeyEvent) -> Vec<OverlayMessage> {
        if matches!(self.mode, Mode::NamingNew { .. }) {
            return self.on_naming_key(key);
        }
        match key.code {
            KeyCode::Esc => vec![OverlayMessage::Close],
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Enter | KeyCode::Tab => self.confirm_row(),
            KeyCode::Char(character) => {
                self.rows.push_query_char(character);
                Vec::new()
            }
            KeyCode::Backspace => {
                self.rows.pop_query_char();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn scroll(&mut self, direction: Direction) -> Vec<OverlayMessage> {
        if matches!(self.mode, Mode::List) {
            self.rows.step(direction, |_| true);
        }
        Vec::new()
    }

    fn click(&mut self, row: u16, _area: Rect) -> Vec<OverlayMessage> {
        if matches!(self.mode, Mode::List) {
            self.rows.select_row(usize::from(row.saturating_sub(1)));
        }
        Vec::new()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        match &self.mode {
            Mode::List => self.render_list(area, buf, theme),
            Mode::NamingNew { name } => {
                let name = name.clone();
                self.render_name_input(&name, area, buf, theme);
            }
        }
    }
}
