use super::App;
use crate::command::{AgentCommand, Command, FilesystemCommand};
use crate::session::session_config_view::LocalConfigView;
use crate::attachment::{AttachmentOutcome, PromptAttachment};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::v1 as acp;

#[derive(Default)]
pub(super) enum SubmissionState {
    #[default]
    Idle,
    Preparing(String),
}

impl SubmissionState {
    fn take(&mut self) -> Option<String> {
        match std::mem::take(self) {
            Self::Preparing(text) => Some(text),
            Self::Idle => None,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::Idle;
    }
}

impl App {
    pub(super) fn submit(&mut self) {
        if self.composer.is_empty() || self.waiting_for_response() || !matches!(self.submission, SubmissionState::Idle) {
            return;
        }

        let mentions = self.composer.selected_mentions();
        let (text, pending_media) = self.composer.take_submission();
        let mut all_attachments: Vec<PromptAttachment> =
            mentions.into_iter().map(|m| PromptAttachment { path: m.path, display_name: m.display_name }).collect();
        all_attachments.extend(pending_media);

        self.submission = SubmissionState::Preparing(text);
        if all_attachments.is_empty() {
            self.finish_submission(AttachmentOutcome {
                blocks: Vec::new(),
                placeholders: Vec::new(),
                warnings: Vec::new(),
            });
        } else {
            self.queue(Command::Filesystem(FilesystemCommand::PrepareSubmission { attachments: all_attachments }));
        }
    }

    pub(super) fn finish_submission(&mut self, outcome: AttachmentOutcome) {
        let Some(text) = self.submission.take() else {
            return;
        };
        self.conversation.append_user_content(&text);
        for placeholder in &outcome.placeholders {
            self.conversation.append_user_content(placeholder);
        }
        for warning in &outcome.warnings {
            self.notify(warning);
        }

        if let Some(message) = self.media_support_error(&outcome.blocks) {
            self.notify(&message);
            return;
        }

        let content = (!outcome.blocks.is_empty()).then_some(outcome.blocks);
        self.start_prompt(text, content);
    }
    fn media_support_error(&self, blocks: &[acp::ContentBlock]) -> Option<String> {
        let requires_image = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)));
        let requires_audio = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Audio(_)));

        if !requires_image && !requires_audio {
            return None;
        }

        if requires_image && !self.session.prompt_capabilities().image {
            return Some("ACP agent does not support image input.".to_string());
        }
        if requires_audio && !self.session.prompt_capabilities().audio {
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
