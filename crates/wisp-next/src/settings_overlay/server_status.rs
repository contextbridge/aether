use crate::theme::Theme;
use acp_utils::notifications::{McpServerStatus, McpServerStatusEntry};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, ListState, Paragraph, StatefulWidget, Widget};

pub(super) struct ServerStatusPane {
    rows: Vec<ServerStatusRow>,
    state: ListState,
}

#[derive(Clone)]
enum ServerStatusRow {
    Header(String),
    Spacer,
    Server { entry: McpServerStatusEntry, indented: bool },
}

impl ServerStatusPane {
    pub(super) fn new(entries: Vec<McpServerStatusEntry>) -> Self {
        let rows = build_rows(entries);
        let selected = rows.iter().position(|row| matches!(row, ServerStatusRow::Server { .. }));
        let mut state = ListState::default();
        state.select(selected);
        Self { rows, state }
    }

    pub(super) fn move_up(&mut self) {
        self.move_selection(-1);
    }

    pub(super) fn move_down(&mut self) {
        self.move_selection(1);
    }

    pub(super) fn selected_entry(&self) -> Option<&McpServerStatusEntry> {
        match self.state.selected().and_then(|selected| self.rows.get(selected))? {
            ServerStatusRow::Server { entry, .. } => Some(entry),
            _ => None,
        }
    }

    pub(super) fn click_row(&mut self, row: usize) -> bool {
        if !matches!(self.rows.get(row), Some(ServerStatusRow::Server { .. })) {
            return false;
        }
        self.state.select(Some(row));
        true
    }

    pub(super) fn update_entries(&mut self, entries: Vec<McpServerStatusEntry>) {
        let selected_name = self.selected_entry().map(|entry| entry.name.clone());
        self.rows = build_rows(entries);
        let selected = selected_name
            .and_then(|name| {
                self.rows
                    .iter()
                    .position(|row| matches!(row, ServerStatusRow::Server { entry, .. } if entry.name == name))
            })
            .or_else(|| self.rows.iter().position(|row| matches!(row, ServerStatusRow::Server { .. })));
        self.state.select(selected);
    }

    pub(super) fn move_selection(&mut self, direction: isize) {
        if self.rows.is_empty() {
            return;
        }
        let start = self.state.selected().unwrap_or_default();
        let mut selected = start;
        loop {
            selected = selected.saturating_add_signed(direction);
            if direction < 0 && selected == 0 && start == 0 {
                selected = self.rows.len() - 1;
            } else if direction > 0 && selected >= self.rows.len() {
                selected = 0;
            }
            if matches!(self.rows[selected], ServerStatusRow::Server { .. }) || selected == start {
                self.state.select(Some(selected));
                break;
            }
        }
    }

    pub(super) fn authentication_message(&self) -> Option<super::SettingsOverlayMessage> {
        self.selected_entry()
            .filter(|entry| entry.can_authenticate())
            .map(|entry| super::SettingsOverlayMessage::AuthenticateServer(entry.name.clone()))
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        if self.rows.is_empty() {
            Paragraph::new(" (no MCP servers configured)")
                .style(Style::new().fg(theme.text_secondary))
                .render(area, buffer);
            return;
        }

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
        StatefulWidget::render(list, area, buffer, &mut self.state);
    }
}

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
pub(super) fn server_status_summary(statuses: &[McpServerStatusEntry]) -> String {
    if statuses.is_empty() {
        return "none".to_string();
    }
    let mut connected = 0usize;
    let mut connecting = 0usize;
    let mut authenticating = 0usize;
    let mut needs_auth = 0usize;
    let mut failed = 0usize;
    for entry in statuses {
        match &entry.status {
            McpServerStatus::Connected { .. } => connected += 1,
            McpServerStatus::Connecting => connecting += 1,
            McpServerStatus::Authenticating => authenticating += 1,
            McpServerStatus::NeedsOAuth => needs_auth += 1,
            McpServerStatus::Failed { .. } => failed += 1,
        }
    }
    let parts: Vec<String> = [
        (connected, "connected"),
        (connecting, "connecting"),
        (authenticating, "authenticating"),
        (needs_auth, "needs auth"),
        (failed, "failed"),
    ]
    .iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    if parts.is_empty() { "none".to_string() } else { parts.join(", ") }
}
