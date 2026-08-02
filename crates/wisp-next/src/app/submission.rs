use super::App;
use crate::session::session_config_view::SessionConfigView;
use crate::session::tasks::Task;
use crate::surfaces::attachments::{AttachmentOutcome, PromptAttachment};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema as acp;

impl App {
    pub(super) fn submit(&mut self) {
        if self.composer.is_empty() || self.turn.prompt_in_flight || self.pending_submission.is_some() {
            return;
        }

        let mentions = self.composer.selected_mentions();
        let (text, pending_media) = self.composer.take_submission();
        let mut all_attachments: Vec<PromptAttachment> =
            mentions.into_iter().map(|m| PromptAttachment { path: m.path, display_name: m.display_name }).collect();
        all_attachments.extend(pending_media);

        self.pending_submission = Some(super::PendingSubmission { text });
        if all_attachments.is_empty() {
            self.finish_submission(AttachmentOutcome {
                blocks: Vec::new(),
                placeholders: Vec::new(),
                warnings: Vec::new(),
            });
        } else {
            self.spawn(Task::PrepareSubmission { attachments: all_attachments });
        }
    }

    pub(super) fn finish_submission(&mut self, outcome: AttachmentOutcome) {
        let Some(pending) = self.pending_submission.take() else {
            return;
        };
        let text = pending.text;
        self.conversation.transcript.push_user_message(&text);
        for placeholder in &outcome.placeholders {
            self.conversation.transcript.push_user_message(placeholder);
        }
        for warning in &outcome.warnings {
            self.notify(warning);
        }

        if let Some(message) = self.media_support_error(&outcome.blocks) {
            self.notify(&message);
            return;
        }

        self.turn.prompt_in_flight = true;
        self.turn.submitted_prompt_count = self.turn.submitted_prompt_count.saturating_add(1);
        let content = (!outcome.blocks.is_empty()).then_some(outcome.blocks);
        if let Err(error) = self.agent.handle.prompt(&self.agent.session_id, &text, content) {
            tracing::error!("failed to send prompt: {error}");
            self.turn.prompt_in_flight = false;
            self.notify(&format!("Failed to send prompt: {error}"));
        }
    }
    fn media_support_error(&self, blocks: &[acp::ContentBlock]) -> Option<String> {
        let requires_image = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Image(_)));
        let requires_audio = blocks.iter().any(|block| matches!(block, acp::ContentBlock::Audio(_)));

        if !requires_image && !requires_audio {
            return None;
        }

        if requires_image && !self.agent.prompt_capabilities.image {
            return Some("ACP agent does not support image input.".to_string());
        }
        if requires_audio && !self.agent.prompt_capabilities.audio {
            return Some("ACP agent does not support audio input.".to_string());
        }

        let config = SessionConfigView::new(&self.agent.config_options);
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
        let Some(generation) = self.composer.prompt_search().map(|picker| picker.search_generation()) else {
            return;
        };
        let params =
            acp_utils::notifications::PromptSearchParams { query, limit: None, search_generation: generation.get() };
        if let Err(error) = self.agent.handle.search_prompts(params)
            && let Some(picker) = self.composer.prompt_search()
        {
            picker.on_failed(generation, format!("search failed: {error}"));
        }
    }
}
