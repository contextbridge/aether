use super::{Effect, Elicitation, ForegroundOperation, SessionCore, TurnEnd};
use crate::command::{Command, GitReviewCommand};
use crate::command_entry::{CommandEntry, builtin_commands};
use acp_utils::client::AcpEvent;
use acp_utils::conversation::TurnFinished;
use acp_utils::notifications::McpNotification;
use agent_client_protocol::schema::v2::{self as acp, CreateElicitationRequest, ElicitationMode, SessionUpdate};
use utils::artifact_review::ArtifactReviewElicitationMeta;

impl SessionCore {
    pub(super) fn reduce_acp_event(&mut self, event: AcpEvent) {
        let current = self.is_current(&event);
        if current && let Some(TurnFinished { stop_reason }) = self.conversation.apply_event(&event) {
            self.end_turn(match stop_reason {
                Some(acp::StopReason::Cancelled) => TurnEnd::Cancelled,
                _ => TurnEnd::Finished,
            });
        }
        match event {
            AcpEvent::SessionUpdate(notification) if current => self.on_session_update(&notification.update),
            AcpEvent::ContextCleared(_) if current => self.foreground.drop_prepared_prompt(),
            AcpEvent::ElicitationRequest { params, responder } => {
                let elicitation = match artifact_review_meta(&params) {
                    Some(meta) => Elicitation::ArtifactReview(meta),
                    None => Elicitation::Request(params),
                };
                self.effects.push(Effect::ShowElicitation { elicitation, responder: responder.into() });
            }
            AcpEvent::McpNotification(notification) => self.on_mcp_notification(&notification),
            AcpEvent::GitDiffEvent(params) => {
                self.effects.push(Effect::Dispatch(Command::GitReview(GitReviewCommand::Forward(params.event))));
            }
            AcpEvent::AuthMethodsUpdated(params) => {
                self.session.set_auth_methods(&params.auth_methods);
                self.effects.push(Effect::AuthMethodsChanged);
            }
            AcpEvent::ConnectionClosed => {
                self.foreground = ForegroundOperation::Idle;
                self.ended_turn = None;
                self.effects.push(Effect::ConnectionClosed);
            }
            AcpEvent::SessionUpdate(_) | AcpEvent::ContextCleared(_) | AcpEvent::SubAgentProgress(_) => {}
        }
        if current {
            self.announce_if_settled();
        }
    }

    /// Server notifications feed the status summary hosts show.
    fn on_mcp_notification(&mut self, notification: &McpNotification) {
        let McpNotification::ServerStatus { servers } = notification;
        self.session.update_server_statuses(servers);
        self.effects.push(Effect::ServerStatusesChanged);
    }

    /// Whether an event concerns the attached session. A session being created has no id yet, so every
    /// event is taken to be for it.
    fn is_current(&self, event: &AcpEvent) -> bool {
        event.session_id().is_none_or(|session_id| session_id == self.session.session_id())
            || matches!(self.foreground, ForegroundOperation::CreatingSession { .. })
    }

    fn on_session_update(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AvailableCommandsUpdate(update) => {
                let mut all = builtin_commands(self.session.capabilities());
                all.extend(update.available_commands.iter().map(CommandEntry::from_acp));
                self.available_commands = all;
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.session.update_config_options(update.config_options.clone());
                self.effects.push(Effect::ConfigOptionsChanged);
            }
            _ => {}
        }
    }
}

fn artifact_review_meta(params: &CreateElicitationRequest) -> Option<ArtifactReviewElicitationMeta> {
    if !matches!(params.mode, ElicitationMode::Form(_)) {
        return None;
    }
    ArtifactReviewElicitationMeta::from_meta(params.meta.as_ref())
}
