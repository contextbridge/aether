use acp_utils::notifications::{McpServerStatus, McpServerStatusEntry};
use tui::{Component, Event, Frame, KeyCode, Line, MouseEventKind, ViewContext};

#[derive(Clone)]
enum ServerStatusRow {
    Header(String),
    Spacer,
    Server { entry: McpServerStatusEntry, indent: usize },
}

pub struct ServerStatusOverlay {
    rows: Vec<ServerStatusRow>,
    selected_row: Option<usize>,
}

pub enum ServerStatusMessage {
    Close,
    Authenticate(String),
}

impl Component for ServerStatusOverlay {
    type Message = ServerStatusMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if let Event::Mouse(mouse) = event {
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.move_selection(-1);
                    Some(vec![])
                }
                MouseEventKind::ScrollDown => {
                    self.move_selection(1);
                    Some(vec![])
                }
                _ => Some(vec![]),
            };
        }

        let Event::Key(key) = event else {
            return None;
        };

        match key.code {
            KeyCode::Esc => Some(vec![ServerStatusMessage::Close]),
            KeyCode::Up => {
                self.move_selection(-1);
                Some(vec![])
            }
            KeyCode::Down => {
                self.move_selection(1);
                Some(vec![])
            }
            KeyCode::Enter => match self.selected_entry() {
                Some(entry) if entry.can_authenticate() => {
                    Some(vec![ServerStatusMessage::Authenticate(entry.name.clone())])
                }
                Some(_) | None => Some(vec![]),
            },
            _ => Some(vec![]),
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        if self.rows.is_empty() {
            return Frame::new(vec![Line::new("  (no MCP servers configured)")]);
        }

        let inner = context.with_size((context.size.width.saturating_sub(2), context.size.height));
        Frame::new(
            self.rows
                .iter()
                .enumerate()
                .map(|(index, row)| match row {
                    ServerStatusRow::Header(label) => Line::new(label.clone()).prepend("  "),
                    ServerStatusRow::Spacer => Line::default(),
                    ServerStatusRow::Server { entry, indent } => {
                        render_server_entry(entry, Some(index) == self.selected_row, *indent, &inner).prepend("  ")
                    }
                })
                .collect(),
        )
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
        let rows = build_rows(entries);
        let selected_row = first_selectable_row(&rows);
        Self { rows, selected_row }
    }

    pub fn update_entries(&mut self, entries: Vec<McpServerStatusEntry>) {
        let selected_name = self.selected_entry().map(|entry| entry.name.clone());
        self.rows = build_rows(entries);
        self.selected_row = selected_name
            .and_then(|name| selectable_row_for_server(&self.rows, &name))
            .or_else(|| first_selectable_row(&self.rows));
    }

    fn selected_entry(&self) -> Option<&McpServerStatusEntry> {
        self.selected_row.and_then(|index| match self.rows.get(index) {
            Some(ServerStatusRow::Server { entry, .. }) => Some(entry),
            _ => None,
        })
    }

    fn move_selection(&mut self, direction: isize) {
        if let Some(current) = self.selected_row
            && let Some(next) = next_selectable_row(&self.rows, current, direction)
        {
            self.selected_row = Some(next);
        }
    }
}

fn build_rows(entries: Vec<McpServerStatusEntry>) -> Vec<ServerStatusRow> {
    if !has_proxied_entries(&entries) {
        return entries.into_iter().map(|entry| ServerStatusRow::Server { entry, indent: 0 }).collect();
    }

    let mut rows = Vec::new();
    let direct_entries: Vec<_> = entries.iter().filter(|entry| !entry.proxy).cloned().collect();
    let proxied_entries: Vec<_> = entries.into_iter().filter(|entry| entry.proxy).collect();

    if !direct_entries.is_empty() {
        rows.push(ServerStatusRow::Header("Direct".to_string()));
        rows.extend(direct_entries.into_iter().map(|entry| ServerStatusRow::Server { entry, indent: 1 }));
        if !proxied_entries.is_empty() {
            rows.push(ServerStatusRow::Spacer);
        }
    }

    if !proxied_entries.is_empty() {
        rows.push(ServerStatusRow::Header("Proxied".to_string()));
        rows.extend(proxied_entries.into_iter().map(|entry| ServerStatusRow::Server { entry, indent: 1 }));
    }

    rows
}

