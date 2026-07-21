#![allow(clippy::cast_possible_truncation)]
use crate::theme::Theme;
use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::{ConfigOptionId, THEME_CONFIG_ID};
use acp_utils::notifications::{
    ElicitationParams, ElicitationResponse, McpServerStatus, McpServerStatusEntry, UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{AuthMethod, SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;
use utils::ReasoningEffort;

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

enum ActivePane {
    Sentinel,
    Menu,
    Picker(SettingsPicker),
    ModelSelector(ModelSelector),
    ServerStatus(ServerStatusPane),
    ProviderLogin(ProviderLoginPane),
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

struct SettingsMenu {
    entries: Vec<SettingsMenuEntry>,
    selected: usize,
}

struct SettingsPicker {
    config_id: String,
    title: String,
    values: Vec<SettingsMenuValue>,
    current_value: String,
    query: String,
    selected: usize,
    filtered: Vec<usize>,
}

struct ModelSelector {
    config_id: String,
    all_items: Vec<SettingsMenuValue>,
    selected_models: HashSet<String>,
    original_models: HashSet<String>,
    query: String,
    focused: usize,
    filtered: Vec<usize>,
    reasoning_effort: Option<ReasoningEffort>,
    original_reasoning_effort: Option<ReasoningEffort>,
}

struct ServerStatusPane {
    rows: Vec<ServerStatusRow>,
    selected: usize,
}

#[derive(Clone)]
enum ServerStatusRow {
    Header(String),
    Spacer,
    Server { entry: McpServerStatusEntry, indented: bool },
}

struct ProviderLoginPane {
    entries: Vec<ProviderLoginEntry>,
    selected: usize,
}

struct ProviderLoginEntry {
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

struct PendingElicitation {
    server_name: String,
    elicitation_id: String,
    #[allow(dead_code)]
    responder: Responder<ElicitationResponse>,
}

impl SettingsOverlay {
    pub fn new(
        config_options: &[SessionConfigOption],
        server_statuses: Vec<McpServerStatusEntry>,
        auth_methods: Vec<AuthMethod>,
    ) -> Self {
        let menu = SettingsMenu::from_config_options(config_options);
        let reasoning = extract_reasoning_effort(config_options);
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

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        if let ActivePane::Menu = self.active_pane {
            self.handle_menu_key(key)
        } else {
            let prev = std::mem::replace(&mut self.active_pane, ActivePane::Sentinel);
            let (next_pane, msgs) = match prev {
                ActivePane::Sentinel | ActivePane::Menu => unreachable!(),
                ActivePane::Picker(mut picker) => {
                    let msgs = self.handle_picker_key(&mut picker, key);
                    let next = std::mem::replace(&mut self.active_pane, ActivePane::Sentinel);
                    let pane = if matches!(next, ActivePane::Sentinel) { ActivePane::Picker(picker) } else { next };
                    (pane, msgs)
                }
                ActivePane::ModelSelector(mut selector) => {
                    let msgs = self.handle_model_selector_key(&mut selector, key);
                    let next = std::mem::replace(&mut self.active_pane, ActivePane::Sentinel);
                    let pane =
                        if matches!(next, ActivePane::Sentinel) { ActivePane::ModelSelector(selector) } else { next };
                    (pane, msgs)
                }
                ActivePane::ServerStatus(mut pane) => {
                    let msgs = self.handle_server_status_key(&mut pane, key);
                    let next = std::mem::replace(&mut self.active_pane, ActivePane::Sentinel);
                    let pane = if matches!(next, ActivePane::Sentinel) { ActivePane::ServerStatus(pane) } else { next };
                    (pane, msgs)
                }
                ActivePane::ProviderLogin(mut pane) => {
                    let msgs = self.handle_provider_login_key(&mut pane, key);
                    let next = std::mem::replace(&mut self.active_pane, ActivePane::Sentinel);
                    let pane =
                        if matches!(next, ActivePane::Sentinel) { ActivePane::ProviderLogin(pane) } else { next };
                    (pane, msgs)
                }
            };
            self.active_pane = next_pane;
            msgs
        }
    }

    pub fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            let text = Line::styled("(terminal too small)", Style::new().fg(theme.text_secondary));
            Paragraph::new(text).render(area, buffer);
            return;
        }

        let block = Block::new()
            .borders(Borders::ALL)
            .title(" Configuration ")
            .border_style(Style::new().fg(theme.text_secondary));
        let inner = block.inner(area);
        block.render(area, buffer);

        let footer_text = self.footer_text();
        let footer_area = Rect { y: area.bottom().saturating_sub(1), height: 1, ..area };
        if footer_area.y > area.y {
            Paragraph::new(Line::from(footer_text)).render(footer_area, buffer);
        }

        match &self.active_pane {
            ActivePane::Menu | ActivePane::Sentinel => self.menu.render(inner, buffer, theme),
            ActivePane::Picker(picker) => picker.render(inner, buffer, theme),
            ActivePane::ModelSelector(selector) => selector.render(inner, buffer, theme),
            ActivePane::ServerStatus(pane) => pane.render(inner, buffer, theme),
            ActivePane::ProviderLogin(pane) => pane.render(inner, buffer, theme),
        }
    }

    pub fn wants_key_capture(&self) -> bool {
        matches!(
            self.active_pane,
            ActivePane::Picker(_)
                | ActivePane::ModelSelector(_)
                | ActivePane::ServerStatus(_)
                | ActivePane::ProviderLogin(_)
        )
    }

    pub fn update_config_options(&mut self, options: &[SessionConfigOption]) {
        self.current_reasoning_effort = extract_reasoning_effort(options);
        self.menu.update_options(options);
    }

    pub fn apply_change(&mut self, change: &SettingsChange) {
        self.menu.apply_change(change);
    }

    fn footer_text(&self) -> String {
        match &self.active_pane {
            ActivePane::ModelSelector(selector) => {
                let effort = ReasoningEffort::config_str(selector.reasoning_effort);
                format!("[Space/Enter] Toggle  [Tab] Effort: {effort}  [Esc] Done")
            }
            ActivePane::Picker(_) => "[Enter] Confirm  [Esc] Back".to_string(),
            ActivePane::ServerStatus(_) => "[Enter] Authenticate OAuth servers  [Esc] Back".to_string(),
            ActivePane::ProviderLogin(_) => "[Enter] Authenticate  [Esc] Back".to_string(),
            ActivePane::Menu | ActivePane::Sentinel => "[Enter] Select  [Esc] Close".to_string(),
        }
    }

    fn handle_menu_key(&mut self, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => return vec![SettingsOverlayMessage::Close],
            KeyCode::Up => self.menu.move_up(),
            KeyCode::Down => self.menu.move_down(),
            KeyCode::Enter => {
                if let Some(entry) = self.menu.selected_entry() {
                    match entry.entry_kind {
                        SettingsMenuEntryKind::McpServers => {
                            self.active_pane =
                                ActivePane::ServerStatus(ServerStatusPane::new(self.server_statuses.clone()));
                        }
                        SettingsMenuEntryKind::ProviderLogins => {
                            let entries = build_provider_login_entries(&self.auth_methods);
                            self.active_pane = ActivePane::ProviderLogin(ProviderLoginPane::new(entries));
                        }
                        _ if entry.multi_select => {
                            let selector = ModelSelector::new(
                                entry.config_id.clone(),
                                entry.values.clone(),
                                &entry.current_raw_value,
                                self.current_reasoning_effort.as_deref(),
                            );
                            self.active_pane = ActivePane::ModelSelector(selector);
                        }
                        _ => {
                            if let Some(picker) = SettingsPicker::from_entry(entry) {
                                self.active_pane = ActivePane::Picker(picker);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        vec![]
    }

    fn handle_picker_key(&mut self, picker: &mut SettingsPicker, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => {
                self.active_pane = ActivePane::Menu;
            }
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::Enter => {
                if let Some(change) = picker.confirm_selection() {
                    self.menu.apply_change(&change);
                    self.active_pane = ActivePane::Menu;
                    if change.config_id == THEME_CONFIG_ID {
                        return vec![SettingsOverlayMessage::SetTheme(change.new_value)];
                    }
                    return vec![SettingsOverlayMessage::SetConfigOption {
                        config_id: change.config_id,
                        value: change.new_value,
                    }];
                }
                self.active_pane = ActivePane::Menu;
            }
            KeyCode::Backspace => {
                picker.pop_query_char();
            }
            KeyCode::Char(c) if !c.is_control() => {
                picker.push_query_char(c);
            }
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
                let changes = selector.confirm();
                self.active_pane = ActivePane::Menu;
                return process_config_changes(changes);
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
            KeyCode::Enter | KeyCode::Char(' ') => selector.toggle_focused(),
            KeyCode::Backspace => selector.pop_query_char(),
            KeyCode::Char(c) if !c.is_control() => {
                selector.push_query_char(c);
            }
            _ => {}
        }
        vec![]
    }

    fn handle_server_status_key(&mut self, pane: &mut ServerStatusPane, key: KeyEvent) -> Vec<SettingsOverlayMessage> {
        match key.code {
            KeyCode::Esc => {
                self.active_pane = ActivePane::Menu;
            }
            KeyCode::Up => pane.move_up(),
            KeyCode::Down => pane.move_down(),
            KeyCode::Enter => {
                if let Some(entry) = pane.selected_entry()
                    && entry.can_authenticate()
                {
                    return vec![SettingsOverlayMessage::AuthenticateServer(entry.name.clone())];
                }
            }
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
            KeyCode::Esc => {
                self.active_pane = ActivePane::Menu;
            }
            KeyCode::Up => pane.move_up(),
            KeyCode::Down => pane.move_down(),
            KeyCode::Enter => {
                if let Some(entry) = pane.selected_entry()
                    && entry.status != ProviderLoginStatus::Authenticating
                {
                    return vec![SettingsOverlayMessage::AuthenticateProvider(entry.method_id.clone())];
                }
            }
            _ => {}
        }
        vec![]
    }

    pub fn update_server_statuses(&mut self, statuses: Vec<McpServerStatusEntry>) {
        self.server_statuses = statuses;
        self.menu.upsert_mcp_servers_entry(&server_status_summary(&self.server_statuses));
        if let ActivePane::ServerStatus(ref mut pane) = self.active_pane {
            pane.update_entries(self.server_statuses.clone());
        }
    }

    pub fn update_auth_methods(&mut self, methods: Vec<AuthMethod>) {
        self.auth_methods = methods;
        let entries = build_provider_login_entries(&self.auth_methods);
        self.menu.upsert_provider_logins_entry(&provider_login_summary(&entries));
        if let ActivePane::ProviderLogin(ref mut pane) = self.active_pane {
            pane.replace_entries(entries);
        }
    }

    pub fn on_authenticate_started(&mut self, method_id: &str) {
        if let ActivePane::ProviderLogin(ref mut pane) = self.active_pane {
            pane.set_authenticating(method_id);
        }
    }

    pub fn on_authenticate_complete(&mut self, method_id: &str) {
        if let ActivePane::ProviderLogin(ref mut pane) = self.active_pane {
            pane.set_logged_in(method_id);
        }
    }

    pub fn on_authenticate_failed(&mut self, method_id: &str) {
        if let ActivePane::ProviderLogin(ref mut pane) = self.active_pane {
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
}

impl Drop for SettingsOverlay {
    fn drop(&mut self) {
        self.cancel_pending_elicitation();
    }
}

impl PendingElicitation {
    fn matches(&self, params: &UrlElicitationCompleteParams) -> bool {
        self.server_name == params.server_name && self.elicitation_id == params.elicitation_id
    }
}

impl ServerStatusPane {
    fn new(entries: Vec<McpServerStatusEntry>) -> Self {
        let rows = build_rows(entries);
        let selected = rows.iter().position(|r| matches!(r, ServerStatusRow::Server { .. })).unwrap_or(0);
        Self { rows, selected }
    }

    fn move_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let start = self.selected;
        loop {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.rows.len() - 1);
            if matches!(self.rows[self.selected], ServerStatusRow::Server { .. }) || self.selected == start {
                break;
            }
        }
    }

    fn move_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let start = self.selected;
        loop {
            self.selected = (self.selected + 1) % self.rows.len();
            if matches!(self.rows[self.selected], ServerStatusRow::Server { .. }) || self.selected == start {
                break;
            }
        }
    }

    fn selected_entry(&self) -> Option<&McpServerStatusEntry> {
        match self.rows.get(self.selected)? {
            ServerStatusRow::Server { entry, .. } => Some(entry),
            _ => None,
        }
    }

    fn update_entries(&mut self, entries: Vec<McpServerStatusEntry>) {
        let selected_name = self.selected_entry().map(|e| e.name.clone());
        self.rows = build_rows(entries);
        self.selected = 0;
        if let Some(name) = selected_name
            && let Some(idx) =
                self.rows.iter().position(|r| matches!(r, ServerStatusRow::Server { entry, .. } if entry.name == name))
        {
            self.selected = idx;
        }
        if !matches!(self.rows.get(self.selected), Some(ServerStatusRow::Server { .. })) {
            self.selected = self.rows.iter().position(|r| matches!(r, ServerStatusRow::Server { .. })).unwrap_or(0);
        }
    }

    fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        if self.rows.is_empty() {
            buffer.set_string(area.x, area.y, " (no MCP servers configured)", Style::new().fg(theme.text_secondary));
            return;
        }

        let mut y = area.y;
        for i in 0..self.rows.len() {
            if y >= area.bottom() {
                break;
            }
            match &self.rows[i] {
                ServerStatusRow::Header(label) => {
                    buffer.set_string(area.x, y, label, Style::new().fg(theme.heading));
                    y += 1;
                }
                ServerStatusRow::Spacer => {
                    y += 1;
                }
                ServerStatusRow::Server { entry, indented } => {
                    let (indicator, detail) = match &entry.status {
                        McpServerStatus::Connected { tool_count } if entry.can_authenticate() => {
                            ("✓", format!("{tool_count} tools, authenticated"))
                        }
                        McpServerStatus::Connected { tool_count } => ("✓", format!("{tool_count} tools")),
                        McpServerStatus::Failed { error } => ("✗", error.clone()),
                        McpServerStatus::Connecting => ("…", "connecting".to_string()),
                        McpServerStatus::Authenticating => ("…", "authenticating".to_string()),
                        McpServerStatus::NeedsOAuth => ("⚡", "needs authentication".to_string()),
                    };

                    let prefix = if *indented { "  " } else { "" };
                    let text = format!(" {prefix}{}  {indicator} {detail}", entry.name);
                    let style = match &entry.status {
                        McpServerStatus::Connected { .. } | McpServerStatus::Connecting => {
                            if i == self.selected {
                                Style::new().fg(theme.background).bg(theme.text_primary)
                            } else {
                                Style::new().fg(theme.text_primary)
                            }
                        }
                        McpServerStatus::Failed { .. } => {
                            if i == self.selected {
                                Style::new().fg(theme.background).bg(theme.error)
                            } else {
                                Style::new().fg(theme.error)
                            }
                        }
                        McpServerStatus::Authenticating | McpServerStatus::NeedsOAuth => {
                            if i == self.selected {
                                Style::new().fg(theme.background).bg(theme.warning)
                            } else {
                                Style::new().fg(theme.warning)
                            }
                        }
                    };

                    buffer.set_string(area.x, y, truncate_str(&text, area.width as usize), style);
                    y += 1;
                }
            }
        }
    }
}

fn build_rows(entries: Vec<McpServerStatusEntry>) -> Vec<ServerStatusRow> {
    let (proxied, direct): (Vec<_>, Vec<_>) = entries.into_iter().partition(|entry| entry.proxied);

    if proxied.is_empty() {
        return direct.into_iter().map(|entry| ServerStatusRow::Server { entry, indented: false }).collect();
    }

    let mut rows = Vec::new();
    if !direct.is_empty() {
        rows.push(ServerStatusRow::Header("Direct".to_string()));
        rows.extend(direct.into_iter().map(|entry| ServerStatusRow::Server { entry, indented: true }));
        rows.push(ServerStatusRow::Spacer);
    }
    rows.push(ServerStatusRow::Header("Proxied".to_string()));
    rows.extend(proxied.into_iter().map(|entry| ServerStatusRow::Server { entry, indented: true }));
    rows
}

impl ProviderLoginPane {
    fn new(entries: Vec<ProviderLoginEntry>) -> Self {
        let selected = 0;
        Self { entries, selected }
    }

    fn move_up(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.entries.len() - 1);
        }
    }

    fn move_down(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    fn selected_entry(&self) -> Option<&ProviderLoginEntry> {
        self.entries.get(self.selected)
    }

    fn set_authenticating(&mut self, method_id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.method_id == method_id) {
            entry.status = ProviderLoginStatus::Authenticating;
        }
    }

    fn set_logged_in(&mut self, method_id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.method_id == method_id) {
            entry.status = ProviderLoginStatus::LoggedIn;
        }
    }

    fn reset_to_needs_login(&mut self, method_id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.method_id == method_id) {
            entry.status = ProviderLoginStatus::NeedsLogin;
        }
    }

    fn replace_entries(&mut self, entries: Vec<ProviderLoginEntry>) {
        let selected_method_id = self.selected_entry().map(|e| e.method_id.clone());
        self.entries = entries;
        self.selected = 0;
        if let Some(method_id) = selected_method_id
            && let Some(idx) = self.entries.iter().position(|e| e.method_id == method_id)
        {
            self.selected = idx;
        }
    }

    fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        if self.entries.is_empty() {
            buffer.set_string(area.x, area.y, " (no providers need login)", Style::new().fg(theme.text_secondary));
            return;
        }

        for i in 0..self.entries.len() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }
            let entry = &self.entries[i];
            let (indicator, detail) = match &entry.status {
                ProviderLoginStatus::NeedsLogin => ("⚡", "needs login"),
                ProviderLoginStatus::Authenticating => ("⏳", "authenticating..."),
                ProviderLoginStatus::LoggedIn => ("✓", "logged in"),
            };

            let text = format!(" {}  {} {}", entry.name, indicator, detail);
            let style = if entry.status == ProviderLoginStatus::LoggedIn {
                if i == self.selected {
                    Style::new().fg(theme.background).bg(theme.success)
                } else {
                    Style::new().fg(theme.success)
                }
            } else if i == self.selected {
                Style::new().fg(theme.background).bg(theme.warning)
            } else {
                Style::new().fg(theme.warning)
            };

            buffer.set_string(area.x, y, truncate_str(&text, area.width as usize), style);
        }
    }
}

