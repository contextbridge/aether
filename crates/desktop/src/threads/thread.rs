use super::driver::{DriverMessage, ThreadDriverHandle};
use super::status::ThreadStatus;
use crate::history::{UNTITLED, git_root};
use crate::target::{ConnectMode, Connector, SessionTarget};
use acp_utils::conversation::ConversationContent;
use acp_utils::notifications::PromptSearchResponse;
use agent_client_protocol::schema::v2::SessionId;
use chrono::{DateTime, Utc};
use clankerdiff_client::{ClientState, DiffReviewEvent};
use clankerdiff_gpui_shell::runtime;
use client_core::command::{Command, CommandResult, GitReviewCommand};
use client_core::composer::submission::Submission;
use client_core::elicitation::ElicitationResponder;
use client_core::session_core::{Effect, Elicitation, SessionCore};
use gpui::{Context, EventEmitter};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadEvent {
    /// The agent's turn ended on its own, with no prompt queued behind it.
    TurnFinished,
    /// The agent asked the user something, and waits for the answer.
    NeedsInput,
    /// Its session, title or status changed, which its sidebar row shows.
    RowChanged,
    /// A scope change, refresh or repository action the user asked for finished.
    ReviewActionFinished(Result<(), String>),
    /// A review of the changes went to the agent as a prompt.
    ReviewSubmitted,
    /// The agent's answer to a prompt search, echoing the query it answers.
    PromptSearchResults { query: String, result: Result<PromptSearchResponse, String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElicitationId(usize);

/// One agent session the app runs or shows: its state, and the driver that connects it to its
/// agent.
pub struct Thread {
    /// Counts the threads in the order the app opened them, to tell their rows apart.
    number: usize,
    session_id: Option<SessionId>,
    target: SessionTarget,
    /// The git root of the target's directory, resolved once since it walks the filesystem.
    project: PathBuf,
    opened_at: DateTime<Utc>,
    core: Option<SessionCore>,
    /// [`ThreadStatus::Idle`] once connected, which [`Self::status`] refines with what the session
    /// is doing.
    connection: ThreadStatus,
    /// The question the agent waits on. Replacing or dropping it cancels the agent's request.
    elicitation: Option<PendingElicitation>,
    elicitations: usize,
    /// A prompt that never reached the agent, for the composer to give back.
    restored_submissions: Vec<Submission>,
    /// The agent's view of the working tree's changes, kept current for as long as the session is
    /// open so the Changes tab's count stays live.
    review: Option<Arc<ClientState>>,
    /// `None` once the thread is stopped. A driver whose connection closed on its own has already
    /// finished.
    driver: Option<ThreadDriverHandle>,
    /// What its sidebar row showed when [`ThreadEvent::RowChanged`] was last emitted.
    row: (Option<SessionId>, String, ThreadStatus),
}

impl Thread {
    /// Starts connecting to `target`'s agent, opening the session `mode` asks for.
    pub(crate) fn connect(
        number: usize,
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
            number,
            row: (session_id.clone(), UNTITLED.to_owned(), ThreadStatus::Connecting),
            session_id,
            project: git_root(target.cwd()),
            opened_at: Utc::now(),
            target,
            core: None,
            connection: ThreadStatus::Connecting,
            elicitation: None,
            elicitations: 0,
            restored_submissions: Vec::new(),
            review: None,
            driver: None,
        };
        thread.open_session(mode, connector, cx);
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

    /// The project the thread belongs to: the git root of its directory, or the directory itself
    /// outside of a checkout.
    pub fn project(&self) -> &Path {
        &self.project
    }

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

    pub fn core(&self) -> Option<&SessionCore> {
        self.core.as_ref()
    }

    pub fn status(&self) -> ThreadStatus {
        match &self.connection {
            ThreadStatus::Idle if self.elicitation.is_some() => ThreadStatus::NeedsInput,
            ThreadStatus::Idle if self.core.as_ref().is_some_and(SessionCore::is_agent_busy) => ThreadStatus::Running,
            connection => connection.clone(),
        }
    }

    pub fn elicitation(&self) -> Option<(ElicitationId, &Elicitation)> {
        self.elicitation.as_ref().map(|pending| (pending.id, &pending.elicitation))
    }

    /// Answers the question the agent waits on through `respond`, which lets its turn go on.
    /// Without a question waiting, `respond` is not called.
    pub fn answer(&mut self, id: ElicitationId, respond: impl FnOnce(ElicitationResponder), cx: &mut Context<Self>) {
        if let Some(pending) = self.elicitation.take_if(|pending| pending.id == id) {
            respond(pending.responder);
            self.changed(cx);
        }
    }

    pub fn review(&self) -> Option<&Arc<ClientState>> {
        self.review.as_ref()
    }

    pub fn changed_files(&self) -> Option<usize> {
        self.review()?.snapshot.as_ref().map(|snapshot| snapshot.document.files.len())
    }

    /// Sends a review of the changes to the agent as a prompt, queued behind any running turn.
    pub fn submit_review(&mut self, prompt: &str, cx: &mut Context<Self>) {
        self.with_core(cx, |core| core.submit_review(prompt));
    }

    /// Hands a scope change, refresh or repository action to the agent's diff server. Its
    /// outcome arrives as [`ThreadEvent::ReviewActionFinished`].
    pub fn send_review_event(&mut self, event: DiffReviewEvent) {
        if self.connected_core().is_some()
            && let Some(driver) = &self.driver
        {
            driver.send(Command::GitReview(GitReviewCommand::Event(event)));
        }
    }

    pub fn take_restored_submissions(&mut self) -> Vec<Submission> {
        std::mem::take(&mut self.restored_submissions)
    }

    /// Sends `text` as a prompt, queued behind any running turn. Unless the agent is connected,
    /// the prompt is given back as the [restored submission](Self::take_restored_submissions).
    pub fn submit(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.send(Submission { text: text.into(), ..Submission::default() }, cx);
    }

    /// Sends `submission`, with the files it mentions, as [`submit`](Self::submit) sends text.
    pub fn send(&mut self, submission: Submission, cx: &mut Context<Self>) {
        match self.connected_core() {
            Some(core) => core.submit(submission),
            None => self.restored_submissions.push(submission),
        }
        self.apply(cx);
    }

    /// Selects `value` for the session option `config_id`, such as its model. The choice shows at
    /// once, and the agent confirms or corrects it.
    pub fn set_config_option(&mut self, config_id: &str, value: &str, cx: &mut Context<Self>) {
        self.with_core(cx, |core| core.set_config_option(config_id, value));
    }

    pub fn can_search_prompts(&self) -> bool {
        self.core.as_ref().is_some_and(|core| core.session().capabilities().prompt_search)
    }

    /// Searches the agent's prompt history for `query`. The answer arrives as
    /// [`ThreadEvent::PromptSearchResults`].
    pub fn search_prompts(&mut self, query: String, cx: &mut Context<Self>) {
        self.with_core(cx, |core| core.search_prompts(query));
    }

    /// Asks the agent to cancel its running turn, together with any question that turn asked.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.elicitation = None;
        self.with_core(cx, SessionCore::cancel);
    }

    /// Ends the thread's agent, and terminates the instance it was launched on, leaving the
    /// thread [`Dormant`](ThreadStatus::Dormant).
    pub(crate) fn stop(&mut self, connector: Arc<dyn Connector>, cx: &mut Context<Self>) {
        if self.driver.take().is_none() {
            return;
        }
        self.connection = ThreadStatus::Dormant;
        self.elicitation = None;
        self.review = None;
        if let Some(instance) = self.target.instance.take() {
            let target = SessionTarget { instance: Some(instance), ..self.target.clone() };
            runtime::spawn(cx, async move {
                if let Err(error) = connector.terminate(&target).await {
                    tracing::warn!(%error, agent = %target.agent.name, "could not terminate the agent's instance");
                }
            })
            .detach();
        }
        self.changed(cx);
    }

    /// Connects again after the connection failed or closed, resuming the thread's session if it
    /// has one and otherwise opening a fresh one.
    pub(crate) fn reconnect(&mut self, connector: Arc<dyn Connector>, cx: &mut Context<Self>) {
        if !self.status().can_reconnect() {
            return;
        }
        let mode = self.session_id.clone().map_or(ConnectMode::NewSession, ConnectMode::Resume);
        self.open_session(mode, connector, cx);
        self.changed(cx);
    }

    /// Replaces any previous connection with a new one that opens the session `mode` asks for.
    fn open_session(&mut self, mode: ConnectMode, connector: Arc<dyn Connector>, cx: &mut Context<Self>) {
        self.connection = ThreadStatus::Connecting;
        self.elicitation = None;
        self.review = None;
        self.driver =
            Some(ThreadDriverHandle::spawn(connector, self.target.clone(), mode, cx, Self::on_driver_message));
    }

    fn on_driver_message(&mut self, message: DriverMessage, cx: &mut Context<Self>) {
        match message {
            DriverMessage::Launched(instance) => self.target.instance = Some(instance),
            DriverMessage::Connected(config) => {
                self.session_id = Some(config.session_response.session_id.clone());
                // Replaces any earlier core, since a resumed session replays its history from the
                // start and would otherwise duplicate what the thread already shows.
                self.core.insert(SessionCore::new(*config)).open_git_review();
                self.connection = ThreadStatus::Idle;
            }
            DriverMessage::ConnectFailed(error) => self.connection = ThreadStatus::Errored(error.to_string()),
            DriverMessage::Event(event) => {
                if let Some(core) = self.core.as_mut() {
                    core.on_acp_event(event);
                }
            }
            DriverMessage::Result(result) => match self.core.as_mut().and_then(|core| core.on_command_result(result)) {
                Some(CommandResult::GitReview(state)) => self.review = Some(state),
                Some(CommandResult::GitReviewAction(result)) => cx.emit(ThreadEvent::ReviewActionFinished(result)),
                Some(CommandResult::PromptSearchResults { query, result }) => {
                    cx.emit(ThreadEvent::PromptSearchResults { query, result });
                }
                _ => {}
            },
        }
        self.apply(cx);
    }

    /// Runs `change` on the session core while the agent is connected, then carries out what it
    /// asked for.
    fn with_core(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut SessionCore)) {
        if let Some(core) = self.connected_core() {
            change(core);
            self.apply(cx);
        }
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
                Effect::ShowElicitation { elicitation, responder } => {
                    let id = ElicitationId(self.elicitations);
                    self.elicitations += 1;
                    self.elicitation = Some(PendingElicitation { id, elicitation, responder });
                    cx.emit(ThreadEvent::NeedsInput);
                }
                Effect::RestoreSubmission(submission) => self.restored_submissions.push(submission),
                Effect::ConnectionClosed => {
                    self.connection = ThreadStatus::Disconnected;
                    self.elicitation = None;
                    self.review = None;
                }
                Effect::TurnFinished => cx.emit(ThreadEvent::TurnFinished),
                Effect::ReviewSubmitted => cx.emit(ThreadEvent::ReviewSubmitted),
                // The core already holds the state the others announce, and the views read it
                // when they re-render.
                _ => {}
            }
        }
        self.changed(cx);
    }

    /// Re-renders the thread, first announcing a change to what its sidebar row shows.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let row = (self.session_id.clone(), self.title(), self.status());
        if row != self.row {
            self.row = row;
            cx.emit(ThreadEvent::RowChanged);
        }
        cx.notify();
    }

    fn connected_core(&mut self) -> Option<&mut SessionCore> {
        match self.connection {
            ThreadStatus::Idle => self.core.as_mut(),
            _ => None,
        }
    }
}

impl EventEmitter<ThreadEvent> for Thread {}

struct PendingElicitation {
    id: ElicitationId,
    elicitation: Elicitation,
    /// Dropping it unanswered cancels the agent's request.
    responder: ElicitationResponder,
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned)
}
