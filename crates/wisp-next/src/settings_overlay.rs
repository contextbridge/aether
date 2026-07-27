mod menu;
mod model_selector;
mod picker;
mod provider_login;
mod server_status;

use self::menu::{MenuRow, SettingsMenu};
use self::model_selector::ModelSelector;
use self::picker::SettingsPicker;
use self::provider_login::{ProviderLoginEntry, ProviderLoginPane, ProviderLoginStatus, build_provider_login_entries};
use self::server_status::ServerStatusPane;
use crate::elicitation::ElicitationResponder;
use crate::render_context::RenderContext;
use crate::selection::Direction;
use crate::session_config_view::SessionConfigView;
use crate::surface::{Action, Surface};
use crate::widgets::key_hints;
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::notifications::{
    ElicitationParams, ElicitationResponse, McpServerStatusEntry, UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{AuthMethod, SessionConfigOption};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget};

const MIN_WIDTH: u16 = 6;
const MIN_HEIGHT: u16 = 3;

#[derive(Debug)]
pub struct SettingsChange {
    pub config_id: String,
    pub new_value: String,
}

#[derive(Debug, Clone)]
pub struct SettingsMenuValue {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    pub is_disabled: bool,
    pub meta: SelectOptionMeta,
}

#[derive(Debug, Clone)]
pub struct SettingsMenuEntry {
    pub config_id: String,
    pub title: String,
    pub values: Vec<SettingsMenuValue>,
    pub current_value_index: usize,
    pub current_raw_value: String,
    pub multi_select: bool,
    pub display_name: Option<String>,
    /// Client-side rather than part of the agent's schema, so it survives a
    /// config-options push that replaces every agent-provided row.
    pub local: bool,
}

/// A settings screen that is not a config option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    McpServers,
    ProviderLogins,
}

impl PaneKind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::McpServers => "MCP Servers",
            Self::ProviderLogins => "Provider Logins",
        }
    }
}

/// The settings overlay: a menu of options, each of which opens a pane.
pub struct SettingsOverlay {
    menu: SettingsMenu,
    /// The pane drawn over the menu. `None` means the menu itself has focus.
    pane: Option<Box<dyn SettingsPane>>,
    current_reasoning_effort: Option<String>,
    live: LiveSettingsData,
    pending_elicitation: Option<PendingElicitation>,
}

/// Agent-pushed state that panes display. The overlay owns it so a pane opened
/// later sees current data, and an open pane can be refreshed in place.
pub(crate) struct LiveSettingsData {
    pub(crate) servers: Vec<McpServerStatusEntry>,
    pub(crate) providers: Vec<ProviderLoginEntry>,
}

/// One screen inside the settings overlay.
///
/// Panes are ordinary [`Surface`]s — they get the same key routing, filtering,
/// and mouse handling as every other layer — plus the two things only a settings
/// pane has: footer hints, and agent-pushed data that can change underneath it.
///
/// The overlay never asks which pane it is holding, so this is a trait object
/// rather than an enum: adding a pane is a new type and a new arm of
/// [`SettingsOverlay::open_selected_pane`], not an edit to four dispatch
/// methods.
trait SettingsPane: Surface {
    /// Key hints for the overlay footer while this pane has focus.
    fn footer(&self) -> Vec<KeyHint>;

    /// Re-reads agent-pushed data after it changes underneath the pane.
    fn refresh(&mut self, live: &LiveSettingsData) {
        let _ = live;
    }

    /// Edits to commit when the pane closes; panes that apply edits immediately
    /// return nothing.
    fn take_changes(&mut self) -> Vec<Action> {
        Vec::new()
    }
}

