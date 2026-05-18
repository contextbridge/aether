use super::menu::{SettingMenuMessage, SettingsMenu};
use super::picker::{SettingsPicker, SettingsPickerMessage};
use crate::components::elicitation_form::{ElicitationForm, ElicitationMessage, ElicitationUi, UrlPrompt};
use crate::components::model_selector::{ModelEntry, ModelSelector, ModelSelectorMessage};
use crate::components::provider_login::{ProviderLoginMessage, ProviderLoginOverlay};
use crate::components::server_status::{ServerStatusMessage, ServerStatusOverlay};
use acp_utils::config_option_id::ConfigOptionId;
use acp_utils::notifications::{
    ElicitationParams, ElicitationResponse, McpServerStatusEntry, UrlElicitationCompleteParams,
};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{self as acp, SessionConfigKind, SessionConfigOption};
use tui::Panel;
use tui::{Component, Cursor, Event, Frame, Line, ViewContext};
use unicode_width::UnicodeWidthStr;

const MIN_HEIGHT: usize = 3;
const MIN_WIDTH: usize = 6;
/// Panel chrome above child content: top border (1) + blank line (1).
const TOP_CHROME: usize = 2;
/// Panel left border width: "│ " = 2 chars.
const BORDER_LEFT_WIDTH: usize = 2;
/// Gap between children inside the container.
const GAP: usize = 1;

enum SettingsPane {
    Menu,
    Picker(SettingsPicker),
    ModelSelector(ModelSelector),
    ServerStatus(ServerStatusOverlay),
    ProviderLogin(ProviderLoginOverlay),
}

pub struct SettingsOverlay {
    menu: SettingsMenu,
    active_pane: SettingsPane,
    pending_elicitation: Option<ElicitationForm>,
    server_statuses: Vec<McpServerStatusEntry>,
    auth_methods: Vec<acp::AuthMethod>,
    current_reasoning_effort: Option<String>,
}

#[derive(Debug)]
pub enum SettingsMessage {
    Close,
    SetConfigOption { config_id: String, value: String },
    SetTheme(tui::Theme),
    AuthenticateServer(String),
    AuthenticateProvider(String),
}

impl SettingsOverlay {
    pub fn new(
        menu: SettingsMenu,
        server_statuses: Vec<McpServerStatusEntry>,
        auth_methods: Vec<acp::AuthMethod>,
    ) -> Self {
        Self {
            menu,
            active_pane: SettingsPane::Menu,
            pending_elicitation: None,
            server_statuses,
            auth_methods,
            current_reasoning_effort: None,
        }
    }

    pub fn with_reasoning_effort_from_options(mut self, options: &[SessionConfigOption]) -> Self {
        self.current_reasoning_effort = Self::extract_reasoning_effort(options);
        self
    }

    fn extract_reasoning_effort(options: &[SessionConfigOption]) -> Option<String> {
        options.iter().find(|opt| opt.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str()).and_then(|opt| {
            match &opt.kind {
                SessionConfigKind::Select(select) => {
                    let value = select.current_value.0.trim();
                    (!value.is_empty() && value != "none").then(|| value.to_string())
                }
                _ => None,
            }
        })
    }

    pub fn build_frame(&mut self, ctx: &ViewContext) -> Frame {
        let cursor = if self.has_picker() {
            Cursor::visible(self.cursor_row_offset(), self.cursor_col())
        } else {
            Cursor::hidden()
        };
        self.render(ctx).with_cursor(cursor)
    }

    pub fn update_child_viewport(&mut self, max_height: usize) {
        match &mut self.active_pane {
            SettingsPane::ModelSelector(ms) => ms.update_viewport(max_height),
            SettingsPane::Picker(p) => p.update_viewport(max_height),
            _ => {}
        }
    }

    pub fn update_config_options(&mut self, options: &[SessionConfigOption]) {
        self.current_reasoning_effort = Self::extract_reasoning_effort(options);
        self.menu.update_options(options);
        super::decorate_menu(&mut self.menu, &self.server_statuses, &self.auth_methods);
    }

    pub fn update_server_statuses(&mut self, statuses: Vec<McpServerStatusEntry>) {
        self.server_statuses = statuses;
        if let SettingsPane::ServerStatus(ref mut overlay) = self.active_pane {
            overlay.update_entries(self.server_statuses.clone());
        }
    }

