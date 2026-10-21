use super::{Effect, ForegroundOperation, SessionCore, TurnEnd};
use crate::command::{AgentCommand, Command, FilesystemCommand, PromptRejection};
use crate::composer::attachment::AttachmentOutcome;
use crate::composer::submission::Submission;
use crate::request::RequestId;
use acp_utils::notifications::PromptSearchParams;
use agent_client_protocol::schema::v2 as acp;

/// A prompt sent to the agent and not yet answered, kept so a rejection can
/// give the submission back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPrompt {
    pub request_id: RequestId,
    pub submission: Submission,
}

impl SessionCore {
    /// Sends `submission` as a prompt, reading its attachments first. While
    /// another foreground operation runs the submission is handed straight
    /// back through [`Effect::RestoreSubmission`].
    pub fn submit(&mut self, submission: Submission) -> Vec<Effect> {
        if !self.foreground.is_idle() {
            self.effects.push(Effect::RestoreSubmission(submission));
            return self.take_effects();
        }
        let attachments = submission.attachments();
        self.foreground = ForegroundOperation::PreparingPrompt(submission);
        if attachments.is_empty() {
            self.finish_submission(AttachmentOutcome { blocks: Vec::new(), warnings: Vec::new() });
        } else {
            self.effects
                .push(Effect::Dispatch(Command::Filesystem(FilesystemCommand::PrepareSubmission { attachments })));
        }
        self.take_effects()
    }

    /// Sends a git review as a normal prompt, queued behind any running turn.
    /// While a prompt is being prepared or a session transition runs, the
    /// review is not sent and a notice says so.
    pub fn submit_review(&mut self, prompt: &str) -> Vec<Effect> {
        if !self.foreground.is_idle() {
            self.raise_notice("Cannot submit review while a prompt or session transition is in progress");
            return self.take_effects();
        }
        self.raise_notice(&format!("Submitted review of working tree diff.\n{prompt}"));
        self.start_prompt(Submission { text: prompt.to_string(), ..Submission::default() }, None);
        self.effects.push(Effect::ReviewSubmitted);
        self.take_effects()
    }

    /// Searches the agent's prompt history for `query`. Results arrive as
    /// [`Effect::PromptSearchResults`], echoing the query they answer.
    pub fn search_prompts(&mut self, query: String) -> Vec<Effect> {
        self.dispatch(AgentCommand::SearchPrompts(PromptSearchParams { query, limit: None }));
        self.take_effects()
    }

    pub(super) fn finish_submission(&mut self, outcome: AttachmentOutcome) {
        let Some(submission) = self.foreground.take_prepared_prompt() else {
            return;
        };
        let media_error = self.session.media_support_error(&outcome.blocks);
        if media_error.is_some() {
            self.effects.push(Effect::RestoreSubmission(submission));
        } else {
            let content = (!outcome.blocks.is_empty()).then_some(outcome.blocks);
            self.start_prompt(submission, content);
        }
        for warning in &outcome.warnings {
            self.raise_notice(warning);
        }
        if let Some(message) = media_error {
            self.raise_notice(&message);
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
                PromptRejection::Failed(error) => self.raise_notice(&format!("Failed to send prompt: {error}")),
            }
        }
        self.announce_if_settled();
    }

    fn start_prompt(&mut self, submission: Submission, content: Option<Vec<acp::ContentBlock>>) {
        let request_id = RequestId::next();
        let text = submission.text.clone();
        self.queued_prompts.push(QueuedPrompt { request_id, submission });
        self.dispatch(AgentCommand::Prompt {
            request_id,
            session_id: self.session.session_id().clone(),
            text,
            content,
        });
    }
}