/// A key and what it does, for [`key_hints`].
pub(crate) type KeyHint = (&'static str, String);

impl SettingsOverlay {
    pub fn new(
        config_options: &[SessionConfigOption],
        server_statuses: Vec<McpServerStatusEntry>,
        auth_methods: &[AuthMethod],
    ) -> Self {
        Self {
            menu: SettingsMenu::from_config_options(config_options),
            pane: None,
            current_reasoning_effort: reasoning_effort_of(config_options),
            live: LiveSettingsData { servers: server_statuses, providers: build_provider_login_entries(auth_methods) },
            pending_elicitation: None,
        }
    }

    pub fn add_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        self.menu.add_local_entries(entries);
    }

    /// Adds the rows that open panes rather than pick a config value.
    pub fn add_status_entries(&mut self) {
        self.menu.upsert_pane_row(PaneKind::McpServers, &self.live.server_summary());
        if !self.live.providers.is_empty() {
            self.menu.upsert_pane_row(PaneKind::ProviderLogins, &self.live.provider_summary());
        }
    }

    pub fn update_config_options(&mut self, options: &[SessionConfigOption]) {
        self.current_reasoning_effort = reasoning_effort_of(options);
        self.menu.update_options(options);
    }

    pub fn apply_change(&mut self, change: &SettingsChange) {
        self.menu.apply_change(change);
    }

    pub fn update_server_statuses(&mut self, statuses: Vec<McpServerStatusEntry>) {
        self.live.servers = statuses;
        self.menu.upsert_pane_row(PaneKind::McpServers, &self.live.server_summary());
        self.refresh_pane();
    }

    pub fn update_auth_methods(&mut self, methods: &[AuthMethod]) {
        self.live.providers = build_provider_login_entries(methods);
        self.menu.upsert_pane_row(PaneKind::ProviderLogins, &self.live.provider_summary());
        self.refresh_pane();
    }

    pub fn on_authenticate_started(&mut self, method_id: &str) {
        self.set_provider_status(method_id, ProviderLoginStatus::Authenticating);
    }

    pub fn on_authenticate_complete(&mut self, method_id: &str) {
        self.set_provider_status(method_id, ProviderLoginStatus::LoggedIn);
    }

    pub fn on_authenticate_failed(&mut self, method_id: &str) {
        self.set_provider_status(method_id, ProviderLoginStatus::NeedsLogin);
    }

    pub fn on_elicitation_request(&mut self, params: ElicitationParams, responder: Responder<ElicitationResponse>) {
        self.cancel_pending_elicitation();
        let elicitation_id = match &params.request {
            acp_utils::notifications::CreateElicitationRequestParams::UrlElicitationParams {
                elicitation_id, ..
            } => elicitation_id.clone(),
            acp_utils::notifications::CreateElicitationRequestParams::FormElicitationParams { .. } => String::new(),
        };
        self.pending_elicitation = Some(PendingElicitation {
            server_name: params.server_name,
            elicitation_id,
            _responder: ElicitationResponder::new(responder),
        });
    }

    pub fn on_url_elicitation_complete(&mut self, params: &UrlElicitationCompleteParams) {
        if self.pending_elicitation.as_ref().is_some_and(|pending| pending.matches(params)) {
            self.cancel_pending_elicitation();
        }
    }

    pub fn cancel_pending_elicitation(&mut self) {
        self.pending_elicitation = None;
    }

    /// Key hints for the current focus, shown in the overlay footer.
    fn footer_hints(&self) -> Vec<KeyHint> {
        self.pane
            .as_ref()
            .map_or_else(|| vec![("Enter", "select".to_string()), ("Esc", "close".to_string())], |pane| pane.footer())
    }

    fn set_provider_status(&mut self, method_id: &str, status: ProviderLoginStatus) {
        if let Some(entry) = self.live.providers.iter_mut().find(|entry| entry.method_id == method_id) {
            entry.status = status;
        }
        self.refresh_pane();
    }

    fn refresh_pane(&mut self) {
        if let Some(pane) = self.pane.as_mut() {
            pane.refresh(&self.live);
        }
    }

    fn on_menu_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Esc => return vec![Action::Close],
            KeyCode::Up => self.menu.step(Direction::Backward),
            KeyCode::Down => self.menu.step(Direction::Forward),
            KeyCode::Enter => self.pane = self.open_selected_pane(),
            _ => {}
        }
        Vec::new()
    }

    fn open_selected_pane(&self) -> Option<Box<dyn SettingsPane>> {
        let pane: Box<dyn SettingsPane> = match self.menu.selected_row()? {
            MenuRow::Pane { kind: PaneKind::McpServers, .. } => {
                Box::new(ServerStatusPane::new(self.live.servers.clone()))
            }
            MenuRow::Pane { kind: PaneKind::ProviderLogins, .. } => {
                Box::new(ProviderLoginPane::new(self.live.providers.clone()))
            }
            MenuRow::Select(entry) if entry.multi_select => Box::new(ModelSelector::new(
                entry.config_id.clone(),
                entry.values.clone(),
                &entry.current_raw_value,
                self.current_reasoning_effort.as_deref(),
            )),
            MenuRow::Select(entry) => Box::new(SettingsPicker::from_entry(entry)?),
        };
        Some(pane)
    }

    /// Handles pane actions locally: [`Action::Close`] returns focus to the
    /// menu, and config edits are mirrored into the menu on the way past so the
    /// UI never lags the agent round-trip.
    fn apply(&mut self, messages: Vec<Action>) -> Vec<Action> {
        messages
            .into_iter()
            .filter(|message| {
                if matches!(message, Action::Close) && self.pane.is_some() {
                    self.pane = None;
                    return false;
                }
                if let Some(change) = config_edit(message) {
                    self.menu.apply_change(&change);
                }
                true
            })
            .collect()
    }
}

