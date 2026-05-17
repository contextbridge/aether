pub mod attachments;
pub mod git_diff_mode;
mod plan_review_mode;
mod screen_router;
mod view;

use crate::session_loading_buffer::SessionLoadingBuffer;
use agent_client_protocol::schema::SessionUpdate;
pub use git_diff_mode::{GitDiffLoadState, GitDiffMode, GitDiffViewMessage};
pub use plan_review_mode::{PlanReviewAction, PlanReviewInput, PlanReviewMode};
use screen_router::ScreenRouter;
use screen_router::ScreenRouterMessage;

use crate::components::conversation_screen::ConversationScreen;
use crate::components::conversation_screen::ConversationScreenMessage;
use crate::components::plan_review::PlanDocument;
use crate::components::status_line::ContextUsageDisplay;
use crate::keybindings::Keybindings;
use crate::settings;
use crate::settings::overlay::{SettingsMessage, SettingsOverlay};
use crate::workspace_status::WorkspaceStatus;
use acp_utils::client::{AcpEvent, AcpPromptHandle};
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::config_option_id::ConfigOptionId;
use acp_utils::notifications::{CreateElicitationRequestParams, ElicitationAction, ElicitationResponse};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{self as acp, SessionId};
use attachments::build_attachment_blocks;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tui::RendererCommand;
use tui::{Component, Event, Frame, KeyEvent, ViewContext};
use utils::plan_review::{PlanReviewDecision, PlanReviewElicitationMeta};

#[derive(Debug, Clone)]
pub struct PromptAttachment {
    pub path: PathBuf,
    pub display_name: String,
}

/// Result of processing a single ACP event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    Render,
    DontRender,
}

pub struct AppInfo {
    pub session_id: SessionId,
    pub agent_name: String,
    pub prompt_capabilities: acp::PromptCapabilities,
    pub config_options: Vec<acp::SessionConfigOption>,
    pub auth_methods: Vec<acp::AuthMethod>,
    pub working_dir: PathBuf,
    pub workspace_status: WorkspaceStatus,
    pub prompt_handle: AcpPromptHandle,
}

#[doc = include_str!("../../docs/app.md")]
pub struct App {
    agent_name: String,
    context_usage: Option<ContextUsageDisplay>,
    exit_requested: bool,
    ctrl_c_pressed_at: Option<Instant>,
    conversation_screen: ConversationScreen,
    prompt_capabilities: acp::PromptCapabilities,
    config_options: Vec<acp::SessionConfigOption>,
    server_statuses: Vec<acp_utils::notifications::McpServerStatusEntry>,
    auth_methods: Vec<acp::AuthMethod>,
    settings_overlay: Option<SettingsOverlay>,
    screen_router: ScreenRouter,
    pending_plan_review_response: Option<Responder<ElicitationResponse>>,
    keybindings: Keybindings,
    session_id: SessionId,
    session_loading_buffer: SessionLoadingBuffer,
    prompt_handle: AcpPromptHandle,
    working_dir: PathBuf,
    workspace_status: WorkspaceStatus,
    content_padding: usize,
}

