#![allow(clippy::cast_possible_truncation)]
mod menu;
mod model_selector;
mod picker;
mod provider_login;
mod server_status;

use self::menu::SettingsMenu;
use self::model_selector::ModelSelector;
#[cfg(test)]
use self::model_selector::{capability_tags, model_label, provider_key, provider_label, reasoning_bar};
use self::picker::SettingsPicker;
use self::provider_login::{ProviderLoginPane, build_provider_login_entries, provider_login_summary};
use self::server_status::{ServerStatusPane, server_status_summary};
use crate::session_config_view::SessionConfigView;
use crate::theme::Theme;
#[cfg(test)]
use crate::wrap::truncate_to_width;
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::notifications::{
    ElicitationParams, ElicitationResponse, McpServerStatusEntry, UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{AuthMethod, SessionConfigOption};
#[cfg(test)]
use agent_client_protocol::schema::{SessionConfigKind, SessionConfigSelectOptions};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

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

pub struct SettingsOverlay {
    active_pane: ActivePane,
    menu: SettingsMenu,
    current_reasoning_effort: Option<String>,
    server_statuses: Vec<McpServerStatusEntry>,
    auth_methods: Vec<AuthMethod>,
    pending_elicitation: Option<PendingElicitation>,
}

#[derive(Debug)]
pub enum SettingsOverlayMessage {
    Close,
    SetConfigOption { config_id: String, value: String },
    SetTheme(String),
    AuthenticateServer(String),
    AuthenticateProvider(String),
}

enum ActivePane {
    Menu,
    Picker(SettingsPicker),
    ModelSelector(ModelSelector),
    ServerStatus(ServerStatusPane),
    ProviderLogin(ProviderLoginPane),
}

enum PaneTransition {
    Stay,
    Menu,
    Picker(SettingsPicker),
    ModelSelector(ModelSelector),
    ServerStatus(ServerStatusPane),
    ProviderLogin(ProviderLoginPane),
    Close,
}

struct PaneActivation {
    transition: PaneTransition,
    change: Option<SettingsChange>,
    message: Option<SettingsOverlayMessage>,
}

impl SettingsOverlay {
    pub fn new(
        config_options: &[SessionConfigOption],
        server_statuses: Vec<McpServerStatusEntry>,
        auth_methods: Vec<AuthMethod>,
    ) -> Self {
        let menu = SettingsMenu::from_config_options(config_options);
        let reasoning =
            SessionConfigView::new(config_options).reasoning_effort().map(|effort| effort.as_str().to_string());
        Self {
            active_pane: ActivePane::Menu,
            menu,
            current_reasoning_effort: reasoning,
            server_statuses,
            auth_methods,
            pending_elicitation: None,
        }
    }

    pub fn add_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        self.menu.entries.splice(0..0, entries);
    }

    pub fn on_mouse_scroll_up(&mut self, _local_y: u16) {
        match &mut self.active_pane {
            ActivePane::Menu => self.menu.move_up(),
            ActivePane::Picker(picker) => picker.move_up(),
            ActivePane::ModelSelector(selector) => selector.move_up(),
            ActivePane::ServerStatus(pane) => pane.move_up(),
            ActivePane::ProviderLogin(pane) => pane.move_up(),
        }
    }

    pub fn on_mouse_scroll_down(&mut self, _local_y: u16) {
        match &mut self.active_pane {
            ActivePane::Menu => self.menu.move_down(),
            ActivePane::Picker(picker) => picker.move_down(),
            ActivePane::ModelSelector(selector) => selector.move_down(),
            ActivePane::ServerStatus(pane) => pane.move_down(),
            ActivePane::ProviderLogin(pane) => pane.move_down(),
        }
    }

    pub fn on_mouse_click(&mut self, local_y: u16, rect: Rect) -> Vec<SettingsOverlayMessage> {
        if rect.width < MIN_WIDTH || rect.height < MIN_HEIGHT {
            return vec![];
        }
        let inner = Block::new().borders(Borders::ALL).title(" Configuration ").inner(rect);
        let row = local_y.saturating_sub(inner.y.saturating_sub(rect.y)) as usize;
        if local_y < inner.y || local_y >= inner.bottom() {
            return vec![];
        }
        let activation = match &mut self.active_pane {
            ActivePane::Menu => {
                self.menu.click_row(row);
                self.menu_activation()
            }
            ActivePane::Picker(picker) => {
                if picker.click_row(row) {
                    picker_activation(picker)
                } else {
                    PaneActivation::stay()
                }
            }
            ActivePane::ModelSelector(selector) => {
                if selector.click_row(row, usize::from(inner.height)) {
                    selector.toggle_focused();
                }
                PaneActivation::stay()
            }
            ActivePane::ServerStatus(pane) => {
                if pane.click_row(row) {
                    server_authentication_activation(pane)
                } else {
                    PaneActivation::stay()
                }
            }
            ActivePane::ProviderLogin(pane) => {
                if pane.click_row(row) {
                    provider_authentication_activation(pane)
                } else {
                    PaneActivation::stay()
                }
            }
        };
        self.apply_activation(activation)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        let previous = std::mem::replace(&mut self.active_pane, ActivePane::Menu);
        let (transition, messages) = match previous {
            ActivePane::Menu => {
                let messages = self.handle_menu_key(key);
                let transition = if messages.iter().any(|message| matches!(message, SettingsOverlayMessage::Close)) {
                    PaneTransition::Close
                } else {
                    self.take_transition(PaneTransition::Menu)
                };
                (transition, messages)
            }
            ActivePane::Picker(mut picker) => {
                let messages = self.handle_picker_key(&mut picker, key);
                let transition = if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                    PaneTransition::Menu
                } else {
                    self.take_transition(PaneTransition::Picker(picker))
                };
                (transition, messages)
            }
            ActivePane::ModelSelector(mut selector) => {
                let messages = self.handle_model_selector_key(&mut selector, key);
                let transition = if key.code == KeyCode::Esc {
                    PaneTransition::Menu
                } else {
                    self.take_transition(PaneTransition::ModelSelector(selector))
                };
                (transition, messages)
            }
            ActivePane::ServerStatus(mut pane) => {
                let messages = self.handle_server_status_key(&mut pane, key);
                let transition = if key.code == KeyCode::Esc {
                    PaneTransition::Menu
                } else {
                    self.take_transition(PaneTransition::ServerStatus(pane))
                };
                (transition, messages)
            }
            ActivePane::ProviderLogin(mut pane) => {
                let messages = self.handle_provider_login_key(&mut pane, key);
                let transition = if key.code == KeyCode::Esc {
                    PaneTransition::Menu
                } else {
                    self.take_transition(PaneTransition::ProviderLogin(pane))
                };
                (transition, messages)
            }
        };
        self.apply_transition(transition);
        messages
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        Clear.render(area, buffer);
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            Paragraph::new(Line::styled("(terminal too small)", Style::new().fg(theme.text_secondary)))
                .render(area, buffer);
            return;
        }
        let block = Block::new()
            .borders(Borders::ALL)
            .title(" Configuration ")
            .style(Style::new().bg(theme.background))
            .border_style(Style::new().fg(theme.text_secondary));
        let inner = block.inner(area);
        block.render(area, buffer);
        let [content_area, footer_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
        Paragraph::new(Line::from(self.footer_text())).render(footer_area, buffer);
        match &mut self.active_pane {
            ActivePane::Menu => self.menu.render(content_area, buffer, theme),
            ActivePane::Picker(picker) => picker.render(content_area, buffer, theme),
            ActivePane::ModelSelector(selector) => selector.render(content_area, buffer, theme),
            ActivePane::ServerStatus(pane) => pane.render(content_area, buffer, theme),
            ActivePane::ProviderLogin(pane) => pane.render(content_area, buffer, theme),
        }
    }

    pub fn wants_key_capture(&self) -> bool {
        !matches!(self.active_pane, ActivePane::Menu)
    }

    pub fn update_config_options(&mut self, options: &[SessionConfigOption]) {
        self.current_reasoning_effort =
            SessionConfigView::new(options).reasoning_effort().map(|effort| effort.as_str().to_string());
        self.menu.update_options(options);
    }

    pub fn apply_change(&mut self, change: &SettingsChange) {
        self.menu.apply_change(change);
    }

    pub fn update_server_statuses(&mut self, statuses: Vec<McpServerStatusEntry>) {
        self.server_statuses = statuses;
        self.menu.upsert_mcp_servers_entry(&server_status_summary(&self.server_statuses));
        if let ActivePane::ServerStatus(pane) = &mut self.active_pane {
            pane.update_entries(self.server_statuses.clone());
        }
    }

    pub fn update_auth_methods(&mut self, methods: Vec<AuthMethod>) {
        self.auth_methods = methods;
        let entries = build_provider_login_entries(&self.auth_methods);
        self.menu.upsert_provider_logins_entry(&provider_login_summary(&entries));
        if let ActivePane::ProviderLogin(pane) = &mut self.active_pane {
            pane.replace_entries(entries);
        }
    }

    pub fn on_authenticate_started(&mut self, method_id: &str) {
        if let ActivePane::ProviderLogin(pane) = &mut self.active_pane {
            pane.set_authenticating(method_id);
        }
    }

    pub fn on_authenticate_complete(&mut self, method_id: &str) {
        if let ActivePane::ProviderLogin(pane) = &mut self.active_pane {
            pane.set_logged_in(method_id);
        }
    }

    pub fn on_authenticate_failed(&mut self, method_id: &str) {
        if let ActivePane::ProviderLogin(pane) = &mut self.active_pane {
            pane.reset_to_needs_login(method_id);
        }
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

    pub fn add_mcp_servers_entry(&mut self) {
        self.menu.upsert_mcp_servers_entry(&server_status_summary(&self.server_statuses));
    }

    pub fn add_provider_logins_entry(&mut self) {
        if !self.auth_methods.is_empty() {
            let entries = build_provider_login_entries(&self.auth_methods);
            self.menu.upsert_provider_logins_entry(&provider_login_summary(&entries));
        }
    }

    fn take_transition(&mut self, fallback: PaneTransition) -> PaneTransition {
        match std::mem::replace(&mut self.active_pane, ActivePane::Menu) {
            ActivePane::Menu => fallback,
            ActivePane::Picker(picker) => PaneTransition::Picker(picker),
            ActivePane::ModelSelector(selector) => PaneTransition::ModelSelector(selector),
            ActivePane::ServerStatus(pane) => PaneTransition::ServerStatus(pane),
            ActivePane::ProviderLogin(pane) => PaneTransition::ProviderLogin(pane),
        }
    }

    fn apply_transition(&mut self, transition: PaneTransition) {
        self.active_pane = match transition {
            PaneTransition::Stay => return,
            PaneTransition::Menu | PaneTransition::Close => ActivePane::Menu,
            PaneTransition::Picker(picker) => ActivePane::Picker(picker),
            PaneTransition::ModelSelector(selector) => ActivePane::ModelSelector(selector),
            PaneTransition::ServerStatus(pane) => ActivePane::ServerStatus(pane),
            PaneTransition::ProviderLogin(pane) => ActivePane::ProviderLogin(pane),
        };
    }

    fn footer_text(&self) -> String {
        match &self.active_pane {
            ActivePane::ModelSelector(selector) => {
                format!("[Space/Enter] Toggle  [Tab] Effort: {}  [Esc] Done", selector.reasoning_label())
            }
            ActivePane::Picker(_) => "[Enter] Confirm  [Esc] Back".to_string(),
            ActivePane::ServerStatus(_) => "[Enter] Authenticate OAuth servers  [Esc] Back".to_string(),
            ActivePane::ProviderLogin(_) => "[Enter] Authenticate  [Esc] Back".to_string(),
            ActivePane::Menu => "[Enter] Select  [Esc] Close".to_string(),
        }
    }

    fn apply_activation(&mut self, activation: PaneActivation) -> Vec<SettingsOverlayMessage> {
        if let Some(change) = activation.change {
            self.menu.apply_change(&change);
        }
        self.apply_transition(activation.transition);
        activation.message.into_iter().collect()
    }

    fn menu_activation(&self) -> PaneActivation {
        let Some(entry) = self.menu.selected_entry() else { return PaneActivation::stay() };
        match entry.entry_kind {
            SettingsMenuEntryKind::McpServers => PaneActivation::server_status(self.server_statuses.clone()),
            SettingsMenuEntryKind::ProviderLogins => {
                PaneActivation::provider_login(build_provider_login_entries(&self.auth_methods))
            }
            _ if entry.multi_select => PaneActivation::model_selector(entry, self.current_reasoning_effort.as_deref()),
            _ => SettingsPicker::from_entry(entry).map_or_else(PaneActivation::stay, PaneActivation::picker),
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => return vec![SettingsOverlayMessage::Close],
            KeyCode::Up => self.menu.move_up(),
            KeyCode::Down => self.menu.move_down(),
            KeyCode::Enter => self.apply_transition(self.menu_activation().transition),
            _ => {}
        }
        vec![]
    }

    fn handle_picker_key(&mut self, picker: &mut SettingsPicker, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => self.active_pane = ActivePane::Menu,
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::Enter => return self.apply_activation(picker_activation(picker)),
            KeyCode::Backspace => picker.pop_query_char(),
            KeyCode::Char(c) if !c.is_control() => picker.push_query_char(c),
            _ => {}
        }
        vec![]
    }

    fn handle_model_selector_key(
        &mut self,
        selector: &mut ModelSelector,
        key: KeyEvent,
    ) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => {
                self.active_pane = ActivePane::Menu;
                return process_config_changes(&selector.confirm());
            }
            KeyCode::Up => {
                selector.move_up();
                selector.clamp_reasoning_to_focused();
            }
            KeyCode::Down => {
                selector.move_down();
                selector.clamp_reasoning_to_focused();
            }
            KeyCode::Tab => selector.cycle_reasoning(),
            KeyCode::BackTab => selector.cycle_reasoning_back(),
            KeyCode::Enter | KeyCode::Char(' ') => selector.toggle_focused(),
            KeyCode::Backspace => selector.pop_query_char(),
            KeyCode::Char(c) if !c.is_control() => selector.push_query_char(c),
            _ => {}
        }
        vec![]
    }

    fn handle_server_status_key(&mut self, pane: &mut ServerStatusPane, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => self.active_pane = ActivePane::Menu,
            KeyCode::Up => pane.move_up(),
            KeyCode::Down => pane.move_down(),
            KeyCode::Enter => return self.apply_activation(server_authentication_activation(pane)),
            _ => {}
        }
        vec![]
    }

    fn handle_provider_login_key(
        &mut self,
        pane: &mut ProviderLoginPane,
        key: KeyEvent,
    ) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => self.active_pane = ActivePane::Menu,
            KeyCode::Up => pane.move_up(),
            KeyCode::Down => pane.move_down(),
            KeyCode::Enter => return self.apply_activation(provider_authentication_activation(pane)),
            _ => {}
        }
        vec![]
    }
}

