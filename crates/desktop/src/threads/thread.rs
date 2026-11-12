use super::driver::{Driver, DriverMessage};
use crate::history::{UNTITLED, git_root};
use crate::target::{AgentConnector, SessionTarget};
use acp_utils::content::first_text;
use acp_utils::conversation::ConversationContent;
use aether_sessions::prompt_title;
use agent_client_protocol::v2::schema::SessionId;
use chrono::{DateTime, Utc};
use clankerdiff_client::{ClientError, ClientState, LookupReply};
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Scope, Signal, Stored, batch, signal, stored};
use client_core::command::Reply;
use client_core::composer::file_listing::{FileListing, RepoFiles};
use client_core::composer::submission::Submission;
use client_core::elicitation::{Elicitation, ElicitationId};
use client_core::session_core::{Effect, SessionCore};
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadStatus {
    Connecting,
    Idle,
    Running,
    Background,
    NeedsInput,
    Errored,
    Disconnected,
    Dormant,
}

#[derive(Clone, Copy)]
pub struct Thread {
    number: usize,
    state: Signal<ThreadState>,
    driver: Stored<Option<Driver>>,
    listeners: Stored<Listeners>,
    scope: Scope,
}

type Listeners = Vec<(Stored<()>, Callback<(ThreadEvent,)>)>;