impl App {
    pub fn new(info: AppInfo) -> Self {
        let AppInfo {
            session_id,
            agent_name,
            prompt_capabilities,
            config_options,
            auth_methods,
            working_dir,
            workspace_status,
            prompt_handle,
        } = info;
        let keybindings = Keybindings::default();
        let wisp_settings = settings::load_or_create_settings();
        let content_padding = settings::resolve_content_padding(&wisp_settings);
        Self {
            agent_name,
            context_usage: None,
            exit_requested: false,
            ctrl_c_pressed_at: None,
            conversation_screen: ConversationScreen::new(keybindings.clone(), content_padding),
            prompt_capabilities,
            config_options,
            server_statuses: Vec::new(),
            auth_methods,
            settings_overlay: None,
            screen_router: ScreenRouter::new(working_dir.clone()),
            pending_plan_review_response: None,
            keybindings,
            session_id,
            session_loading_buffer: SessionLoadingBuffer::new(),
            prompt_handle,
            working_dir,
            workspace_status,
            content_padding,
        }
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    pub fn exit_confirmation_active(&self) -> bool {
        self.ctrl_c_pressed_at.is_some()
    }

    pub fn has_settings_overlay(&self) -> bool {
        self.settings_overlay.is_some()
    }

    pub fn needs_mouse_capture(&self) -> bool {
        self.settings_overlay.is_some() || self.screen_router.is_full_screen_mode()
    }

    pub fn wants_tick(&self) -> bool {
        self.conversation_screen.wants_tick() || self.ctrl_c_pressed_at.is_some()
    }

    fn git_diff_mode_mut(&mut self) -> &mut GitDiffMode {
        self.screen_router.git_diff_mode_mut()
    }

    pub fn on_acp_event(&mut self, event: AcpEvent) -> EventOutcome {
        match event {
            AcpEvent::SessionUpdate { session_id, update } => {
                return self.on_acp_session_update(&session_id, *update);
            }
            AcpEvent::ContextCleared(_) => {
                self.conversation_screen.reset_after_context_cleared();
                self.context_usage = None;
            }
            AcpEvent::ContextUsage(params) => {
                self.context_usage = params
                    .context_limit
                    .filter(|limit| *limit > 0)
                    .map(|limit| ContextUsageDisplay::new(params.input_tokens, limit));
            }
            AcpEvent::SubAgentProgress(progress) => self.conversation_screen.on_sub_agent_progress(&progress),
            AcpEvent::AuthMethodsUpdated(params) => self.update_auth_methods(params.auth_methods),
            AcpEvent::McpNotification(notification) => self.on_mcp_notification(notification),
            AcpEvent::PromptDone(stop_reason) => self.on_prompt_done(stop_reason),
            AcpEvent::PromptError(error) => {
                self.session_loading_buffer.clear();
                self.conversation_screen.on_prompt_error(&error);
            }
            AcpEvent::ElicitationRequest { params, responder } => self.on_elicitation_request(params, responder),
            AcpEvent::AuthenticateComplete { method_id } => self.on_authenticate_complete(&method_id),
            AcpEvent::AuthenticateFailed { method_id, error } => self.on_authenticate_failed(&method_id, &error),
            AcpEvent::SessionsListed { sessions } => {
                let current_id = &self.session_id;
                let filtered: Vec<_> = sessions.into_iter().filter(|s| s.session_id != *current_id).collect();
                self.conversation_screen.open_session_picker(filtered);
            }
            // SessionLoaded intentionally does NOT restore previous config selections:
            // when the user loads an existing session, the server's stored config for
            // that session is authoritative.
            AcpEvent::SessionLoaded { session_id, config_options } => {
                let replay_updates = self.session_loading_buffer.take(&session_id);
                self.session_id = session_id;
                for update in replay_updates {
                    self.on_session_update(&update);
                }
                self.update_config_options(&config_options);
            }
            AcpEvent::NewSessionCreated { session_id, config_options } => {
                self.session_loading_buffer.clear();
                let previous_selections = current_config_selections(&self.config_options);
                self.session_id = session_id;
                self.update_config_options(&config_options);
                self.context_usage = None;
                self.restore_config_selections(&previous_selections);
            }
            AcpEvent::ConnectionClosed => {
                self.session_loading_buffer.clear();
                self.exit_requested = true;
            }
        }
        EventOutcome::Render
    }

    async fn handle_key(&mut self, commands: &mut Vec<RendererCommand>, key_event: KeyEvent) {
        if self.keybindings.exit.matches(key_event) {
            match self.ctrl_c_pressed_at {
                Some(_) => {
                    self.exit_requested = true;
                }
                None => {
                    self.ctrl_c_pressed_at = Some(Instant::now());
                }
            }
            return;
        }

        if self.keybindings.toggle_git_diff.matches(key_event) && !self.conversation_screen.has_modal() {
            if let Some(msg) = self.screen_router.toggle_git_diff() {
                self.handle_screen_router_message(commands, msg).await;
            }
            return;
        }

        let event = Event::Key(key_event);

        if self.screen_router.is_full_screen_mode() {
            for msg in self.screen_router.on_event(&event).await.unwrap_or_default() {
                self.handle_screen_router_message(commands, msg).await;
            }
        } else if self.settings_overlay.is_some() {
            self.handle_settings_overlay_event(commands, &event).await;
        } else {
            let outcome = self.conversation_screen.on_event(&event).await;
            let consumed = outcome.is_some();
            self.handle_conversation_messages(commands, outcome).await;
            if !consumed {
                self.handle_fallthrough_keybindings(key_event);
            }
        }
    }

    async fn submit_prompt(&mut self, user_input: String, attachments: Vec<PromptAttachment>) {
        let outcome = build_attachment_blocks(&attachments).await;
        self.conversation_screen.conversation.push_user_message("");
        self.conversation_screen.conversation.push_user_message(&user_input);
        for placeholder in &outcome.transcript_placeholders {
            self.conversation_screen.conversation.push_user_message(placeholder);
        }
        for w in outcome.warnings {
            self.conversation_screen.conversation.push_user_message(&format!("[wisp] {w}"));
        }

        if let Some(message) = self.media_support_error(&outcome.blocks) {
            self.conversation_screen.reject_local_prompt(&message);
            return;
        }

        let _ = self.prompt_handle.prompt(
            &self.session_id,
            &user_input,
            if outcome.blocks.is_empty() { None } else { Some(outcome.blocks) },
        );
    }

    async fn handle_conversation_messages(
        &mut self,
        commands: &mut Vec<RendererCommand>,
        outcome: Option<Vec<ConversationScreenMessage>>,
    ) {
        for msg in outcome.unwrap_or_default() {
            match msg {
                ConversationScreenMessage::SendPrompt { user_input, attachments } => {
                    self.conversation_screen.waiting_for_response = true;
                    self.submit_prompt(user_input, attachments).await;
                }
                ConversationScreenMessage::ClearScreen => {
                    commands.push(RendererCommand::ClearScreen);
                }
                ConversationScreenMessage::NewSession => {
                    commands.push(RendererCommand::ClearScreen);
                    let _ = self.prompt_handle.new_session(&self.working_dir);
                }
                ConversationScreenMessage::OpenSettings => {
                    self.open_settings_overlay();
                }
                ConversationScreenMessage::OpenSessionPicker => {
                    let _ = self.prompt_handle.list_sessions();
                }
                ConversationScreenMessage::LoadSession { session_id, cwd } => {
                    self.session_loading_buffer.begin_load(session_id.clone());
                    if let Err(e) = self.prompt_handle.load_session(&session_id, &cwd) {
                        self.session_loading_buffer.remove(&session_id);
                        tracing::warn!("Failed to load session: {e}");
                    }
                }
            }
        }
    }

    fn handle_fallthrough_keybindings(&self, key_event: KeyEvent) {
        if self.keybindings.cycle_reasoning.matches(key_event) {
            if let Some((id, val)) = settings::cycle_reasoning_option(&self.config_options) {
                let _ = self.prompt_handle.set_config_option(&self.session_id, &id, &val);
            }
            return;
        }

        if self.keybindings.cycle_mode.matches(key_event) {
            if let Some((id, val)) = settings::cycle_quick_option(&self.config_options) {
                let _ = self.prompt_handle.set_config_option(&self.session_id, &id, &val);
            }
            return;
        }

        if self.keybindings.cancel.matches(key_event)
            && self.conversation_screen.is_waiting()
            && let Err(e) = self.prompt_handle.cancel(&self.session_id)
        {
            tracing::warn!("Failed to send cancel: {e}");
        }
    }

    async fn handle_settings_overlay_event(&mut self, commands: &mut Vec<RendererCommand>, event: &Event) {
        let Some(ref mut overlay) = self.settings_overlay else {
            return;
        };
        let messages = overlay.on_event(event).await.unwrap_or_default();

        for msg in messages {
            match msg {
                SettingsMessage::Close => {
                    self.settings_overlay = None;
                    return;
                }
                SettingsMessage::SetConfigOption { config_id, value } => {
                    let _ = self.prompt_handle.set_config_option(&self.session_id, &config_id, &value);
                }
                SettingsMessage::SetTheme(theme) => {
                    commands.push(RendererCommand::SetTheme(theme));
                }
                SettingsMessage::AuthenticateServer(name) => {
                    let _ = self.prompt_handle.authenticate_mcp_server(&self.session_id, &name);
                }
                SettingsMessage::AuthenticateProvider(ref method_id) => {
                    if let Some(ref mut overlay) = self.settings_overlay {
                        overlay.on_authenticate_started(method_id);
                    }
                    let _ = self.prompt_handle.authenticate(method_id);
                }
            }
        }
    }

    fn open_settings_overlay(&mut self) {
        self.settings_overlay =
            Some(settings::create_overlay(&self.config_options, &self.server_statuses, &self.auth_methods));
    }

    fn update_config_options(&mut self, config_options: &[acp::SessionConfigOption]) {
        self.config_options = config_options.to_vec();
        if let Some(ref mut overlay) = self.settings_overlay {
            overlay.update_config_options(config_options);
        }
    }

    fn update_auth_methods(&mut self, auth_methods: Vec<acp::AuthMethod>) {
        self.auth_methods = auth_methods;
        if let Some(ref mut overlay) = self.settings_overlay {
            overlay.update_auth_methods(self.auth_methods.clone());
        }
    }

    fn restore_config_selections(&self, previous: &[(String, String)]) {
        let new_selections = current_config_selections(&self.config_options);
        for (id, old_value) in previous {
            let still_exists = new_selections.iter().any(|(new_id, _)| new_id == id);
            if !still_exists {
                tracing::debug!(config_id = id, "config option no longer present in new session");
                continue;
            }
            let server_reset = new_selections.iter().any(|(new_id, new_val)| new_id == id && new_val != old_value);
            if server_reset && let Err(e) = self.prompt_handle.set_config_option(&self.session_id, id, old_value) {
                tracing::warn!(config_id = id, error = %e, "failed to restore config option");
            }
        }
    }

    async fn handle_screen_router_message(&mut self, commands: &mut Vec<RendererCommand>, msg: ScreenRouterMessage) {
        match msg {
            ScreenRouterMessage::LoadGitDiff | ScreenRouterMessage::RefreshGitDiff => {
                self.git_diff_mode_mut().complete_load().await;
            }
            ScreenRouterMessage::SendPrompt { user_input } => {
                if self.conversation_screen.is_waiting() {
                    return;
                }

                self.conversation_screen.waiting_for_response = true;
                self.submit_prompt(user_input, Vec::new()).await;
                self.screen_router.close_git_diff();
            }
            ScreenRouterMessage::FinishPlanReview(action) => {
                let response = plan_review_response(action);
                if let Some(responder) = self.pending_plan_review_response.take() {
                    let _ = responder.respond(response);
                }
            }
        }
        let _ = commands;
    }

    fn on_acp_session_update(&mut self, session_id: &SessionId, update: SessionUpdate) -> EventOutcome {
        let Some(update) = self.session_loading_buffer.push(session_id, update) else {
            return EventOutcome::DontRender;
        };
        self.on_session_update(&update);
        EventOutcome::Render
    }

    fn on_session_update(&mut self, update: &acp::SessionUpdate) {
        self.conversation_screen.on_session_update(update);

        if let acp::SessionUpdate::ConfigOptionUpdate(config_update) = update {
            self.update_config_options(&config_update.config_options);
        }
    }

    fn on_prompt_done(&mut self, stop_reason: acp::StopReason) {
        self.conversation_screen.on_prompt_done(stop_reason);
    }

    fn on_elicitation_request(
        &mut self,
        params: acp_utils::notifications::ElicitationParams,
        responder: Responder<ElicitationResponse>,
    ) {
        self.settings_overlay = None;

        if let Some(meta) = plan_review_meta_from_request(&params.request) {
            if let Some(existing) = self.pending_plan_review_response.replace(responder) {
                let _ = existing.respond(cancel_response());
            }
            let document = PlanDocument::parse(meta.plan_path, &meta.markdown);
            let input = PlanReviewInput { title: meta.title, document };
            self.screen_router.open_plan_review(input);
            return;
        }

        self.conversation_screen.on_elicitation_request(params, responder);
    }

    fn on_mcp_notification(&mut self, notification: acp_utils::notifications::McpNotification) {
        use acp_utils::notifications::McpNotification;
        match notification {
            McpNotification::ServerStatus { servers } => {
                if let Some(ref mut overlay) = self.settings_overlay {
                    overlay.update_server_statuses(servers.clone());
                }
                self.server_statuses = servers;
            }
            McpNotification::UrlElicitationComplete(params) => {
                self.conversation_screen.on_url_elicitation_complete(&params);
            }
        }
    }

    fn on_authenticate_complete(&mut self, method_id: &str) {
        if let Some(ref mut overlay) = self.settings_overlay {
            overlay.on_authenticate_complete(method_id);
        }
    }

    fn on_authenticate_failed(&mut self, method_id: &str, error: &str) {
        tracing::warn!("Provider auth failed for {method_id}: {error}");
        if let Some(ref mut overlay) = self.settings_overlay {
            overlay.on_authenticate_failed(method_id);
        }
    }

    fn media_support_error(&self, blocks: &[acp::ContentBlock]) -> Option<String> {
        let requires_image = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)));
        let requires_audio = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Audio(_)));

        if !requires_image && !requires_audio {
            return None;
        }

        if requires_image && !self.prompt_capabilities.image {
            return Some("ACP agent does not support image input.".to_string());
        }
        if requires_audio && !self.prompt_capabilities.audio {
            return Some("ACP agent does not support audio input.".to_string());
        }

        let option =
            self.config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::Model.as_str())?;
        let acp::SessionConfigKind::Select(select) = &option.kind else {
            return None;
        };

        let values: Vec<_> =
            select.current_value.0.split(',').map(str::trim).filter(|value| !value.is_empty()).collect();

        if values.is_empty() {
            return None;
        }

        let acp::SessionConfigSelectOptions::Ungrouped(options) = &select.options else {
            return None;
        };

        let selected_meta: Vec<_> = values
            .iter()
            .filter_map(|value| {
                options
                    .iter()
                    .find(|option| option.value.0.as_ref() == *value)
                    .map(|option| SelectOptionMeta::from_meta(option.meta.as_ref()))
            })
            .collect();

        if selected_meta.len() != values.len() {
            return Some("Current model selection is missing prompt capability metadata.".into());
        }

        if requires_image && selected_meta.iter().any(|meta| !meta.supports_image) {
            return Some("Current model selection does not support image input.".to_string());
        }
        if requires_audio && selected_meta.iter().any(|meta| !meta.supports_audio) {
            return Some("Current model selection does not support audio input.".to_string());
        }

        None
    }
}

