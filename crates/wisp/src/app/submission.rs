use super::{App, ForegroundOperation};
use crate::attachment::AttachmentOutcome;
use crate::command::{AgentCommand, Command, FilesystemCommand, PromptRejection};
use crate::request::RequestId;
use crate::session::session_config_view::LocalConfigView;
use crate::surfaces::composer::Submission;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::v2 as acp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPrompt {
    pub request_id: RequestId,
    pub submission: Submission,
}

impl App {
    pub(super) fn submit(&mut self) {
        if self.composer.is_empty() || !self.foreground.is_idle() {
            return;
        }

        if self.session.workspace_access() == crate::session::WorkspaceAccess::Remote
            && (!self.composer.selected_mentions().is_empty() || !self.composer.pending_media().is_empty())
        {
            self.notify("Path attachments are unavailable for remote workspaces; remove attachments before sending");
            return;
        }
        let submission = self.composer.take_submission();
        let attachments = submission.attachments();

        self.foreground = ForegroundOperation::PreparingPrompt(submission);
        if attachments.is_empty() {
            self.finish_submission(AttachmentOutcome { blocks: Vec::new(), warnings: Vec::new() });
        } else {
            self.queue(Command::Filesystem(FilesystemCommand::PrepareSubmission { attachments }));
        }
    }

    pub(super) fn finish_submission(&mut self, outcome: AttachmentOutcome) {
        let Some(submission) = self.foreground.take_prepared_prompt() else {
            return;
        };
        let media_error = self.media_support_error(&outcome.blocks);
        if media_error.is_some() {
            self.composer.restore_submission(submission);
        } else {
            let content = (!outcome.blocks.is_empty()).then_some(outcome.blocks);
            self.start_prompt(submission, content);
        }
        for warning in &outcome.warnings {
            self.notify(warning);
        }
        if let Some(message) = media_error {
            self.notify(&message);
        }
    }

    pub(super) fn finish_prompt(&mut self, request_id: RequestId, result: Result<(), PromptRejection>) {
        let Some(index) = self.queued_prompts.iter().position(|prompt| prompt.request_id == request_id) else {
            return;
        };
        let prompt = self.queued_prompts.remove(index);
        let Err(rejection) = result else { return };
        self.composer.restore_submission(prompt.submission);
        if let PromptRejection::Failed(error) = rejection {
            self.notify(&format!("Failed to send prompt: {error}"));
        }
    }

    fn media_support_error(&self, blocks: &[acp::ContentBlock]) -> Option<String> {
        let requires_image = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)));
        let requires_audio = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Audio(_)));

        if !requires_image && !requires_audio {
            return None;
        }

        if requires_image && self.session.prompt_capabilities().image.is_none() {
            return Some("ACP agent does not support image input.".to_string());
        }
        if requires_audio && self.session.prompt_capabilities().audio.is_none() {
            return Some("ACP agent does not support audio input.".to_string());
        }

        let config = LocalConfigView::new(self.session.config_options());
        let values = config.current_values(ConfigOptionId::Model);
        if values.is_empty() {
            return None;
        }
        let selected_meta = config.selected_model_metadata();

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

    pub(super) fn send_prompt_search_query(&mut self, query: String) {
        if self.composer.prompt_search().is_none() {
            return;
        }
        let params = acp_utils::notifications::PromptSearchParams { query, limit: None };
        self.queue(Command::Agent(AgentCommand::SearchPrompts(params)));
    }
}