fn has_proxied_entries(entries: &[McpServerStatusEntry]) -> bool {
    entries.iter().any(|entry| entry.proxy)
}

fn first_selectable_row(rows: &[ServerStatusRow]) -> Option<usize> {
    rows.iter().position(|row| matches!(row, ServerStatusRow::Server { .. }))
}

fn selectable_row_for_server(rows: &[ServerStatusRow], name: &str) -> Option<usize> {
    rows.iter().position(|row| match row {
        ServerStatusRow::Server { entry, .. } => entry.name == name,
        _ => false,
    })
}

fn next_selectable_row(rows: &[ServerStatusRow], current: usize, direction: isize) -> Option<usize> {
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| matches!(row, ServerStatusRow::Server { .. }).then_some(index))
        .collect();
    if selectable.is_empty() {
        return None;
    }

    let position = selectable.iter().position(|index| *index == current).unwrap_or(0);
    let next_position = if direction < 0 {
        position.checked_sub(1).unwrap_or(selectable.len() - 1)
    } else {
        (position + 1) % selectable.len()
    };
    selectable.get(next_position).copied()
}

fn render_server_entry(entry: &McpServerStatusEntry, selected: bool, indent: usize, context: &ViewContext) -> Line {
    let (indicator, detail) = match &entry.status {
        McpServerStatus::Connected { tool_count } if entry.can_authenticate() => {
            ("✓", format!("{tool_count} tools, authenticated"))
        }
        McpServerStatus::Connected { tool_count } => ("✓", format!("{tool_count} tools")),
        McpServerStatus::Failed { error } => ("✗", error.clone()),
        McpServerStatus::Authenticating => ("…", "authenticating".to_string()),
        McpServerStatus::NeedsOAuth => ("⚡", "needs authentication".to_string()),
    };
    let text = format!("{}{}  {indicator} {detail}", "  ".repeat(indent), entry.name);
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
            McpServerStatusEntry::new("math", McpServerStatus::Connected { tool_count: 3 }).as_proxied(),
            McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
                .with_auth_capability(McpServerAuthCapability::OAuth)
                .as_proxied(),
        ]
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
        assert_eq!(text[2], "");
        assert_eq!(text[3].trim(), "Proxied");
        assert!(text[4].contains("  math  ✓ 3 tools"));
        assert!(text[5].contains("  linear  ⚡ needs authentication"));
    }

    #[test]
    fn does_not_render_proxy_status_row() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        let ctx = ViewContext::new((80, 24));
        let rendered = overlay.render(&ctx).lines().iter().map(tui::Line::plain_text).collect::<Vec<_>>().join("\n");

        assert!(!rendered.contains("proxy  ✓ 1 tool"));
    }

    #[tokio::test]
    async fn navigation_skips_headers_and_spacers() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());

        assert_eq!(overlay.selected_row, Some(1));
        overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Down, tui::KeyModifiers::NONE))).await;
        assert_eq!(overlay.selected_row, Some(4));
        overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Up, tui::KeyModifiers::NONE))).await;
        assert_eq!(overlay.selected_row, Some(1));
        overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Up, tui::KeyModifiers::NONE))).await;
        assert_eq!(overlay.selected_row, Some(5));
    }

    #[tokio::test]
    async fn enter_on_proxied_oauth_server_emits_nested_server_name() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        overlay.selected_row = Some(5);

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

        let outcome =
            overlay.on_event(&Event::Key(tui::KeyEvent::new(tui::KeyCode::Enter, tui::KeyModifiers::NONE))).await;
        assert!(outcome.unwrap().is_empty());
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
    fn update_entries_preserves_selection_by_server_name() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        overlay.selected_row = Some(5);

        overlay.update_entries(vec![
            McpServerStatusEntry::new("linear", McpServerStatus::Connected { tool_count: 7 }).as_proxied(),
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 3 }),
        ]);

        assert_eq!(overlay.selected_entry().map(|entry| entry.name.as_str()), Some("linear"));
    }

    #[test]
    fn update_entries_falls_back_to_first_selectable_row() {
        let mut overlay = ServerStatusOverlay::new(mixed_entries());
        overlay.selected_row = Some(5);

        overlay.update_entries(vec![McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 3 })]);
        assert_eq!(overlay.selected_entry().map(|entry| entry.name.as_str()), Some("github"));
    }
}