impl SettingsMenu {
    fn from_config_options(options: &[SessionConfigOption]) -> Self {
        let entries: Vec<SettingsMenuEntry> = options
            .iter()
            .filter(|opt| opt.id.0.as_ref() != ConfigOptionId::ReasoningEffort.as_str())
            .filter_map(|opt| {
                let SessionConfigKind::Select(ref select) = opt.kind else {
                    return None;
                };

                let flat_options = match &select.options {
                    SessionConfigSelectOptions::Ungrouped(opts) => opts.clone(),
                    SessionConfigSelectOptions::Grouped(groups) => {
                        groups.iter().flat_map(|g| g.options.clone()).collect()
                    }
                    _ => return None,
                };

                if flat_options.is_empty() {
                    return None;
                }

                let current_value_index =
                    flat_options.iter().position(|o| o.value == select.current_value).unwrap_or(0);

                let values: Vec<SettingsMenuValue> = flat_options
                    .into_iter()
                    .map(|o| SettingsMenuValue {
                        value: o.value.0.to_string(),
                        name: o.name,
                        is_disabled: o.description.as_deref().is_some_and(|d| d.starts_with("Unavailable:")),
                        description: o.description,
                        meta: SelectOptionMeta::from_meta(o.meta.as_ref()),
                    })
                    .collect();

                let multi_select = ConfigOptionMeta::from_meta(opt.meta.as_ref()).multi_select;

                let display_name = if multi_select && select.current_value.0.contains(',') {
                    let parts: Vec<&str> = select.current_value.0.split(',').map(str::trim).collect();
                    let names: Vec<&str> = parts
                        .iter()
                        .filter_map(|val| values.iter().find(|v| v.value == *val).map(|v| v.name.as_str()))
                        .collect();
                    if names.is_empty() { Some(format!("{} models", parts.len())) } else { Some(names.join(", ")) }
                } else {
                    None
                };

                Some(SettingsMenuEntry {
                    config_id: opt.id.0.to_string(),
                    title: opt.name.clone(),
                    values,
                    current_value_index,
                    current_raw_value: select.current_value.0.to_string(),
                    entry_kind: SettingsMenuEntryKind::Select,
                    multi_select,
                    display_name,
                })
            })
            .collect();

        Self { entries, selected: 0 }
    }

