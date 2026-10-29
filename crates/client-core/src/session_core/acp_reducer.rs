use super::{CommandEntry, Effect, ForegroundOperation, SessionCore, TurnEnd};
use crate::elicitation::{Elicitation, ElicitationId, PendingElicitation, form_fields};
use crate::session_model::SessionSetting;
use acp_utils::client::AcpEvent;
use acp_utils::conversation::TurnFinished;
use acp_utils::elicitation::source_mcp_server_name;
use acp_utils::notifications::McpNotification;
use agent_client_protocol::schema::v2::{
    self as acp, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction, ElicitationMode, SessionUpdate,
};
use utils::artifact_review::ArtifactReviewElicitationMeta;

impl SessionCore {
    pub(super) fn on_acp_event(&mut self, event: AcpEvent) {
        let current = self.is_current(&event);
        if let Some(TurnFinished { stop_reason }) = self.conversation.apply_event(&event) {
            self.end_turn(match stop_reason {
                Some(acp::StopReason::Cancelled) => TurnEnd::Cancelled,
                _ => TurnEnd::Finished,
            });
        }
        match event {
            AcpEvent::SessionUpdate(notification) if current => self.on_session_update(&notification.update),
            AcpEvent::ContextCleared(_) if current => {
                self.foreground.take_prepared_prompt();
            }
            AcpEvent::ElicitationRequest { params, responder } => {
                if let Some(elicitation) = classify(*params) {
                    self.elicitation = Some(PendingElicitation::new(ElicitationId::next(), elicitation, responder));
                    self.effects.push(Effect::ElicitationRequested);
                } else {
                    tracing::warn!("Declining an elicitation whose mode or schema is unsupported");
                    let _ = responder.respond(CreateElicitationResponse::new(ElicitationAction::Decline));
                }
            }
            AcpEvent::McpNotification(notification) => self.on_mcp_notification(&notification),
            AcpEvent::AuthMethodsUpdated(params) => {
                self.session.set_auth_methods(&params.auth_methods);
                self.settings_changed(SessionSetting::AuthMethods);
            }
            AcpEvent::ConnectionClosed => {
                self.foreground = ForegroundOperation::Idle;
                self.ended_turn = None;
                self.dismiss_elicitation();
                self.git_review = super::GitReview::Closed;
                self.effects.push(Effect::ConnectionClosed);
            }
            AcpEvent::GitDiffEvent(_) | AcpEvent::SessionUpdate(_) | AcpEvent::ContextCleared(_) => {}
        }
        if current {
            self.announce_if_settled();
        }
    }

    fn on_mcp_notification(&mut self, notification: &McpNotification) {
        let McpNotification::ServerStatus { servers } = notification;
        self.session.update_server_statuses(servers);
        self.settings_changed(SessionSetting::ServerStatuses);
    }

    fn is_current(&self, event: &AcpEvent) -> bool {
        event.session_id().is_none_or(|session_id| session_id == self.session_id())
            || matches!(self.foreground, ForegroundOperation::CreatingSession { .. })
    }

    fn on_session_update(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.available_commands = update.available_commands.iter().map(command_entry).collect();
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.session.update_config_options(update.config_options.clone());
                self.settings_changed(SessionSetting::ConfigOptions);
            }
            _ => {}
        }
    }
}

fn classify(request: CreateElicitationRequest) -> Option<Elicitation> {
    let source = source_mcp_server_name(request.meta.as_ref()).unwrap_or("Agent").to_owned();
    let message = request.message;
    match request.mode {
        ElicitationMode::Form(form) => match ArtifactReviewElicitationMeta::from_meta(request.meta.as_ref()) {
            Some(meta) => Some(Elicitation::ArtifactReview(meta)),
            None => form_fields(&form.requested_schema).map(|fields| Elicitation::Form { source, message, fields }),
        },
        ElicitationMode::Url(url) => Some(Elicitation::Url { source, message, url: url.url }),
        _ => None,
    }
}

fn command_entry(command: &acp::AvailableCommand) -> CommandEntry {
    CommandEntry {
        name: command.name.clone(),
        description: command.description.clone(),
        has_input: command.input.is_some(),
        hint: match &command.input {
            Some(acp::AvailableCommandInput::Text(input)) => Some(input.hint.clone()),
            _ => None,
        },
    }
}