    pub fn update_auth_methods(&mut self, auth_methods: Vec<acp::AuthMethod>) {
        self.auth_methods = auth_methods;
        super::decorate_menu(&mut self.menu, &self.server_statuses, &self.auth_methods);
        let login_entries = super::build_login_entries(&self.auth_methods);
        if let SettingsPane::ProviderLogin(ref mut overlay) = self.active_pane {
            overlay.replace_entries(login_entries);
        }
    }

    pub fn on_authenticate_started(&mut self, method_id: &str) {
        if let SettingsPane::ProviderLogin(ref mut overlay) = self.active_pane {
            overlay.set_authenticating(method_id);
        }
    }

    pub fn on_authenticate_complete(&mut self, method_id: &str) {
        if let SettingsPane::ProviderLogin(ref mut overlay) = self.active_pane {
            overlay.set_logged_in(method_id);
        }
    }

    pub fn on_authenticate_failed(&mut self, method_id: &str) {
        if let SettingsPane::ProviderLogin(ref mut overlay) = self.active_pane {
            overlay.reset_to_needs_login(method_id);
        }
    }

    pub fn on_elicitation_request(&mut self, params: ElicitationParams, responder: Responder<ElicitationResponse>) {
        if let Some(mut prior) = self.pending_elicitation.take() {
            prior.cancel_pending();
        }
        self.pending_elicitation = Some(ElicitationForm::from_params(params, responder));
    }

    pub fn on_url_elicitation_complete(&mut self, params: &UrlElicitationCompleteParams) {
        if self.pending_elicitation.as_mut().is_some_and(|form| form.accept_url_complete(params)) {
            self.pending_elicitation = None;
        }
    }

    pub fn needs_mouse_capture(&self) -> bool {
        self.pending_elicitation.is_none()
    }

    pub fn cursor_col(&self) -> usize {
        match &self.active_pane {
            SettingsPane::Picker(picker) => {
                let prefix = format!("  {} search: ", picker.title);
                BORDER_LEFT_WIDTH + UnicodeWidthStr::width(prefix.as_str()) + UnicodeWidthStr::width(picker.query())
            }
            SettingsPane::ModelSelector(selector) => {
                let prefix = "  Model search: ";
                BORDER_LEFT_WIDTH + UnicodeWidthStr::width(prefix) + UnicodeWidthStr::width(selector.query())
            }
            _ => 0,
        }
    }

    /// Returns the row offset of the cursor within the overlay (0-indexed from top of overlay).
    /// Only meaningful when a search-based submenu is open (picker or model selector).
    pub fn cursor_row_offset(&self) -> usize {
        match &self.active_pane {
            SettingsPane::Picker(_) | SettingsPane::ModelSelector(_) => TOP_CHROME,
            _ => 0,
        }
    }

    pub fn has_picker(&self) -> bool {
        matches!(self.active_pane, SettingsPane::Picker(_))
    }

    fn footer_line(&self, context: &ViewContext) -> Line {
        if let Some(form) = &self.pending_elicitation {
            return match &form.ui {
                ElicitationUi::Url(_) => shortcut_footer(
                    &[("[Enter]", " Open Browser  "), ("[C]", " Copy Link  "), ("[Esc]", " Cancel")],
                    context,
                ),
                ElicitationUi::Form(_) => shortcut_footer(&[("[Enter]", " Submit  "), ("[Esc]", " Cancel")], context),
            };
        }
        let text = match &self.active_pane {
            SettingsPane::ModelSelector(selector) => {
                let effort = utils::ReasoningEffort::config_str(selector.reasoning_effort());
                format!("[Space/Enter] Toggle  [Tab] Effort: {effort}  [Esc] Done")
            }
            SettingsPane::Picker(_) => "[Enter] Confirm  [Esc] Back".to_string(),
            SettingsPane::ServerStatus(_) => "[Enter] Authenticate OAuth servers  [Esc] Back".to_string(),
            SettingsPane::ProviderLogin(_) => "[Enter] Authenticate  [Esc] Back".to_string(),
            SettingsPane::Menu => "[Enter] Select  [Esc] Close".to_string(),
        };
        Line::new(text)
    }
}

fn shortcut_footer(parts: &[(&str, &str)], context: &ViewContext) -> Line {
    let mut line = Line::default();
    for (shortcut, label) in parts {
        line.push_with_style(*shortcut, tui::Style::fg(context.theme.warning()).bold());
        line.push_with_style(*label, tui::Style::fg(context.theme.muted()));
    }
    line
}