impl Surface for SettingsOverlay {
    /// The overlay owns every key: the menu and its panes have their own
    /// keymaps, so nothing falls through to the shared list navigation.
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let Some(pane) = self.pane.as_mut() else {
            return Some(self.on_menu_key(key));
        };
        // Esc leaves the pane rather than the overlay, committing whatever the
        // pane batched up while it was open.
        let messages = if key.code == KeyCode::Esc {
            let mut messages = pane.take_changes();
            messages.push(Action::Close);
            messages
        } else {
            pane.on_key(key)
        };
        Some(self.apply(messages))
    }

    fn scroll(&mut self, direction: Direction) -> Vec<Action> {
        let Some(pane) = self.pane.as_mut() else {
            self.menu.step(direction);
            return Vec::new();
        };
        let messages = pane.scroll(direction);
        self.apply(messages)
    }

    fn click(&mut self, row: u16, column: u16) -> Vec<Action> {
        let Some(pane) = self.pane.as_mut() else {
            if self.menu.click_at(row) {
                self.pane = self.open_selected_pane();
            }
            return Vec::new();
        };
        let messages = pane.click(row, column);
        self.apply(messages)
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        Clear.render(area, buf);
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            Paragraph::new(Line::styled("(terminal too small)", Style::new().fg(theme.text_secondary)))
                .render(area, buf);
            return None;
        }
        // The key hints ride on the bottom border rather than claiming a row of
        // their own, so a short terminal spends every inner row on content.
        let mut footer = key_hints(&self.footer_hints(), theme);
        footer.spans.insert(0, Span::raw(" "));
        footer.push_span(Span::raw(" "));
        let block = Block::bordered()
            .title(" Configuration ")
            .title_bottom(footer)
            .style(Style::new().bg(theme.background))
            .border_style(Style::new().fg(theme.text_secondary));
        let inner = block.inner(area);
        block.render(area, buf);

        let Some(pane) = self.pane.as_mut() else {
            self.menu.render(inner, buf, theme);
            return None;
        };
        pane.render(inner, buf, cx)
    }
}

impl LiveSettingsData {
    fn server_summary(&self) -> String {
        server_status::summary(&self.servers)
    }

    fn provider_summary(&self) -> String {
        provider_login::summary(&self.providers)
    }
}

/// An elicitation the overlay answers in place. Holding the responder is what
/// keeps it outstanding; dropping this cancels it.
struct PendingElicitation {
    server_name: String,
    elicitation_id: String,
    _responder: ElicitationResponder,
}

impl PendingElicitation {
    fn matches(&self, params: &UrlElicitationCompleteParams) -> bool {
        self.server_name == params.server_name && self.elicitation_id == params.elicitation_id
    }
}

fn reasoning_effort_of(options: &[SessionConfigOption]) -> Option<String> {
    SessionConfigView::new(options).reasoning_effort().map(|effort| effort.as_str().to_string())
}

/// The outbound message for a config edit. Theme is a client-side setting, so it
/// takes a different route than an agent config option.
pub(crate) fn message_for_change(change: &SettingsChange) -> Action {
    if change.config_id == acp_utils::config_option_id::THEME_CONFIG_ID {
        Action::SetTheme(change.new_value.clone())
    } else {
        Action::SetConfigOption { config_id: change.config_id.clone(), value: change.new_value.clone() }
    }
}

/// The config edit `message` carries, when it is one — the inverse of
/// [`message_for_change`], so the overlay can mirror edits into its own menu.
fn config_edit(message: &Action) -> Option<SettingsChange> {
    match message {
        Action::SetTheme(value) => Some(SettingsChange {
            config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
            new_value: value.clone(),
        }),
        Action::SetConfigOption { config_id, value } => {
            Some(SettingsChange { config_id: config_id.clone(), new_value: value.clone() })
        }
        _ => None,
    }
}

/// Joins the non-zero `(count, label)` buckets into "2 connected, 1 failed",
/// falling back to `empty` when every bucket is zero.
pub(crate) fn summarize(buckets: &[(usize, &str)], empty: &str) -> String {
    let parts: Vec<String> =
        buckets.iter().filter(|(count, _)| *count > 0).map(|(count, label)| format!("{count} {label}")).collect();
    if parts.is_empty() { empty.to_string() } else { parts.join(", ") }
}
