use acp_utils::notifications::SessionPreviewResponse;
use agent_client_protocol::schema::{self as acp, SessionId};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::HashMap;

pub struct SessionPicker {
    sessions: Vec<acp::SessionInfo>,
    selected: usize,
    query: String,
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
        Self { sessions, selected: 0, query: String::new(), preview_enabled, previews: HashMap::new() }
    }

    pub fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    pub fn initial_preview_request(&self) -> Option<String> {
        if !self.preview_enabled || self.sessions.is_empty() {
            return None;
        }
        Some(self.sessions[self.selected].session_id.0.to_string())
    }

    pub fn on_preview_loaded(&mut self, preview: SessionPreviewResponse) {
        self.previews.insert(preview.session_id.clone(), PreviewState::Loaded(preview));
    }

    pub fn on_preview_failed(&mut self, session_id: &str, error: String) {
        self.previews.insert(session_id.to_string(), PreviewState::Error(error));
    }

    pub fn select_row(&mut self, row: usize) {
        let filtered = self.filtered_sessions();
        if !filtered.is_empty() {
            let idx = row.min(filtered.len().saturating_sub(1));
            self.selected = filtered[idx];
        }
    }

    pub fn scroll_up(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.sessions.len() - 1);
            if let Some(req) = self.preview_request_for(self.selected) {
                let _ = req;
            }
        }
    }

    pub fn scroll_down(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
            if let Some(req) = self.preview_request_for(self.selected) {
                let _ = req;
            }
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
            KeyCode::Up => {
                if !self.sessions.is_empty() {
                    self.selected = self.selected.checked_sub(1).unwrap_or(self.sessions.len() - 1);
                    if let Some(req) = self.preview_request_for(self.selected) {
                        messages.push(SessionPickerMessage::RequestPreview { session_id: req });
                    }
                }
            }
            KeyCode::Down => {
                if !self.sessions.is_empty() {
                    self.selected = (self.selected + 1) % self.sessions.len();
                    if let Some(req) = self.preview_request_for(self.selected) {
                        messages.push(SessionPickerMessage::RequestPreview { session_id: req });
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(session) = self.sessions.get(self.selected) {
                    return Some(vec![SessionPickerMessage::LoadSession {
                        session_id: SessionId::new(session.session_id.0.to_string()),
                        cwd: session.cwd.clone(),
                    }]);
                }
            }
            KeyCode::Char(c) => {
                self.query.push(c);
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            _ => {}
        }

        Some(messages)
    }

    fn filtered_sessions(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.sessions.len()).collect();
        }
        let q = self.query.to_ascii_lowercase();
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
            let list_width = area.width / 2;
            let list_area = Rect::new(area.x, area.y, list_width, area.height);
            let preview_area = Rect::new(area.x + list_width, area.y, area.width - list_width, area.height);
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
                if self.query.is_empty() { String::new() } else { format!("'{}'", self.query) }
            ))
            .style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let filtered = self.filtered_sessions();
        if filtered.is_empty() {
            Paragraph::new("  (no matching sessions)").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let mut lines: Vec<Line> = Vec::new();
        let max_title_width = 48usize;

        for &idx in &filtered {
            let session = &self.sessions[idx];
            let title = session
                .title
                .as_deref()
                .unwrap_or_else(|| session.cwd.file_name().map_or("?", |n| n.to_str().unwrap_or("?")));
            let cwd = session
                .cwd
                .file_name()
                .map_or_else(|| session.cwd.display().to_string(), |n| n.to_string_lossy().into_owned());

            let display = format!("{}  {}", truncate_str(title, max_title_width), cwd);
            let is_selected = idx == self.selected;

            let style = if is_selected {
                Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)
            } else {
                Style::new().fg(theme.text_secondary)
            };

            lines.push(Line::from(vec![
                Span::styled("  ", style),
                Span::styled(truncate_str(&display, inner.width as usize), style),
            ]));
        }

        Paragraph::new(lines).render(inner, buf);
    }

    fn render_preview(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block =
            Block::default().borders(Borders::ALL).title(" Preview ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let session = &self.sessions[self.selected];
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

fn truncate_str(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    value.chars().take(width.saturating_sub(1)).chain(std::iter::once('…')).collect()
}