impl Component for SettingsOverlay {
    type Message = SettingsMessage;

    #[allow(clippy::too_many_lines)]
    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        if !matches!(event, Event::Key(_) | Event::Mouse(_)) {
            return None;
        }

        if let Some(form) = self.pending_elicitation.as_mut() {
            let outcome = form.on_event(event).await;
            if outcome.unwrap_or_default().into_iter().any(|msg| matches!(msg, ElicitationMessage::Responded)) {
                self.pending_elicitation = None;
            }
            return Some(vec![]);
        }

        match &mut self.active_pane {
            SettingsPane::ServerStatus(overlay) => {
                let outcome = overlay.on_event(event).await;
                match outcome.unwrap_or_default().into_iter().next() {
                    Some(ServerStatusMessage::Close) => {
                        self.active_pane = SettingsPane::Menu;
                        Some(vec![])
                    }
                    Some(ServerStatusMessage::Authenticate(name)) => {
                        Some(vec![SettingsMessage::AuthenticateServer(name)])
                    }
                    None => Some(vec![]),
                }
            }
            SettingsPane::ProviderLogin(overlay) => {
                let outcome = overlay.on_event(event).await;
                match outcome.unwrap_or_default().into_iter().next() {
                    Some(ProviderLoginMessage::Close) => {
                        self.active_pane = SettingsPane::Menu;
                        Some(vec![])
                    }
                    Some(ProviderLoginMessage::Authenticate(method_id)) => {
                        Some(vec![SettingsMessage::AuthenticateProvider(method_id)])
                    }
                    None => Some(vec![]),
                }
            }
            SettingsPane::ModelSelector(selector) => {
                let outcome = selector.on_event(event).await;
                match outcome.unwrap_or_default().into_iter().next() {
                    Some(ModelSelectorMessage::Done(changes)) => {
                        self.active_pane = SettingsPane::Menu;
                        if changes.is_empty() { Some(vec![]) } else { Some(super::process_config_changes(changes)) }
                    }
                    None => Some(vec![]),
                }
            }
            SettingsPane::Picker(picker) => {
                let outcome = picker.on_event(event).await;
                match outcome.unwrap_or_default().into_iter().next() {
                    Some(SettingsPickerMessage::Close) => {
                        self.active_pane = SettingsPane::Menu;
                        Some(vec![])
                    }
                    Some(SettingsPickerMessage::ApplySelection(change)) => {
                        if let Some(change) = change {
                            self.menu.apply_change(&change);
                            self.active_pane = SettingsPane::Menu;
                            Some(super::process_config_changes(vec![change]))
                        } else {
                            self.active_pane = SettingsPane::Menu;
                            Some(vec![])
                        }
                    }
                    None => Some(vec![]),
                }
            }
            SettingsPane::Menu => {
                let outcome = self.menu.on_event(event).await;
                let messages = outcome.unwrap_or_default();
                match messages.as_slice() {
                    [SettingMenuMessage::CloseAll] => Some(vec![SettingsMessage::Close]),
                    [SettingMenuMessage::OpenSelectedPicker] => {
                        if let Some(picker) = self.menu.selected_entry().and_then(SettingsPicker::from_entry) {
                            self.active_pane = SettingsPane::Picker(picker);
                        }
                        Some(vec![])
                    }
                    [SettingMenuMessage::OpenModelSelector] => {
                        if let Some(entry) = self.menu.selected_entry() {
                            let current = Some(entry.current_raw_value.as_str()).filter(|v| !v.is_empty());
                            let items: Vec<ModelEntry> = entry
                                .values
                                .iter()
                                .map(|v| ModelEntry {
                                    value: v.value.clone(),
                                    name: v.name.clone(),
                                    reasoning_levels: v.meta.reasoning_levels.clone(),
                                    supports_image: v.meta.supports_image,
                                    supports_audio: v.meta.supports_audio,
                                    disabled_reason: if v.is_disabled {
                                        v.description
                                            .as_ref()
                                            .map(|d| d.strip_prefix("Unavailable: ").unwrap_or(d).to_string())
                                    } else {
                                        None
                                    },
                                })
                                .collect();
                            self.active_pane = SettingsPane::ModelSelector(ModelSelector::new(
                                items,
                                entry.config_id.clone(),
                                current,
                                self.current_reasoning_effort.as_deref(),
                            ));
                        }
                        Some(vec![])
                    }
                    [SettingMenuMessage::OpenMcpServers] => {
                        self.active_pane =
                            SettingsPane::ServerStatus(ServerStatusOverlay::new(self.server_statuses.clone()));
                        Some(vec![])
                    }
                    [SettingMenuMessage::OpenProviderLogins] => {
                        let entries = super::build_login_entries(&self.auth_methods);
                        self.active_pane = SettingsPane::ProviderLogin(ProviderLoginOverlay::new(entries));
                        Some(vec![])
                    }
                    _ => Some(vec![]),
                }
            }
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let height = (context.size.height.saturating_sub(1)) as usize;
        let width = context.size.width as usize;
        if height < MIN_HEIGHT || width < MIN_WIDTH {
            return Frame::new(vec![Line::new("(terminal too small)")]);
        }

        let footer = self.footer_line(context);
        #[allow(clippy::cast_possible_truncation)]
        let child_max_height = height.saturating_sub(4) as u16;
        let inner_w = Panel::inner_width(context.size.width);
        let child_context = context.with_size((inner_w, child_max_height));

        let child_lines = match &mut self.active_pane {
            SettingsPane::ServerStatus(overlay) => overlay.render(&child_context).into_lines(),
            SettingsPane::ProviderLogin(overlay) => overlay.render(&child_context).into_lines(),
            SettingsPane::ModelSelector(selector) => selector.render(&child_context).into_lines(),
            SettingsPane::Picker(picker) => picker.render(&child_context).into_lines(),
            SettingsPane::Menu => self.menu.render(&child_context).into_lines(),
        };

        let mut container =
            Panel::new(context.theme.muted()).title(" Configuration ").footer_line(footer).fill_height(height).gap(GAP);
        container.push(child_lines);
        if let Some(form) = self.pending_elicitation.as_mut() {
            container.push(render_settings_elicitation(form, &child_context).into_lines());
        }
        container.render(context)
    }
}

