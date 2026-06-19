use tui::{Component, Event, Frame, Line, SelectItem, SelectList, SelectListMessage, ViewContext};

pub struct ProviderLoginOverlay {
    list: SelectList<ProviderLoginEntry>,
}

pub struct ProviderLoginEntry {
    pub method_id: String,
    pub name: String,
    pub status: ProviderLoginStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderLoginStatus {
    NeedsLogin,
    Authenticating,
    LoggedIn,
}

pub enum ProviderLoginMessage {
    Close,
    Authenticate(String),
}

impl SelectItem for ProviderLoginEntry {
    fn render_item(&self, selected: bool, context: &ViewContext) -> Line {
        let (indicator, detail) = match &self.status {
            ProviderLoginStatus::NeedsLogin => ("⚡", "needs login"),
            ProviderLoginStatus::Authenticating => ("⏳", "authenticating..."),
            ProviderLoginStatus::LoggedIn => ("✓", "logged in"),
        };
        let text = format!("{}  {indicator} {detail}", self.name);
        if self.status == ProviderLoginStatus::LoggedIn {
            if selected {
                Line::with_style(text, context.theme.selected_row_style_with_fg(context.theme.success()))
            } else {
                Line::styled(text, context.theme.success())
            }
        } else if selected {
            Line::with_style(text, context.theme.selected_row_style_with_fg(context.theme.warning()))
        } else {
            Line::styled(text, context.theme.warning())
        }
    }
}

impl Component for ProviderLoginOverlay {
    type Message = ProviderLoginMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        let outcome = self.list.on_event(event).await;
        match outcome.as_deref() {
            Some([SelectListMessage::Close]) => Some(vec![ProviderLoginMessage::Close]),
            Some([SelectListMessage::Select(_)]) => {
                if let Some(entry) = self.list.selected_item()
                    && entry.status != ProviderLoginStatus::Authenticating
                {
                    return Some(vec![ProviderLoginMessage::Authenticate(entry.method_id.clone())]);
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

pub fn provider_login_summary(entries: &[ProviderLoginEntry]) -> String {
    if entries.is_empty() {
        return "all logged in".to_string();
    }
    let needs_login = entries.iter().filter(|e| e.status == ProviderLoginStatus::NeedsLogin).count();
    let authenticating = entries.iter().filter(|e| e.status == ProviderLoginStatus::Authenticating).count();
    let logged_in = entries.iter().filter(|e| e.status == ProviderLoginStatus::LoggedIn).count();
    let parts: Vec<String> =
        [(needs_login, "needs login"), (authenticating, "authenticating"), (logged_in, "logged in")]
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, label)| format!("{count} {label}"))
            .collect();
    if parts.is_empty() { "all logged in".to_string() } else { parts.join(", ") }
}

impl ProviderLoginOverlay {
    pub fn new(entries: Vec<ProviderLoginEntry>) -> Self {
        Self { list: SelectList::new(entries, "no providers need login") }
    }

    pub fn replace_entries(&mut self, entries: Vec<ProviderLoginEntry>) {
        let selected_method_id = self.list.selected_item().map(|entry| entry.method_id.clone());

        self.list.set_items(entries);

        if let Some(selected_method_id) = selected_method_id
            && let Some(index) = self.list.items().iter().position(|entry| entry.method_id == selected_method_id)
        {
            self.list.set_selected(index);
        }
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[ProviderLoginEntry] {
        self.list.items()
    }

    pub fn reset_to_needs_login(&mut self, method_id: &str) {
        if let Some(entry) = self.list.items_mut().iter_mut().find(|e| e.method_id == method_id) {
            entry.status = ProviderLoginStatus::NeedsLogin;
        }
    }

    pub fn set_logged_in(&mut self, method_id: &str) {
        if let Some(entry) = self.list.items_mut().iter_mut().find(|e| e.method_id == method_id) {
            entry.status = ProviderLoginStatus::LoggedIn;
        }
    }

    pub fn set_authenticating(&mut self, method_id: &str) {
        if let Some(entry) = self.list.items_mut().iter_mut().find(|e| e.method_id == method_id) {
            entry.status = ProviderLoginStatus::Authenticating;
        }
    }

    #[cfg(test)]
    pub fn remove_entry(&mut self, method_id: &str) {
        self.list.retain(|e| e.method_id != method_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{KeyCode, KeyEvent, KeyModifiers};

    fn sample_entries() -> Vec<ProviderLoginEntry> {
        vec![ProviderLoginEntry {
            method_id: "codex".to_string(),
            name: "Codex".to_string(),
            status: ProviderLoginStatus::NeedsLogin,
        }]
    }

    #[test]
    fn renders_entries_with_status_indicators() {
        let mut overlay = ProviderLoginOverlay::new(sample_entries());
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);

        assert_eq!(frame.lines().len(), 1);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("Codex"), "should contain provider name");
        assert!(text.contains("⚡"), "needs login should show bolt");
    }

    #[tokio::test]
    async fn enter_on_needs_login_emits_authenticate() {
        let mut overlay = ProviderLoginOverlay::new(sample_entries());
        let outcome = overlay.on_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))).await;
        let messages = outcome.unwrap();
        match messages.as_slice() {
            [ProviderLoginMessage::Authenticate(id)] => assert_eq!(id, "codex"),
            _ => panic!("Expected Authenticate message"),
        }
    }

