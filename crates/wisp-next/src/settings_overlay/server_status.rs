use super::{LiveSettingsData, PaneOutcome, SettingsPane, summarize};
use crate::overlay::OverlayMessage;
use crate::selection::{Direction, SelectionState};
use crate::theme::Theme;
use acp_utils::notifications::{McpServerStatus, McpServerStatusEntry};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, Paragraph, StatefulWidget, Widget};

/// Read-only view of MCP server health, with authentication for OAuth servers.
pub(super) struct ServerStatusPane {
    rows: Vec<ServerStatusRow>,
    selection: SelectionState,
}

#[derive(Clone)]
enum ServerStatusRow {
    Header(String),
    Spacer,
    Server { entry: McpServerStatusEntry, indented: bool },
}

impl ServerStatusPane {
    pub(super) fn new(entries: Vec<McpServerStatusEntry>) -> Self {
        let mut pane = Self { rows: Vec::new(), selection: SelectionState::default() };
        pane.set_rows(entries, None);
        pane
    }

    fn selected_entry(&self) -> Option<&McpServerStatusEntry> {
        match self.selection.selected().and_then(|selected| self.rows.get(selected))? {
            ServerStatusRow::Server { entry, .. } => Some(entry),
            ServerStatusRow::Header(_) | ServerStatusRow::Spacer => None,
        }
    }

    fn authenticate(&self) -> PaneOutcome {
        PaneOutcome::message(
            self.selected_entry()
                .filter(|entry| entry.can_authenticate())
                .map(|entry| OverlayMessage::AuthenticateServer(entry.name.clone())),
        )
    }

    /// Rebuilds the row list, keeping focus on `keep` (or the first server).
    fn set_rows(&mut self, entries: Vec<McpServerStatusEntry>, keep: Option<&str>) {
        self.rows = build_rows(entries);
        let selected = keep
            .and_then(|name| {
                self.rows
                    .iter()
                    .position(|row| matches!(row, ServerStatusRow::Server { entry, .. } if entry.name == name))
            })
            .or_else(|| self.rows.iter().position(is_server));
        self.selection.select(selected, self.rows.len());
    }
}

impl SettingsPane for ServerStatusPane {
    fn on_key(&mut self, key: KeyEvent) -> PaneOutcome {
        match key.code {
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Enter => return self.authenticate(),
            _ => {}
        }
        PaneOutcome::default()
    }

    fn click(&mut self, row: usize, _height: usize) -> PaneOutcome {
        if !self.rows.get(row).is_some_and(is_server) {
            return PaneOutcome::default();
        }
        self.selection.select(Some(row), self.rows.len());
        self.authenticate()
    }

    fn scroll(&mut self, direction: Direction) {
        let rows = &self.rows;
        self.selection.step(rows.len(), direction, |index| is_server(&rows[index]));
    }

    fn refresh(&mut self, live: &LiveSettingsData) {
        let keep = self.selected_entry().map(|entry| entry.name.clone());
        self.set_rows(live.servers.clone(), keep.as_deref());
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if self.rows.is_empty() {
            Paragraph::new(" (no MCP servers configured)")
                .style(Style::new().fg(theme.text_secondary))
                .render(area, buf);
            return;
        }

        self.selection.ensure_visible(self.rows.len(), usize::from(area.height));
        let items = self.rows.iter().map(|row| match row {
            ServerStatusRow::Header(label) => ListItem::new(label.clone()).style(Style::new().fg(theme.heading)),
            ServerStatusRow::Spacer => ListItem::new(""),
            ServerStatusRow::Server { entry, indented } => {
                let (indicator, detail) = server_status_detail(entry);
                let prefix = if *indented { "  " } else { "" };
                let style = match &entry.status {
                    McpServerStatus::Connected { .. } | McpServerStatus::Connecting => {
                        Style::new().fg(theme.text_primary)
                    }
                    McpServerStatus::Failed { .. } => Style::new().fg(theme.error),
                    McpServerStatus::Authenticating | McpServerStatus::NeedsOAuth => Style::new().fg(theme.warning),
                };
                ListItem::new(format!(" {prefix}{}  {indicator} {detail}", entry.name)).style(style)
            }
        });
        let list = List::new(items)
            .highlight_style(Style::new().fg(theme.background).bg(theme.text_primary))
            .scroll_padding(1);
        StatefulWidget::render(list, area, buf, self.selection.list_state_mut());
    }

    fn footer(&self) -> String {
        "[Enter] Authenticate OAuth servers  [Esc] Back".to_string()
    }
}

fn is_server(row: &ServerStatusRow) -> bool {
    matches!(row, ServerStatusRow::Server { .. })
}

/// Groups servers under Direct/Proxied headings, but only when both kinds are
/// present — a flat list needs no headings.
fn build_rows(entries: Vec<McpServerStatusEntry>) -> Vec<ServerStatusRow> {
    let (proxied, direct): (Vec<_>, Vec<_>) = entries.into_iter().partition(|entry| entry.proxied);

    if proxied.is_empty() {
        return direct.into_iter().map(|entry| ServerStatusRow::Server { entry, indented: false }).collect();
    }

    let mut rows = Vec::new();
    if !direct.is_empty() {
        rows.push(ServerStatusRow::Header("Direct".to_string()));
        rows.extend(direct.into_iter().map(|entry| ServerStatusRow::Server { entry, indented: true }));
        rows.push(ServerStatusRow::Spacer);
    }
    rows.push(ServerStatusRow::Header("Proxied".to_string()));
    rows.extend(proxied.into_iter().map(|entry| ServerStatusRow::Server { entry, indented: true }));
    rows
}

fn server_status_detail(entry: &McpServerStatusEntry) -> (&'static str, String) {
    match &entry.status {
        McpServerStatus::Connected { tool_count } if entry.can_authenticate() => {
            ("✓", format!("{tool_count} tools, authenticated"))
        }
        McpServerStatus::Connected { tool_count } => ("✓", format!("{tool_count} tools")),
        McpServerStatus::Failed { error } => ("✗", error.clone()),
        McpServerStatus::Connecting => ("…", "connecting".to_string()),
        McpServerStatus::Authenticating => ("…", "authenticating".to_string()),
        McpServerStatus::NeedsOAuth => ("⚡", "needs authentication".to_string()),
    }
}

pub(super) fn summary(statuses: &[McpServerStatusEntry]) -> String {
    let count = |matches: fn(&McpServerStatus) -> bool| statuses.iter().filter(|e| matches(&e.status)).count();
    summarize(
        &[
            (count(|s| matches!(s, McpServerStatus::Connected { .. })), "connected"),
            (count(|s| matches!(s, McpServerStatus::Connecting)), "connecting"),
            (count(|s| matches!(s, McpServerStatus::Authenticating)), "authenticating"),
            (count(|s| matches!(s, McpServerStatus::NeedsOAuth)), "needs auth"),
            (count(|s| matches!(s, McpServerStatus::Failed { .. })), "failed"),
        ],
        "none",
    )
}