    fn move_up(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.entries.len() - 1);
        }
    }

    fn move_down(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
        }
    }

    fn selected_entry(&self) -> Option<&SettingsMenuEntry> {
        self.entries.get(self.selected)
    }

    fn update_options(&mut self, options: &[SessionConfigOption]) {
        let local_entries: Vec<SettingsMenuEntry> =
            self.entries.iter().filter(|e| !matches!(e.entry_kind, SettingsMenuEntryKind::Select)).cloned().collect();
        *self = Self::from_config_options(options);
        self.entries.splice(0..0, local_entries);
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    fn apply_change(&mut self, change: &SettingsChange) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.config_id == change.config_id) {
            entry.current_raw_value.clone_from(&change.new_value);
            if let Some(index) = entry.values.iter().position(|v| v.value == change.new_value) {
                entry.current_value_index = index;
            }
        }
    }

    fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let visible = usize::from(area.height).min(self.entries.len());
        let start = if self.selected >= visible { self.selected - visible + 1 } else { 0 };

        for i in 0..visible {
            let idx = start + i;
            let Some(entry) = self.entries.get(idx) else {
                break;
            };
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }

            let current_name = entry
                .display_name
                .as_deref()
                .or_else(|| entry.values.get(entry.current_value_index).map(|v| v.name.as_str()))
                .unwrap_or("?");

            let text = format!("{}: {}", entry.title, current_name);
            let style = if idx == self.selected {
                Style::new().fg(theme.background).bg(theme.text_primary)
            } else {
                Style::new().fg(theme.text_primary)
            };

            let truncated = truncate_str(&text, usize::from(area.width).saturating_sub(2));
            let line = format!(" {truncated}");
            buffer.set_string(area.x, y, &line, style);
        }

        if self.entries.is_empty() {
            buffer.set_string(area.x, area.y, " (no settings options)", Style::new().fg(theme.text_secondary));
        }
    }

    fn upsert_mcp_servers_entry(&mut self, summary: &str) {
        let entry = SettingsMenuEntry {
            config_id: "_mcp_servers".to_string(),
            title: "MCP Servers".to_string(),
            values: vec![SettingsMenuValue {
                value: summary.to_string(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: summary.to_string(),
            entry_kind: SettingsMenuEntryKind::McpServers,
            multi_select: false,
            display_name: None,
        };

        if let Some(pos) = self.entries.iter().position(|e| matches!(e.entry_kind, SettingsMenuEntryKind::McpServers)) {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }

    fn upsert_provider_logins_entry(&mut self, summary: &str) {
        let entry = SettingsMenuEntry {
            config_id: "_provider_logins".to_string(),
            title: "Provider Logins".to_string(),
            values: vec![SettingsMenuValue {
                value: summary.to_string(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: summary.to_string(),
            entry_kind: SettingsMenuEntryKind::ProviderLogins,
            multi_select: false,
            display_name: None,
        };

        if let Some(pos) =
            self.entries.iter().position(|e| matches!(e.entry_kind, SettingsMenuEntryKind::ProviderLogins))
        {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }
}

impl SettingsPicker {
    fn from_entry(entry: &SettingsMenuEntry) -> Option<Self> {
        let current_value = entry.values.get(entry.current_value_index)?.value.clone();
        let values = entry.values.clone();
        let filtered: Vec<usize> = (0..values.len()).collect();
        let selected = values.iter().position(|v| v.value == current_value).unwrap_or(0);

        Some(Self {
            config_id: entry.config_id.clone(),
            title: entry.title.clone(),
            values,
            current_value,
            query: String::new(),
            selected,
            filtered,
        })
    }

    fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.filtered.len() - 1);
            self.ensure_selectable();
        }
    }

    fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
            self.ensure_selectable();
        }
    }

    fn push_query_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    fn pop_query_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = (0..self.values.len())
            .filter(|&i| {
                let v = &self.values[i];
                v.name.to_lowercase().contains(&q) || v.value.to_lowercase().contains(&q)
            })
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
        self.ensure_selectable();
    }

    fn ensure_selectable(&mut self) {
        while self.filtered.get(self.selected).is_some_and(|&i| self.values[i].is_disabled) {
            if self.selected == 0 {
                break;
            }
            self.selected -= 1;
        }
    }

    fn confirm_selection(&self) -> Option<SettingsChange> {
        let value_idx = *self.filtered.get(self.selected)?;
        let selected = &self.values[value_idx];
        if selected.is_disabled || selected.value == self.current_value {
            return None;
        }
        Some(SettingsChange { config_id: self.config_id.clone(), new_value: selected.value.clone() })
    }

    fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let header = format!(" {} search: {}", self.title, self.query);
        buffer.set_string(
            area.x,
            area.y,
            truncate_str(&header, area.width as usize),
            Style::new().fg(theme.text_secondary),
        );

        if self.filtered.is_empty() {
            buffer.set_string(area.x, area.y + 1, " (no matches found)", Style::new().fg(theme.text_secondary));
            return;
        }

        let list_start = 1usize;
        let max_items = usize::from(area.height).saturating_sub(list_start);
        let offset = if self.selected >= max_items { self.selected - max_items + 1 } else { 0 };

        for i in 0..max_items {
            let idx = offset + i;
            let Some(&value_idx) = self.filtered.get(idx) else {
                break;
            };
            let y = area.y + list_start as u16 + i as u16;
            if y >= area.bottom() {
                break;
            }

            let v = &self.values[value_idx];
            let label = if v.name == v.value { v.name.clone() } else { format!("{} ({})", v.name, v.value) };

            let style = if idx == self.selected {
                Style::new().fg(theme.background).bg(theme.text_primary)
            } else if v.is_disabled {
                Style::new().fg(theme.text_secondary)
            } else {
                Style::new().fg(theme.text_primary)
            };

            buffer.set_string(area.x, y, truncate_str(&label, area.width as usize), style);
        }
    }
}

