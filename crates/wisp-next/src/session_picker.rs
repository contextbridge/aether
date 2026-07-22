use crate::edit_buffer::EditBuffer;
use crate::selection::SelectionState;
use crate::wrap::truncate_to_width;
use acp_utils::notifications::SessionPreviewResponse;
use agent_client_protocol::schema::{self as acp, SessionId};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, StatefulWidget, Widget};
use std::collections::HashMap;

pub struct SessionPicker {
    sessions: Vec<acp::SessionInfo>,
    selection: SelectionState,
    query: EditBuffer,
    preview_enabled: bool,
    previews: HashMap<String, PreviewState>,
}

#[derive(Clone)]
enum PreviewState {
    Loading,
    Loaded(SessionPreviewResponse),
    Error(String),
}

pub enum SessionPickerMessage {
    Close,
    LoadSession { session_id: SessionId, cwd: std::path::PathBuf },
    RequestPreview { session_id: String },
}

impl SessionPicker {
    pub fn new(sessions: Vec<acp::SessionInfo>, preview_enabled: bool) -> Self {
        let selection = SelectionState::new(sessions.len());
        Self { sessions, selection, query: EditBuffer::default(), preview_enabled, previews: HashMap::new() }
    }

    pub fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    pub fn initial_preview_request(&self) -> Option<String> {
        if !self.preview_enabled || self.sessions.is_empty() {
            return None;
        }
        let selected = self.selected_session_index()?;
        Some(self.sessions[selected].session_id.0.to_string())
    }

    pub fn on_preview_loaded(&mut self, preview: SessionPreviewResponse) {
        self.previews.insert(preview.session_id.clone(), PreviewState::Loaded(preview));
    }

    pub fn on_preview_failed(&mut self, session_id: &str, error: String) {
        self.previews.insert(session_id.to_string(), PreviewState::Error(error));
    }

    pub fn select_row(&mut self, row: usize) {
        self.selection.select_row(row, self.filtered_sessions().len());
    }

    pub fn scroll_up(&mut self) {
        let len = self.filtered_sessions().len();
        self.selection.previous(len);
        if let Some(index) = self.selected_session_index() {
            let _ = self.preview_request_for(index);
        }
    }

    pub fn scroll_down(&mut self) {
        let len = self.filtered_sessions().len();
        self.selection.next(len);
        if let Some(index) = self.selected_session_index() {
            let _ = self.preview_request_for(index);
        }
    }