struct ThreadState {
    session_id: Option<SessionId>,
    target: SessionTarget,
    project: Option<PathBuf>,
    opened_at: DateTime<Utc>,
    connector: Arc<AgentConnector>,
    core: Option<SessionCore>,
    link: Link,
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
    ) -> Self {
        let thread = Self {
            number,
            state: signal(ThreadState {
                session_id: session_id.clone(),
                project: target.cwd().map(git_root),
                opened_at,
                target,
                connector,
                core: None,
                link: Link::Connecting,
            }),
            driver: stored(None),
            listeners: stored(Vec::new()),
            scope: Scope::new(),
        };
        thread.open_session(session_id);
        thread
    }

    pub fn number(&self) -> usize {
        self.number
    }

    pub fn is_alive(&self) -> bool {
        self.state.is_alive()
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.state.with(|state| state.session_id.clone())
    }

    pub fn target(&self) -> SessionTarget {
        self.state.with(|state| state.target.clone())
    }

    pub fn project(&self) -> Option<PathBuf> {
        self.state.with(|state| state.project.clone())
    }

    pub fn opened_at(&self) -> DateTime<Utc> {
        self.state.with(|state| state.opened_at)
    }

    pub fn title(&self) -> String {
        self.state.with(|state| {
            let prompt = state.core.as_ref().and_then(|core| {
                core.conversation().items().iter().find_map(|item| match item.content() {
                    ConversationContent::User(blocks) => Some(blocks),
                    _ => None,
                })
            });
            prompt.map_or_else(|| UNTITLED.to_owned(), |blocks| prompt_title(first_text(blocks)))
        })
    }

    pub fn with_core<T>(&self, read: impl FnOnce(&SessionCore) -> T) -> Option<T> {
        self.state.with(|state| state.core.as_ref().map(read))
    }

    pub fn with_connected_core<T>(&self, read: impl FnOnce(&SessionCore) -> T) -> Option<T> {
        self.state.with(|state| state.connected_core().map(read))
    }

    pub fn status(&self) -> ThreadStatus {
        self.state.with(ThreadState::status)
    }

    pub fn error(&self) -> Option<String> {
        self.state.with(|state| match &state.link {
            Link::Failed(error) => Some(error.clone()),
            _ => None,
        })
    }

    pub fn elicitation(&self) -> Option<(ElicitationId, Elicitation)> {
        self.with_connected_core(|core| core.elicitation().map(|(id, elicitation)| (id, elicitation.clone()))).flatten()
    }

    pub fn pending_review(&self) -> Option<(ElicitationId, ArtifactReviewElicitationMeta)> {
        match self.elicitation()? {
            (id, Elicitation::ArtifactReview(meta)) => Some((id, meta)),
            _ => None,
        }
    }

    pub fn has_pending_review(&self) -> bool {
        self.with_connected_core(|core| matches!(core.elicitation(), Some((_, Elicitation::ArtifactReview(_)))))
            .unwrap_or(false)
    }

    pub fn review(&self) -> Option<Arc<ClientState>> {
        self.with_connected_core(|core| core.git_review().cloned()).flatten()
    }

    pub fn changed_files(&self) -> Option<usize> {
        self.with_connected_core(|core| {
            core.git_review().and_then(|review| review.snapshot.as_ref().map(|snapshot| snapshot.files.len()))
        })
        .flatten()
    }

    pub fn listed_files(&self) -> Result<RepoFiles, String> {
        self.state.with(|state| match state.core.as_ref().map_or(FileListing::Pending, SessionCore::files) {
            FileListing::Pending if state.status().is_stopped() => {
                Err("Start the thread again to browse its files".to_owned())
            }
            listing => listing.into_result(),
        })
    }

    pub fn send(&self, submission: Submission) {
        if self.state.with_untracked(|state| state.connected_core().is_some()) {
            self.update_core(|core| core.submit(submission));
        } else {
            self.emit(&ThreadEvent::SubmissionRestored(submission));
        }
    }

    pub fn report_error(&self, message: &str) {
        self.mutate(|state| {
            if let Some(core) = state.core.as_mut() {
                core.report_error(message);
            }
        });
    }

    pub fn can_search_prompts(&self) -> bool {
        self.with_connected_core(|core| core.session().capabilities().prompt_search).unwrap_or(false)
    }

    pub fn update_core(&self, change: impl FnOnce(&mut SessionCore)) {
        if !self.state.is_alive() {
            return;
        }
        self.mutate(|state| {
            if let Some(core) = state.core.as_mut().filter(|_| state.link == Link::Connected) {
                change(core);
            }
        });
    }

    pub fn reconnect(&self) {
        let session_id = self.state.with_untracked(|state| {
            matches!(state.link, Link::Failed(_) | Link::Closed).then(|| state.session_id.clone())
        });
        if let Some(session_id) = session_id {
            self.open_session(session_id);
        }
    }

    pub fn on_event(&self, listener: impl Fn(ThreadEvent) + 'static) {
        let subscription = (stored(()), Callback::from(listener));
        self.listeners.update(|listeners| {
            listeners.retain(|(alive, _)| alive.is_alive());
            listeners.push(subscription);
        });
    }

    pub(crate) fn stop(&self) {
        if self.driver.update(Option::take).is_none() {
            return;
        }
        self.state.update(|state| state.link = Link::Stopped);
    }

    pub(super) fn receive(&self, messages: Vec<DriverMessage>) -> bool {
        if !self.state.is_alive() {
            return false;
        }
        batch(|| {
            for message in messages {
                self.on_driver_message(message);
            }
        });
        true
    }

    fn on_driver_message(&self, message: DriverMessage) {
        self.mutate(|state| match message {
            DriverMessage::Launched(launched) => state.target.set_instance(launched),
            DriverMessage::Connected(config) => {
                state.session_id = Some(config.session_response.session_id.clone());
                state.core = Some(SessionCore::new(*config));
                state.link = Link::Connected;
            }
            DriverMessage::OpenReview(source) => {
                if let Some(core) = state.core.as_mut() {
                    match source {
                        Some(source) => core.open_git_review_from(source),
                        None => core.open_git_review(),
                    }
                }
            }
            DriverMessage::ConnectFailed(error) => state.link = Link::Failed(error.to_string()),
            DriverMessage::Input(input) => {
                if let Some(core) = state.core.as_mut() {
                    core.on_input(input);
                }
            }
        });
    }

    fn open_session(&self, session_id: Option<SessionId>) {
        let (connector, target) = self.state.with_untracked(|state| (state.connector.clone(), state.target.clone()));
        let thread = *self;
        let driver = self.scope.run(|| Driver::spawn(connector, target, session_id, thread));
        self.state.update(|state| state.link = Link::Connecting);
        self.driver.set(Some(driver));
    }

    fn mutate(&self, change: impl FnOnce(&mut ThreadState)) {
        let effects = self.state.try_update(|state| {
            change(state);
            let effects = state.core.as_mut().map(SessionCore::take_effects).unwrap_or_default();
            if effects.iter().any(|effect| matches!(effect, Effect::ConnectionClosed)) {
                state.link = Link::Closed;
            }
            effects
        });
        batch(|| {
            for effect in effects.unwrap_or_default() {
                self.apply(effect);
            }
        });
    }

    fn apply(&self, effect: Effect) {
        let event = match effect {
            Effect::Dispatch(command) => {
                return self.driver.with(|driver| {
                    if let Some(driver) = driver {
                        driver.send(command);
                    }
                });
            }
            Effect::ElicitationRequested => ThreadEvent::NeedsInput,
            Effect::RestoreSubmission(submission) => ThreadEvent::SubmissionRestored(submission),
            Effect::TurnFinished => ThreadEvent::TurnFinished,
            Effect::ReviewSubmitted => ThreadEvent::ReviewSubmitted,
            Effect::Reply(Reply::GitReviewAction(result)) => ThreadEvent::ReviewActionFinished(result),
            Effect::Reply(Reply::GitReviewLookup(reply)) => ThreadEvent::ReviewLookupFinished(reply),
            Effect::PromptSearchUpdated => ThreadEvent::PromptSearchUpdated,
            Effect::ConnectionClosed
            | Effect::ElicitationDismissed
            | Effect::SessionChanged
            | Effect::SessionSettingsChanged(_)
            | Effect::GitReviewChanged
            | Effect::WorkspacesListed(_)
            | Effect::Reply(_) => return,
        };
        self.emit(&event);
    }

    fn emit(&self, event: &ThreadEvent) {
        let listeners = self.listeners.with(|listeners| {
            listeners.iter().filter(|(alive, _)| alive.is_alive()).map(|(_, listener)| *listener).collect::<Vec<_>>()
        });
        for listener in listeners {
            listener.call(event.clone());
        }
    }
}

impl ThreadState {
    fn connected_core(&self) -> Option<&SessionCore> {
        self.core.as_ref().filter(|_| self.link == Link::Connected)
    }

    fn status(&self) -> ThreadStatus {
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
}

impl PartialEq for Thread {
    fn eq(&self, other: &Self) -> bool {
        self.number == other.number && self.state == other.state
    }
}

impl Eq for Thread {}

impl Hash for Thread {
    fn hash<T: Hasher>(&self, state: &mut T) {
        self.number.hash(state);
    }
}

impl Debug for Thread {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_tuple("Thread").field(&self.number).finish()
    }
}

impl ThreadStatus {
    pub fn is_stopped(self) -> bool {
        matches!(self, Self::Errored | Self::Disconnected | Self::Dormant)
    }

    pub fn problem(self) -> Option<&'static str> {
        match self {
            Self::Errored => Some("The agent could not start"),
            Self::Disconnected => Some("The agent connection closed"),
            _ => None,
        }
    }
}
