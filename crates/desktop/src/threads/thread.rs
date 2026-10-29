use super::driver::{DriverMessage, ThreadDriverHandle};
use super::status::ThreadStatus;
use crate::history::{UNTITLED, git_root};
use crate::target::{AgentConnector, SessionTarget};
use acp_utils::conversation::ConversationContent;
use aether_sessions::prompt_title;
use agent_client_protocol::schema::v2::{self as acp, SessionId};
use chrono::{DateTime, Utc};
use clankerdiff_client::{ClientError, ClientState, LookupReply};
use client_core::command::Reply;
use client_core::composer::submission::Submission;
use client_core::elicitation::{Elicitation, ElicitationId};
use client_core::session_core::{Effect, SessionCore};
use client_core::session_model::SessionSetting;
use gpui::{App, Context, Entity, EventEmitter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadEvent {
    TurnFinished,
    NeedsInput,
    SubmissionRestored(Submission),
    ReviewActionFinished(Result<(), ClientError>),
    ReviewLookupFinished(LookupReply),
    ReviewSubmitted,
    PromptSearchUpdated,
}

pub struct Thread {
    number: usize,
    session_id: Option<SessionId>,
    target: SessionTarget,
    project: Option<PathBuf>,
    opened_at: DateTime<Utc>,
    connector: Arc<AgentConnector>,
    core: Option<SessionCore>,
    config_revision: u64,
    link: Link,
    driver: Option<ThreadDriverHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Link {
    Connecting,
    Connected,
    Failed(String),
    Closed,
    Stopped,
}

impl Thread {
    pub(crate) fn connect(
        number: usize,
        target: SessionTarget,
        session_id: Option<SessionId>,
        connector: Arc<AgentConnector>,
        opened_at: DateTime<Utc>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut thread = Self {
            number,
            session_id: session_id.clone(),
            project: target.cwd().map(git_root),
            opened_at,
            target,
            connector,
            core: None,
            config_revision: 0,
            link: Link::Connecting,
            driver: None,
        };
        thread.open_session(session_id, cx);
        thread
    }

    pub fn number(&self) -> usize {
        self.number
    }

    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    pub fn target(&self) -> &SessionTarget {
        &self.target
    }

    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    pub fn opened_at(&self) -> DateTime<Utc> {
        self.opened_at
    }

    pub fn title(&self) -> String {
        let prompt = self.core.as_ref().and_then(|core| {
            core.conversation().items().iter().find_map(|item| match item.content() {
                ConversationContent::User(blocks) => Some(blocks),
                _ => None,
            })
        });
        prompt.map_or_else(|| UNTITLED.to_owned(), |blocks| prompt_title(first_text(blocks)))
    }

    pub fn core(&self) -> Option<&SessionCore> {
        self.core.as_ref()
    }

    pub fn connected_core(&self) -> Option<&SessionCore> {
        self.core.as_ref().filter(|_| self.link == Link::Connected)
    }

    pub fn config_revision(&self) -> u64 {
        self.config_revision
    }

    pub fn status(&self) -> ThreadStatus {
        match &self.link {
            Link::Connecting => ThreadStatus::Connecting,
            Link::Failed(_) => ThreadStatus::Errored,
            Link::Closed => ThreadStatus::Disconnected,
            Link::Stopped => ThreadStatus::Dormant,
            Link::Connected => match self.core.as_ref() {
                Some(core) if core.elicitation().is_some() => ThreadStatus::NeedsInput,
                Some(core) if core.waiting_for_response() => ThreadStatus::Running,
                Some(core) if core.is_agent_busy() => ThreadStatus::Background,
                _ => ThreadStatus::Idle,
            },
        }
    }

    pub fn error(&self) -> Option<&str> {
        match &self.link {
            Link::Failed(error) => Some(error),
            _ => None,
        }
    }

    pub fn elicitation(&self) -> Option<(ElicitationId, &Elicitation)> {
        self.connected_core()?.elicitation()
    }

    pub fn pending_review(&self) -> Option<(ElicitationId, &ArtifactReviewElicitationMeta)> {
        match self.elicitation()? {
            (id, Elicitation::ArtifactReview(meta)) => Some((id, meta)),
            _ => None,
        }
    }

    pub fn review(&self) -> Option<&Arc<ClientState>> {
        self.connected_core()?.git_review()
    }

    pub fn changed_files(&self) -> Option<usize> {
        self.review()?.snapshot.as_ref().map(|snapshot| snapshot.files.len())
    }

    pub fn send(&mut self, submission: Submission, cx: &mut Context<Self>) {
        if self.connected_core().is_some() {
            self.update_core(cx, |core| core.submit(submission));
        } else {
            cx.emit(ThreadEvent::SubmissionRestored(submission));
        }
    }

    pub fn report_error(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(core) = self.core.as_mut() {
            core.report_error(message);
            cx.notify();
        }
    }

    pub fn can_search_prompts(&self) -> bool {
        self.connected_core().is_some_and(|core| core.session().capabilities().prompt_search)
    }

    pub(crate) fn update_core(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut SessionCore)) {
        if let Some(core) = self.core.as_mut().filter(|_| self.link == Link::Connected) {
            change(core);
            self.apply(cx);
        }
    }

    pub(crate) fn with_core(thread: &Entity<Self>, cx: &mut App, change: impl FnOnce(&mut SessionCore)) {
        thread.update(cx, |thread, cx| thread.update_core(cx, change));
    }

    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.link, Link::Failed(_) | Link::Closed) {
            return;
        }
        self.open_session(self.session_id.clone(), cx);
        cx.notify();
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if self.driver.take().is_none() {
            return;
        }
        self.link = Link::Stopped;
        cx.notify();
    }

    pub(super) fn on_driver_message(&mut self, message: DriverMessage, cx: &mut Context<Self>) {
        match message {
            DriverMessage::Launched(launched) => self.target.set_instance(launched),
            DriverMessage::Connected(config) => {
                self.session_id = Some(config.session_response.session_id.clone());
                self.core = Some(SessionCore::new(*config));
                self.config_revision += 1;
                self.link = Link::Connected;
            }
            DriverMessage::OpenReview(source) => {
                if let Some(core) = self.core.as_mut() {
                    match source {
                        Some(source) => core.open_git_review_from(source),
                        None => core.open_git_review(),
                    }
                }
            }
            DriverMessage::ConnectFailed(error) => self.link = Link::Failed(error.to_string()),
            DriverMessage::Input(input) => {
                if let Some(core) = self.core.as_mut() {
                    core.on_input(input);
                }
            }
        }
        self.apply(cx);
    }

    fn open_session(&mut self, session_id: Option<SessionId>, cx: &mut Context<Self>) {
        self.link = Link::Connecting;
        self.driver = Some(ThreadDriverHandle::spawn(self.connector.clone(), self.target.clone(), session_id, cx));
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let effects = self.core.as_mut().map(SessionCore::take_effects).unwrap_or_default();
        for effect in effects {
            match effect {
                Effect::Dispatch(command) => {
                    if let Some(driver) = &self.driver {
                        driver.send(command);
                    }
                }
                Effect::ElicitationRequested => cx.emit(ThreadEvent::NeedsInput),
                Effect::RestoreSubmission(submission) => cx.emit(ThreadEvent::SubmissionRestored(submission)),
                Effect::ConnectionClosed => self.link = Link::Closed,
                Effect::TurnFinished => cx.emit(ThreadEvent::TurnFinished),
                Effect::ReviewSubmitted => cx.emit(ThreadEvent::ReviewSubmitted),
                Effect::Reply(Reply::GitReviewAction(result)) => cx.emit(ThreadEvent::ReviewActionFinished(result)),
                Effect::Reply(Reply::GitReviewLookup(reply)) => cx.emit(ThreadEvent::ReviewLookupFinished(reply)),
                Effect::PromptSearchUpdated => cx.emit(ThreadEvent::PromptSearchUpdated),
                Effect::SessionSettingsChanged(SessionSetting::ConfigOptions) => self.config_revision += 1,
                Effect::ElicitationDismissed
                | Effect::SessionChanged
                | Effect::SessionSettingsChanged(SessionSetting::AuthMethods | SessionSetting::ServerStatuses)
                | Effect::GitReviewChanged
                | Effect::WorkspacesListed(_)
                | Effect::Reply(_) => {}
            }
        }
        cx.notify();
    }
}

impl EventEmitter<ThreadEvent> for Thread {}

fn first_text(blocks: &[acp::ContentBlock]) -> Option<&str> {
    blocks.iter().find_map(|block| match block {
        acp::ContentBlock::Text(text) => Some(text.text.trim()).filter(|text| !text.is_empty()),
        _ => None,
    })
}