fn render_settings_elicitation(form: &mut ElicitationForm, context: &ViewContext) -> Frame {
    if let ElicitationUi::Url(prompt) = &form.ui {
        render_settings_url_prompt(prompt, context)
    } else {
        form.render(context)
    }
}

fn render_settings_url_prompt(prompt: &UrlPrompt, context: &ViewContext) -> Frame {
    let mut lines = vec![
        Line::new(format!("    ↳ Open browser to authorize {} MCP access", prompt.server_name)),
        Line::new(format!("      {}", prompt.host.as_deref().unwrap_or(&prompt.url))),
    ];

    if !prompt.warnings.is_empty() {
        for warning in &prompt.warnings {
            lines.push(Line::with_style(format!("      {warning}"), tui::Style::fg(context.theme.warning())));
        }
    }

    if let Some(message) = &prompt.copy_message {
        lines.push(Line::with_style(format!("      {message}"), tui::Style::fg(context.theme.muted())));
    }

    if let Some(error) = &prompt.launch_error {
        lines.push(Line::styled(format!("      {error}"), context.theme.error()));
    }

    Frame::new(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::provider_login::ProviderLoginStatus;
    use crate::settings::types::SettingsMenuEntryKind;
    use crate::test_helpers::key;
    use acp_utils::config_option_id::THEME_CONFIG_ID;
    use acp_utils::notifications::{McpServerAuthCapability, McpServerStatus};
    use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectOption};
    use tui::KeyCode;

    fn select_opt(id: &'static str, name: &'static str) -> SessionConfigSelectOption {
        SessionConfigSelectOption::new(id, name)
    }

    fn config_select(
        id: &'static str,
        label: &'static str,
        current: &'static str,
        opts: Vec<SessionConfigSelectOption>,
    ) -> SessionConfigOption {
        SessionConfigOption::select(id, label, current, opts)
    }

    fn provider_model_options(multi_select_model: bool) -> Vec<SessionConfigOption> {
        let provider = config_select(
            "provider",
            "Provider",
            "openrouter",
            vec![select_opt("openrouter", "OpenRouter"), select_opt("ollama", "Ollama")],
        );
        let mut model = config_select(
            "model",
            "Model",
            "gpt-4o",
            vec![select_opt("gpt-4o", "GPT-4o"), select_opt("claude", "Claude")],
        );
        if multi_select_model {
            let mut meta = serde_json::Map::new();
            meta.insert("multi_select".to_string(), serde_json::Value::Bool(true));
            model = model.meta(meta);
        }
        vec![provider, model]
    }

    fn make_menu() -> SettingsMenu {
        SettingsMenu::from_config_options(&provider_model_options(false))
    }

    fn make_multi_select_menu() -> SettingsMenu {
        SettingsMenu::from_config_options(&provider_model_options(true))
    }

    fn make_server_statuses() -> Vec<McpServerStatusEntry> {
        vec![
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
            McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)
                .with_auth_capability(McpServerAuthCapability::OAuth),
        ]
    }

    fn make_auth_methods() -> Vec<acp::AuthMethod> {
        vec![
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("anthropic", "Anthropic")),
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("openrouter", "OpenRouter")),
        ]
    }

    async fn send_keys(overlay: &mut SettingsOverlay, codes: &[KeyCode]) {
        for code in codes {
            overlay.on_event(&key(*code)).await;
        }
    }

    fn render_footer(overlay: &mut SettingsOverlay) -> String {
        let context = ViewContext::new((80, 24));
        let height = (context.size.height.saturating_sub(1)) as usize;
        overlay.update_child_viewport(height.saturating_sub(4));
        let frame = overlay.render(&context);
        let lines = frame.lines();
        lines[lines.len() - 2].plain_text()
    }

    fn new_overlay() -> SettingsOverlay {
        SettingsOverlay::new(make_menu(), vec![], vec![])
    }

    fn new_multi_select_overlay() -> SettingsOverlay {
        SettingsOverlay::new(make_multi_select_menu(), vec![], vec![])
    }

    /// Open the model selector on a multi-select overlay (Down to model row, Enter to open).
    async fn open_model_selector() -> SettingsOverlay {
        let mut overlay = new_multi_select_overlay();
        send_keys(&mut overlay, &[KeyCode::Down, KeyCode::Enter]).await;
        assert!(render_footer(&mut overlay).contains("Toggle"));
        overlay
    }

    fn assert_footer_contains(overlay: &mut SettingsOverlay, needle: &str) {
        let footer = render_footer(overlay);
        assert!(footer.contains(needle), "expected footer to contain '{needle}'; got: {footer}");
    }

    fn has_entry_kind(overlay: &SettingsOverlay, kind: SettingsMenuEntryKind) -> bool {
        overlay.menu.options().iter().any(|e| e.entry_kind == kind)
    }

    #[tokio::test]
    async fn esc_closes_overlay() {
        let mut overlay = new_overlay();
        let messages = overlay.on_event(&key(KeyCode::Esc)).await.unwrap();
        assert!(matches!(messages.as_slice(), [SettingsMessage::Close]));
    }

    #[tokio::test]
    async fn enter_opens_picker() {
        let mut overlay = new_overlay();
        let outcome = overlay.on_event(&key(KeyCode::Enter)).await;
        assert!(outcome.is_some());
        assert!(overlay.has_picker());
    }

    #[tokio::test]
    async fn picker_esc_closes_picker_not_overlay() {
        let mut overlay = new_overlay();
        send_keys(&mut overlay, &[KeyCode::Enter]).await;
        assert!(overlay.has_picker());

        let messages = overlay.on_event(&key(KeyCode::Esc)).await.unwrap();
        assert!(!overlay.has_picker());
        assert!(messages.is_empty(), "overlay should remain open");
    }

    #[tokio::test]
    async fn picker_confirm_returns_settings_change_action() {
        let mut overlay = new_overlay();
        send_keys(&mut overlay, &[KeyCode::Enter, KeyCode::Down]).await;
        let messages = overlay.on_event(&key(KeyCode::Enter)).await.unwrap();

        match messages.as_slice() {
            [SettingsMessage::SetConfigOption { config_id, value }] => {
                assert_eq!(config_id, "provider");
                assert_eq!(value, "ollama");
            }
            other => panic!("expected SetConfigOption, got: {other:?}"),
        }
    }

    /// Shared setup for theme-picker tests: creates temp dir, theme menu, sends Enter/Down/Enter.
    fn run_theme_picker_test(check: impl FnOnce(&SettingsOverlay)) {
        use crate::test_helpers::with_wisp_home;

        let temp_dir = tempfile::TempDir::new().unwrap();
        with_wisp_home(temp_dir.path(), || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut menu = SettingsMenu::from_config_options(&[]);
                menu.add_theme_entry(None, &["nord.tmTheme".to_string()]);
                let mut overlay = SettingsOverlay::new(menu, vec![], vec![]);
                send_keys(&mut overlay, &[KeyCode::Enter, KeyCode::Down, KeyCode::Enter]).await;
                check(&overlay);
            });
        });
    }

    #[test]
    fn settings_overlay_picker_confirm_updates_menu_row_immediately() {
        run_theme_picker_test(|overlay| {
            let entry = &overlay.menu.options()[0];
            assert_eq!(entry.config_id, THEME_CONFIG_ID);
            assert_eq!(entry.current_raw_value, "nord.tmTheme");
            assert_eq!(entry.current_value_index, 1);
        });
    }

    #[test]
    fn settings_overlay_picker_confirm_persists_theme_to_settings() {
        run_theme_picker_test(|_overlay| {
            let settings = crate::settings::load_or_create_settings();
            assert_eq!(settings.theme.file.as_deref(), Some("nord.tmTheme"));
        });
    }

    #[tokio::test]
    async fn cursor_col_and_row_offset() {
        // Without picker: cursor col is 0
        let overlay = new_overlay();
        assert_eq!(overlay.cursor_col(), 0);

        // With picker open: cursor col includes border + prefix, row offset = TOP_CHROME
        let mut overlay = new_overlay();
        send_keys(&mut overlay, &[KeyCode::Enter]).await;
        assert!(overlay.cursor_col() > 0);
        assert_eq!(overlay.cursor_row_offset(), TOP_CHROME);
    }

    #[tokio::test]
    async fn model_selector_esc_without_toggle_returns_no_change() {
        let mut overlay = open_model_selector().await;
        let messages = overlay.on_event(&key(KeyCode::Esc)).await.unwrap();
        assert_footer_contains(&mut overlay, "[Enter] Select");
        assert!(messages.is_empty(), "escape without toggling should produce no change");
    }

    #[tokio::test]
    async fn model_selector_esc_after_deselecting_all_returns_no_change() {
        let mut overlay = open_model_selector().await;
        send_keys(&mut overlay, &[KeyCode::Char(' ')]).await; // deselect pre-selected

        let messages = overlay.on_event(&key(KeyCode::Esc)).await.unwrap();
        assert_footer_contains(&mut overlay, "[Enter] Select");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn model_selector_enter_toggles_not_confirms() {
        let mut overlay = open_model_selector().await;
        send_keys(&mut overlay, &[KeyCode::Enter]).await;
        assert_footer_contains(&mut overlay, "Toggle");
    }

    #[tokio::test]
    async fn model_selector_uses_overlay_reasoning_prefill_after_menu_removal() {
        use crate::settings::types::{SettingsMenuEntry, SettingsMenuEntryKind, SettingsMenuValue};
        use acp_utils::config_meta::SelectOptionMeta;

        let menu = SettingsMenu::from_entries(vec![SettingsMenuEntry {
            config_id: "model".to_string(),
            title: "Model".to_string(),
            values: vec![
                SettingsMenuValue {
                    value: "claude-opus".to_string(),
                    name: "Claude Opus".to_string(),
                    description: None,
                    is_disabled: false,
                    meta: SelectOptionMeta {
                        reasoning_levels: vec![
                            utils::ReasoningEffort::Low,
                            utils::ReasoningEffort::Medium,
                            utils::ReasoningEffort::High,
                        ],
                        supports_image: false,
                        supports_audio: false,
                    },
                },
                SettingsMenuValue {
                    value: "gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                    description: None,
                    is_disabled: false,
                    meta: SelectOptionMeta::default(),
                },
            ],
            current_value_index: 0,
            current_raw_value: "claude-opus".to_string(),
            entry_kind: SettingsMenuEntryKind::Select,
            multi_select: true,
            display_name: None,
        }]);

        let reasoning_options = vec![
            config_select(
                "model",
                "Model",
                "claude-opus",
                vec![select_opt("claude-opus", "Claude Opus"), select_opt("gpt-4o", "GPT-4o")],
            ),
            config_select(
                "reasoning_effort",
                "Reasoning Effort",
                "medium",
                vec![
                    select_opt("none", "None"),
                    select_opt("low", "Low"),
                    select_opt("medium", "Medium"),
                    select_opt("high", "High"),
                ],
            ),
        ];
        let mut overlay =
            SettingsOverlay::new(menu, vec![], vec![]).with_reasoning_effort_from_options(&reasoning_options);

        send_keys(&mut overlay, &[KeyCode::Enter]).await;
        assert_footer_contains(&mut overlay, "Effort: medium");

        send_keys(&mut overlay, &[KeyCode::Tab]).await;
        assert_footer_contains(&mut overlay, "Effort: high");
        let messages = overlay.on_event(&key(KeyCode::Esc)).await.unwrap();

        let reasoning_msg = messages.iter().find(
            |m| matches!(m, SettingsMessage::SetConfigOption { config_id, .. } if config_id == "reasoning_effort"),
        );
        assert!(reasoning_msg.is_some(), "expected reasoning_effort change; got: {messages:?}");
        match reasoning_msg.unwrap() {
            SettingsMessage::SetConfigOption { value, .. } => {
                assert_eq!(value, "high", "reasoning should be high after one right from medium");
            }
            other => panic!("expected SetConfigOption, got: {other:?}"),
        }
    }

    #[test]
    fn update_settings_options_preserves_mcp_servers_entry() {
        use crate::settings::types::SettingsMenuEntryKind;
        use crate::test_helpers::with_wisp_home;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let themes_dir = temp_dir.path().join("themes");
        std::fs::create_dir_all(&themes_dir).unwrap();
        std::fs::write(themes_dir.join("custom.tmTheme"), "x").unwrap();

        with_wisp_home(temp_dir.path(), || {
            let mut menu = make_menu();
            menu.add_mcp_servers_entry("1 connected, 1 needs auth");
            let mut overlay = SettingsOverlay::new(menu, make_server_statuses(), vec![]);

            assert!(
                has_entry_kind(&overlay, SettingsMenuEntryKind::McpServers),
                "MCP servers entry should exist before update"
            );

            let new_options = vec![
                config_select(
                    "provider",
                    "Provider",
                    "ollama",
                    vec![select_opt("openrouter", "OpenRouter"), select_opt("ollama", "Ollama")],
                ),
                config_select("model", "Model", "llama", vec![select_opt("llama", "Llama")]),
            ];
            overlay.update_config_options(&new_options);

            assert!(
                overlay.menu.options().iter().any(|e| e.config_id == THEME_CONFIG_ID),
                "Theme entry should survive update_config_options"
            );
            assert!(
                has_entry_kind(&overlay, SettingsMenuEntryKind::McpServers),
                "MCP servers entry should survive update_config_options"
            );
        });
    }

    #[tokio::test]
    async fn authenticate_complete_sets_logged_in_status() {
        let mut menu = make_menu();
        menu.add_provider_logins_entry("2 needs login");
        let mut overlay = SettingsOverlay::new(menu, vec![], make_auth_methods());
        send_keys(&mut overlay, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]).await;
        assert!(matches!(overlay.active_pane, SettingsPane::ProviderLogin(_)));

        overlay.on_authenticate_complete("anthropic");

        assert!(matches!(overlay.active_pane, SettingsPane::ProviderLogin(_)));
        if let SettingsPane::ProviderLogin(ref inner) = overlay.active_pane {
            let entry = inner
                .entries()
                .iter()
                .find(|e| e.method_id == "anthropic")
                .expect("anthropic entry should still exist");
            assert_eq!(entry.status, ProviderLoginStatus::LoggedIn);
        }
    }

    #[tokio::test]
    async fn connected_oauth_server_authenticate_message_propagates() {
        let mut menu = make_menu();
        menu.add_mcp_servers_entry("1 connected");
        let statuses = vec![
            McpServerStatusEntry::new("remote", McpServerStatus::Connected { tool_count: 2 })
                .with_auth_capability(McpServerAuthCapability::OAuth),
        ];
        let mut overlay = SettingsOverlay::new(menu, statuses, vec![]);
        send_keys(&mut overlay, &[KeyCode::Down, KeyCode::Down, KeyCode::Enter]).await;

        let messages = overlay.on_event(&key(KeyCode::Enter)).await.unwrap();
        match messages.as_slice() {
            [SettingsMessage::AuthenticateServer(name)] => assert_eq!(name, "remote"),
            other => panic!("expected AuthenticateServer, got: {other:?}"),
        }
    }
}
