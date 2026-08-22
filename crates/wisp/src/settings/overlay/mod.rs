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
use crate::renderer::DrawContext;
use crate::session::platform::{BrowserOpener, ClipboardWriter};
use crate::session::session_config_view::{LocalConfigOption, LocalConfigView};
use crate::surfaces::input::{ElicitationOutput, MouseAction, SettingsOutput, UiEvent, is_press};
use crate::surfaces::modal::{ElicitationModal, frame::ModalFrame};
use crate::theme::Theme;
use crate::view::selection::Direction;
use crate::view::widgets::key_hints;
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::notifications::{ElicitationParams, ElicitationResponse, McpServerStatusEntry};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::v1::AuthMethod;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Widget};

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
    pub group: Option<String>,
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
    pane: Option<SettingsPane>,
    current_reasoning_effort: Option<String>,
    live: LiveSettingsData,
    /// A request the overlay answers itself, drawn under the pane and owning
    /// input until it is answered. Authenticating an OAuth server asks for a
    /// browser this way, so the pane that started it has to survive the ask.
    pending_elicitation: Option<ElicitationModal>,
}

/// Agent-pushed state that panes display. The overlay owns it so a pane opened
/// later sees current data, and an open pane can be refreshed in place.
pub(crate) struct LiveSettingsData {
    pub(crate) servers: Vec<McpServerStatusEntry>,
    pub(crate) providers: Vec<ProviderLoginEntry>,
}

pub(crate) use crate::view::widgets::KeyHint;

enum SettingsPane {
    ServerStatus(ServerStatusPane),
    ProviderLogin(ProviderLoginPane),
    ModelSelector(ModelSelector),
    Picker(SettingsPicker),
}

impl SettingsPane {
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<SettingsOutput> {
        match self {
            Self::ServerStatus(pane) => pane.on_ui_event(event),
            Self::ProviderLogin(pane) => pane.on_ui_event(event),
            Self::ModelSelector(pane) => pane.on_ui_event(event),
            Self::Picker(pane) => pane.on_ui_event(event),
        }
    }

    pub(crate) fn footer(&self) -> Vec<KeyHint> {
        match self {
            Self::ServerStatus(_) => confirm_back_footer("authenticate OAuth servers"),
            Self::ProviderLogin(_) => confirm_back_footer("authenticate"),
            Self::ModelSelector(pane) => pane.footer(),
            Self::Picker(_) => confirm_back_footer("confirm"),
        }
    }

    fn refresh(&mut self, live: &LiveSettingsData) {
        match self {
            Self::ServerStatus(pane) => pane.refresh(live),
            Self::ProviderLogin(pane) => pane.refresh(live),
            Self::ModelSelector(_) | Self::Picker(_) => {}
        }
    }

    fn take_changes(&mut self) -> Vec<SettingsOutput> {
        match self {
            Self::ServerStatus(_) | Self::ProviderLogin(_) | Self::Picker(_) => Vec::new(),
            Self::ModelSelector(pane) => pane.take_changes(),
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        match self {
            Self::ServerStatus(pane) => pane.render(area, buf, cx.theme),
            Self::ProviderLogin(pane) => pane.render(area, buf, cx.theme),
            Self::ModelSelector(pane) => pane.render(area, buf, cx.theme),
            Self::Picker(pane) => pane.render(area, buf, cx.theme),
        }
    }
}
impl SettingsOverlay {
    pub fn new(
        config_options: &[LocalConfigOption],
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

    /// Adds the given rows, replacing any already shown for the same option.
    pub fn upsert_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        self.menu.upsert_local_entries(entries);
    }

    /// Adds the rows that open panes rather than pick a config value.
    pub fn add_status_entries(&mut self) {
        self.menu.upsert_pane_row(PaneKind::McpServers, &self.live.server_summary());
        if !self.live.providers.is_empty() {
            self.menu.upsert_pane_row(PaneKind::ProviderLogins, &self.live.provider_summary());
        }
    }

