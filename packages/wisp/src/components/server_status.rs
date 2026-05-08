use acp_utils::notifications::{McpServerStatus, McpServerStatusEntry};
use tui::{Component, Event, Frame, Line, SelectItem, SelectList, SelectListMessage, ViewContext};

pub struct ServerStatusOverlay {
    list: SelectList<ServerStatusRow>,
}

pub enum ServerStatusMessage {
    Close,
    Authenticate(String),
}

impl Component for ServerStatusOverlay {
    type Message = ServerStatusMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let outcome = self.list.on_event(event).await;
        match outcome.as_deref() {
            Some([SelectListMessage::Close]) => Some(vec![ServerStatusMessage::Close]),
            Some([SelectListMessage::Select(_)]) => {
                if let Some(ServerStatusRow::Server { entry, .. }) = self.list.selected_item()
                    && entry.can_authenticate()
                {
                    return Some(vec![ServerStatusMessage::Authenticate(entry.name.clone())]);
                }
                Some(vec![])
            }
            _ => outcome.map(|_| vec![]),
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        self.list.render(context)
    }
}

pub fn server_status_summary(statuses: &[McpServerStatusEntry]) -> String {
    if statuses.is_empty() {
        return "none".to_string();
    }
    let (mut c, mut a, mut n, mut f) = (0usize, 0usize, 0usize, 0usize);
    for s in statuses {
        match &s.status {
            McpServerStatus::Connected { .. } => c += 1,
            McpServerStatus::Authenticating => a += 1,
            McpServerStatus::NeedsOAuth => n += 1,
            McpServerStatus::Failed { .. } => f += 1,
        }
    }
    [(c, "connected"), (a, "authenticating"), (n, "needs auth"), (f, "failed")]
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, label)| format!("{count} {label}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl ServerStatusOverlay {
    pub fn new(entries: Vec<McpServerStatusEntry>) -> Self {
        Self { list: SelectList::new(build_rows(entries), "no MCP servers configured") }
    }

    pub fn update_entries(&mut self, entries: Vec<McpServerStatusEntry>) {
        let selected_name = match self.list.selected_item() {
            Some(ServerStatusRow::Server { entry, .. }) => Some(entry.name.clone()),
            _ => None,
        };
        self.list.set_items(build_rows(entries));
        if let Some(name) = selected_name {
            self.list.select_where(|row| matches!(row, ServerStatusRow::Server { entry, .. } if entry.name == name));
        }
    }
}

#[derive(Clone)]
enum ServerStatusRow {
    Header(String),
    Spacer,
    Server { entry: McpServerStatusEntry, indented: bool },
}

impl SelectItem for ServerStatusRow {
    fn render_item(&self, selected: bool, context: &ViewContext) -> Line {
        match self {
            ServerStatusRow::Header(label) => Line::new(label.clone()),
            ServerStatusRow::Spacer => Line::default(),
            ServerStatusRow::Server { entry, indented } => render_server_entry(entry, selected, *indented, context),
        }
    }