    #[tokio::test]
    async fn enter_on_authenticating_is_noop() {
        let mut entries = sample_entries();
        entries[0].status = ProviderLoginStatus::Authenticating;
        let mut overlay = ProviderLoginOverlay::new(entries);
        let outcome = overlay.on_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))).await;
        assert!(outcome.unwrap().is_empty());
    }

    #[tokio::test]
    async fn esc_closes_overlay() {
        let mut overlay = ProviderLoginOverlay::new(sample_entries());
        let outcome = overlay.on_event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))).await;
        let messages = outcome.unwrap();
        assert!(matches!(messages.as_slice(), [ProviderLoginMessage::Close]));
    }

    #[test]
    fn empty_entries_shows_placeholder() {
        let mut overlay = ProviderLoginOverlay::new(vec![]);
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);
        assert!(frame.lines()[0].plain_text().contains("no providers need login"));
    }

    #[test]
    fn set_authenticating_updates_status() {
        let mut overlay = ProviderLoginOverlay::new(sample_entries());
        overlay.set_authenticating("codex");
        assert_eq!(overlay.entries()[0].status, ProviderLoginStatus::Authenticating);
    }

    #[test]
    fn remove_entry_clamps_selection() {
        let entries = vec![
            ProviderLoginEntry {
                method_id: "a".to_string(),
                name: "A".to_string(),
                status: ProviderLoginStatus::NeedsLogin,
            },
            ProviderLoginEntry {
                method_id: "b".to_string(),
                name: "B".to_string(),
                status: ProviderLoginStatus::NeedsLogin,
            },
        ];
        let mut overlay = ProviderLoginOverlay::new(entries);
        overlay.list.set_selected(1);
        overlay.remove_entry("b");
        assert_eq!(overlay.entries().len(), 1);
        assert_eq!(overlay.list.selected_index(), 0);
    }

    #[test]
    fn provider_login_summary_formats_correctly() {
        assert_eq!(provider_login_summary(&[]), "all logged in");
        assert_eq!(provider_login_summary(&sample_entries()), "1 needs login");
    }

    #[test]
    fn provider_login_summary_shows_logged_in() {
        let entries = vec![ProviderLoginEntry {
            method_id: "codex".to_string(),
            name: "Codex".to_string(),
            status: ProviderLoginStatus::LoggedIn,
        }];
        assert_eq!(provider_login_summary(&entries), "1 logged in");
    }

    #[test]
    fn provider_login_summary_mixed_statuses() {
        let entries = vec![
            ProviderLoginEntry {
                method_id: "a".to_string(),
                name: "A".to_string(),
                status: ProviderLoginStatus::NeedsLogin,
            },
            ProviderLoginEntry {
                method_id: "b".to_string(),
                name: "B".to_string(),
                status: ProviderLoginStatus::LoggedIn,
            },
        ];
        assert_eq!(provider_login_summary(&entries), "1 needs login, 1 logged in");
    }

    #[test]
    fn set_logged_in_updates_status() {
        let mut overlay = ProviderLoginOverlay::new(sample_entries());
        overlay.set_logged_in("codex");
        assert_eq!(overlay.entries()[0].status, ProviderLoginStatus::LoggedIn);
    }

    #[test]
    fn renders_logged_in_with_check_mark() {
        let entries = vec![ProviderLoginEntry {
            method_id: "codex".to_string(),
            name: "Codex".to_string(),
            status: ProviderLoginStatus::LoggedIn,
        }];
        let mut overlay = ProviderLoginOverlay::new(entries);
        let ctx = ViewContext::new((80, 24));
        let frame = overlay.render(&ctx);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("✓"), "logged in should show check mark");
        assert!(text.contains("logged in"), "should show 'logged in' text");
    }

    #[tokio::test]
    async fn enter_on_logged_in_emits_authenticate_for_reauth() {
        let entries = vec![ProviderLoginEntry {
            method_id: "codex".to_string(),
            name: "Codex".to_string(),
            status: ProviderLoginStatus::LoggedIn,
        }];
        let mut overlay = ProviderLoginOverlay::new(entries);
        let outcome = overlay.on_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))).await;
        let messages = outcome.unwrap();
        match messages.as_slice() {
            [ProviderLoginMessage::Authenticate(id)] => assert_eq!(id, "codex"),
            _ => panic!("Expected Authenticate message for re-auth"),
        }
    }
}