impl Drop for SettingsOverlay {
    fn drop(&mut self) {
        self.cancel_pending_elicitation();
    }
}

impl PaneActivation {
    fn stay() -> Self {
        Self { transition: PaneTransition::Stay, change: None, message: None }
    }
    fn picker(picker: SettingsPicker) -> Self {
        Self { transition: PaneTransition::Picker(picker), change: None, message: None }
    }
    fn model_selector(entry: &SettingsMenuEntry, effort: Option<&str>) -> Self {
        Self {
            transition: PaneTransition::ModelSelector(ModelSelector::new(
                entry.config_id.clone(),
                entry.values.clone(),
                &entry.current_raw_value,
                effort,
            )),
            change: None,
            message: None,
        }
    }
    fn server_status(entries: Vec<McpServerStatusEntry>) -> Self {
        Self { transition: PaneTransition::ServerStatus(ServerStatusPane::new(entries)), change: None, message: None }
    }
    fn provider_login(entries: Vec<provider_login::ProviderLoginEntry>) -> Self {
        Self { transition: PaneTransition::ProviderLogin(ProviderLoginPane::new(entries)), change: None, message: None }
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

fn picker_activation(picker: &SettingsPicker) -> PaneActivation {
    let change = picker.confirm_selection();
    let message = change.as_ref().map(message_for_change);
    PaneActivation { transition: PaneTransition::Menu, change, message }
}
fn server_authentication_activation(pane: &ServerStatusPane) -> PaneActivation {
    PaneActivation { transition: PaneTransition::Stay, change: None, message: pane.authentication_message() }
}
fn provider_authentication_activation(pane: &ProviderLoginPane) -> PaneActivation {
    PaneActivation { transition: PaneTransition::Stay, change: None, message: pane.authentication_message() }
}
fn message_for_change(change: &SettingsChange) -> SettingsOverlayMessage {
    if change.config_id == acp_utils::config_option_id::THEME_CONFIG_ID {
        SettingsOverlayMessage::SetTheme(change.new_value.clone())
    } else {
        SettingsOverlayMessage::SetConfigOption { config_id: change.config_id.clone(), value: change.new_value.clone() }
    }
}
fn process_config_changes(changes: &[SettingsChange]) -> Vec<SettingsOverlayMessage> {
    changes.iter().map(message_for_change).collect()
}

#[cfg(test)]
mod tests;