    pub fn update_config_options(&mut self, options: &[LocalConfigOption]) {
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

    /// Queues the request for the overlay to answer in place, against the given
    /// host handlers so tests observe URL opens without spawning a browser.
    pub fn on_elicitation_request(
        &mut self,
        params: ElicitationParams,
        responder: Responder<ElicitationResponse>,
        browser_opener: BrowserOpener,
        clipboard_writer: ClipboardWriter,
    ) {
        self.pending_elicitation =
            Some(ElicitationModal::with_url_handlers(params, responder, browser_opener, clipboard_writer));
    }

    /// Drops an unanswered request, which cancels it.
    pub fn cancel_pending_elicitation(&mut self) {
        self.pending_elicitation = None;
    }

    /// Key hints for the current focus, shown in the overlay footer.
    fn footer_hints(&self) -> Vec<KeyHint> {
        if let Some(pending) = self.pending_elicitation.as_ref() {
            return pending.key_hints();
        }
        self.pane
            .as_ref()
            .map_or_else(|| vec![("Enter", "select".into()), ("Esc", "close".into())], SettingsPane::footer)
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

    /// Draws whichever of the menu or the open pane has focus.
    fn render_content(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        let Some(pane) = self.pane.as_mut() else {
            self.menu.render(area, buf, cx.theme);
            return None;
        };
        pane.render(area, buf, cx)
    }

    /// Splits `inner` into the rows the menu or pane keeps and the rows a
    /// pending request takes at the bottom, separated by a blank line. The pane
    /// keeps at least one row so the request never draws over it entirely.
    fn split_for_prompt(&self, inner: Rect, theme: &Theme) -> (Rect, Option<Rect>) {
        let Some(pending) = self.pending_elicitation.as_ref() else {
            return (inner, None);
        };
        // The prompt lines up with the menu and pane rows, which carry their own
        // leading space.
        let width = inner.width.saturating_sub(1);
        let height = pending.inline_height(theme, width).min(inner.height.saturating_sub(2));
        if height == 0 {
            return (inner, None);
        }
        let [content, _gap, prompt] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(height)]).areas(inner);
        let [_indent, prompt] = Layout::horizontal([Constraint::Length(1), Constraint::Min(0)]).areas(prompt);
        (content, Some(prompt))
    }

    /// Handles an event the menu itself owns, because no pane or request is open
    /// above it.
    fn on_menu_event(&mut self, event: &UiEvent) -> Vec<SettingsOutput> {
        match event {
            UiEvent::Key(key) if is_press(*key) => return self.on_menu_key(*key),
            // The menu has no editor, so a paste has nowhere to land.
            UiEvent::Key(_) | UiEvent::Paste(_) => {}
            UiEvent::Mouse(MouseAction::ScrollUp, _) => self.menu.step(Direction::Backward),
            UiEvent::Mouse(MouseAction::ScrollDown, _) => self.menu.step(Direction::Forward),
            UiEvent::Mouse(MouseAction::Click, position) => {
                if self.menu.click_at(position.1) {
                    self.pane = self.open_selected_pane();
                }
            }
        }
        Vec::new()
    }

    fn on_menu_key(&mut self, key: KeyEvent) -> Vec<SettingsOutput> {
        match key.code {
            KeyCode::Esc => return vec![SettingsOutput::Close],
            KeyCode::Up => self.menu.step(Direction::Backward),
            KeyCode::Down => self.menu.step(Direction::Forward),
            KeyCode::Enter => self.pane = self.open_selected_pane(),
            _ => {}
        }
        Vec::new()
    }

    fn open_selected_pane(&self) -> Option<SettingsPane> {
        let pane = match self.menu.selected_row()? {
            MenuRow::Pane { kind: PaneKind::McpServers, .. } => {
                SettingsPane::ServerStatus(ServerStatusPane::new(self.live.servers.clone()))
            }
            MenuRow::Pane { kind: PaneKind::ProviderLogins, .. } => {
                SettingsPane::ProviderLogin(ProviderLoginPane::new(self.live.providers.clone()))
            }
            MenuRow::Select(entry) if entry.multi_select => SettingsPane::ModelSelector(ModelSelector::new(
                entry.config_id.clone(),
                entry.values.clone(),
                &entry.current_raw_value,
                self.current_reasoning_effort.as_deref(),
            )),
            MenuRow::Select(entry) => SettingsPane::Picker(SettingsPicker::from_entry(entry)?),
        };
        Some(pane)
    }

