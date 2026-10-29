use super::{Effect, ForegroundOperation, SessionCore, TurnEnd};
use crate::command::RequestId;
use crate::command::{AgentCommand, Command, PromptRejection};
use crate::composer::attachment::AttachmentOutcome;
use crate::composer::submission::Submission;
use acp_utils::notifications::PromptSearchParams;
use agent_client_protocol::schema::v2 as acp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPrompt {
    pub request_id: RequestId,
    pub submission: Submission,
}

impl SessionCore {
    pub fn submit(&mut self, submission: Submission) {
        if !self.foreground.is_idle() {
            self.effects.push(Effect::RestoreSubmission(submission));
            return;
        }
        let attachments = submission.attachments();
        self.foreground = ForegroundOperation::PreparingPrompt(submission);
        if attachments.is_empty() {
            self.finish_submission(AttachmentOutcome::default());
        } else {
            let access = self.session.workspace_access();
            self.effects.push(Effect::Dispatch(Command::PrepareSubmission { attachments, access }));
        }
    }

    pub fn submit_review(&mut self, prompt: &str) {
        if !self.foreground.is_idle() {
            self.notify("Cannot submit review while a prompt or session transition is in progress");
            return;
        }
        self.notify(&format!("Submitted review of working tree diff.\n{prompt}"));
        self.start_prompt(Submission::from(prompt), Vec::new());
        self.effects.push(Effect::ReviewSubmitted);
    }

    pub fn search_prompts(&mut self, query: &str) {
        if self.prompt_search.set_query(query) {
            self.dispatch(AgentCommand::SearchPrompts(PromptSearchParams { query: query.to_owned(), limit: None }));
        } else {
            self.effects.push(Effect::PromptSearchUpdated);
        }
    }

    pub(super) fn finish_submission(&mut self, outcome: AttachmentOutcome) {
        let Some(submission) = self.foreground.take_prepared_prompt() else {
            return;
        };
        let media_error = self.session.media_support_error(&outcome.blocks);
        if media_error.is_some() {
            self.effects.push(Effect::RestoreSubmission(submission));
        } else {
            self.start_prompt(submission, outcome.blocks);
        }
        for warning in &outcome.warnings {
            self.notify(warning);
        }
        if let Some(message) = media_error {
            self.report_error(&message);
        }
    }

    pub(super) fn finish_prompt(&mut self, request_id: RequestId, result: Result<(), PromptRejection>) {
        let Some(index) = self.queued_prompts.iter().position(|prompt| prompt.request_id == request_id) else {
            return;
        };
        let prompt = self.queued_prompts.remove(index);
        if let Err(rejection) = result {
            self.effects.push(Effect::RestoreSubmission(prompt.submission));
            match rejection {
                PromptRejection::Cancelled => self.end_turn(TurnEnd::Cancelled),
                PromptRejection::Failed(error) => self.report_error(&format!("Failed to send prompt: {error}")),
            }
        }
        self.announce_if_settled();
    }

    fn start_prompt(&mut self, submission: Submission, attachments: Vec<acp::ContentBlock>) {
        let request_id = RequestId::next();
        let text = (!submission.text.trim().is_empty()).then(|| acp::ContentBlock::from(submission.text.clone()));
        let content = text.into_iter().chain(attachments).collect();
        self.queued_prompts.push(QueuedPrompt { request_id, submission });
        self.dispatch(AgentCommand::Prompt { request_id, session_id: self.session_id().clone(), content });
    }
}
