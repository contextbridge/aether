use super::{App, ConfigOptionId, PromptAttachment, SessionConfigView, acp};

impl App {
    pub(super) fn submit(&mut self) {
        if self.composer.is_empty() || self.prompt_in_flight {
            return;
        }

        let mentions = self.composer.selected_mentions();
        let (text, pending_media) = self.composer.take_submission();
        let mut all_attachments: Vec<PromptAttachment> =
            mentions.into_iter().map(|m| PromptAttachment { path: m.path, display_name: m.display_name }).collect();
        all_attachments.extend(pending_media);

        let outcome = crate::attachments::build_attachments(&all_attachments);
        self.transcript.push_user_message(&text);
        for placeholder in &outcome.placeholders {
            self.transcript.push_user_message(placeholder);
        }
        for warning in &outcome.warnings {
            self.transcript.push_user_message(&format!("[wisp-next] {warning}"));
        }

        if let Some(message) = self.media_support_error(&outcome.blocks) {
            self.transcript.push_user_message(&format!("[wisp-next] {message}"));
            return;
        }

        self.prompt_in_flight = true;
        self.submitted_prompt_count = self.submitted_prompt_count.saturating_add(1);
        let content = (!outcome.blocks.is_empty()).then_some(outcome.blocks);
        if let Err(e) = self.prompt_handle.prompt(&self.session_id, &text, content) {
            tracing::error!("failed to send prompt: {e}");
            self.prompt_in_flight = false;
            self.transcript.push_user_message(&format!("[wisp-next] Failed to send prompt: {e}"));
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

        let config = SessionConfigView::new(&self.config_options);
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
        let Some(generation) = self.composer.prompt_search_generation() else {
            return;
        };
        let params = acp_utils::notifications::PromptSearchParams { query, limit: None, search_generation: generation };
        if let Err(e) = self.prompt_handle.search_prompts(params) {
            self.composer.prompt_search_on_failed(generation, format!("search failed: {e}"));
        }
    }
}
