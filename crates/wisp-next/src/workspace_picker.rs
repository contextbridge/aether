use crate::edit_buffer::EditBuffer;
use crate::filterable_list::{FilterableList, FilterableListRender};
use crate::widgets::TextInput;
use crate::workspace_status::home_relative_path;
use crate::wrap::truncate_to_width;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Widget};
use std::path::{Path, PathBuf};

pub struct WorkspacePicker {
    rows: FilterableList<WorkspaceRow>,
    parent_dir: Option<PathBuf>,
    mode: Mode,
}

pub enum WorkspacePickerMessage {
    Close,
    Move { target: WorkspaceMoveTarget },
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

    pub fn select_row(&mut self, row: usize) {
        if matches!(self.mode, Mode::List) {
            self.rows.select_row(row);
        }
    }

    pub fn scroll_up(&mut self) {
        if matches!(self.mode, Mode::List) {
            self.rows.select_previous();
        }
    }

    pub fn scroll_down(&mut self) {
        if matches!(self.mode, Mode::List) {
            self.rows.select_next();
        }
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Vec<WorkspacePickerMessage>> {
        use crossterm::event::KeyCode;

        match &mut self.mode {
            Mode::List => match key.code {
                KeyCode::Esc => return Some(vec![WorkspacePickerMessage::Close]),
                KeyCode::Up => self.rows.select_previous(),
                KeyCode::Down => self.rows.select_next(),
                KeyCode::Enter | KeyCode::Tab => {
                    if let Some(row) = self.rows.selected_entry().cloned() {
                        return Some(self.confirm_row(row));
                    }
                }
                KeyCode::Char(c) => self.rows.push_query_char(c),
                KeyCode::Backspace => self.rows.pop_query_char(),
                _ => {}
            },
            Mode::NamingNew { name } => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::List;
                }
                KeyCode::Enter => {
                    let trimmed = name.text().trim();
                    if !trimmed.is_empty() {
                        return Some(vec![WorkspacePickerMessage::Move {
                            target: WorkspaceMoveTarget::New { name: trimmed.to_string() },
                        }]);
                    }
                }
                KeyCode::Char(c) => name.insert_char(c),
                KeyCode::Backspace => {
                    name.backspace();
                }
                _ => {}
            },
        }
        Some(vec![])
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        match &self.mode {
            Mode::List => self.render_list(area, buf, theme),
            Mode::NamingNew { name } => self.render_name_input(name, area, buf, theme),
        }
    }

    fn confirm_row(&mut self, row: WorkspaceRow) -> Vec<WorkspacePickerMessage> {
        match row {
            WorkspaceRow::Existing(entry) => {
                vec![WorkspacePickerMessage::Move { target: WorkspaceMoveTarget::Existing { path: entry.path } }]
            }
            WorkspaceRow::CreateNew => {
                self.mode = Mode::NamingNew { name: EditBuffer::default() };
                vec![]
            }
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let title = format!(
            " Workspaces {} ",
            if self.rows.query().is_empty() { String::new() } else { format!("'{}'", self.rows.query()) }
        );
        let item_width = area.width.saturating_sub(2) as usize;
        self.rows.render(
            area,
            buf,
            FilterableListRender {
                title,
                empty_message: "  (no matching workspaces)",
                border_style: Style::new().fg(theme.text_primary),
                empty_style: Style::new().fg(theme.muted),
                highlight_style: Style::new().fg(theme.text_primary).bg(theme.sidebar_bg),
            },
            |row, _| {
                let (text, style) = match row {
                    WorkspaceRow::Existing(entry) => {
                        (format!("  {}", home_relative_path(&entry.path)), Style::new().fg(theme.text_secondary))
                    }
                    WorkspaceRow::CreateNew => (format!("  {CREATE_NEW_LABEL}"), Style::new().fg(theme.info)),
                };
                ListItem::new(truncate_to_width(&text, item_width)).style(style)
            },
        );
    }

    fn render_name_input(&self, name: &EditBuffer, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block =
            ratatui::widgets::Block::bordered().title(" New workspace ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let max_rows = inner.height as usize;
        let mut lines: Vec<Line> = Vec::with_capacity(max_rows);
        lines.push(Line::from(""));

        if let Some(parent) = &self.parent_dir {
            lines.push(Line::from(vec![Span::styled(
                format!("  will be created in {}/", home_relative_path(parent)),
                Style::new().fg(theme.muted),
            )]));
        }

        while lines.len() < max_rows {
            lines.push(Line::from(""));
        }

        Paragraph::new(lines).render(inner, buf);
        let input_area = Rect::new(inner.x, inner.y, inner.width, 1);
        TextInput::new(name)
            .prefix("  Name: ")
            .prefix_style(Style::new().fg(theme.text_primary).bg(theme.sidebar_bg))
            .style(Style::new().fg(theme.text_primary).bg(theme.sidebar_bg))
            .render(input_area, buf);
    }
}
