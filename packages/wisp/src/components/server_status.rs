use acp_utils::notifications::{McpServerStatus, McpServerStatusEntry};
use tui::{Component, Event, Frame, Line, SelectItem, SelectList, SelectListMessage, ViewContext};

struct ServerItem(McpServerStatusEntry);

impl SelectItem for ServerItem {
    fn render_item(&self, selected: bool, context: &ViewContext) -> Line {
        let (indicator, detail) = match &self.0.status {
            McpServerStatus::Connected { tool_count } if self.0.can_authenticate() => {
                ("✓", format!("{tool_count} tools, authenticated"))
            }
            McpServerStatus::Connected { tool_count } => ("✓", format!("{tool_count} tools")),
            McpServerStatus::Failed { error } => ("✗", error.clone()),
            McpServerStatus::Authenticating => ("…", "authenticating".to_string()),
            McpServerStatus::NeedsOAuth => ("⚡", "needs authentication".to_string()),
        };
        let text = format!("{}  {indicator} {detail}", self.0.name);
        match &self.0.status {
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
}

pub struct ServerStatusOverlay {
    list: SelectList<ServerItem>,
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
                if let Some(item) = self.list.selected_item()
                    && item.0.can_authenticate()
                {
                    return Some(vec![ServerStatusMessage::Authenticate(item.0.name.clone())]);
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
        let items: Vec<ServerItem> = entries.into_iter().map(ServerItem).collect();
        Self { list: SelectList::new(items, "no MCP servers configured") }
    }

    pub fn update_entries(&mut self, entries: Vec<McpServerStatusEntry>) {
        let prev_index = self.list.selected_index();
        let items: Vec<ServerItem> = entries.into_iter().map(ServerItem).collect();
        self.list.set_items(items);
        let max = self.list.len().saturating_sub(1);
        self.list.set_selected(prev_index.min(max));
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

    #[test]
    fn renders_all_entries_with_status_indicators() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);

        assert_eq!(frame.lines().len(), 3);
        let text0 = frame.lines()[0].plain_text();
        assert!(text0.contains("github"), "should contain server name");
        assert!(text0.contains("✓"), "connected should show checkmark");
        assert!(text0.contains("5 tools"), "should show tool count");

        let text1 = frame.lines()[1].plain_text();
        assert!(text1.contains("linear"), "should contain server name");
        assert!(text1.contains("⚡"), "needs auth should show bolt");

        let text2 = frame.lines()[2].plain_text();
        assert!(text2.contains("slack"), "should contain server name");
        assert!(text2.contains("✗"), "failed should show X");
        assert!(text2.contains("connection timeout"), "should show error");
    }

    #[test]
    fn selected_entry_has_pointer() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);

        assert!(frame.lines()[0].plain_text().starts_with("  "));
        assert!(frame.lines()[1].plain_text().starts_with("  "));
    }

    #[tokio::test]
    async fn navigation_wraps_around() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());

        overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Up, tui::KeyModifiers::NONE))).await;
        assert_eq!(overlay.list.selected_index(), 2);

        overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Down, tui::KeyModifiers::NONE))).await;
        assert_eq!(overlay.list.selected_index(), 0);
    }

    #[tokio::test]
    async fn enter_on_needs_oauth_emits_authenticate() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        overlay.list.set_selected(1); // linear - NeedsOAuth

        let outcome =
            overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Enter, tui::KeyModifiers::NONE))).await;
        let messages = outcome.unwrap();
        match messages.as_slice() {
            [ServerStatusMessage::Authenticate(name)] => assert_eq!(name, "linear"),
            _ => panic!("Expected Authenticate message"),
        }
    }

    #[tokio::test]
    async fn enter_on_connected_without_auth_is_noop() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        // index 0 = github (Connected)

        let outcome =
            overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Enter, tui::KeyModifiers::NONE))).await;
        assert!(outcome.unwrap().is_empty());
    }

    #[tokio::test]
    async fn enter_on_connected_oauth_server_emits_authenticate_for_reauth() {
        let mut overlay = ServerStatusOverlay::new(vec![
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 })
                .with_auth_capability(McpServerAuthCapability::OAuth),
        ]);

        let outcome =
            overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Enter, tui::KeyModifiers::NONE))).await;
        let messages = outcome.unwrap();
        match messages.as_slice() {
            [ServerStatusMessage::Authenticate(name)] => assert_eq!(name, "github"),
            _ => panic!("Expected Authenticate message"),
        }
    }

    #[tokio::test]
    async fn esc_closes_overlay() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        let outcome =
            overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Esc, tui::KeyModifiers::NONE))).await;
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
    fn update_entries_clamps_index() {
        let mut overlay = ServerStatusOverlay::new(sample_entries());
        overlay.list.set_selected(2);

        overlay.update_entries(vec![McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 3 })]);
        assert_eq!(overlay.list.selected_index(), 0);
    }
}
