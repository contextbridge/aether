mod menu;
mod model_selector;
mod picker;
mod provider_login;
mod server_status;

use self::menu::SettingsMenu;
use self::model_selector::ModelSelector;
use self::picker::SettingsPicker;
use self::provider_login::{ProviderLoginEntry, ProviderLoginPane, ProviderLoginStatus, build_provider_login_entries};
use self::server_status::ServerStatusPane;
use crate::render_context::RenderContext;
use crate::selection::Direction;
use crate::session_config_view::SessionConfigView;
use crate::surface::{ListFilter, Surface, SurfaceMessage};
use crate::theme::Theme;
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::notifications::{
    ElicitationParams, ElicitationResponse, McpServerStatusEntry, UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{AuthMethod, SessionConfigOption};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
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
    pub entry_kind: SettingsMenuEntryKind,
    pub multi_select: bool,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsMenuEntryKind {
    Select,
    Theme,
    McpServers,
    ProviderLogins,
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
/// Every pane navigates, renders, and reports the same three things, so the
/// overlay drives them all through this trait instead of matching on which is
/// open.
pub(crate) trait SettingsPane {
    /// Handles the keys unique to this pane, returning `None` to fall back to
    /// the shared navigation and filter keys in [`SettingsPane::on_key`].
    fn on_pane_key(&mut self, key: KeyEvent) -> Option<PaneOutcome>;

    /// Selects whatever is drawn at terminal row `row`, if anything is.
    fn click(&mut self, row: u16) -> PaneOutcome;

    fn scroll(&mut self, direction: Direction);

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Key hints for the overlay footer while this pane has focus.
    fn footer(&self) -> String;

    /// The query this pane filters its list by, when it has one. Supplying it
    /// is what makes typing and backspace filter.
    fn filter(&mut self) -> Option<&mut dyn ListFilter> {
        None
    }

    /// Changes to commit when the pane closes; panes that apply edits
    /// immediately return nothing.
    fn take_changes(&mut self) -> Vec<SettingsChange> {
        Vec::new()
    }

    /// Re-reads agent-pushed data after it changes underneath the pane.
    fn refresh(&mut self, live: &LiveSettingsData) {
        let _ = live;
    }

    /// Routes a keystroke: pane-specific keys first, then the navigation and
    /// filter keys every pane shares.
    fn on_key(&mut self, key: KeyEvent) -> PaneOutcome {
        if let Some(outcome) = self.on_pane_key(key) {
            return outcome;
        }
        match key.code {
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Backspace => {
                if let Some(filter) = self.filter() {
                    filter.pop_query_char();
                }
            }
            KeyCode::Char(character) if !character.is_control() => {
                if let Some(filter) = self.filter() {
                    filter.push_query_char(character);
                }
            }
            _ => {}
        }
        PaneOutcome::default()
    }
}

/// What a pane produced from one interaction.
#[derive(Default)]
pub(crate) struct PaneOutcome {
    /// Config edits to apply to the menu and forward to the agent.
    pub(crate) changes: Vec<SettingsChange>,
    /// Messages that are not config edits, such as authentication requests.
    pub(crate) messages: Vec<SurfaceMessage>,
    /// Return focus to the menu.
    pub(crate) back: bool,
}

impl PaneOutcome {
    pub(crate) fn message(message: Option<SurfaceMessage>) -> Self {
        Self { messages: message.into_iter().collect(), ..Self::default() }
    }
}

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
        self.menu.entries.splice(0..0, entries);
    }

    /// Adds the rows that open panes rather than pick a config value.
    pub fn add_status_entries(&mut self) {
        self.menu.upsert_pane_entry(SettingsMenuEntryKind::McpServers, &self.live.server_summary());
        if !self.live.providers.is_empty() {
            self.menu.upsert_pane_entry(SettingsMenuEntryKind::ProviderLogins, &self.live.provider_summary());
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
        self.menu.upsert_pane_entry(SettingsMenuEntryKind::McpServers, &self.live.server_summary());
        self.refresh_pane();
    }

    pub fn update_auth_methods(&mut self, methods: &[AuthMethod]) {
        self.live.providers = build_provider_login_entries(methods);
        self.menu.upsert_pane_entry(SettingsMenuEntryKind::ProviderLogins, &self.live.provider_summary());
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
        self.pending_elicitation =
            Some(PendingElicitation { server_name: params.server_name, elicitation_id, responder });
    }

    pub fn on_url_elicitation_complete(&mut self, params: &UrlElicitationCompleteParams) {
        if self.pending_elicitation.as_ref().is_some_and(|pending| pending.matches(params)) {
            self.cancel_pending_elicitation();
        }
    }

    pub fn cancel_pending_elicitation(&mut self) {
        if let Some(pending) = self.pending_elicitation.take() {
            let _ = pending.responder.respond(ElicitationResponse {
                action: acp_utils::notifications::ElicitationAction::Cancel,
                content: None,
            });
        }
    }

    /// Key hints for the current focus, shown in the overlay footer.
    pub fn footer_text(&self) -> String {
        self.pane.as_ref().map_or_else(|| "[Enter] Select  [Esc] Close".to_string(), |pane| pane.footer())
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

    fn on_menu_key(&mut self, key: KeyEvent) -> Vec<SurfaceMessage> {
        match key.code {
            KeyCode::Esc => return vec![SurfaceMessage::Close],
            KeyCode::Up => self.menu.move_up(),
            KeyCode::Down => self.menu.move_down(),
            KeyCode::Enter => self.pane = self.open_selected_pane(),
            _ => {}
        }
        Vec::new()
    }

    fn open_selected_pane(&self) -> Option<Box<dyn SettingsPane>> {
        let entry = self.menu.selected_entry()?;
        match entry.entry_kind {
            SettingsMenuEntryKind::McpServers => Some(Box::new(ServerStatusPane::new(self.live.servers.clone()))),
            SettingsMenuEntryKind::ProviderLogins => {
                Some(Box::new(ProviderLoginPane::new(self.live.providers.clone())))
            }
            _ if entry.multi_select => Some(Box::new(ModelSelector::new(
                entry.config_id.clone(),
                entry.values.clone(),
                &entry.current_raw_value,
                self.current_reasoning_effort.as_deref(),
            ))),
            _ => SettingsPicker::from_entry(entry).map(|picker| Box::new(picker) as Box<dyn SettingsPane>),
        }
    }

    /// Applies a pane's result: config edits update the menu immediately so the
    /// UI never lags the agent round-trip, and become outbound messages.
    fn apply(&mut self, outcome: PaneOutcome) -> Vec<SurfaceMessage> {
        if outcome.back {
            self.pane = None;
        }
        let mut messages = outcome.messages;
        for change in &outcome.changes {
            self.menu.apply_change(change);
            messages.push(message_for_change(change));
        }
        messages
    }
}

impl Surface for SettingsOverlay {
    /// The overlay owns every key: the menu and its panes have their own
    /// keymaps, so nothing falls through to the shared list navigation.
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<SurfaceMessage>> {
        let Some(pane) = self.pane.as_mut() else {
            return Some(self.on_menu_key(key));
        };
        let outcome = if key.code == KeyCode::Esc {
            PaneOutcome { changes: pane.take_changes(), back: true, ..PaneOutcome::default() }
        } else {
            pane.on_key(key)
        };
        Some(self.apply(outcome))
    }

    fn scroll(&mut self, direction: Direction) -> Vec<SurfaceMessage> {
        match self.pane.as_mut() {
            Some(pane) => pane.scroll(direction),
            None => self.menu.step(direction),
        }
        Vec::new()
    }

    fn click(&mut self, row: u16, _column: u16) -> Vec<SurfaceMessage> {
        let Some(pane) = self.pane.as_mut() else {
            if self.menu.click_at(row) {
                self.pane = self.open_selected_pane();
            }
            return Vec::new();
        };
        let outcome = pane.click(row);
        self.apply(outcome)
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        Clear.render(area, buf);
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            Paragraph::new(Line::styled("(terminal too small)", Style::new().fg(theme.text_secondary)))
                .render(area, buf);
            return None;
        }
        let block = Block::bordered()
            .title(" Configuration ")
            .style(Style::new().bg(theme.background))
            .border_style(Style::new().fg(theme.text_secondary));
        let inner = block.inner(area);
        block.render(area, buf);

        let [content_area, footer_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        Paragraph::new(Line::from(self.footer_text())).render(footer_area, buf);
        match self.pane.as_mut() {
            Some(pane) => pane.render(content_area, buf, theme),
            None => self.menu.render(content_area, buf, theme),
        }
        None
    }
}

impl Drop for SettingsOverlay {
    fn drop(&mut self) {
        self.cancel_pending_elicitation();
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

struct PendingElicitation {
    server_name: String,
    elicitation_id: String,
    responder: Responder<ElicitationResponse>,
}

impl PendingElicitation {
    fn matches(&self, params: &UrlElicitationCompleteParams) -> bool {
        self.server_name == params.server_name && self.elicitation_id == params.elicitation_id
    }
}

fn reasoning_effort_of(options: &[SessionConfigOption]) -> Option<String> {
    SessionConfigView::new(options).reasoning_effort().map(|effort| effort.as_str().to_string())
}

fn message_for_change(change: &SettingsChange) -> SurfaceMessage {
    if change.config_id == acp_utils::config_option_id::THEME_CONFIG_ID {
        SurfaceMessage::SetTheme(change.new_value.clone())
    } else {
        SurfaceMessage::SetConfigOption { config_id: change.config_id.clone(), value: change.new_value.clone() }
    }
}

/// Joins the non-zero `(count, label)` buckets into "2 connected, 1 failed",
/// falling back to `empty` when every bucket is zero.
pub(crate) fn summarize(buckets: &[(usize, &str)], empty: &str) -> String {
    let parts: Vec<String> =
        buckets.iter().filter(|(count, _)| *count > 0).map(|(count, label)| format!("{count} {label}")).collect();
    if parts.is_empty() { empty.to_string() } else { parts.join(", ") }
}

#[cfg(test)]
mod tests;