    /// Handles pane outputs locally: `Close` returns focus to the
    /// menu, and config edits are mirrored into the menu on the way past so the
    /// UI never lags the agent round-trip.
    fn apply(&mut self, messages: Vec<SettingsOutput>) -> Vec<SettingsOutput> {
        messages
            .into_iter()
            .filter(|message| {
                if matches!(message, SettingsOutput::Close) && self.pane.is_some() {
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

impl SettingsOverlay {
    /// Routes every event to whichever of the three things inside the overlay
    /// owns input, innermost first.
    ///
    /// One entry point rather than one override per event kind, because the
    /// triage is the same for all of them — and because only routing them
    /// together makes "a request closing returns focus to the pane behind it"
    /// hold for a click as reliably as it does for a keystroke.
    pub fn on_ui_event(&mut self, event: UiEvent) -> Vec<SettingsOutput> {
        if let Some(pending) = self.pending_elicitation.as_mut() {
            // Answering a request returns focus to the pane behind it rather
            // than closing the overlay the request arrived in.
            if pending.on_ui_event(event).iter().any(|action| matches!(action, ElicitationOutput::Close)) {
                self.pending_elicitation = None;
            }
            return Vec::new();
        }
        let Some(pane) = self.pane.as_mut() else {
            return self.on_menu_event(&event);
        };
        // Esc leaves the pane rather than the overlay, committing whatever the
        // pane batched up while it was open.
        let messages = match event {
            UiEvent::Key(key) if is_press(key) && key.code == KeyCode::Esc => {
                let mut messages = pane.take_changes();
                messages.push(SettingsOutput::Close);
                messages
            }
            event => pane.on_ui_event(event),
        };
        self.apply(messages)
    }

    /// A URL request wants the terminal's own text selection back, so the URL
    /// can be copied by hand when opening a browser is not an option.
    pub(crate) fn needs_mouse_capture(&self) -> bool {
        self.pending_elicitation.as_ref().is_none_or(ElicitationModal::needs_mouse_capture)
    }
}

impl SettingsOverlay {
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        let theme = cx.theme;
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            Clear.render(area, buf);
            Paragraph::new(Line::styled("(terminal too small)", Style::new().fg(theme.text_secondary)))
                .render(area, buf);
            return None;
        }
        let footer = key_hints(&self.footer_hints(), theme);
        let frame = ModalFrame::new(
            "Configuration",
            Some(footer),
            Constraint::Percentage(80),
            Constraint::Percentage(80),
            theme,
        );
        let inner = frame.inner(area);
        (&frame).render(area, buf);

        let (content, prompt) = self.split_for_prompt(inner, theme);
        let cursor = self.render_content(content, buf, cx);

        let (Some(prompt), Some(pending)) = (prompt, self.pending_elicitation.as_mut()) else {
            return cursor;
        };
        pending.render_inline(prompt, buf, cx.theme);
        None
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

fn reasoning_effort_of(options: &[LocalConfigOption]) -> Option<String> {
    LocalConfigView::new(options).reasoning_effort().map(|effort| effort.as_str().to_string())
}

/// The outbound message for a config edit. Theme is a client-side setting, so it
/// takes a different route than an agent config option.
pub(crate) fn message_for_change(change: &SettingsChange) -> SettingsOutput {
    if change.config_id == acp_utils::config_option_id::THEME_CONFIG_ID {
        SettingsOutput::SetTheme(change.new_value.clone())
    } else {
        SettingsOutput::SetConfigOption { config_id: change.config_id.clone(), value: change.new_value.clone() }
    }
}

/// The config edit `message` carries, when it is one — the inverse of
/// [`message_for_change`], so the overlay can mirror edits into its own menu.
fn config_edit(message: &SettingsOutput) -> Option<SettingsChange> {
    match message {
        SettingsOutput::SetTheme(value) => Some(SettingsChange {
            config_id: acp_utils::config_option_id::THEME_CONFIG_ID.to_string(),
            new_value: value.clone(),
        }),
        SettingsOutput::SetConfigOption { config_id, value } => {
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

fn confirm_back_footer(enter_label: &'static str) -> Vec<KeyHint> {
    vec![("Enter", enter_label.into()), ("Esc", "back".into())]
}

pub(crate) fn value_match_key(value: &SettingsMenuValue) -> String {
    format!("{} {}", value.name, value.value)
}
