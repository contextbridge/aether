use super::{KeyHint, LiveSettingsData, SettingsPane, summarize};
use crate::list_view::ListView;
use crate::render_context::RenderContext;
use crate::selection::{Direction, SelectionState};
use crate::surface::{Action, Surface, one};
use acp_utils::notifications::{McpServerStatus, McpServerStatusEntry};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Widget;

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

impl Surface for ServerStatusPane {
    fn activate(&mut self) -> Vec<Action> {
        one(self
            .selected_entry()
            .filter(|entry| entry.can_authenticate())
            .map(|entry| Action::AuthenticateServer(entry.name.clone())))
    }

    /// Headers and spacers are not selectable, so a click on one is ignored
    /// rather than moving focus to a row that cannot be authenticated.
    fn click(&mut self, row: u16, _column: u16) -> Vec<Action> {
        let restore = self.selection.selected();
        if !self.selection.select_at(row, self.rows.len()) {
            return Vec::new();
        }
        if !self.selection.selected().and_then(|index| self.rows.get(index)).is_some_and(is_server) {
            self.selection.select(restore, self.rows.len());
            return Vec::new();
        }
        self.activate()
    }

    fn scroll(&mut self, direction: Direction) -> Vec<Action> {
        let rows = &self.rows;
        self.selection.step(rows.len(), direction, |index| is_server(&rows[index]));
        Vec::new()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        let rows = self.rows.iter().map(|row| match row {
            ServerStatusRow::Header(label) => Line::styled(label.clone(), Style::new().fg(theme.heading)),
            ServerStatusRow::Spacer => Line::default(),
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
                let row_prefix = (area.width > 30).then_some(" ").unwrap_or("");
                let name_separator = if area.width <= 30 { " " } else { "  " };
                Line::styled(format!("{row_prefix}{prefix}{}{name_separator}{indicator} {detail}", entry.name), style)
            }
        });
        ListView::new(rows.collect(), &mut self.selection, theme)
            .pane(" (no MCP servers configured)")
            .render(area, buf);
        None
    }
}

impl SettingsPane for ServerStatusPane {
    fn refresh(&mut self, live: &LiveSettingsData) {
        let keep = self.selected_entry().map(|entry| entry.name.clone());
        self.set_rows(live.servers.clone(), keep.as_deref());
    }

    fn footer(&self) -> Vec<KeyHint> {
        vec![("Enter", "authenticate OAuth servers".into()), ("Esc", "back".into())]
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
