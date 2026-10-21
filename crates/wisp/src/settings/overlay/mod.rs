mod menu;
mod model_selector;
mod picker;
mod provider_login;
mod server_status;

use self::menu::{MenuRow, SettingsMenu};
use self::model_selector::ModelSelector;
use self::picker::SettingsPicker;
pub(crate) use self::provider_login::ProviderLoginStatus;
use self::provider_login::{ProviderLoginEntry, ProviderLoginPane, build_provider_login_entries};
use self::server_status::ServerStatusPane;
use crate::renderer::DrawContext;
use crate::session::platform::{BrowserOpener, ClipboardWriter};
use crate::surfaces::input::UiOutput;
use crate::surfaces::modal::{ElicitationModal, frame::ModalFrame};
use crate::theme::Theme;
use crate::view::input::{Nav, UiEvent};
use crate::view::widgets::key_hints;
use acp_utils::notifications::McpServerStatusEntry;
use agent_client_protocol::schema::v2::{AuthMethod, CreateElicitationRequest};
use client_core::elicitation::ElicitationResponder;
use client_core::session_config_view::{LocalConfigOption, LocalConfigValue, LocalConfigView};
use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
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
pub struct SettingsMenuEntry {
    pub config_id: String,
    pub title: String,
    pub values: Vec<LocalConfigValue>,
    pub current_raw_value: String,
    pub multi_select: bool,
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

    fn summary(self, live: &LiveSettingsData) -> String {
        match self {
            Self::McpServers => server_status::summary(&live.servers),
            Self::ProviderLogins => provider_login::summary(&live.providers),
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
    pub(crate) fn on_ui_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
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

    fn take_changes(&mut self) -> Option<Vec<UiOutput>> {
        match self {
            Self::ServerStatus(_) | Self::ProviderLogin(_) | Self::Picker(_) => Some(Vec::new()),
            Self::ModelSelector(pane) => pane.take_changes(),
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        match self {
            Self::ServerStatus(pane) => pane.render(area, buf, theme),
            Self::ProviderLogin(pane) => pane.render(area, buf, theme),
            Self::ModelSelector(pane) => pane.render(area, buf, theme),
            Self::Picker(pane) => pane.render(area, buf, theme),
        }
    }
}
impl SettingsOverlay {
    pub fn new(
        config_options: &[LocalConfigOption],
        server_statuses: Vec<McpServerStatusEntry>,
        auth_methods: &[AuthMethod],
    ) -> Self {
        let mut overlay = Self {
            menu: SettingsMenu::from_config_options(config_options),
            pane: None,
            current_reasoning_effort: reasoning_effort_of(config_options),
            live: LiveSettingsData { servers: server_statuses, providers: build_provider_login_entries(auth_methods) },
            pending_elicitation: None,
        };
        overlay.sync_pane_rows();
        overlay
    }

    /// Adds the given rows, replacing any already shown for the same option.
    pub fn upsert_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        self.menu.upsert_local_entries(entries);
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
        self.refresh_pane();
    }

    pub fn update_auth_methods(&mut self, methods: &[AuthMethod]) {
        self.live.providers = build_provider_login_entries(methods);
        self.sync_pane_rows();
        self.refresh_pane();
    }

    pub(crate) fn set_provider_status(&mut self, method_id: &str, status: ProviderLoginStatus) {
        if let Some(entry) = self.live.providers.iter_mut().find(|entry| entry.method_id == method_id) {
            entry.status = status;
        }
        self.refresh_pane();
    }

    /// Queues the request for the overlay to answer in place, against the given
    /// host handlers so tests observe URL opens without spawning a browser.
    pub fn on_elicitation_request(
        &mut self,
        params: CreateElicitationRequest,
        responder: ElicitationResponder,
        browser_opener: BrowserOpener,
        clipboard_writer: ClipboardWriter,
    ) {
        self.pending_elicitation =
            ElicitationModal::with_url_handlers(params, responder, browser_opener, clipboard_writer);
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

    fn sync_pane_rows(&mut self) {
        self.menu.show_pane_row(PaneKind::McpServers, true);
        self.menu.show_pane_row(PaneKind::ProviderLogins, !self.live.providers.is_empty());
    }

    fn refresh_pane(&mut self) {
        if let Some(pane) = self.pane.as_mut() {
            pane.refresh(&self.live);
        }
    }

    fn render_content(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        match self.pane.as_mut() {
            Some(pane) => pane.render(area, buf, theme),
            None => self.menu.render(area, buf, theme, &self.live),
        }
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
    fn on_menu_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
        match self.menu.selection.on_nav_event(event, self.menu.rows.len(), |_| true) {
            Nav::Close => return vec![UiOutput::Close],
            Nav::Activate | Nav::Clicked => self.pane = self.open_selected_pane(),
            Nav::Moved | Nav::Unhandled => {}
        }
        Vec::new()
    }

    fn open_selected_pane(&self) -> Option<SettingsPane> {
        let pane = match self.menu.selected_row()? {
            MenuRow::Pane(PaneKind::McpServers) => {
                SettingsPane::ServerStatus(ServerStatusPane::new(self.live.servers.clone()))
            }
            MenuRow::Pane(PaneKind::ProviderLogins) => {
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

    fn apply(&mut self, outputs: Vec<UiOutput>) -> Vec<UiOutput> {
        outputs
            .into_iter()
            .filter(|output| match output {
                UiOutput::Close if self.pane.is_some() => {
                    self.pane = None;
                    false
                }
                _ => true,
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
    pub fn on_ui_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
        if let Some(pending) = self.pending_elicitation.as_mut() {
            // Answering a request returns focus to the pane behind it rather
            // than closing the overlay the request arrived in.
            if pending.on_ui_event(event).iter().any(|output| matches!(output, UiOutput::Close)) {
                self.pending_elicitation = None;
            }
            return Vec::new();
        }
        let Some(pane) = self.pane.as_mut() else {
            return self.on_menu_event(event);
        };
        // Esc leaves the pane rather than the overlay, committing whatever the
        // pane batched up while it was open.
        let outputs = match event {
            UiEvent::Key(key) if key.code == KeyCode::Esc => pane
                .take_changes()
                .map(|mut outputs| {
                    outputs.push(UiOutput::Close);
                    outputs
                })
                .unwrap_or_default(),
            event => pane.on_ui_event(event),
        };
        self.apply(outputs)
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
            Paragraph::new(Line::styled("(terminal too small)", theme.text_secondary)).render(area, buf);
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
        self.render_content(content, buf, theme);

        let (Some(prompt), Some(pending)) = (prompt, self.pending_elicitation.as_mut()) else {
            return None;
        };
        pending.render_inline(prompt, buf, theme)
    }
}

fn reasoning_effort_of(options: &[LocalConfigOption]) -> Option<String> {
    LocalConfigView::new(options).reasoning_effort().map(|effort| effort.as_str().to_string())
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

pub(crate) fn value_match_key(value: &LocalConfigValue) -> String {
    format!("{} {}", value.name, value.value)
}
