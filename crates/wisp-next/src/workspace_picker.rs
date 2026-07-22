use crate::workspace_status::home_relative_path;
use acp_utils::notifications::{WorkspaceEntry, WorkspaceMoveTarget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::path::{Path, PathBuf};

pub struct WorkspacePicker {
    rows: Vec<WorkspaceRow>,
    selected: usize,
    query: String,
    parent_dir: Option<PathBuf>,
    mode: Mode,
}

pub enum WorkspacePickerMessage {
    Close,
    Move { target: WorkspaceMoveTarget },
}

enum Mode {
    List,
    NamingNew { name: String },
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
        Self { rows, selected: 0, query: String::new(), parent_dir, mode: Mode::List }
    }

    pub fn has_workspaces(&self) -> bool {
        self.rows.iter().any(|r| matches!(r, WorkspaceRow::Existing(_)))
    }

    pub fn select_row(&mut self, row: usize) {
        if matches!(self.mode, Mode::List) {
            let filtered = self.filtered();
            if !filtered.is_empty() {
                let idx = row.min(filtered.len().saturating_sub(1));
                self.selected = idx;
            }
        }
    }

    pub fn scroll_up(&mut self) {
        if matches!(self.mode, Mode::List) {
            let filtered = self.filtered();
            if !filtered.is_empty() {
                self.selected = self.selected.checked_sub(1).unwrap_or(filtered.len() - 1);
            }
        }
    }

    pub fn scroll_down(&mut self) {
        if matches!(self.mode, Mode::List) {
            let filtered = self.filtered();
            if !filtered.is_empty() {
                self.selected = (self.selected + 1) % filtered.len();
            }
        }
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Vec<WorkspacePickerMessage>> {
        use crossterm::event::KeyCode;

        match &mut self.mode {
            Mode::List => {
                let filtered = self.filtered();
                match key.code {
                    KeyCode::Esc => return Some(vec![WorkspacePickerMessage::Close]),
                    KeyCode::Up => {
                        if !filtered.is_empty() {
                            self.selected = self.selected.checked_sub(1).unwrap_or(filtered.len() - 1);
                        }
                    }
                    KeyCode::Down => {
                        if !filtered.is_empty() {
                            self.selected = (self.selected + 1) % filtered.len();
                        }
                    }
                    KeyCode::Enter | KeyCode::Tab => {
                        if let Some(row) = filtered.get(self.selected).and_then(|&i| self.rows.get(i)) {
                            return Some(self.confirm_row(row.clone()));
                        }
                    }
                    KeyCode::Char(c) => {
                        self.query.push(c);
                        self.selected = 0;
                    }
                    KeyCode::Backspace => {
                        self.query.pop();
                        self.selected = 0;
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
                    let trimmed = name.trim();
                    if trimmed.is_empty() {
                        Some(vec![])
                    } else {
                        Some(vec![WorkspacePickerMessage::Move {
                            target: WorkspaceMoveTarget::New { name: trimmed.to_string() },
                        }])
                    }
                }
                KeyCode::Char(c) => {
                    name.push(c);
                    Some(vec![])
                }
                KeyCode::Backspace => {
                    name.pop();
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
                self.mode = Mode::NamingNew { name: String::new() };
                vec![]
            }
        }
    }

    fn filtered(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let q = self.query.to_ascii_lowercase();
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
                if self.query.is_empty() { String::new() } else { format!("'{}'", self.query) }
            ))
            .style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let filtered = self.filtered();
        let max_rows = inner.height as usize;
        let mut lines: Vec<Line> = Vec::with_capacity(max_rows);

        if filtered.is_empty() {
            lines.push(Line::styled("  (no matching workspaces)", Style::new().fg(theme.muted)));
        } else {
            for (row_idx, &item_idx) in filtered.iter().take(max_rows).enumerate() {
                let row = &self.rows[item_idx];
                let (text, is_create) = match row {
                    WorkspaceRow::Existing(entry) => (format!("  {}", home_relative_path(&entry.path)), false),
                    WorkspaceRow::CreateNew => (format!("  {CREATE_NEW_LABEL}"), true),
                };
                let is_selected = row_idx == self.selected;
                let style = if is_selected && !is_create {
                    Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)
                } else if is_selected && is_create {
                    Style::new().fg(theme.info).bg(theme.sidebar_bg)
                } else if is_create {
                    Style::new().fg(theme.info)
                } else {
                    Style::new().fg(theme.text_secondary)
                };
                let truncated = truncate(&text, inner.width as usize);
                lines.push(Line::from(vec![Span::styled(truncated, style)]));
            }
        }

        while lines.len() < max_rows {
            lines.push(Line::from(""));
        }

        Paragraph::new(lines).render(inner, buf);
    }

    fn render_name_input(&self, name: &str, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block =
            Block::default().borders(Borders::ALL).title(" New workspace ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let max_rows = inner.height as usize;
        let mut lines: Vec<Line> = Vec::with_capacity(max_rows);

        let display = if name.is_empty() { " " } else { name };
        lines.push(Line::from(vec![Span::styled(
            format!("  Name: {display}"),
            Style::new().fg(theme.text_primary).bg(theme.sidebar_bg),
        )]));
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