    fn preview_request_for(&mut self, index: usize) -> Option<String> {
        if !self.preview_enabled || index >= self.sessions.len() {
            return None;
        }
        let id = self.sessions[index].session_id.0.to_string();
        if self.previews.contains_key(&id) {
            return None;
        }
        self.previews.insert(id.clone(), PreviewState::Loading);
        Some(id)
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Vec<SessionPickerMessage>> {
        use crossterm::event::KeyCode;
        let mut messages = Vec::new();

        match key.code {
            KeyCode::Esc => return Some(vec![SessionPickerMessage::Close]),
            KeyCode::Up | KeyCode::Down => {
                let len = self.filtered_sessions().len();
                if key.code == KeyCode::Up {
                    self.selection.previous(len);
                } else {
                    self.selection.next(len);
                }
                if let Some(index) = self.selected_session_index()
                    && let Some(req) = self.preview_request_for(index)
                {
                    messages.push(SessionPickerMessage::RequestPreview { session_id: req });
                }
            }
            KeyCode::Enter => {
                if let Some(session) = self.selected_session_index().and_then(|index| self.sessions.get(index)) {
                    return Some(vec![SessionPickerMessage::LoadSession {
                        session_id: SessionId::new(session.session_id.0.to_string()),
                        cwd: session.cwd.clone(),
                    }]);
                }
            }
            KeyCode::Char(c) => {
                self.query.insert_char(c);
                self.selection.select_first(self.filtered_sessions().len());
            }
            KeyCode::Backspace => {
                self.query.backspace();
                self.selection.select_first(self.filtered_sessions().len());
            }
            _ => {}
        }

        Some(messages)
    }

    fn filtered_sessions(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.sessions.len()).collect();
        }
        let q = self.query.text().to_ascii_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let title = s.title.as_deref().unwrap_or("");
                let cwd = s.cwd.display().to_string();
                if title.to_ascii_lowercase().contains(&q) || cwd.to_ascii_lowercase().contains(&q) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect()
    }

    fn selected_session_index(&self) -> Option<usize> {
        let selected = self.selection.selected()?;
        self.filtered_sessions().get(selected).copied()
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        if !self.has_sessions() {
            let block =
                Block::default().borders(Borders::ALL).title(" Sessions ").style(Style::new().fg(theme.text_primary));
            let inner = block.inner(area);
            block.render(area, buf);
            Paragraph::new("  No previous sessions found.").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let is_wide = area.width >= 96;

        if is_wide {
            let [list_area, preview_area] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
            self.render_list(list_area, buf, theme);
            self.render_preview(preview_area, buf, theme);
        } else {
            self.render_list(area, buf, theme);
        }
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " Sessions {} ",
                if self.query.is_empty() { String::new() } else { format!("'{}'", self.query.text()) }
            ))
            .style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let filtered = self.filtered_sessions();
        if filtered.is_empty() {
            Paragraph::new("  (no matching sessions)").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let items = filtered.into_iter().map(|index| {
            let session = &self.sessions[index];
            let title = session
                .title
                .as_deref()
                .unwrap_or_else(|| session.cwd.file_name().map_or("?", |name| name.to_str().unwrap_or("?")));
            let cwd = session
                .cwd
                .file_name()
                .map_or_else(|| session.cwd.display().to_string(), |name| name.to_string_lossy().into_owned());
            let display = format!("  {}  {}", truncate_to_width(title, 48), cwd);
            ListItem::new(truncate_to_width(&display, inner.width as usize))
                .style(Style::new().fg(theme.text_secondary))
        });
        let list = List::new(items).highlight_style(Style::new().fg(theme.text_primary).bg(theme.sidebar_bg));
        let mut state = *self.selection.list_state();
        StatefulWidget::render(list, inner, buf, &mut state);
    }

    fn render_preview(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block =
            Block::default().borders(Borders::ALL).title(" Preview ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(session) = self.selected_session_index().and_then(|index| self.sessions.get(index)) else {
            return;
        };
        let id = session.session_id.0.to_string();

        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(vec![Span::styled(
            format!(" Title: {}", session.title.as_deref().unwrap_or("(untitled)")),
            Style::new().fg(theme.text_primary),
        )]));
        lines.push(Line::from(vec![Span::styled(
            format!(" Path: {}", session.cwd.display()),
            Style::new().fg(theme.muted),
        )]));

        if let Some(ts) = &session.updated_at {
            lines.push(Line::from(vec![Span::styled(format!(" Updated: {ts}"), Style::new().fg(theme.muted))]));
        }

        match self.previews.get(&id) {
            None => {
                if self.preview_enabled {
                    lines.push(Line::from(vec![Span::styled(" Loading preview...", Style::new().fg(theme.muted))]));
                }
            }
            Some(PreviewState::Loading) => {
                lines.push(Line::from(vec![Span::styled(" Loading...", Style::new().fg(theme.muted))]));
            }
            Some(PreviewState::Error(err)) => {
                lines.push(Line::from(vec![Span::styled(format!(" Error: {err}"), Style::new().fg(theme.error))]));
            }
            Some(PreviewState::Loaded(preview)) => {
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        " Model: {}  Mode: {}",
                        preview.model,
                        preview.selected_mode.as_deref().unwrap_or("default")
                    ),
                    Style::new().fg(theme.muted),
                )]));
                if preview.tool_call_count > 0 {
                    lines.push(Line::from(vec![Span::styled(
                        format!(" Tool calls: {}", preview.tool_call_count),
                        Style::new().fg(theme.muted),
                    )]));
                }
                lines.push(Line::from(""));
                for turn in &preview.transcript {
                    let role = match turn.role {
                        acp_utils::notifications::SessionPreviewRole::User => "user",
                        acp_utils::notifications::SessionPreviewRole::Assistant => "assistant",
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {role}: "), Style::new().fg(theme.accent)),
                        Span::styled(&turn.text, Style::new().fg(theme.text_secondary)),
                    ]));
                }
                if preview.truncated {
                    lines.push(Line::from(vec![Span::styled(" ... preview truncated", Style::new().fg(theme.muted))]));
                }
            }
        }

        Paragraph::new(lines).render(inner, buf);
    }
}
