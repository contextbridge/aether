use super::driver::{DriverMessage, ThreadDriverHandle};
use super::status::{ConnectionPhase, ThreadStatus};
use crate::history::{UNTITLED, git_root};
use crate::target::{ConnectMode, Connector, SessionTarget};
use acp_utils::conversation::ConversationContent;
use acp_utils::notifications::PromptSearchResponse;
use agent_client_protocol::schema::v2::SessionId;
use chrono::{DateTime, Utc};
use clankerdiff_client::{ClientState, DiffReviewEvent};
use client_core::command::{Command, GitReviewCommand};
use client_core::composer::submission::Submission;
use client_core::elicitation::ElicitationResponder;
use client_core::session_core::{Effect, Elicitation, SessionCore};
use gpui::{Context, EventEmitter};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Identifies a thread within the running app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(u64);

/// What a thread announces to the rest of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadEvent {
    /// The agent's turn ended on its own, with no prompt queued behind it.
    TurnFinished,
    /// The agent asked the user something, and waits for the answer.
    NeedsInput,
}

/// What a thread announces to its Changes tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewEvent {
    /// A scope change, refresh or repository action the user asked for finished.
    ActionFinished(Result<(), String>),
    /// A review of the changes went to the agent as a prompt.
    Submitted,
}

/// The agent's answer to a prompt search, echoing the query it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSearchEvent {
    pub query: String,
    pub result: Result<PromptSearchResponse, String>,
}

/// One agent session the app runs or shows: its state, and the driver that connects it to its
/// agent.
pub struct Thread {
    id: ThreadId,
    session_id: Option<SessionId>,
    target: SessionTarget,
    /// The git root of the target's directory, resolved once since it walks the filesystem.
    project: PathBuf,
    opened_at: DateTime<Utc>,
    /// `None` until the session opens.
    core: Option<SessionCore>,
    phase: ConnectionPhase,
    /// The question the agent waits on. Replacing or dropping it cancels the agent's request.
    elicitation: Option<PendingElicitation>,
    /// A prompt that never reached the agent, for the composer to give back.
    restored_submission: Option<Submission>,
    /// The agent's view of the working tree's changes, kept current for as long as the session is
    /// open so the Changes tab's count stays live.
    review: Option<Arc<ClientState>>,
    /// `None` once the thread is stopped. A driver whose connection closed on its own has already
    /// finished.
    driver: Option<ThreadDriverHandle>,
}

impl ThreadId {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Thread {
    /// Starts connecting to `target`'s agent, opening the session `mode` asks for.
    pub(crate) fn connect(
        id: ThreadId,
        target: SessionTarget,
        mode: ConnectMode,
        connector: Arc<dyn Connector>,
        cx: &mut Context<Self>,
    ) -> Self {
        let session_id = match &mode {
            ConnectMode::NewSession => None,
            ConnectMode::Resume(session_id) => Some(session_id.clone()),
        };
        let mut thread = Self {
            id,
            session_id,
            project: git_root(target.cwd()),
            opened_at: Utc::now(),
            target,
            core: None,
            phase: ConnectionPhase::Connecting,
            elicitation: None,
            restored_submission: None,
            review: None,
            driver: None,
        };
        thread.open_session(mode, connector, cx);
        thread
    }

    pub fn id(&self) -> ThreadId {
        self.id
    }

    /// The agent session, once the agent has opened or resumed it.
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    pub fn target(&self) -> &SessionTarget {
        &self.target
    }

    /// The project the thread belongs to: the git root of its directory, or the directory itself
    /// outside of a checkout.
    pub fn project(&self) -> &Path {
        &self.project
    }

    /// When the app opened the thread.
    pub fn opened_at(&self) -> DateTime<Utc> {
        self.opened_at
    }

    /// The first line of the thread's first prompt, or [`UNTITLED`] before there is one.
    pub fn title(&self) -> String {
        let prompt = self.core.as_ref().and_then(|core| {
            core.conversation().items().iter().find(|item| matches!(item.content(), ConversationContent::User(_)))
        });
        prompt.and_then(|item| first_line(&item.text()?)).unwrap_or_else(|| UNTITLED.to_owned())
    }

