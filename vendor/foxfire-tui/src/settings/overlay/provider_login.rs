use super::summarize;
use crate::surfaces::input::{UiOutput, one};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::input::{Nav, UiEvent};
use agent_client_protocol::v2::schema::{AuthMethod, AuthState, AuthStatusResponse};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;

/// Provider authentication status, with a login action per provider.
pub(super) struct ProviderLoginPane {
    entries: FilterableList<ProviderLoginEntry>,
}

#[derive(Clone)]
pub(crate) struct ProviderLoginEntry {
    pub(crate) method_id: String,
    name: String,
    pub(crate) status: ProviderLoginStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderLoginStatus {
    Available,
    Authenticating,
    LoggedIn,
    Failed,
}

impl ProviderLoginPane {
    pub(super) fn new(entries: Vec<ProviderLoginEntry>) -> Self {
        Self { entries: list_of(entries) }
    }
}

impl ProviderLoginPane {
    pub(crate) fn on_ui_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
        match self.entries.on_nav_event(event) {
            Nav::Activate | Nav::Clicked => one(self
                .entries
                .selected_entry()
                .filter(|entry| entry.status != ProviderLoginStatus::Authenticating)
                .map(|entry| UiOutput::AuthenticateProvider(entry.method_id.clone()))),
            Nav::Close | Nav::Moved | Nav::Unhandled => Vec::new(),
        }
    }
}

impl ProviderLoginPane {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let (view, selection) = self.entries.view(theme, |entry| {
            let (indicator, detail, style) = match entry.status {
                ProviderLoginStatus::Available => ("→", "log in", theme.text_primary),
                ProviderLoginStatus::Authenticating => ("⏳", "authenticating...", theme.warning),
                ProviderLoginStatus::LoggedIn => ("✓", "logged in", theme.success),
                ProviderLoginStatus::Failed => ("✗", "login failed", theme.error),
            };
            Line::styled(format!(" {}  {indicator} {detail}", entry.name), style)
        });
        let view = view
            .empty_message(" (no login methods)")
            .highlight_style((theme.background, theme.warning))
            .highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING);
        StatefulWidget::render(view, area, buf, selection);
    }
}
impl ProviderLoginPane {
    pub(crate) fn refresh(&mut self, providers: &[ProviderLoginEntry]) {
        let keep = self.entries.selected_entry().map(|entry| entry.method_id.clone());
        self.entries = list_of(providers.to_vec());
        if let Some(method_id) = keep
            && let Some(index) = self.entries.entries().iter().position(|entry| entry.method_id == method_id)
        {
            self.entries.select_index(index);
        }
    }
}

fn list_of(entries: Vec<ProviderLoginEntry>) -> FilterableList<ProviderLoginEntry> {
    FilterableList::new(entries, |entry| entry.name.clone())
}

pub(crate) fn build_provider_login_entries(methods: &[AuthMethod]) -> Vec<ProviderLoginEntry> {
    methods
        .iter()
        .map(|method| ProviderLoginEntry {
            method_id: method.method_id().0.to_string(),
            name: method.name().to_string(),
            status: ProviderLoginStatus::Available,
        })
        .collect()
}

pub(super) fn summary(status: Option<&AuthStatusResponse>, entries: &[ProviderLoginEntry]) -> String {
    if entries.iter().any(|entry| entry.status == ProviderLoginStatus::Authenticating) {
        return "authenticating".to_string();
    }
    match status {
        Some(status) if !status.accounts.is_empty() => {
            let labels: Vec<&str> = status.accounts.iter().map(|account| account.label.as_str()).collect();
            format!("signed in: {}", labels.join(", "))
        }
        Some(status) => state_label(&status.state).to_string(),
        None => {
            let count = |status: ProviderLoginStatus| entries.iter().filter(|entry| entry.status == status).count();
            summarize(
                &[(count(ProviderLoginStatus::LoggedIn), "logged in"), (count(ProviderLoginStatus::Failed), "failed")],
                "log in",
            )
        }
    }
}

fn state_label(state: &AuthState) -> &'static str {
    match state {
        AuthState::Authenticated => "signed in",
        AuthState::Unauthenticated => "not signed in",
        AuthState::NotRequired => "no login needed",
        _ => "unknown",
    }
}