impl ModelSelector {
    fn new(
        config_id: String,
        items: Vec<SettingsMenuValue>,
        current_selection: &str,
        current_reasoning_effort: Option<&str>,
    ) -> Self {
        let selected_models: HashSet<String> =
            current_selection.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

        let reasoning = current_reasoning_effort.and_then(|s| s.parse().ok());
        let original_models = selected_models.clone();
        let original_reasoning_effort = reasoning;

        let filtered: Vec<usize> = (0..items.len()).collect();
        let focused = items
            .iter()
            .position(|v| !v.is_disabled && selected_models.contains(&v.value))
            .or_else(|| items.iter().position(|v| !v.is_disabled))
            .unwrap_or(0);

        Self {
            config_id,
            all_items: items,
            selected_models,
            original_models,
            query: String::new(),
            focused,
            filtered,
            reasoning_effort: reasoning,
            original_reasoning_effort,
        }
    }

    fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            self.focused = self.focused.checked_sub(1).unwrap_or(self.filtered.len() - 1);
            self.ensure_enabled();
        }
    }

    fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.focused = (self.focused + 1) % self.filtered.len();
            self.ensure_enabled();
        }
    }

    fn ensure_enabled(&mut self) {
        if self.filtered.get(self.focused).is_some_and(|&i| self.all_items[i].is_disabled) {
            // find first non-disabled
            for (offset, &i) in self.filtered.iter().enumerate() {
                if !self.all_items[i].is_disabled {
                    self.focused = offset;
                    return;
                }
            }
        }
    }

    fn toggle_focused(&mut self) {
        if let Some(&value_idx) = self.filtered.get(self.focused) {
            let v = &self.all_items[value_idx];
            if !v.is_disabled && !self.selected_models.remove(&v.value) {
                self.selected_models.insert(v.value.clone());
            }
        }
    }

    fn cycle_reasoning(&mut self) {
        if let Some(&value_idx) = self.filtered.get(self.focused) {
            let v = &self.all_items[value_idx];
            if !v.is_disabled && !v.meta.reasoning_levels.is_empty() {
                self.reasoning_effort = ReasoningEffort::cycle_within(self.reasoning_effort, &v.meta.reasoning_levels);
            }
        }
    }

    fn clamp_reasoning_to_focused(&mut self) {
        if let Some(effort) = self.reasoning_effort
            && let Some(&value_idx) = self.filtered.get(self.focused)
        {
            let v = &self.all_items[value_idx];
            if v.meta.reasoning_levels.is_empty() {
                self.reasoning_effort = None;
            } else {
                self.reasoning_effort = Some(effort.clamp_to(&v.meta.reasoning_levels));
            }
        }
    }

    fn push_query_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    fn pop_query_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = (0..self.all_items.len())
            .filter(|&i| {
                let v = &self.all_items[i];
                v.name.to_lowercase().contains(&q) || v.value.to_lowercase().contains(&q)
            })
            .collect();
        self.focused = self.focused.min(self.filtered.len().saturating_sub(1));
        self.ensure_enabled();
    }

    fn confirm(&self) -> Vec<SettingsChange> {
        let mut changes = Vec::new();
        if !self.selected_models.is_empty() && self.selected_models != self.original_models {
            let joined = self.selected_models.iter().cloned().collect::<Vec<_>>().join(",");
            changes.push(SettingsChange { config_id: self.config_id.clone(), new_value: joined });
        }
        if self.reasoning_effort != self.original_reasoning_effort {
            changes.push(SettingsChange {
                config_id: ConfigOptionId::ReasoningEffort.as_str().to_string(),
                new_value: ReasoningEffort::config_str(self.reasoning_effort).to_string(),
            });
        }
        changes
    }

    fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let header = format!(" Model search: {}", self.query);
        buffer.set_string(
            area.x,
            area.y,
            truncate_str(&header, area.width as usize),
            Style::new().fg(theme.text_secondary),
        );

        let mut line_offset = 1u16;

        // Show selected models summary
        if self.selected_models.is_empty() {
            line_offset += 1;
        } else {
            let names: Vec<&str> = self
                .all_items
                .iter()
                .filter(|item| self.selected_models.contains(&item.value))
                .map(|item| item.name.as_str())
                .collect();
            let selected_text = format!(" Selected: {}", names.join(", "));
            buffer.set_string(
                area.x,
                area.y + line_offset,
                truncate_str(&selected_text, area.width as usize),
                Style::new().fg(theme.text_secondary),
            );
            line_offset += 2;
        }

        if self.filtered.is_empty() {
            buffer.set_string(
                area.x,
                area.y + line_offset,
                " (no matches found)",
                Style::new().fg(theme.text_secondary),
            );
            return;
        }

        let remaining_height = usize::from(area.height).saturating_sub(line_offset as usize);
        if remaining_height == 0 {
            return;
        }

        let mut last_provider: Option<&str> = None;
        let mut row = 0usize;
        let mut list_idx = self.focused.saturating_sub(remaining_height.saturating_sub(1));

        while row < remaining_height && list_idx < self.filtered.len() {
            let value_idx = self.filtered[list_idx];
            let v = &self.all_items[value_idx];
            let y = area.y + line_offset + row as u16;
            if y >= area.bottom() {
                break;
            }

            let provider = provider_key(&v.value);

            if last_provider != Some(provider) {
                last_provider = Some(provider);
                let heading = provider_label(&v.name, provider);
                buffer.set_string(
                    area.x,
                    y,
                    truncate_str(&heading, area.width as usize),
                    Style::new().fg(theme.heading),
                );
                row += 1;
                if row >= remaining_height {
                    break;
                }
                let model_y = area.y + line_offset + row as u16;
                if model_y >= area.bottom() {
                    break;
                }
                render_model_row(self, v, list_idx, model_y, area, buffer, theme);
                row += 1;
                list_idx += 1;
                continue;
            }

            render_model_row(self, v, list_idx, y, area, buffer, theme);
            row += 1;
            list_idx += 1;
        }
    }
}