    /// The session state, including its conversation, once the session is open.
    pub fn core(&self) -> Option<&SessionCore> {
        self.core.as_ref()
    }

    pub fn status(&self) -> ThreadStatus {
        ThreadStatus::derive(&self.phase, self.core.as_ref(), self.elicitation())
    }

    /// What the agent is asking the user, while it waits for an answer.
    pub fn elicitation(&self) -> Option<&Elicitation> {
        self.elicitation.as_ref().map(|pending| &pending.elicitation)
    }

    /// Answers the question the agent waits on through `respond`, which lets its turn go on.
    /// Without a question waiting, `respond` is not called.
    pub fn answer(&mut self, respond: impl FnOnce(ElicitationResponder), cx: &mut Context<Self>) {
        if let Some(pending) = self.elicitation.take() {
            respond(pending.responder);
            cx.notify();
        }
    }

    /// The working tree's changes as the agent last reported them, while the session is open.
    pub fn review(&self) -> Option<&ClientState> {
        self.review.as_deref()
    }

    /// How many files the working tree's changes touch, once the agent has reported them.
    pub fn changed_files(&self) -> Option<usize> {
        self.review()?.snapshot.as_ref().map(|snapshot| snapshot.document.files.len())
    }

    /// Sends a review of the changes to the agent as a prompt, queued behind any running turn.
    pub fn submit_review(&mut self, prompt: &str, cx: &mut Context<Self>) {
        if let Some(core) = self.connected_core() {
            let effects = core.submit_review(prompt);
            self.apply(effects, cx);
        }
    }

    /// Hands a scope change, refresh or repository action to the agent's diff server. Its
    /// outcome arrives as [`ReviewEvent::ActionFinished`].
    pub fn send_review_event(&mut self, event: DiffReviewEvent) {
        if self.connected_core().is_some()
            && let Some(driver) = &self.driver
        {
            driver.send(Command::GitReview(GitReviewCommand::Event(event)));
        }
    }

    /// Takes back a prompt that never reached the agent, so the user can edit and resend it.
    pub fn take_restored_submission(&mut self) -> Option<Submission> {
        self.restored_submission.take()
    }

    /// Sends `text` as a prompt, queued behind any running turn. Unless the agent is connected,
    /// the prompt is given back as the [restored submission](Self::take_restored_submission).
    pub fn submit(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.send(Submission { text: text.into(), ..Submission::default() }, cx);
    }

    /// Sends `submission`, with the files it mentions, as [`submit`](Self::submit) sends text.
    pub fn send(&mut self, submission: Submission, cx: &mut Context<Self>) {
        let effects = match self.connected_core() {
            Some(core) => core.submit(submission),
            None => vec![Effect::RestoreSubmission(submission)],
        };
        self.apply(effects, cx);
    }

    /// Selects `value` for the session option `config_id`, such as its model. The choice shows at
    /// once, and the agent confirms or corrects it.
    pub fn set_config_option(&mut self, config_id: &str, value: &str, cx: &mut Context<Self>) {
        if let Some(core) = self.connected_core() {
            let effects = core.set_config_option(config_id, value);
            self.apply(effects, cx);
        }
    }

    /// Whether the agent can search the prompts sent in all its sessions.
    pub fn can_search_prompts(&self) -> bool {
        self.core.as_ref().is_some_and(|core| core.session().capabilities().prompt_search)
    }

    /// Searches the agent's prompt history for `query`. The answer arrives as a
    /// [`PromptSearchEvent`].
    pub fn search_prompts(&mut self, query: String, cx: &mut Context<Self>) {
        if let Some(core) = self.connected_core() {
            let effects = core.search_prompts(query);
            self.apply(effects, cx);
        }
    }