impl Component for App {
    type Message = RendererCommand;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<RendererCommand>> {
        let mut commands = Vec::new();
        match event {
            Event::Key(key_event) => self.handle_key(&mut commands, *key_event).await,
            Event::Paste(_) => {
                self.settings_overlay = None;
                if self.screen_router.is_full_screen_mode() {
                    for msg in self.screen_router.on_event(event).await.unwrap_or_default() {
                        self.handle_screen_router_message(&mut commands, msg).await;
                    }
                } else {
                    let outcome = self.conversation_screen.on_event(event).await;
                    self.handle_conversation_messages(&mut commands, outcome).await;
                }
            }
            Event::Tick => {
                if let Some(instant) = self.ctrl_c_pressed_at
                    && instant.elapsed() > Duration::from_secs(1)
                {
                    self.ctrl_c_pressed_at = None;
                }
                let now = Instant::now();
                self.conversation_screen.on_tick(now);
            }
            Event::Mouse(_) => {
                if self.screen_router.is_full_screen_mode() {
                    for msg in self.screen_router.on_event(event).await.unwrap_or_default() {
                        self.handle_screen_router_message(&mut commands, msg).await;
                    }
                } else if self.settings_overlay.is_some() {
                    self.handle_settings_overlay_event(&mut commands, event).await;
                }
            }
            Event::Resize(_) => {}
        }
        Some(commands)
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        self.conversation_screen.refresh_caches(ctx);

        let height = (ctx.size.height.saturating_sub(1)) as usize;
        if let Some(ref mut overlay) = self.settings_overlay
            && height >= 3
        {
            overlay.update_child_viewport(height.saturating_sub(4));
        }

        view::build_frame(self, ctx)
    }
}

