use super::{LiveSettingsData, summarize};
use crate::surfaces::input::{Nav, SettingsOutput, UiEvent, one};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use agent_client_protocol::schema::v1::AuthMethod;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;

/// Provider authentication status, with a login action per provider.
pub(super) struct ProviderLoginPane {
    entries: FilterableList<ProviderLoginEntry>,
}

pub(super) struct ProviderLoginView<'a> {
    theme: &'a Theme,
}

impl<'a> ProviderLoginView<'a> {
    pub(super) fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }
}

#[derive(Clone)]
pub(crate) struct ProviderLoginEntry {
    pub(crate) method_id: String,
    name: String,
    pub(crate) status: ProviderLoginStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderLoginStatus {
    NeedsLogin,
    Authenticating,
    LoggedIn,
}

impl ProviderLoginPane {
    pub(super) fn new(entries: Vec<ProviderLoginEntry>) -> Self {
        Self { entries: list_of(entries) }
    }
}

impl ProviderLoginPane {
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<SettingsOutput> {
        match self.entries.on_nav_event(&event) {
            Nav::Close => vec![SettingsOutput::Close],
            Nav::Activate | Nav::Clicked => one(self
                .entries
                .selected_entry()
                .filter(|entry| entry.status != ProviderLoginStatus::Authenticating)
                .map(|entry| SettingsOutput::AuthenticateProvider(entry.method_id.clone()))),
            Nav::Moved | Nav::Unhandled => Vec::new(),
        }
    }
}

impl ProviderLoginPane {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        StatefulWidget::render(ProviderLoginView::new(theme), area, buf, &mut self.entries);
        None
    }
}

impl StatefulWidget for ProviderLoginView<'_> {
    type State = FilterableList<ProviderLoginEntry>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let (view, selection) = state.view(self.theme, |entry| {
            let (indicator, detail, style) = match entry.status {
                ProviderLoginStatus::NeedsLogin => ("⚡", "needs login", Style::new().fg(self.theme.warning)),
                ProviderLoginStatus::Authenticating => ("⏳", "authenticating...", Style::new().fg(self.theme.warning)),
                ProviderLoginStatus::LoggedIn => ("✓", "logged in", Style::new().fg(self.theme.success)),
            };
            Line::styled(format!(" {}  {indicator} {detail}", entry.name), style)
        });
        let view = view
            .empty_message(" (no providers need login)")
            .highlight_style(Style::new().fg(self.theme.background).bg(self.theme.warning))
            .highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING);
        StatefulWidget::render(view, area, buf, selection);
    }
}
impl ProviderLoginPane {
    pub(crate) fn refresh(&mut self, live: &LiveSettingsData) {
        let keep = self.entries.selected_entry().map(|entry| entry.method_id.clone());
        self.entries = list_of(live.providers.clone());
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
            method_id: method.id().0.to_string(),
            name: method.name().to_string(),
            status: if method.description() == Some("authenticated") {
                ProviderLoginStatus::LoggedIn
            } else {
                ProviderLoginStatus::NeedsLogin
            },
        })
        .collect()
}

pub(super) fn summary(entries: &[ProviderLoginEntry]) -> String {
    let count = |status: ProviderLoginStatus| entries.iter().filter(|entry| entry.status == status).count();
    summarize(
        &[
            (count(ProviderLoginStatus::NeedsLogin), "needs login"),
            (count(ProviderLoginStatus::Authenticating), "authenticating"),
            (count(ProviderLoginStatus::LoggedIn), "logged in"),
        ],
        "all logged in",
    )
}
