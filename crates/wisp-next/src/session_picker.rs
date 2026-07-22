use crate::edit_buffer::EditBuffer;
use crate::filterable_list::{FilterableList, FilterableListRender};
use crate::wrap::truncate_to_width;
use acp_utils::notifications::SessionPreviewResponse;
use agent_client_protocol::schema::{self as acp, SessionId};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, Paragraph, Widget};
use std::collections::HashMap;

pub struct SessionPicker {
    sessions: FilterableList<acp::SessionInfo>,
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
        Self {
            sessions: FilterableList::new(sessions, |session| {
                format!("{} {}", session.title.as_deref().unwrap_or(""), session.cwd.display())
            }),
            query: EditBuffer::default(),
            preview_enabled,
            previews: HashMap::new(),
        }
    }

    pub fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    pub fn initial_preview_request(&self) -> Option<String> {
        if !self.preview_enabled || self.sessions.is_empty() {
            return None;
        }
        let selected = self.selected_session_index()?;
        Some(self.sessions.entries()[selected].session_id.0.to_string())
    }

    pub fn on_preview_loaded(&mut self, preview: SessionPreviewResponse) {
        self.previews.insert(preview.session_id.clone(), PreviewState::Loaded(preview));
    }

    pub fn on_preview_failed(&mut self, session_id: &str, error: String) {
        self.previews.insert(session_id.to_string(), PreviewState::Error(error));
    }

    pub fn select_row(&mut self, row: usize) {
        self.sessions.select_row(row);
    }

    pub fn scroll_up(&mut self) {
        self.sessions.select_previous();
        if let Some(index) = self.selected_session_index() {
            let _ = self.preview_request_for(index);
        }
    }

    pub fn scroll_down(&mut self) {
        self.sessions.select_next();
        if let Some(index) = self.selected_session_index() {
            let _ = self.preview_request_for(index);
        }
    }

    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Vec<SessionPickerMessage>> {
        use crossterm::event::KeyCode;
        let mut messages = Vec::new();

        match key.code {
            KeyCode::Esc => return Some(vec![SessionPickerMessage::Close]),
            KeyCode::Up | KeyCode::Down => {
                if key.code == KeyCode::Up {
                    self.sessions.select_previous();
                } else {
                    self.sessions.select_next();
                }
                if let Some(index) = self.selected_session_index()
                    && let Some(req) = self.preview_request_for(index)
                {
                    messages.push(SessionPickerMessage::RequestPreview { session_id: req });
                }
            }
            KeyCode::Enter => {
                if let Some(session) =
                    self.selected_session_index().and_then(|index| self.sessions.entries().get(index))
                {
                    return Some(vec![SessionPickerMessage::LoadSession {
                        session_id: SessionId::new(session.session_id.0.to_string()),
                        cwd: session.cwd.clone(),
                    }]);
                }
            }
            KeyCode::Char(c) => {
                self.query.insert_char(c);
                self.sessions.set_query(self.query.text());
            }
            KeyCode::Backspace => {
                self.query.backspace();
                self.sessions.set_query(self.query.text());
            }
            _ => {}
        }

        Some(messages)
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        if !self.has_sessions() {
            let block =
                ratatui::widgets::Block::bordered().title(" Sessions ").style(Style::new().fg(theme.text_primary));
            let inner = block.inner(area);
            block.render(area, buf);
            Paragraph::new("  No previous sessions found.").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        if area.width >= 96 {
            let [list_area, preview_area] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
            self.render_list(list_area, buf, theme);
            self.render_preview(preview_area, buf, theme);
        } else {
            self.render_list(area, buf, theme);
        }
    }

    fn preview_request_for(&mut self, index: usize) -> Option<String> {
        let session = self.sessions.entries().get(index)?;
        if !self.preview_enabled {
            return None;
        }
        let id = session.session_id.0.to_string();
        if self.previews.contains_key(&id) {
            return None;
        }
        self.previews.insert(id.clone(), PreviewState::Loading);
        Some(id)
    }

    fn selected_session_index(&self) -> Option<usize> {
        self.sessions.selected_index()
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let title = format!(
            " Sessions {} ",
            if self.query.is_empty() { String::new() } else { format!("'{}'", self.query.text()) }
        );
        let item_width = area.width.saturating_sub(2) as usize;
        self.sessions.render(
            area,
            buf,
            FilterableListRender {
                title,
                empty_message: "  (no matching sessions)",
                border_style: Style::new().fg(theme.text_primary),
                empty_style: Style::new().fg(theme.muted),
                highlight_style: Style::new().fg(theme.text_primary).bg(theme.sidebar_bg),
            },
            |session, _| {
                let title = session
                    .title
                    .as_deref()
                    .unwrap_or_else(|| session.cwd.file_name().map_or("?", |name| name.to_str().unwrap_or("?")));
                let cwd = session
                    .cwd
                    .file_name()
                    .map_or_else(|| session.cwd.display().to_string(), |name| name.to_string_lossy().into_owned());
                let display = format!("  {}  {cwd}", truncate_to_width(title, 48));
                ListItem::new(truncate_to_width(&display, item_width)).style(Style::new().fg(theme.text_secondary))
            },
        );
    }

    fn render_preview(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block = ratatui::widgets::Block::bordered().title(" Preview ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(session) = self.selected_session_index().and_then(|index| self.sessions.entries().get(index)) else {
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
            None if self.preview_enabled => {
                lines.push(Line::from(vec![Span::styled(" Loading preview...", Style::new().fg(theme.muted))]));
            }
            None => {}
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
