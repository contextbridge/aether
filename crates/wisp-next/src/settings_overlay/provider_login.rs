use crate::filterable_list::FilterableList;
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use agent_client_protocol::schema::AuthMethod;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::ListItem;

pub(super) struct ProviderLoginPane {
    entries: FilterableList<ProviderLoginEntry>,
}

pub(super) struct ProviderLoginEntry {
    method_id: String,
    name: String,
    status: ProviderLoginStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderLoginStatus {
    NeedsLogin,
    Authenticating,
    LoggedIn,
}

impl ProviderLoginPane {
    pub(super) fn new(entries: Vec<ProviderLoginEntry>) -> Self {
        Self { entries: FilterableList::new(entries, |entry| entry.name.clone()) }
    }

    pub(super) fn move_up(&mut self) {
        self.entries.select_previous();
    }

    pub(super) fn move_down(&mut self) {
        self.entries.select_next();
    }

    pub(super) fn selected_entry(&self) -> Option<&ProviderLoginEntry> {
        self.entries.selected_entry()
    }

    pub(super) fn click_row(&mut self, row: usize) -> bool {
        self.entries.select_row(row);
        self.entries.selected_entry().is_some()
    }

    pub(super) fn set_authenticating(&mut self, method_id: &str) {
        if let Some(entry) = self.entries.entries().iter().position(|entry| entry.method_id == method_id) {
            self.entries.entries_mut()[entry].status = ProviderLoginStatus::Authenticating;
        }
    }

    pub(super) fn set_logged_in(&mut self, method_id: &str) {
        if let Some(entry) = self.entries.entries().iter().position(|entry| entry.method_id == method_id) {
            self.entries.entries_mut()[entry].status = ProviderLoginStatus::LoggedIn;
        }
    }

    pub(super) fn reset_to_needs_login(&mut self, method_id: &str) {
        if let Some(entry) = self.entries.entries().iter().position(|entry| entry.method_id == method_id) {
            self.entries.entries_mut()[entry].status = ProviderLoginStatus::NeedsLogin;
        }
    }

    pub(super) fn replace_entries(&mut self, entries: Vec<ProviderLoginEntry>) {
        let selected_method_id = self.selected_entry().map(|entry| entry.method_id.clone());
        self.entries = FilterableList::new(entries, |entry| entry.name.clone());
        if let Some(method_id) = selected_method_id
            && let Some(index) = self.entries.entries().iter().position(|entry| entry.method_id == method_id)
        {
            self.entries.select_index(index);
        }
    }

    pub(super) fn authentication_message(&self) -> Option<super::SettingsOverlayMessage> {
        self.selected_entry()
            .filter(|entry| entry.status != ProviderLoginStatus::Authenticating)
            .map(|entry| super::SettingsOverlayMessage::AuthenticateProvider(entry.method_id.clone()))
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.entries.render_items(
            area,
            buffer,
            " (no providers need login)",
            Style::new().fg(theme.text_secondary),
            Style::new().fg(theme.background).bg(theme.warning),
            |entry, _| {
                let (indicator, detail, style) = match entry.status {
                    ProviderLoginStatus::NeedsLogin => ("⚡", "needs login", Style::new().fg(theme.warning)),
                    ProviderLoginStatus::Authenticating => ("⏳", "authenticating...", Style::new().fg(theme.warning)),
                    ProviderLoginStatus::LoggedIn => ("✓", "logged in", Style::new().fg(theme.success)),
                };
                ListItem::new(truncate_to_width(
                    &format!(" {}  {} {detail}", entry.name, indicator),
                    usize::from(area.width),
                ))
                .style(style)
            },
        );
    }
}

pub(super) fn build_provider_login_entries(methods: &[AuthMethod]) -> Vec<ProviderLoginEntry> {
    methods
        .iter()
        .map(|m| {
            let status = if m.description() == Some("authenticated") {
                ProviderLoginStatus::LoggedIn
            } else {
                ProviderLoginStatus::NeedsLogin
            };
            ProviderLoginEntry { method_id: m.id().0.to_string(), name: m.name().to_string(), status }
        })
        .collect()
}

pub(super) fn provider_login_summary(entries: &[ProviderLoginEntry]) -> String {
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