fn provider_key(value: &str) -> &str {
    if let Some(provider) = value.strip_prefix("__unavailable:") {
        return provider;
    }
    value.split_once(':').map_or("Other", |(p, _)| p)
}

fn provider_label(name: &str, key: &str) -> String {
    if let Some((provider, _)) = name.split_once(" / ") {
        return provider.to_string();
    }
    if key.is_empty() {
        return "Other".to_string();
    }
    let mut chars = key.chars();
    let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let rest = chars.as_str().to_lowercase();
    format!("{first}{rest}")
}

fn model_label(name: &str) -> &str {
    name.split_once(" / ").map_or(name, |(_, model)| model)
}

fn capability_tags(supports_image: bool, supports_audio: bool) -> &'static str {
    match (supports_image, supports_audio) {
        (true, true) => "img  audio",
        (true, false) => "img",
        (false, true) => "audio",
        (false, false) => "",
    }
}

fn reasoning_bar(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> String {
    let current_idx = effort.and_then(|e| levels.iter().position(|&l| l == e)).unwrap_or(usize::MAX);
    let parts: Vec<String> = levels
        .iter()
        .enumerate()
        .map(
            |(i, _level)| {
                if i <= current_idx && current_idx != usize::MAX { "■".to_string() } else { "·".to_string() }
            },
        )
        .collect();
    let name = ReasoningEffort::config_str(effort);
    format!("{} [{}]", name, parts.join(""))
}

fn extract_reasoning_effort(options: &[SessionConfigOption]) -> Option<String> {
    options.iter().find(|opt| opt.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str()).and_then(|opt| match &opt
        .kind
    {
        SessionConfigKind::Select(select) => {
            let value = select.current_value.0.trim();
            (!value.is_empty() && value != "none").then(|| value.to_string())
        }
        _ => None,
    })
}

fn render_model_row(
    selector: &ModelSelector,
    v: &SettingsMenuValue,
    list_idx: usize,
    y: u16,
    area: Rect,
    buffer: &mut Buffer,
    theme: &Theme,
) {
    if v.is_disabled {
        let reason = v.description.as_deref().and_then(|d| d.strip_prefix("Unavailable: ")).unwrap_or("unavailable");
        let label = format!("    {model}  {reason}", model = model_label(&v.name));
        buffer.set_string(area.x, y, truncate_str(&label, area.width as usize), Style::new().fg(theme.text_secondary));
        return;
    }

    let check = if selector.selected_models.contains(&v.value) { "[x] " } else { "[ ] " };
    let model = model_label(&v.name);

    let is_focused = list_idx == selector.focused;
    let style = if is_focused {
        Style::new().fg(theme.background).bg(theme.text_primary)
    } else {
        Style::new().fg(theme.text_primary)
    };

    let mut label = format!("{check}{model}");
    if is_focused {
        if !v.meta.reasoning_levels.is_empty() {
            label.push_str("    ");
            label.push_str(&reasoning_bar(selector.reasoning_effort, &v.meta.reasoning_levels));
        }
        let caps = capability_tags(v.meta.supports_image, v.meta.supports_audio);
        if !caps.is_empty() {
            label.push_str("    ");
            label.push_str(caps);
        }
    }

    buffer.set_string(area.x + 2, y, truncate_str(&label, area.width as usize - 2), style);
}

fn process_config_changes(changes: Vec<SettingsChange>) -> Vec<SettingsOverlayMessage> {
    let mut messages = Vec::new();
    for change in changes {
        if change.config_id == THEME_CONFIG_ID {
            messages.push(SettingsOverlayMessage::SetTheme(change.new_value));
        } else {
            messages
                .push(SettingsOverlayMessage::SetConfigOption { config_id: change.config_id, value: change.new_value });
        }
    }
    messages
}

fn truncate_str(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let width = UnicodeWidthStr::width(s);
    if width <= max_width {
        return s.to_string();
    }
    let mut result = String::with_capacity(max_width);
    let mut current_width = 0;
    for c in s.chars() {
        let cw = UnicodeWidthStr::width(c.to_string().as_str());
        if current_width + cw > max_width.saturating_sub(2) {
            result.push('…');
            break;
        }
        result.push(c);
        current_width += cw;
    }
    result
}

fn server_status_summary(statuses: &[McpServerStatusEntry]) -> String {
    if statuses.is_empty() {
        return "none".to_string();
    }
    let mut connected = 0usize;
    let mut connecting = 0usize;
    let mut authenticating = 0usize;
    let mut needs_auth = 0usize;
    let mut failed = 0usize;
    for entry in statuses {
        match &entry.status {
            McpServerStatus::Connected { .. } => connected += 1,
            McpServerStatus::Connecting => connecting += 1,
            McpServerStatus::Authenticating => authenticating += 1,
            McpServerStatus::NeedsOAuth => needs_auth += 1,
            McpServerStatus::Failed { .. } => failed += 1,
        }
    }
    let parts: Vec<String> = [
        (connected, "connected"),
        (connecting, "connecting"),
        (authenticating, "authenticating"),
        (needs_auth, "needs auth"),
        (failed, "failed"),
    ]
    .iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    if parts.is_empty() { "none".to_string() } else { parts.join(", ") }
}

fn build_provider_login_entries(methods: &[AuthMethod]) -> Vec<ProviderLoginEntry> {
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

fn provider_login_summary(entries: &[ProviderLoginEntry]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use acp_utils::config_meta::ConfigOptionMeta;
    use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};

    fn sel(id: &str, name: &str, current: &str, values: &[(&str, &str)]) -> SessionConfigOption {
        let options: Vec<SessionConfigSelectOption> =
            values.iter().map(|(v, n)| SessionConfigSelectOption::new((*v).to_string(), (*n).to_string())).collect();
        SessionConfigOption::select(id.to_string(), name.to_string(), current.to_string(), options)
    }

    #[test]
    fn menu_builds_entries_from_config_options() {
        let opts = vec![
            sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o"), ("claude", "Claude")]),
            sel("mode", "Mode", "code", &[("code", "Code"), ("chat", "Chat")]),
        ];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert_eq!(overlay.menu.entries.len(), 2);
        assert_eq!(overlay.menu.entries[0].config_id, "model");
        assert_eq!(overlay.menu.entries[0].current_value_index, 0);
        assert_eq!(overlay.menu.entries[1].config_id, "mode");
    }

    #[test]
    fn menu_finds_current_value() {
        let opts =
            vec![sel("model", "Model", "claude", &[("gpt-4o", "GPT-4o"), ("claude", "Claude"), ("llama", "Llama")])];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert_eq!(overlay.menu.entries[0].current_value_index, 1);
    }

    #[test]
    fn menu_navigation_wraps() {
        let opts = vec![
            sel("a", "A", "v1", &[("v1", "V1")]),
            sel("b", "B", "v1", &[("v1", "V1")]),
            sel("c", "C", "v1", &[("v1", "V1")]),
        ];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert_eq!(overlay.menu.selected, 0);

        overlay.on_key(KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE));
        assert_eq!(overlay.menu.selected, 2);

        overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
        assert_eq!(overlay.menu.selected, 0);
    }

    #[test]
    fn menu_skips_empty_values() {
        let empty = SessionConfigOption::select("x", "X", "v", Vec::<SessionConfigSelectOption>::new());
        let opts = vec![empty, sel("model", "Model", "a", &[("a", "A")])];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert_eq!(overlay.menu.entries.len(), 1);
        assert_eq!(overlay.menu.entries[0].config_id, "model");
    }

    #[test]
    fn menu_excludes_reasoning_effort() {
        let opts = vec![
            sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
            sel("reasoning_effort", "Reasoning", "high", &[("none", "None"), ("low", "Low"), ("high", "High")]),
        ];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert!(overlay.menu.entries.iter().any(|e| e.config_id == "model"));
        assert!(!overlay.menu.entries.iter().any(|e| e.config_id == "reasoning_effort"));
    }

    #[test]
    fn multi_select_detected_from_meta() {
        let mut opt = sel("model", "Model", "a", &[("a", "A"), ("b", "B")]);
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        assert!(overlay.menu.entries[0].multi_select);
    }

    #[test]
    fn multi_select_with_comma_shows_model_names() {
        let mut opt = sel("model", "Model", "a,b", &[("a", "Alpha"), ("b", "Beta")]);
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        let display = overlay.menu.entries[0].display_name.as_deref().unwrap();
        assert!(display.contains("Alpha"), "display: {display}");
        assert!(display.contains("Beta"), "display: {display}");
    }

    #[test]
    fn esc_closes_overlay_from_menu() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        let msgs = overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(msgs.as_slice(), [SettingsOverlayMessage::Close]));
    }

    #[test]
    fn enter_opens_picker_for_single_select() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(overlay.active_pane, ActivePane::Picker(_)));
    }

    #[test]
    fn enter_opens_model_selector_for_multi_select() {
        let mut opt = sel("model", "Model", "a", &[("a", "A"), ("b", "B")]);
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(overlay.active_pane, ActivePane::ModelSelector(_)));
    }

    #[test]
    fn picker_esc_returns_to_menu() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(overlay.active_pane, ActivePane::Picker(_)));
        overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(overlay.active_pane, ActivePane::Menu));
    }

    #[test]
    fn picker_confirm_returns_set_config_option() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        // Navigate down to different value
        overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
        let msgs = overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        match msgs.as_slice() {
            [SettingsOverlayMessage::SetConfigOption { config_id, value }] => {
                assert_eq!(config_id, "model");
                assert_eq!(value, "b");
            }
            other => panic!("expected SetConfigOption, got: {other:?}"),
        }
    }

    #[test]
    fn picker_confirm_applies_change_to_menu() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
        assert_eq!(overlay.menu.entries[0].current_value_index, 1);
    }

    #[test]
    fn picker_confirm_no_change_returns_empty() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        let msgs = overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(msgs.is_empty());
    }

    #[test]
    fn picker_query_filters_by_name() {
        let opts = vec![sel(
            "model",
            "Model",
            "gpt",
            &[
                ("openrouter:gpt-4o", "GPT-4o"),
                ("openrouter:claude", "Claude Sonnet"),
                ("openrouter:gemini", "Gemini Pro"),
            ],
        )];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        for c in "gem".chars() {
            overlay.on_key(KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        if let ActivePane::Picker(ref picker) = overlay.active_pane {
            assert_eq!(picker.filtered.len(), 1);
            let idx = picker.filtered[0];
            assert!(picker.values[idx].name.contains("Gemini"));
        } else {
            panic!("expected picker");
        }
    }

    #[test]
    fn picker_disabled_option_not_selectable() {
        let opt = SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![SessionConfigSelectOption::new("a", "A"), SessionConfigSelectOption::new("b", "B")],
        );
        let mut values = opt.clone();
        // set b as disabled
        if let SessionConfigKind::Select(ref mut select) = values.kind {
            select.options = SessionConfigSelectOptions::Ungrouped(vec![
                SessionConfigSelectOption::new("a", "A"),
                SessionConfigSelectOption::new("b".to_string(), "B".to_string())
                    .description("Unavailable: need key".to_string()),
            ]);
        }
        let overlay = SettingsOverlay::new(&[values], vec![], vec![]);
        // Just check the entry has the disabled flag
        assert!(overlay.menu.entries[0].values[1].is_disabled);
    }

    #[test]
    fn model_selector_enter_toggles() {
        let mut opt = sel(
            "model",
            "Model",
            "",
            &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
        );
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(overlay.active_pane, ActivePane::ModelSelector(_)));

        // Toggle first model
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
            assert_eq!(selector.selected_models.len(), 1);
        }

        // Toggle again
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
            assert!(selector.selected_models.is_empty());
        }
    }

    #[test]
    fn model_selector_esc_returns_to_menu() {
        let mut opt = sel("model", "Model", "", &[("a:m1", "A / M1")]);
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        let msgs = overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        assert!(matches!(overlay.active_pane, ActivePane::Menu));
        assert!(msgs.is_empty());
    }

    #[test]
    fn model_selector_returns_changes_on_esc() {
        let mut opt = sel(
            "model",
            "Model",
            "",
            &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
        );
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));

        let msgs = overlay.on_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        match msgs.as_slice() {
            [SettingsOverlayMessage::SetConfigOption { config_id, value }] => {
                assert_eq!(config_id, "model");
                assert!(value.contains("anthropic:opus"));
            }
            other => panic!("expected SetConfigOption, got: {other:?}"),
        }
    }

    #[test]
    fn model_selector_preselects_from_current_value() {
        let mut opt = sel(
            "model",
            "Model",
            "anthropic:opus,anthropic:sonnet",
            &[("anthropic:opus", "Anthropic / Opus"), ("anthropic:sonnet", "Anthropic / Sonnet")],
        );
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
            assert_eq!(selector.selected_models.len(), 2);
        }
    }

    #[test]
    fn update_config_options_refreshes_menu() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));
        overlay.on_key(KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE));

        let new_opts = vec![sel("model", "Model", "b", &[("a", "A"), ("b", "B")])];
        overlay.update_config_options(&new_opts);
        assert_eq!(overlay.menu.entries[0].current_value_index, 1);
        assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
    }

    #[test]
    fn reasoning_effort_extracted_from_options() {
        let opts = vec![
            sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
            sel("reasoning_effort", "Reasoning", "high", &[("none", "None"), ("low", "Low"), ("high", "High")]),
        ];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert_eq!(overlay.current_reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn reasoning_effort_none_filtered_out() {
        let opts = vec![
            sel("model", "Model", "gpt-4o", &[("gpt-4o", "GPT-4o")]),
            sel("reasoning_effort", "Reasoning", "none", &[("none", "None"), ("low", "Low")]),
        ];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        assert_eq!(overlay.current_reasoning_effort, None);
    }

    #[test]
    fn empty_config_options_creates_empty_menu() {
        let overlay = SettingsOverlay::new(&[], vec![], vec![]);
        assert!(overlay.menu.entries.is_empty());
    }

    #[test]
    fn small_terminal_renders_placeholder() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
        let overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 5, 2);
        let mut buffer = Buffer::empty(area);
        overlay.render(area, &mut buffer, &theme);
        let mut text = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                text.push(buffer.cell((x, y)).map_or(' ', |c| c.symbol().chars().next().unwrap_or(' ')));
            }
        }
        assert!(text.contains("term"), "text: {text}");
    }

    #[test]
    fn truncate_str_handles_unicode() {
        assert_eq!(truncate_str("hello", 3), "h…");
        assert_eq!(truncate_str("a界b", 3), "a…");
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("", 5), "");
        assert_eq!(truncate_str("abc", 0), "");
    }

    #[test]
    fn provider_label_handles_slash_format() {
        assert_eq!(provider_label("Anthropic / Claude Sonnet", "anthropic"), "Anthropic");
    }

    #[test]
    fn provider_label_capitalizes_key() {
        assert_eq!(provider_label("OpenRouter / GPT-4o", "openrouter"), "OpenRouter");
    }

    #[test]
    fn model_label_extracts_after_slash() {
        assert_eq!(model_label("Anthropic / Claude Sonnet"), "Claude Sonnet");
        assert_eq!(model_label("GPT-4o"), "GPT-4o");
    }

    #[test]
    fn reasoning_bar_shows_correct_indicators() {
        use utils::ReasoningEffort::*;
        let levels = &[Low, Medium, High];
        let bar = reasoning_bar(Some(Medium), levels);
        assert!(bar.contains("■"), "bar: {bar}");
        assert!(bar.contains("·"), "bar: {bar}");
        assert!(bar.contains("medium"), "bar: {bar}");

        let bar_none = reasoning_bar(None, levels);
        assert!(!bar_none.contains("■"), "bar_none: {bar_none}");
        assert!(bar_none.contains("none"), "bar_none: {bar_none}");
    }

    #[test]
    fn capability_tags_all_combinations() {
        assert_eq!(capability_tags(false, false), "");
        assert_eq!(capability_tags(true, false), "img");
        assert_eq!(capability_tags(false, true), "audio");
        assert_eq!(capability_tags(true, true), "img  audio");
    }

    #[test]
    fn provider_key_handles_unavailable_prefix() {
        assert_eq!(provider_key("__unavailable:moonshot"), "moonshot");
    }

    #[test]
    fn update_options_with_mcp_and_theme_entries() {
        // Theme and MCP entries are added by App, not the overlay itself
        let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        let new_opts = vec![
            sel("model", "Model", "b", &[("a", "A"), ("b", "B")]),
            sel("mode", "Mode", "code", &[("code", "Code")]),
        ];
        overlay.update_config_options(&new_opts);
        assert_eq!(overlay.menu.entries.len(), 2);
        assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
    }

    #[test]
    fn apply_change_updates_menu_entry() {
        let opts = vec![sel("model", "Model", "a", &[("a", "A"), ("b", "B")])];
        let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);
        overlay.apply_change(&SettingsChange { config_id: "model".to_string(), new_value: "b".to_string() });
        assert_eq!(overlay.menu.entries[0].current_raw_value, "b");
        assert_eq!(overlay.menu.entries[0].current_value_index, 1);
    }

    #[test]
    fn model_selector_query_filters() {
        let mut opt = sel(
            "model",
            "Model",
            "",
            &[
                ("anthropic:opus", "Anthropic / Opus"),
                ("openai:gpt-4o", "OpenAI / GPT-4o"),
                ("google:gemini", "Google / Gemini"),
            ],
        );
        opt = opt.meta(ConfigOptionMeta { multi_select: true }.into_meta());
        let mut overlay = SettingsOverlay::new(&[opt], vec![], vec![]);
        overlay.on_key(KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE));

        for c in "gpt".chars() {
            overlay.on_key(KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        if let ActivePane::ModelSelector(ref selector) = overlay.active_pane {
            assert_eq!(selector.filtered.len(), 1);
            let idx = selector.filtered[0];
            assert!(selector.all_items[idx].name.contains("GPT"), "name: {}", selector.all_items[idx].name);
        } else {
            panic!("expected model selector");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_overlay_cancels_pending_elicitation() {
        tokio::task::LocalSet::new()
            .run_until(async {
                use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationAction, ElicitationParams};
                use acp_utils::testing::test_connection;

                let opts = vec![sel("model", "Model", "a", &[("a", "A")])];
                let mut overlay = SettingsOverlay::new(&opts, vec![], vec![]);

                let (cx, mut peer) = test_connection().await;
                let (responder, response_rx) = peer.fake_elicitation(&cx).await;
                overlay.on_elicitation_request(
                    ElicitationParams {
                        server_name: "test".into(),
                        request: CreateElicitationRequestParams::UrlElicitationParams {
                            meta: None,
                            message: String::new(),
                            url: "https://example.com".into(),
                            elicitation_id: "el-1".into(),
                        },
                    },
                    responder,
                );

                drop(overlay);

                let response = response_rx.await.unwrap();
                assert_eq!(response.action, ElicitationAction::Cancel);
            })
            .await;
    }
}
