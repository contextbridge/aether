use super::{LiveSettingsData, SettingsPane, summarize};
use crate::filterable_list::FilterableList;
use crate::render_context::RenderContext;
use crate::selection::Direction;
use crate::surface::{Surface, SurfaceMessage};
use crate::wrap::truncate_to_width;
use agent_client_protocol::schema::AuthMethod;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::{ListItem, Widget};

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
    NeedsLogin,
    Authenticating,
    LoggedIn,
}

impl ProviderLoginPane {
    pub(super) fn new(entries: Vec<ProviderLoginEntry>) -> Self {
        Self { entries: list_of(entries) }
    }

    /// Starts login for the focused provider, unless one is already running.
    fn authenticate(&self) -> Vec<SurfaceMessage> {
        self.entries
            .selected_entry()
            .filter(|entry| entry.status != ProviderLoginStatus::Authenticating)
            .map(|entry| SurfaceMessage::AuthenticateProvider(entry.method_id.clone()))
            .into_iter()
            .collect()
    }
}

impl Surface for ProviderLoginPane {
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<SurfaceMessage>> {
        (key.code == KeyCode::Enter).then(|| self.authenticate())
    }

    fn click(&mut self, row: u16, _column: u16) -> Vec<SurfaceMessage> {
        if self.entries.select_at(row) { self.authenticate() } else { Vec::new() }
    }

    fn scroll(&mut self, direction: Direction) -> Vec<SurfaceMessage> {
        self.entries.step(direction, |_| true);
        Vec::new()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        self.entries
            .view(theme, " (no providers need login)", |entry| {
                let (indicator, detail, style) = match entry.status {
                    ProviderLoginStatus::NeedsLogin => ("⚡", "needs login", Style::new().fg(theme.warning)),
                    ProviderLoginStatus::Authenticating => ("⏳", "authenticating...", Style::new().fg(theme.warning)),
                    ProviderLoginStatus::LoggedIn => ("✓", "logged in", Style::new().fg(theme.success)),
                };
                ListItem::new(truncate_to_width(
                    &format!(" {}  {indicator} {detail}", entry.name),
                    usize::from(area.width),
                ))
                .style(style)
            })
            .highlight_style(Style::new().fg(theme.background).bg(theme.warning))
            .render(area, buf);
        None
    }
}

impl SettingsPane for ProviderLoginPane {
    fn refresh(&mut self, live: &LiveSettingsData) {
        let keep = self.entries.selected_entry().map(|entry| entry.method_id.clone());
        self.entries = list_of(live.providers.clone());
        if let Some(method_id) = keep
            && let Some(index) = self.entries.entries().iter().position(|entry| entry.method_id == method_id)
        {
            self.entries.select_index(index);
        }
    }

    fn footer(&self) -> String {
        "[Enter] Authenticate  [Esc] Back".to_string()
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
