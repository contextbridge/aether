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
use crate::surfaces::input::{UiOutput, one};
use crate::surfaces::modal::{ElicitationModal, frame::ModalFrame};
use crate::theme::Theme;
use crate::view::input::{Nav, UiEvent};
use crate::view::widgets::key_hints;
use agent_client_protocol::schema::v2::AuthMethod;
use client_core::elicitation::ElicitationId;
use client_core::session_config_view::{LocalConfigOption, LocalConfigSelect, LocalConfigValue, LocalConfigView};
use client_core::session_model::{SessionModel, SessionSetting};
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
    pub select: LocalConfigSelect,
    /// Client-side rather than part of the agent's schema, so it survives a
    /// config-options push that replaces every agent-provided row.
    pub local: bool,
}

/// A settings screen that is not a config option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneKind {
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

    fn summary(self, session: &SessionModel, providers: &[ProviderLoginEntry]) -> String {
        match self {
            Self::McpServers => server_status::summary(session.server_statuses()),
            Self::ProviderLogins => provider_login::summary(providers),
        }
    }
}

/// The settings overlay: a menu of options, each of which opens a pane.
pub struct SettingsOverlay {
    menu: SettingsMenu,
    /// The pane drawn over the menu. `None` means the menu itself has focus.
    pane: Option<SettingsPane>,
    /// Built from the session's auth methods, plus the login status the
    /// overlay tracks while a provider authenticates.
    providers: Vec<ProviderLoginEntry>,
    /// A request the overlay answers itself, drawn under the pane and owning
    /// input until it is answered. Authenticating an OAuth server asks for a
    /// browser this way, so the pane that started it has to survive the ask.
    pending_elicitation: Option<ElicitationModal>,
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
    pub fn new(session: &SessionModel) -> Self {
        let mut overlay = Self {
            menu: SettingsMenu::from_config_options(session.config_options()),
            pane: None,
            providers: build_provider_login_entries(session.auth_methods()),
            pending_elicitation: None,
        };
        overlay.sync_pane_rows();
        overlay
    }

    /// Adds the given rows, replacing any already shown for the same option.
    pub fn upsert_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        self.menu.upsert_local_entries(entries);
    }

    pub fn on_setting_changed(&mut self, setting: SessionSetting, session: &SessionModel) {
        match setting {
            SessionSetting::ConfigOptions => self.menu.update_options(session.config_options()),
            SessionSetting::AuthMethods => self.update_auth_methods(session.auth_methods()),
            SessionSetting::ServerStatuses => {
                if let Some(SettingsPane::ServerStatus(pane)) = self.pane.as_mut() {
                    pane.refresh(session.server_statuses());
                }
            }
        }
    }

    pub fn apply_change(&mut self, change: &SettingsChange) {
        self.menu.apply_change(change);
    }

    pub(crate) fn set_provider_status(&mut self, method_id: &str, status: ProviderLoginStatus) {
        if let Some(entry) = self.providers.iter_mut().find(|entry| entry.method_id == method_id) {
            entry.status = status;
        }
        self.refresh_providers();
    }

    pub(crate) fn show_elicitation(&mut self, modal: ElicitationModal) {
        self.pending_elicitation = Some(modal);
    }

    pub fn take_pending_elicitation(&mut self) -> Option<ElicitationId> {
        self.pending_elicitation.take().map(|pending| pending.id())
    }

    pub(crate) fn pending_elicitation_mut(&mut self) -> Option<&mut ElicitationModal> {
        self.pending_elicitation.as_mut()
    }

    pub(crate) fn dismiss_elicitation(&mut self, id: ElicitationId) {
        self.pending_elicitation.take_if(|pending| pending.id() == id);
    }

    pub fn on_ui_event(&mut self, event: &UiEvent, session: &SessionModel) -> Vec<UiOutput> {
        if let Some(pending) = self.pending_elicitation.as_mut() {
            return one(pending.on_ui_event(event));
        }
        let Some(pane) = self.pane.as_mut() else {
            return self.on_menu_event(event, session);
        };
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

    pub(crate) fn needs_mouse_capture(&self) -> bool {
        self.pending_elicitation.as_ref().is_none_or(ElicitationModal::needs_mouse_capture)
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        cx: &mut DrawContext<'_>,
        session: &SessionModel,
    ) -> Option<Position> {
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
        self.render_content(content, buf, theme, session);

        let (Some(prompt), Some(pending)) = (prompt, self.pending_elicitation.as_mut()) else {
            return None;
        };
        pending.render_inline(prompt, buf, theme)
    }
}

impl SettingsOverlay {
    fn update_auth_methods(&mut self, methods: &[AuthMethod]) {
        self.providers = build_provider_login_entries(methods);
        self.sync_pane_rows();
        self.refresh_providers();
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
        self.menu.show_pane_row(PaneKind::ProviderLogins, !self.providers.is_empty());
    }

    fn refresh_providers(&mut self) {
        if let Some(SettingsPane::ProviderLogin(pane)) = self.pane.as_mut() {
            pane.refresh(&self.providers);
        }
    }

    fn render_content(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, session: &SessionModel) {
        match self.pane.as_mut() {
            Some(pane) => pane.render(area, buf, theme),
            None => self.menu.render(area, buf, theme, |kind| kind.summary(session, &self.providers)),
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
    fn on_menu_event(&mut self, event: &UiEvent, session: &SessionModel) -> Vec<UiOutput> {
        match self.menu.selection.on_nav_event(event, self.menu.rows.len(), |_| true) {
            Nav::Close => return vec![UiOutput::Close],
            Nav::Activate | Nav::Clicked => self.pane = self.open_selected_pane(session),
            Nav::Moved | Nav::Unhandled => {}
        }
        Vec::new()
    }

    fn open_selected_pane(&self, session: &SessionModel) -> Option<SettingsPane> {
        let pane = match self.menu.selected_row()? {
            MenuRow::Pane(PaneKind::McpServers) => {
                SettingsPane::ServerStatus(ServerStatusPane::new(session.server_statuses().to_vec()))
            }
            MenuRow::Pane(PaneKind::ProviderLogins) => {
                SettingsPane::ProviderLogin(ProviderLoginPane::new(self.providers.clone()))
            }
            MenuRow::Select(entry) if entry.select.multi_select => SettingsPane::ModelSelector(ModelSelector::new(
                entry.config_id.clone(),
                &entry.select,
                reasoning_effort_of(session.config_options()).as_deref(),
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
