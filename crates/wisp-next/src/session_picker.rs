use crate::filterable_list::FilterableList;
use crate::overlay::{Overlay, OverlayMessage};
use crate::selection::Direction;
use crate::wrap::truncate_to_width;
use acp_utils::notifications::SessionPreviewResponse;
use agent_client_protocol::schema::{self as acp, SessionId};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, ListItem, Paragraph, Widget};
use std::collections::HashMap;

/// Picker for resuming a previous session, with an optional preview pane.
pub struct SessionPicker {
    sessions: FilterableList<acp::SessionInfo>,
    preview_enabled: bool,
    previews: HashMap<String, PreviewState>,
}

/// Below this width the preview pane is dropped and the list gets the full area.
const PREVIEW_MIN_WIDTH: u16 = 96;

#[derive(Clone)]
enum PreviewState {
    Loading,
    Loaded(SessionPreviewResponse),
    Error(String),
}

impl SessionPicker {
    pub fn new(sessions: Vec<acp::SessionInfo>, preview_enabled: bool) -> Self {
        Self {
            sessions: FilterableList::new(sessions, |session| {
                format!("{} {}", session.title.as_deref().unwrap_or(""), session.cwd.display())
            }),
            preview_enabled,
            previews: HashMap::new(),
        }
    }

    pub fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    /// Preview to fetch for the initially selected row, if previews are supported.
    pub fn initial_preview_request(&self) -> Option<String> {
        self.preview_enabled.then(|| self.sessions.selected_entry())?.map(|session| session.session_id.0.to_string())
    }

    pub fn on_preview_loaded(&mut self, preview: SessionPreviewResponse) {
        self.previews.insert(preview.session_id.clone(), PreviewState::Loaded(preview));
    }

    pub fn on_preview_failed(&mut self, session_id: &str, error: String) {
        self.previews.insert(session_id.to_string(), PreviewState::Error(error));
    }

    /// Marks the selected session's preview as in-flight and returns the request
    /// for it, or nothing when it is already loading or cached.
    fn preview_request(&mut self) -> Vec<OverlayMessage> {
        if !self.preview_enabled {
            return Vec::new();
        }
        let Some(session_id) = self.sessions.selected_entry().map(|session| session.session_id.0.to_string()) else {
            return Vec::new();
        };
        if self.previews.contains_key(&session_id) {
            return Vec::new();
        }
        self.previews.insert(session_id.clone(), PreviewState::Loading);
        vec![OverlayMessage::RequestSessionPreview { session_id }]
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let title = self.sessions.search_title("Sessions");
        let item_width = usize::from(area.width.saturating_sub(2));
        self.sessions
            .view(theme, "  (no matching sessions)", |session| {
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
            })
            .bordered(title)
            .render(area, buf);
    }

    fn render_preview(&self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        let block = Block::bordered().title(" Preview ").style(Style::new().fg(theme.text_primary));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(session) = self.sessions.selected_entry() else {
            return;
        };
        let muted = Style::new().fg(theme.muted);
        let mut lines = vec![
            Line::styled(
                format!(" Title: {}", session.title.as_deref().unwrap_or("(untitled)")),
                Style::new().fg(theme.text_primary),
            ),
            Line::styled(format!(" Path: {}", session.cwd.display()), muted),
        ];

        if let Some(timestamp) = &session.updated_at {
            lines.push(Line::styled(format!(" Updated: {timestamp}"), muted));
        }

        match self.previews.get(session.session_id.0.as_ref()) {
            None if self.preview_enabled => lines.push(Line::styled(" Loading preview...", muted)),
            None => {}
            Some(PreviewState::Loading) => lines.push(Line::styled(" Loading...", muted)),
            Some(PreviewState::Error(error)) => {
                lines.push(Line::styled(format!(" Error: {error}"), Style::new().fg(theme.error)));
            }
            Some(PreviewState::Loaded(preview)) => {
                lines.push(Line::styled(
                    format!(
                        " Model: {}  Mode: {}",
                        preview.model,
                        preview.selected_mode.as_deref().unwrap_or("default")
                    ),
                    muted,
                ));
                if preview.tool_call_count > 0 {
                    lines.push(Line::styled(format!(" Tool calls: {}", preview.tool_call_count), muted));
                }
                lines.push(Line::raw(""));
                for turn in &preview.transcript {
                    let role = match turn.role {
                        acp_utils::notifications::SessionPreviewRole::User => "user",
                        acp_utils::notifications::SessionPreviewRole::Assistant => "assistant",
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {role}: "), Style::new().fg(theme.accent)),
                        Span::styled(turn.text.clone(), Style::new().fg(theme.text_secondary)),
                    ]));
                }
                if preview.truncated {
                    lines.push(Line::styled(" ... preview truncated", muted));
                }
            }
        }

        Paragraph::new(lines).render(inner, buf);
    }
}

impl Overlay for SessionPicker {
    fn on_key(&mut self, key: KeyEvent) -> Vec<OverlayMessage> {
        match key.code {
            KeyCode::Esc => vec![OverlayMessage::Close],
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Enter => self
                .sessions
                .selected_entry()
                .map(|session| OverlayMessage::LoadSession {
                    session_id: SessionId::new(session.session_id.0.to_string()),
                    cwd: session.cwd.clone(),
                })
                .into_iter()
                .collect(),
            KeyCode::Char(character) => {
                self.sessions.push_query_char(character);
                self.preview_request()
            }
            KeyCode::Backspace => {
                self.sessions.pop_query_char();
                self.preview_request()
            }
            _ => Vec::new(),
        }
    }

    fn scroll(&mut self, direction: Direction) -> Vec<OverlayMessage> {
        self.sessions.step(direction, |_| true);
        self.preview_request()
    }

    fn click(&mut self, row: u16, _area: Rect) -> Vec<OverlayMessage> {
        self.sessions.select_row(usize::from(row.saturating_sub(1)));
        self.preview_request()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &crate::theme::Theme) {
        if !self.has_sessions() {
            let block = Block::bordered().title(" Sessions ").style(Style::new().fg(theme.text_primary));
            let inner = block.inner(area);
            block.render(area, buf);
            Paragraph::new("  No previous sessions found.").style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        if area.width >= PREVIEW_MIN_WIDTH {
            let [list_area, preview_area] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
            self.render_list(list_area, buf, theme);
            self.render_preview(preview_area, buf, theme);
        } else {
            self.render_list(area, buf, theme);
        }
    }
}