    /// Asks the agent to cancel its running turn, together with any question that turn asked.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(core) = self.connected_core() {
            let effects = core.cancel();
            self.elicitation = None;
            self.apply(effects, cx);
        }
    }

    /// Ends the thread's agent, leaving the thread [`Dormant`](ThreadStatus::Dormant).
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if self.driver.take().is_some() {
            self.phase = ConnectionPhase::Stopped;
            self.elicitation = None;
            self.review = None;
            cx.notify();
        }
    }

    /// Connects again after the connection failed or closed, resuming the thread's session if it
    /// has one and otherwise opening a fresh one.
    pub(crate) fn reconnect(&mut self, connector: Arc<dyn Connector>, cx: &mut Context<Self>) {
        if !self.status().can_reconnect() {
            return;
        }
        let mode = self.session_id.clone().map_or(ConnectMode::NewSession, ConnectMode::Resume);
        self.open_session(mode, connector, cx);
        cx.notify();
    }

    /// Replaces any previous connection with a new one that opens the session `mode` asks for.
    fn open_session(&mut self, mode: ConnectMode, connector: Arc<dyn Connector>, cx: &mut Context<Self>) {
        self.phase = ConnectionPhase::Connecting;
        self.elicitation = None;
        self.review = None;
        self.driver =
            Some(ThreadDriverHandle::spawn(connector, self.target.clone(), mode, cx, Self::on_driver_message));
    }

    fn on_driver_message(&mut self, message: DriverMessage, cx: &mut Context<Self>) {
        let effects = match message {
            DriverMessage::Connected(config) => {
                self.session_id = Some(config.session_response.session_id.clone());
                // Replaces any earlier core, since a resumed session replays its history from the
                // start and would otherwise duplicate what the thread already shows.
                let core = self.core.insert(SessionCore::new(*config));
                self.phase = ConnectionPhase::Connected;
                core.open_git_review()
            }
            DriverMessage::ConnectFailed(error) => {
                self.phase = ConnectionPhase::Failed(error.to_string());
                Vec::new()
            }
            DriverMessage::Event(event) => self.core.as_mut().map(|core| core.on_acp_event(event)).unwrap_or_default(),
            DriverMessage::Result(result) => {
                self.core.as_mut().map(|core| core.on_command_result(result)).unwrap_or_default()
            }
        };
        self.apply(effects, cx);
    }

    /// Carries out what the session core asked for, then re-renders the thread.
    fn apply(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Dispatch(command) => {
                    if let Some(driver) = &self.driver {
                        driver.send(command);
                    }
                }
                Effect::ShowElicitation { elicitation, responder } => {
                    self.elicitation = Some(PendingElicitation { elicitation, responder });
                    cx.emit(ThreadEvent::NeedsInput);
                }
                Effect::RestoreSubmission(submission) => self.restored_submission = Some(submission),
                Effect::ConnectionClosed => {
                    self.phase = ConnectionPhase::Closed;
                    self.elicitation = None;
                    self.review = None;
                }
                Effect::TurnFinished => cx.emit(ThreadEvent::TurnFinished),
                Effect::GitReviewState(state) => self.review = Some(state),
                Effect::GitReviewActionFinished(result) => cx.emit(ReviewEvent::ActionFinished(result)),
                Effect::ReviewSubmitted => cx.emit(ReviewEvent::Submitted),
                Effect::PromptSearchResults { query, result } => cx.emit(PromptSearchEvent { query, result }),
                // The core already holds the state these announce, and the views read it when
                // they re-render. No command that lists sessions or workspaces, or indexes files
                // through the core, is ever sent.
                Effect::Notice(_)
                | Effect::SessionsListed(_)
                | Effect::SessionPreviewLoaded { .. }
                | Effect::WorkspacesListed(_)
                | Effect::FilesIndexed { .. }
                | Effect::ConfigOptionsChanged
                | Effect::AuthMethodsChanged
                | Effect::ServerStatusesChanged
                | Effect::AuthenticationFinished { .. }
                | Effect::SessionCreated
                | Effect::ResumeStarted
                | Effect::SessionResumed
                | Effect::WorkspaceMoved => {}
            }
        }
        cx.notify();
    }

    fn connected_core(&mut self) -> Option<&mut SessionCore> {
        match self.phase {
            ConnectionPhase::Connected => self.core.as_mut(),
            _ => None,
        }
    }
}

impl EventEmitter<ThreadEvent> for Thread {}

impl EventEmitter<ReviewEvent> for Thread {}

impl EventEmitter<PromptSearchEvent> for Thread {}

struct PendingElicitation {
    elicitation: Elicitation,
    /// Dropping it unanswered cancels the agent's request.
    responder: ElicitationResponder,
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned)
}