fn plan_review_meta_from_request(request: &CreateElicitationRequestParams) -> Option<PlanReviewElicitationMeta> {
    match request {
        CreateElicitationRequestParams::FormElicitationParams { meta, .. } => {
            PlanReviewElicitationMeta::parse(meta.as_ref().map(|meta| &meta.0))
        }
        CreateElicitationRequestParams::UrlElicitationParams { .. } => None,
    }
}

fn plan_review_response(action: PlanReviewAction) -> ElicitationResponse {
    match action {
        PlanReviewAction::Approve => ElicitationResponse {
            action: ElicitationAction::Accept,
            content: Some(PlanReviewDecision::Approve.response_content(None)),
        },
        PlanReviewAction::RequestChanges { feedback } => ElicitationResponse {
            action: ElicitationAction::Accept,
            content: Some(PlanReviewDecision::Deny.response_content(Some(&feedback))),
        },
        PlanReviewAction::Cancel => cancel_response(),
    }
}

fn cancel_response() -> ElicitationResponse {
    ElicitationResponse { action: ElicitationAction::Cancel, content: None }
}

fn current_config_selections(options: &[acp::SessionConfigOption]) -> Vec<(String, String)> {
    options
        .iter()
        .filter_map(|opt| {
            let acp::SessionConfigKind::Select(ref select) = opt.kind else {
                return None;
            };
            Some((opt.id.0.to_string(), select.current_value.0.to_string()))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use acp_utils::client::PromptCommand;
    use tokio::sync::mpsc;

    pub fn test_workspace_status() -> WorkspaceStatus {
        WorkspaceStatus::new("~/code/foo", Some("main".to_string()))
    }

    pub fn make_app() -> App {
        make_app_with_options("test", acp::PromptCapabilities::new(), &[], vec![], AcpPromptHandle::noop())
    }

    pub fn make_app_with_config(config_options: &[acp::SessionConfigOption]) -> App {
        make_app_with_options("test", acp::PromptCapabilities::new(), config_options, vec![], AcpPromptHandle::noop())
    }

    pub fn make_app_with_auth(auth_methods: Vec<acp::AuthMethod>) -> App {
        make_app_with_options("test", acp::PromptCapabilities::new(), &[], auth_methods, AcpPromptHandle::noop())
    }

    pub fn make_app_with_config_recording(
        config_options: &[acp::SessionConfigOption],
    ) -> (App, mpsc::UnboundedReceiver<PromptCommand>) {
        let (handle, rx) = AcpPromptHandle::recording();
        let app = make_app_with_options("test", acp::PromptCapabilities::new(), config_options, vec![], handle);
        (app, rx)
    }

    pub fn make_app_with_session_id(session_id: &str) -> App {
        make_app_with_options(session_id, acp::PromptCapabilities::new(), &[], vec![], AcpPromptHandle::noop())
    }

    pub fn make_app_with_config_and_capabilities_recording(
        config_options: &[acp::SessionConfigOption],
        prompt_capabilities: acp::PromptCapabilities,
    ) -> (App, mpsc::UnboundedReceiver<PromptCommand>) {
        let (handle, rx) = AcpPromptHandle::recording();
        let app = make_app_with_options("test", prompt_capabilities, config_options, vec![], handle);
        (app, rx)
    }

    fn make_app_with_options(
        session_id: &str,
        prompt_capabilities: acp::PromptCapabilities,
        config_options: &[acp::SessionConfigOption],
        auth_methods: Vec<acp::AuthMethod>,
        prompt_handle: AcpPromptHandle,
    ) -> App {
        App::new(AppInfo {
            session_id: SessionId::new(session_id),
            agent_name: "test-agent".to_string(),
            prompt_capabilities,
            config_options: config_options.to_vec(),
            auth_methods,
            working_dir: PathBuf::from("."),
            workspace_status: test_workspace_status(),
            prompt_handle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;
    use crate::components::command_picker::CommandEntry;
    use crate::components::conversation_screen::Modal;
    use crate::components::conversation_window::SegmentContent;
    use crate::components::elicitation_form::ElicitationForm;
    use crate::settings::{DEFAULT_CONTENT_PADDING, ThemeSettings as WispThemeSettings, WispSettings, save_settings};
    use crate::test_helpers::with_wisp_home;
    use acp_utils::testing::test_connection;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::task::LocalSet;
    use tui::testing::render_component;
    use tui::{Frame, KeyCode, KeyModifiers, Renderer, Theme, ViewContext};
    use utils::plan_review::PlanReviewElicitationMeta;

    fn make_renderer() -> Renderer<Vec<u8>> {
        Renderer::new(Vec::new(), Theme::default(), (80, 24))
    }

    fn render_app(renderer: &mut Renderer<Vec<u8>>, app: &mut App, context: &ViewContext) -> Frame {
        renderer.render_frame(|ctx| app.render(ctx)).unwrap();
        app.render(context)
    }

    fn frame_contains(output: &Frame, text: &str) -> bool {
        output.lines().iter().any(|line| line.plain_text().contains(text))
    }

    async fn send_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.on_event(&Event::Key(KeyEvent::new(code, modifiers))).await;
    }

    fn setup_themes_dir(files: &[&str]) -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let themes_dir = temp_dir.path().join("themes");
        fs::create_dir_all(&themes_dir).unwrap();
        for f in files {
            fs::write(themes_dir.join(f), "x").unwrap();
        }
        temp_dir
    }

    fn make_plan_entry(name: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
        acp::PlanEntry::new(name, acp::PlanEntryPriority::Medium, status)
    }

    fn make_plan_review_params(markdown: &str) -> acp_utils::notifications::ElicitationParams {
        let meta = PlanReviewElicitationMeta::new(Path::new("/tmp/test-plan.md"), markdown)
            .to_json()
            .expect("serialize plan review metadata");

        acp_utils::notifications::ElicitationParams {
            server_name: "plan-server".to_string(),
            request: acp_utils::notifications::CreateElicitationRequestParams::FormElicitationParams {
                meta: Some(
                    serde_json::from_value(serde_json::Value::Object(meta))
                        .expect("deserialize plan review metadata into rmcp meta"),
                ),
                message: "Approve plan?".to_string(),
                requested_schema: acp_utils::ElicitationSchema::builder()
                    .required_string("decision")
                    .optional_string("feedback")
                    .build()
                    .expect("build plan review requested schema"),
            },
        }
    }

    fn mode_model_options(
        current_mode: impl Into<String>,
        current_model: impl Into<String>,
    ) -> Vec<acp::SessionConfigOption> {
        vec![
            acp::SessionConfigOption::select(
                "mode",
                "Mode",
                current_mode.into(),
                vec![
                    acp::SessionConfigSelectOption::new("Planner", "Planner"),
                    acp::SessionConfigSelectOption::new("Coder", "Coder"),
                ],
            )
            .category(acp::SessionConfigOptionCategory::Mode),
            acp::SessionConfigOption::select(
                "model",
                "Model",
                current_model.into(),
                vec![
                    acp::SessionConfigSelectOption::new("gpt-4o", "GPT-4o"),
                    acp::SessionConfigSelectOption::new("claude", "Claude"),
                ],
            )
            .category(acp::SessionConfigOptionCategory::Model),
        ]
    }

    fn image_model_options() -> Vec<acp::SessionConfigOption> {
        vec![
            acp::SessionConfigOption::select(
                "model",
                "Model",
                "anthropic:claude-sonnet-4-5",
                vec![
                    acp::SessionConfigSelectOption::new("anthropic:claude-sonnet-4-5", "Claude Sonnet").meta(
                        SelectOptionMeta { reasoning_levels: vec![], supports_image: true, supports_audio: false }
                            .into_meta(),
                    ),
                    acp::SessionConfigSelectOption::new("deepseek:deepseek-chat", "DeepSeek").meta(
                        SelectOptionMeta { reasoning_levels: vec![], supports_image: false, supports_audio: false }
                            .into_meta(),
                    ),
                ],
            )
            .category(acp::SessionConfigOptionCategory::Model),
        ]
    }

    #[test]
    fn settings_overlay_with_themes() {
        let temp_dir = setup_themes_dir(&["sage.tmTheme"]);
        with_wisp_home(temp_dir.path(), || {
            let mut app = make_app();
            app.open_settings_overlay();
            assert!(app.settings_overlay.is_some());
        });

        let temp_dir = setup_themes_dir(&["sage.tmTheme", "nord.tmTheme"]);
        with_wisp_home(temp_dir.path(), || {
            let settings = WispSettings {
                theme: WispThemeSettings { file: Some("nord.tmTheme".to_string()) },
                content_padding: None,
            };
            save_settings(&settings).unwrap();
            let mut app = make_app();
            app.open_settings_overlay();
            assert!(app.settings_overlay.is_some());
        });
    }

    #[test]
    fn command_picker_cursor_stays_in_input_prompt() {
        let mut app = make_app();
        let mut renderer = make_renderer();
        app.conversation_screen.prompt_composer.open_command_picker_with_entries(vec![CommandEntry {
            name: "settings".to_string(),
            description: "Open settings".to_string(),
            has_input: false,
            hint: None,
            builtin: true,
        }]);

        let context = ViewContext::new((120, 40));
        let output = render_app(&mut renderer, &mut app, &context);
        let input_row =
            output.lines().iter().position(|line| line.plain_text().contains("> ")).expect("input prompt should exist");
        assert_eq!(output.cursor().row, input_row);
    }

    #[test]
    fn settings_overlay_replaces_conversation_window() {
        let options = vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            "m1",
            vec![acp::SessionConfigSelectOption::new("m1", "M1")],
        )];
        let mut app = make_app_with_config(&options);
        let mut renderer = make_renderer();
        app.open_settings_overlay();

        let ctx = ViewContext::new((120, 40));
        assert!(frame_contains(&render_app(&mut renderer, &mut app, &ctx), "Configuration"));
        app.settings_overlay = None;
        assert!(!frame_contains(&render_app(&mut renderer, &mut app, &ctx), "Configuration"));
    }

    #[test]
    fn extract_model_display_handles_comma_separated_value() {
        use crate::components::status_line::extract_model_display;
        let options = vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            "a:x,b:y",
            vec![
                acp::SessionConfigSelectOption::new("a:x", "Alpha / X"),
                acp::SessionConfigSelectOption::new("b:y", "Beta / Y"),
                acp::SessionConfigSelectOption::new("c:z", "Gamma / Z"),
            ],
        )];
        assert_eq!(extract_model_display(&options).as_deref(), Some("Alpha / X + Beta / Y"));
    }

    #[test]
    fn extract_reasoning_effort_returns_none_for_none_value() {
        use crate::components::status_line::extract_reasoning_effort;
        use acp_utils::config_option_id::ConfigOptionId;
        let options = vec![acp::SessionConfigOption::select(
            ConfigOptionId::ReasoningEffort.as_str(),
            "Reasoning",
            "none",
            vec![
                acp::SessionConfigSelectOption::new("none", "None"),
                acp::SessionConfigSelectOption::new("low", "Low"),
            ],
        )];
        assert_eq!(extract_reasoning_effort(&options), None);
    }

    #[test]
    fn render_hides_plan_header_when_no_entries_are_visible() {
        let mut app = make_app();
        let mut renderer = make_renderer();
        let grace_period = app.conversation_screen.plan_tracker.grace_period;
        app.conversation_screen.plan_tracker.replace(
            vec![make_plan_entry("1", acp::PlanEntryStatus::Completed)],
            Instant::now().checked_sub(grace_period + Duration::from_millis(1)).unwrap(),
        );
        app.conversation_screen.plan_tracker.on_tick(Instant::now());

        let output = render_app(&mut renderer, &mut app, &ViewContext::new((120, 40)));
        assert!(!frame_contains(&output, "Plan"));
    }

    #[test]
    fn plan_version_increments_on_replace_and_clear() {
        let mut app = make_app();
        let v0 = app.conversation_screen.plan_tracker.version();

        app.conversation_screen
            .plan_tracker
            .replace(vec![make_plan_entry("Task A", acp::PlanEntryStatus::Pending)], Instant::now());
        let v1 = app.conversation_screen.plan_tracker.version();
        assert!(v1 > v0, "replace should increment version");

        app.conversation_screen.plan_tracker.clear();
        assert!(app.conversation_screen.plan_tracker.version() > v1, "clear should increment version");
    }

    #[test]
    fn sessions_listed_filters_out_current_session() {
        let mut app = make_app_with_session_id("current-session");
        app.on_acp_event(AcpEvent::SessionsListed {
            sessions: vec![
                acp::SessionInfo::new("other-session-1", PathBuf::from("/project"))
                    .title("First other session".to_string()),
                acp::SessionInfo::new("current-session", PathBuf::from("/project"))
                    .title("Current session title".to_string()),
                acp::SessionInfo::new("other-session-2", PathBuf::from("/other"))
                    .title("Second other session".to_string()),
            ],
        });

        let Some(Modal::SessionPicker(picker)) = &mut app.conversation_screen.active_modal else {
            panic!("expected session picker modal");
        };
        let lines = render_component(|ctx| picker.render(ctx), 60, 10).get_lines();

        let has = |text: &str| lines.iter().any(|l| l.contains(text));
        assert!(!has("Current session title"), "current session should be filtered out");
        assert!(has("First other session"), "first other session should be present");
        assert!(has("Second other session"), "second other session should be present");
    }

    #[tokio::test]
    async fn custom_exit_keybinding_triggers_exit() {
        use crate::keybindings::KeyBinding;
        let mut app = make_app();
        app.keybindings.exit = KeyBinding::new(KeyCode::Char('q'), KeyModifiers::CONTROL);

        send_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(!app.exit_requested(), "default Ctrl+C should not exit");
        assert!(!app.exit_confirmation_active(), "Ctrl+C should not trigger exit confirmation when rebound");

        send_key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL).await;
        assert!(!app.exit_requested(), "first Ctrl+Q should trigger confirmation, not exit");
        assert!(app.exit_confirmation_active(), "first Ctrl+Q should activate confirmation");

        send_key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL).await;
        assert!(app.exit_requested(), "second Ctrl+Q should exit");
    }

    #[tokio::test]
    async fn ctrl_g_toggles_git_diff_viewer() {
        let mut app = make_app();

        send_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
        assert!(app.screen_router.is_git_diff(), "should open git diff");

        send_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
        assert!(!app.screen_router.is_git_diff(), "should close git diff");
    }

    #[tokio::test]
    async fn needs_mouse_capture_in_git_diff() {
        let mut app = make_app();
        assert!(!app.needs_mouse_capture());

        send_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
        assert!(app.needs_mouse_capture());

        send_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
        assert!(!app.needs_mouse_capture());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ctrl_g_blocked_during_elicitation() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (responder, _rx) = peer.fake_elicitation(&cx).await;
                app.conversation_screen.active_modal = Some(Modal::Elicitation(ElicitationForm::from_params(
                    acp_utils::notifications::ElicitationParams {
                        server_name: "test-server".to_string(),
                        request: acp_utils::notifications::CreateElicitationRequestParams::FormElicitationParams {
                            meta: None,
                            message: "test".to_string(),
                            requested_schema: acp_utils::ElicitationSchema::builder().build().unwrap(),
                        },
                    },
                    responder,
                )));

                send_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
                assert!(!app.screen_router.is_git_diff(), "git diff should not open during elicitation");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn plan_review_elicitation_opens_full_screen_review() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (responder, _rx) = peer.fake_elicitation(&cx).await;

                app.on_elicitation_request(make_plan_review_params("# Plan\n\n- item"), responder);

                assert!(app.screen_router.is_plan_review(), "plan review mode should open");
                assert!(app.conversation_screen.active_modal.is_none(), "plan review should bypass modal form");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn regular_form_elicitation_still_uses_modal_form() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (responder, _rx) = peer.fake_elicitation(&cx).await;

                app.on_elicitation_request(
                    acp_utils::notifications::ElicitationParams {
                        server_name: "test-server".to_string(),
                        request: acp_utils::notifications::CreateElicitationRequestParams::FormElicitationParams {
                            meta: None,
                            message: "regular form".to_string(),
                            requested_schema: acp_utils::ElicitationSchema::builder().build().unwrap(),
                        },
                    },
                    responder,
                );

                assert!(!app.screen_router.is_plan_review());
                assert!(matches!(app.conversation_screen.active_modal, Some(Modal::Elicitation(_))));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn plan_review_finish_routes_response_and_closes_mode() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_elicitation(&cx).await;
                app.on_elicitation_request(make_plan_review_params("# Plan"), responder);

                send_key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE).await;

                assert!(!app.screen_router.is_plan_review(), "plan review mode should close after finish");
                let response = rx.await.expect("plan review response should be sent");
                assert_eq!(response.action, acp_utils::notifications::ElicitationAction::Accept);
                assert_eq!(response.content.expect("approve content")["decision"], "approve");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn plan_review_cancel_routes_cancel_response() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (responder, rx) = peer.fake_elicitation(&cx).await;
                app.on_elicitation_request(make_plan_review_params("# Plan"), responder);

                send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;

                let response = rx.await.expect("plan review response should be sent");
                assert_eq!(response.action, acp_utils::notifications::ElicitationAction::Cancel);
                assert!(response.content.is_none());
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacing_pending_plan_review_cancels_the_previous_response() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (first_responder, first_rx) = peer.fake_elicitation(&cx).await;
                let (second_responder, second_rx) = peer.fake_elicitation(&cx).await;

                app.on_elicitation_request(make_plan_review_params("# First"), first_responder);
                app.on_elicitation_request(make_plan_review_params("# Second"), second_responder);

                let first_response = first_rx.await.expect("first plan review response should be sent");
                assert_eq!(first_response.action, acp_utils::notifications::ElicitationAction::Cancel);
                assert!(first_response.content.is_none());
                assert!(app.screen_router.is_plan_review(), "replacement plan review should stay open");

                send_key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE).await;

                let second_response = second_rx.await.expect("replacement plan review response should be sent");
                assert_eq!(second_response.action, acp_utils::notifications::ElicitationAction::Accept);
                assert_eq!(second_response.content.expect("approve content")["decision"], "approve");
            })
            .await;
    }

    #[tokio::test]
    async fn esc_in_diff_mode_does_not_cancel() {
        let mut app = make_app();
        app.conversation_screen.waiting_for_response = true;
        app.screen_router.enter_git_diff_for_test();

        send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;

        assert!(!app.exit_requested());
        assert!(
            app.conversation_screen.waiting_for_response,
            "Esc should NOT cancel a running prompt while git diff mode is active"
        );
    }

    #[tokio::test]
    async fn git_diff_submit_sends_prompt_and_closes_diff_when_idle() {
        use acp_utils::client::PromptCommand;

        let (mut app, mut rx) = make_app_with_config_recording(&[]);
        app.screen_router.enter_git_diff_for_test();

        let mut commands = Vec::new();
        app.handle_screen_router_message(
            &mut commands,
            ScreenRouterMessage::SendPrompt { user_input: "Looks good".to_string() },
        )
        .await;

        assert!(!app.screen_router.is_git_diff(), "successful submit should exit git diff mode");
        assert!(app.conversation_screen.waiting_for_response, "submit should transition into waiting state");

        let cmd = rx.try_recv().expect("expected Prompt command to be sent");
        match cmd {
            PromptCommand::Prompt { text, .. } => {
                assert!(text.contains("Looks good"));
            }
            other => panic!("expected Prompt command, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn git_diff_submit_while_waiting_is_ignored_and_keeps_diff_open() {
        let (mut app, mut rx) = make_app_with_config_recording(&[]);
        app.conversation_screen.waiting_for_response = true;
        app.screen_router.enter_git_diff_for_test();

        let mut commands = Vec::new();
        app.handle_screen_router_message(
            &mut commands,
            ScreenRouterMessage::SendPrompt { user_input: "Needs follow-up".to_string() },
        )
        .await;

        assert!(app.screen_router.is_git_diff(), "blocked submit should keep git diff mode open");
        assert!(rx.try_recv().is_err(), "no prompt should be sent while waiting");
    }

    #[tokio::test]
    async fn mouse_scroll_ignored_in_conversation_mode() {
        use tui::{MouseEvent, MouseEventKind};
        let mut app = make_app();
        let mouse = MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        app.on_event(&Event::Mouse(mouse)).await;
    }

    #[tokio::test]
    async fn prompt_composer_submit_pushes_echo_lines() {
        use crate::components::conversation_window::SegmentContent;
        let mut app = make_app();
        let mut commands = Vec::new();
        app.handle_conversation_messages(
            &mut commands,
            Some(vec![ConversationScreenMessage::SendPrompt { user_input: "hello".to_string(), attachments: vec![] }]),
        )
        .await;

        let has_hello = app
            .conversation_screen
            .conversation
            .segments()
            .any(|seg| matches!(seg, SegmentContent::UserMessage(text) if text == "hello"));
        assert!(has_hello, "conversation buffer should contain the user input");
    }

    #[tokio::test]
    async fn unsupported_media_is_blocked_locally() {
        let (mut app, mut rx) = make_app_with_config_and_capabilities_recording(
            &image_model_options(),
            acp::PromptCapabilities::new().image(true).audio(false),
        );
        let mut commands = Vec::new();
        let temp = tempfile::tempdir().unwrap();
        let audio_path = temp.path().join("clip.wav");
        std::fs::write(&audio_path, b"fake wav").unwrap();

        app.handle_conversation_messages(
            &mut commands,
            Some(vec![ConversationScreenMessage::SendPrompt {
                user_input: "listen".to_string(),
                attachments: vec![PromptAttachment { path: audio_path, display_name: "clip.wav".to_string() }],
            }]),
        )
        .await;

        assert!(rx.try_recv().is_err(), "prompt should be blocked locally");
        assert!(!app.conversation_screen.waiting_for_response);
        let messages: Vec<_> = app
            .conversation_screen
            .conversation
            .segments()
            .filter_map(|segment| match segment {
                SegmentContent::UserMessage(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(messages.iter().any(|text| text == "listen"));
        assert!(messages.iter().any(|text| text == "[audio attachment: clip.wav]"));
        assert!(messages.iter().any(|text| {
            text == "[wisp] ACP agent does not support audio input."
                || text == "[wisp] Current model selection does not support audio input."
        }));
    }

    #[test]
    fn replayed_media_user_chunks_render_placeholders() {
        use crate::components::conversation_window::SegmentContent;
        let mut app = make_app();

        app.on_session_update(&acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Image(
            acp::ImageContent::new("aW1n", "image/png"),
        ))));
        app.on_session_update(&acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Audio(
            acp::AudioContent::new("YXVkaW8=", "audio/wav"),
        ))));

        let segments: Vec<_> = app.conversation_screen.conversation.segments().collect();
        assert!(matches!(
            segments[0],
            SegmentContent::UserMessage(text) if text == "[image attachment]"
        ));
        assert!(matches!(
            segments[1],
            SegmentContent::UserMessage(text) if text == "[audio attachment]"
        ));
    }

    #[test]
    fn prompt_composer_open_settings() {
        let mut app = make_app();
        let mut commands = Vec::new();
        tokio::runtime::Runtime::new().unwrap().block_on(
            app.handle_conversation_messages(&mut commands, Some(vec![ConversationScreenMessage::OpenSettings])),
        );
        assert!(app.settings_overlay.is_some(), "settings overlay should be opened");
    }

    #[test]
    fn settings_overlay_close_clears_overlay() {
        let mut app = make_app();
        app.open_settings_overlay();
        app.settings_overlay = None;
        assert!(app.settings_overlay.is_none(), "close should clear overlay");
    }

    #[tokio::test]
    async fn tick_advances_spinner_animations() {
        let mut app = make_app();
        let tool_call = acp::ToolCall::new("tool-1".to_string(), "test_tool");
        app.conversation_screen.tool_call_statuses.on_tool_call(&tool_call);
        app.conversation_screen.progress_indicator.update(0, 1, true);

        let ctx = ViewContext::new((80, 24));
        let tool_before = app.conversation_screen.tool_call_statuses.render_tool("tool-1", &ctx);
        let prog_before = app.conversation_screen.progress_indicator.render(&ctx);

        app.on_event(&Event::Tick).await;

        let tool_after = app.conversation_screen.tool_call_statuses.render_tool("tool-1", &ctx);
        let prog_after = app.conversation_screen.progress_indicator.render(&ctx);

        assert_ne!(
            tool_before.lines()[0].plain_text(),
            tool_after.lines()[0].plain_text(),
            "tick should advance tool spinner"
        );
        assert_ne!(
            prog_before.lines()[1].plain_text(),
            prog_after.lines()[1].plain_text(),
            "tick should advance progress spinner"
        );
    }

    #[test]
    fn on_prompt_error_clears_waiting_state() {
        let mut app = make_app();
        app.conversation_screen.waiting_for_response = true;
        app.conversation_screen.on_prompt_error(&acp::Error::internal_error());
        assert!(!app.conversation_screen.waiting_for_response);
        assert!(!app.exit_requested());
    }

    #[test]
    fn auth_events_and_connection_close_exit_behavior() {
        let mut app =
            make_app_with_auth(vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new("anthropic", "Anthropic"))]);
        app.on_authenticate_complete("anthropic");
        assert!(!app.exit_requested(), "authenticate_complete should not exit");

        let mut app = make_app();
        app.on_authenticate_failed("anthropic", "bad token");
        assert!(!app.exit_requested(), "authenticate_failed should not exit");

        let mut app = make_app();
        app.on_acp_event(AcpEvent::ConnectionClosed);
        assert!(app.exit_requested(), "connection_closed should exit");
    }

    #[tokio::test]
    async fn clear_screen_returns_clear_command() {
        let mut app = make_app();
        let mut commands = Vec::new();
        app.handle_conversation_messages(&mut commands, Some(vec![ConversationScreenMessage::ClearScreen])).await;
        assert!(
            commands.iter().any(|c| matches!(c, RendererCommand::ClearScreen)),
            "should contain ClearScreen command"
        );
    }

    #[tokio::test]
    async fn cancel_sends_directly_via_prompt_handle() {
        let mut app = make_app();
        app.conversation_screen.waiting_for_response = true;
        send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE).await;
        assert!(!app.exit_requested());
    }

    #[test]
    fn new_session_restores_changed_config_selections() {
        use acp_utils::client::PromptCommand;

        let (mut app, mut rx) = make_app_with_config_recording(&mode_model_options("Planner", "gpt-4o"));
        app.update_config_options(&mode_model_options("Coder", "gpt-4o"));

        app.on_acp_event(AcpEvent::NewSessionCreated {
            session_id: SessionId::new("new-session"),
            config_options: mode_model_options("Planner", "gpt-4o"),
        });

        assert_eq!(app.session_id, SessionId::new("new-session"));
        assert!(app.context_usage.is_none());

        let cmd = rx.try_recv().expect("expected a SetConfigOption command");
        match cmd {
            PromptCommand::SetConfigOption { config_id, value, .. } => {
                assert_eq!(config_id, "mode");
                assert_eq!(value, "Coder");
            }
            other => panic!("expected SetConfigOption, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "model was unchanged, no extra command expected");
    }

    #[tokio::test]
    async fn url_completion_appends_status_text_for_known_pending_id() {
        let mut app = make_app();

        app.conversation_screen.pending_url_elicitations.insert(("github".to_string(), "el-1".to_string()));

        let params = acp_utils::notifications::UrlElicitationCompleteParams {
            server_name: "github".to_string(),
            elicitation_id: "el-1".to_string(),
        };
        app.conversation_screen.on_url_elicitation_complete(&params);

        let messages: Vec<_> = app
            .conversation_screen
            .conversation
            .segments()
            .filter_map(|seg| match seg {
                SegmentContent::UserMessage(text) if text.contains("github") && text.contains("finished") => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(messages.len(), 1, "should show completion message for known ID");
        assert!(messages[0].to_lowercase().contains("retry"), "completion message should mention retry");
    }

    #[tokio::test]
    async fn url_completion_ignores_unknown_id() {
        let mut app = make_app();

        // No pending elicitations registered
        let params = acp_utils::notifications::UrlElicitationCompleteParams {
            server_name: "unknown-server".to_string(),
            elicitation_id: "el-unknown".to_string(),
        };
        app.conversation_screen.on_url_elicitation_complete(&params);

        let has_completion = app
            .conversation_screen
            .conversation
            .segments()
            .any(|seg| matches!(seg, SegmentContent::UserMessage(t) if t.contains("finished")));
        assert!(!has_completion, "should not show completion message for unknown ID");
    }

    #[tokio::test]
    async fn url_completion_ignores_mismatched_server_name_for_known_id() {
        let mut app = make_app();

        app.conversation_screen.pending_url_elicitations.insert(("github".to_string(), "el-1".to_string()));

        let params = acp_utils::notifications::UrlElicitationCompleteParams {
            server_name: "linear".to_string(),
            elicitation_id: "el-1".to_string(),
        };
        app.conversation_screen.on_url_elicitation_complete(&params);

        assert!(
            app.conversation_screen.pending_url_elicitations.contains(&("github".to_string(), "el-1".to_string())),
            "mismatched server name should not clear the pending elicitation"
        );
        let has_completion = app
            .conversation_screen
            .conversation
            .segments()
            .any(|seg| matches!(seg, SegmentContent::UserMessage(t) if t.contains("finished")));
        assert!(!has_completion, "should not show completion message for mismatched server name");
    }

    #[tokio::test]
    async fn url_completion_ignores_duplicate_id() {
        let mut app = make_app();

        app.conversation_screen.pending_url_elicitations.insert(("github".to_string(), "el-1".to_string()));

        let params = acp_utils::notifications::UrlElicitationCompleteParams {
            server_name: "github".to_string(),
            elicitation_id: "el-1".to_string(),
        };

        // First completion should show message
        app.conversation_screen.on_url_elicitation_complete(&params);
        // Second completion should be silently ignored (ID already removed)
        app.conversation_screen.on_url_elicitation_complete(&params);

        let count = app
            .conversation_screen
            .conversation
            .segments()
            .filter(|seg| matches!(seg, SegmentContent::UserMessage(t) if t.contains("finished")))
            .count();
        assert_eq!(count, 1, "should show exactly one completion message, not duplicates");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ctrl_g_blocked_during_url_elicitation_modal() {
        LocalSet::new()
            .run_until(async {
                let mut app = make_app();
                let (cx, mut peer) = test_connection().await;
                let (responder, _rx) = peer.fake_elicitation(&cx).await;
                app.conversation_screen.active_modal = Some(Modal::Elicitation(ElicitationForm::from_params(
                    acp_utils::notifications::ElicitationParams {
                        server_name: "test-server".to_string(),
                        request: acp_utils::notifications::CreateElicitationRequestParams::UrlElicitationParams {
                            meta: None,
                            message: "Auth".to_string(),
                            url: "https://example.com/auth".to_string(),
                            elicitation_id: "el-1".to_string(),
                        },
                    },
                    responder,
                )));

                send_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL).await;
                assert!(!app.screen_router.is_git_diff(), "git diff should not open during URL elicitation modal");
            })
            .await;
    }

    #[tokio::test]
    async fn reset_after_context_cleared_clears_pending_url_elicitations() {
        let mut app = make_app();
        app.conversation_screen.pending_url_elicitations.insert(("github".to_string(), "el-1".to_string()));
        app.conversation_screen.pending_url_elicitations.insert(("linear".to_string(), "el-2".to_string()));

        app.conversation_screen.reset_after_context_cleared();

        assert!(
            app.conversation_screen.pending_url_elicitations.is_empty(),
            "pending URL elicitations should be cleared on reset"
        );
    }

    #[tokio::test]
    async fn first_ctrl_c_does_not_exit() {
        let mut app = make_app();
        send_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(!app.exit_requested(), "first Ctrl-C should not exit");
        assert!(app.exit_confirmation_active(), "first Ctrl-C should activate confirmation");
    }

    #[tokio::test]
    async fn second_ctrl_c_exits() {
        let mut app = make_app();
        send_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(!app.exit_requested());
        send_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).await;
        assert!(app.exit_requested(), "second Ctrl-C should exit");
    }

    #[tokio::test]
    async fn ctrl_c_confirmation_expires_on_tick() {
        let mut app = make_app();
        app.ctrl_c_pressed_at = Some(Instant::now().checked_sub(Duration::from_secs(4)).unwrap());
        assert!(app.exit_confirmation_active());
        app.on_event(&Event::Tick).await;
        assert!(!app.exit_confirmation_active(), "confirmation should expire after timeout");
    }

    #[test]
    fn status_line_shows_warning_when_confirmation_active() {
        use crate::components::status_line::StatusLine;
        let options = vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            "m1",
            vec![acp::SessionConfigSelectOption::new("m1", "M1")],
        )];
        let workspace_status = test_workspace_status();
        let status = StatusLine {
            workspace_status: &workspace_status,
            agent_name: "test-agent",
            config_options: &options,
            context_usage: None,
            waiting_for_response: false,
            unhealthy_server_count: 0,
            content_padding: DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: true,
        };
        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("Ctrl-C again to exit"), "should show warning, got: {text}");
        assert!(!text.contains("test-agent"), "should not show agent name during confirmation, got: {text}");
    }
}