    fn is_selectable(&self) -> bool {
        matches!(self, ServerStatusRow::Server { .. })
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

fn render_server_entry(entry: &McpServerStatusEntry, selected: bool, indented: bool, context: &ViewContext) -> Line {
    let (indicator, detail) = match &entry.status {
        McpServerStatus::Connected { tool_count } if entry.can_authenticate() => {
            ("✓", format!("{tool_count} tools, authenticated"))
        }
        McpServerStatus::Connected { tool_count } => ("✓", format!("{tool_count} tools")),
        McpServerStatus::Failed { error } => ("✗", error.clone()),
        McpServerStatus::Authenticating => ("…", "authenticating".to_string()),
        McpServerStatus::NeedsOAuth => ("⚡", "needs authentication".to_string()),
    };
    let prefix = if indented { "  " } else { "" };
    let text = format!("{prefix}{}  {indicator} {detail}", entry.name);
    match &entry.status {
        McpServerStatus::Connected { .. } => {
            if selected {
                Line::with_style(text, context.theme.selected_row_style())
            } else {
                Line::new(text)
            }
        }
        McpServerStatus::Failed { .. } => {
            if selected {
                Line::with_style(text, context.theme.selected_row_style_with_fg(context.theme.error()))
            } else {
                Line::styled(text, context.theme.error())
            }
        }
        McpServerStatus::Authenticating | McpServerStatus::NeedsOAuth => {
            if selected {
                Line::with_style(text, context.theme.selected_row_style_with_fg(context.theme.warning()))
            } else {
                Line::styled(text, context.theme.warning())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::notifications::McpServerAuthCapability;

    fn sample_entries() -> Vec<McpServerStatusEntry> {
        vec![
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
            McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
                .with_auth_capability(McpServerAuthCapability::OAuth),
            McpServerStatusEntry::new("slack", McpServerStatus::Failed { error: "connection timeout".to_string() }),
        ]
    }

    fn mixed_entries() -> Vec<McpServerStatusEntry> {
        vec![
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
            McpServerStatusEntry::new("math", McpServerStatus::Connected { tool_count: 3 }).with_proxied(true),
            McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
                .with_auth_capability(McpServerAuthCapability::OAuth)
                .with_proxied(true),
        ]
    }

    fn key(code: tui::KeyCode) -> Event {
        Event::Key(tui::KeyEvent::new(code, tui::KeyModifiers::NONE))
    }

    #[test]
    fn renders_flat_entries_when_no_proxy_exists() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);

        assert_eq!(frame.lines().len(), 3);
        assert!(frame.lines()[0].plain_text().contains("github"));
        assert!(frame.lines()[0].plain_text().contains("✓"));
        assert!(frame.lines()[0].plain_text().contains("5 tools"));
        assert!(frame.lines()[1].plain_text().contains("linear"));
        assert!(frame.lines()[1].plain_text().contains("⚡"));
        assert!(frame.lines()[2].plain_text().contains("slack"));
        assert!(frame.lines()[2].plain_text().contains("✗"));
        assert!(frame.lines()[2].plain_text().contains("connection timeout"));
        assert!(!frame.lines().iter().any(|line| line.plain_text().contains("Direct")));
    }

    #[test]
    fn renders_direct_and_proxied_sections_when_proxy_exists() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        let ctx = ViewContext::new((80, 24));
        let text: Vec<_> = overlay.render(&ctx).lines().iter().map(tui::Line::plain_text).collect();

        assert_eq!(text[0].trim(), "Direct");
        assert!(text[1].contains("  github  ✓ 5 tools"));
        assert!(text[2].trim().is_empty());
        assert_eq!(text[3].trim(), "Proxied");
        assert!(text[4].contains("  math  ✓ 3 tools"));
        assert!(text[5].contains("  linear  ⚡ needs authentication"));
        assert!(!text.join("\n").contains("proxy  ✓ 1 tool"));
    }

    #[tokio::test]
    async fn navigation_skips_headers_and_spacers() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());

        assert_eq!(overlay.list.selected_index(), 1);
        overlay.on_event(&key(tui::KeyCode::Down)).await;
        assert_eq!(overlay.list.selected_index(), 4);
        overlay.on_event(&key(tui::KeyCode::Up)).await;
        assert_eq!(overlay.list.selected_index(), 1);
        overlay.on_event(&key(tui::KeyCode::Up)).await;
        assert_eq!(overlay.list.selected_index(), 5);
    }

    #[tokio::test]
    async fn enter_on_proxied_oauth_server_emits_nested_server_name() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        overlay.list.set_selected(5);

        let outcome = overlay.on_event(&key(tui::KeyCode::Enter)).await;
        let messages = outcome.unwrap();
        match messages.as_slice() {
            [ServerStatusMessage::Authenticate(name)] => assert_eq!(name, "linear"),
            _ => panic!("Expected Authenticate message"),
        }
    }

    #[tokio::test]
    async fn enter_on_connected_without_auth_is_noop() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());

        let outcome = overlay.on_event(&key(tui::KeyCode::Enter)).await;
        assert!(outcome.unwrap().is_empty());
    }

    #[tokio::test]
    async fn esc_closes_overlay() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        let outcome = overlay.on_event(&key(tui::KeyCode::Esc)).await;
        let messages = outcome.unwrap();
        assert!(matches!(messages.as_slice(), [ServerStatusMessage::Close]));
    }

    #[test]
    fn empty_entries_shows_placeholder() {
        let mut overlay = ServerStatusOverlay::new(vec![]);
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);
        assert_eq!(frame.lines().len(), 1);
        assert!(frame.lines()[0].plain_text().contains("no MCP servers configured"));
    }

    #[test]
    fn update_entries_preserves_selection_by_server_name() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        overlay.list.set_selected(5);

        overlay.update_entries(vec![
            McpServerStatusEntry::new("linear", McpServerStatus::Connected { tool_count: 7 }).with_proxied(true),
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 3 }),
        ]);

        let selected = match overlay.list.selected_item() {
            Some(ServerStatusRow::Server { entry, .. }) => Some(entry.name.as_str()),
            _ => None,
        };
        assert_eq!(selected, Some("linear"));
    }

    #[test]
    fn update_entries_falls_back_to_first_selectable_row() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        overlay.list.set_selected(5);

        overlay.update_entries(vec![McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 3 })]);

        let selected = match overlay.list.selected_item() {
            Some(ServerStatusRow::Server { entry, .. }) => Some(entry.name.as_str()),
            _ => None,
        };
        assert_eq!(selected, Some("github"));
    }
}
