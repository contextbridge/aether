use crate::edit_buffer::EditBuffer;
use crate::selection::SelectionState;
use crate::widgets::TextInput;
use crate::workspace_status::home_relative_path;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, StatefulWidget, Widget};
use std::path::{Path, PathBuf};

pub struct WorkspacePicker {
    rows: Vec<WorkspaceRow>,
    selection: SelectionState,
    query: EditBuffer,
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
        let selection = SelectionState::new(rows.len());
        Self { rows, selection, query: EditBuffer::default(), parent_dir, mode: Mode::List }
    }

    pub fn has_workspaces(&self) -> bool {
        self.rows.iter().any(|r| matches!(r, WorkspaceRow::Existing(_)))
    }

    pub fn select_row(&mut self, row: usize) {
        if matches!(self.mode, Mode::List) {
            let len = self.filtered().len();
            self.selection.select_row(row, len);
        }
    }

    pub fn scroll_up(&mut self) {
        if matches!(self.mode, Mode::List) {
            let len = self.filtered().len();
            self.selection.previous(len);
        }
    }

    pub fn scroll_down(&mut self) {
        if matches!(self.mode, Mode::List) {
            let len = self.filtered().len();
            self.selection.next(len);
        }
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Vec<WorkspacePickerMessage>> {
        use crossterm::event::KeyCode;

        match &mut self.mode {
            Mode::List => {
                let filtered = self.filtered();
                match key.code {
                    KeyCode::Esc => return Some(vec![WorkspacePickerMessage::Close]),
                    KeyCode::Up => self.selection.previous(filtered.len()),
                    KeyCode::Down => self.selection.next(filtered.len()),
                    KeyCode::Enter | KeyCode::Tab => {
                        if let Some(row) = self
                            .selection
                            .selected()
                            .and_then(|selected| filtered.get(selected))
                            .and_then(|&index| self.rows.get(index))
                        {
                            return Some(self.confirm_row(row.clone()));
                        }
                    }
                    KeyCode::Char(c) => {
                        self.query.insert_char(c);
                        self.selection.select_first(self.filtered().len());
                    }
                    KeyCode::Backspace => {
                        self.query.backspace();
                        self.selection.select_first(self.filtered().len());
                    }
                    _ => {}
                }
                Some(vec![])
            }
            Mode::NamingNew { name } => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::List;
                    Some(vec![])
                }
                KeyCode::Enter => {
                    let trimmed = name.text().trim();
                    if trimmed.is_empty() {
                        Some(vec![])
                    } else {
                        Some(vec![WorkspacePickerMessage::Move {
                            target: WorkspaceMoveTarget::New { name: trimmed.to_string() },
                        }])
                    }
                }
                KeyCode::Char(c) => {
                    name.insert_char(c);
                    Some(vec![])
                }
                KeyCode::Backspace => {
                    name.backspace();
                    Some(vec![])
                }
                _ => Some(vec![]),
            },
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

    fn filtered(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let q = self.query.text().to_ascii_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                let text = match row {
                    WorkspaceRow::Existing(entry) => home_relative_path(&entry.path).to_ascii_lowercase(),
                    WorkspaceRow::CreateNew => CREATE_NEW_LABEL.to_ascii_lowercase(),
                };
                if text.contains(&q) { Some(i) } else { None }
            })
            .collect()
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        match &self.mode {
            Mode::List => self.render_list(area, buf, theme),
            Mode::NamingNew { name } => self.render_name_input(name, area, buf, theme),
        }
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " Workspaces {} ",
                if self.query.is_empty() { String::new() } else { format!("'{}'", self.query.text()) }
            ))
            .style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let filtered = self.filtered();
        if filtered.is_empty() {
            Paragraph::new("  (no matching workspaces)").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let items = filtered.into_iter().map(|item_index| {
            let row = &self.rows[item_index];
            let (text, style) = match row {
                WorkspaceRow::Existing(entry) => {
                    (format!("  {}", home_relative_path(&entry.path)), Style::new().fg(theme.text_secondary))
                }
                WorkspaceRow::CreateNew => (format!("  {CREATE_NEW_LABEL}"), Style::new().fg(theme.info)),
            };
            ListItem::new(truncate(&text, inner.width as usize)).style(style)
        });
        let list = List::new(items).highlight_style(Style::new().fg(theme.text_primary).bg(theme.sidebar_bg));
        let mut state = self.selection.list_state().clone();
        let visible_rows = usize::from(inner.height);
        if let Some(selected) = state.selected() {
            *state.offset_mut() = selected.saturating_sub(visible_rows.saturating_sub(1));
        }
        StatefulWidget::render(list, inner, buf, &mut state);
    }

    fn render_name_input(&self, name: &EditBuffer, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block =
            Block::default().borders(Borders::ALL).title(" New workspace ").style(Style::new().fg(theme.text_primary));
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

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    value.chars().take(width.saturating_sub(1)).chain(std::iter::once('…')).collect()
}
