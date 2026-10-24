use acp_utils::client::AcpEvent;
use acp_utils::conversation::ConversationContent;
use acp_utils::testing::{FakeAgent, idle_notification, running_notification};
use agent_client_protocol::schema::v2::{
    self as acp, ContentBlock, ImageContent, NewSessionResponse, SessionConfigOption, SessionConfigSelectOption,
    SessionId, StopReason,
};
use client_core::command::{AgentCommand, Command, CommandResult, GitReviewCommand, PromptRejection, ReviewSource};
use client_core::composer::attachment::{AttachmentOutcome, PromptAttachment};
use client_core::composer::submission::Submission;
use client_core::request::RequestId;
use client_core::runtime::CommandDispatcher;
use client_core::session::{Connection, Session};
use client_core::session_core::{Effect, ForegroundOperation, SessionCore};
use client_core::testing::SessionCoreConfigBuilder;
use std::path::Path;
use tokio::sync::mpsc;
use tokio::task::LocalSet;

#[test]
fn media_the_agent_cannot_take_gives_the_submission_back() {
    let mut core = core();
    let photo = PromptAttachment { path: "/workspace/photo.png".into(), display_name: "photo.png".into() };
    let with_photo = Submission { media: vec![photo], ..submission("what is this?") };
    core.submit(with_photo.clone());

    let image = ContentBlock::Image(ImageContent::new("aGk=", "image/png"));
    deliver(&mut core, CommandResult::SubmissionPrepared(AttachmentOutcome { blocks: vec![image], warnings: vec![] }));

    assert_eq!(restored(&mut core), Some(with_photo));
    assert_eq!(core.queued_prompts(), []);
    assert_eq!(notices(&core), ["[test] ACP agent does not support image input."]);
}

#[test]
fn a_rejected_prompt_gives_its_submission_back() {
    for (rejection, expected_notices) in [
        (PromptRejection::Cancelled, Vec::new()),
        (PromptRejection::Failed("offline".into()), vec!["[test] Failed to send prompt: offline"]),
    ] {
        let mut core = core();
        let request_id = submit(&mut core, "retry me");

        deliver(&mut core, answered(request_id, Err(rejection)));

        assert_eq!(restored(&mut core), Some(submission("retry me")));
        assert_eq!(core.queued_prompts(), []);
        assert_eq!(notices(&core), expected_notices);
    }
}

#[test]
fn a_finished_turn_is_announced_once_whether_it_goes_idle_before_or_after_its_prompt_is_answered() {
    for replies in [
        [Reply::Idle(StopReason::EndTurn), Reply::Answered(Ok(()))],
        [Reply::Answered(Ok(())), Reply::Idle(StopReason::EndTurn)],
    ] {
        let mut core = core();
        let request_id = started_turn(&mut core, "hello");

        assert_eq!(reply(&mut core, request_id, replies.clone()), 1, "{replies:?}");
    }
}

#[test]
fn a_cancelled_turn_is_never_announced() {
    let cancelled = [
        [Reply::Idle(StopReason::Cancelled), Reply::Answered(Ok(()))],
        [Reply::Answered(Ok(())), Reply::Idle(StopReason::Cancelled)],
        [Reply::Idle(StopReason::Cancelled), Reply::Answered(Err(PromptRejection::Cancelled))],
        [Reply::Answered(Err(PromptRejection::Cancelled)), Reply::Idle(StopReason::Cancelled)],
        [Reply::Idle(StopReason::EndTurn), Reply::Answered(Err(PromptRejection::Cancelled))],
        [Reply::Answered(Err(PromptRejection::Cancelled)), Reply::Idle(StopReason::EndTurn)],
    ];
    for replies in cancelled {
        let mut core = core();
        let request_id = started_turn(&mut core, "hello");

        assert_eq!(reply(&mut core, request_id, replies.clone()), 0, "{replies:?}");
    }
}

#[test]
fn a_turn_is_announced_only_once_the_prompt_queued_behind_it_finishes_too() {
    let mut core = core();
    let first = started_turn(&mut core, "first");
    let second = submit(&mut core, "second");

    let first_replies = [Reply::Answered(Ok(())), Reply::Idle(StopReason::EndTurn)];
    assert_eq!(reply(&mut core, first, first_replies), 0, "the second prompt is still queued");

    core.on_acp_event(running_notification("test-session").into());
    let second_replies = [Reply::Idle(StopReason::EndTurn), Reply::Answered(Ok(()))];
    assert_eq!(reply(&mut core, second, second_replies), 1);
}

#[test]
fn a_new_session_starts_empty_and_keeps_the_previous_selections() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().config_options(vec![model("slow")]).build());
    core.notify("old notice");
    core.new_session();
    assert_eq!(core.conversation().items(), []);

    let created = NewSessionResponse::new("fresh").config_options(vec![model("fast")]);
    deliver(&mut core, CommandResult::NewSession(Ok(created)));

    assert_eq!(core.session().session_id(), &SessionId::new("fresh"));
    assert!(core.foreground().is_idle());
    assert!(core.take_effects().iter().any(|effect| matches!(
        effect,
        Effect::Dispatch(Command::Agent(AgentCommand::SetConfigOption { session_id, config_id, value, .. }))
            if *session_id == SessionId::new("fresh") && config_id == "model" && *value == "slow".into()
    )));
}

#[test]
fn a_stale_resume_result_is_ignored() {
    let mut core = core();
    core.resume_session(&SessionId::new("wanted"), Path::new("/workspace"));

    let result = Ok(acp::ResumeSessionResponse::new());
    deliver(&mut core, CommandResult::ResumeSession { session_id: SessionId::new("abandoned"), result });

    assert!(matches!(core.foreground(), ForegroundOperation::ResumingSession { .. }));
}

#[test]
fn resuming_a_session_replays_its_history_into_an_empty_conversation() {
    LocalSet::new().block_on(&runtime(), async {
        let agent = FakeAgent::default()
            .new_session_response(NewSessionResponse::new("current"))
            .replay_message("saved", "replayed answer");
        let mut live = LiveSession::connect(agent).await;
        live.core.notify("from the old session");

        live.core.resume_session(&SessionId::new("saved"), Path::new("/saved"));
        live.run_until(|core| core.foreground().is_idle() && !core.conversation().items().is_empty()).await;

        assert_eq!(live.core.session().session_id(), &SessionId::new("saved"));
        assert_eq!(live.core.session().working_dir(), Path::new("/saved"));
        assert_eq!(texts(&live.core), ["replayed answer"]);
        live.dispatcher.shutdown().await;
    });
}

#[test]
fn a_connection_resumes_a_saved_session_and_its_replayed_history_reaches_a_fresh_core() {
    LocalSet::new().block_on(&runtime(), async {
        let agent = FakeAgent::default().replay_message("saved", "replayed answer");
        let connection = Connection::connect_to(client_info(), agent.serve(), "/saved".into()).await.unwrap();

        let mut live = LiveSession::start(connection.resume_session(SessionId::new("saved")).await.unwrap());
        live.run_until(|core| !core.conversation().items().is_empty()).await;

        assert_eq!(live.core.session().session_id(), &SessionId::new("saved"));
        assert_eq!(texts(&live.core), ["replayed answer"]);
        assert!(live.core.conversation().turn().is_idle());
        live.dispatcher.shutdown().await;
    });
}

/// A core attached to a real ACP connection, with every command it dispatches run and every
/// agent event fed back in.
struct LiveSession {
    core: SessionCore,
    dispatcher: CommandDispatcher,
    events: mpsc::UnboundedReceiver<AcpEvent>,
}

impl LiveSession {
    async fn connect(agent: FakeAgent) -> Self {
        let connection = Connection::connect_to(client_info(), agent.serve(), "/workspace".into()).await.unwrap();
        Self::start(connection.new_session().await.unwrap())
    }

    fn start(session: Session) -> Self {
        let (config, handle, events) = session.into_core("test");
        Self { core: SessionCore::new(config), dispatcher: CommandDispatcher::new(handle), events }
    }

    /// Runs the core's commands and feeds it their results and the agent's events until `done`
    /// holds.
    async fn run_until(&mut self, done: impl Fn(&SessionCore) -> bool) {
        loop {
            let effects = self.core.take_effects();
            if effects.is_empty() {
                if done(&self.core) {
                    return;
                }
                let has_pending_tasks = self.dispatcher.has_pending_tasks();
                tokio::select! {
                    Some(result) = self.dispatcher.next_result(), if has_pending_tasks => {
                        self.core.on_command_result(result);
                    }
                    Some(event) = self.events.recv() => self.core.on_acp_event(event),
                }
            }
            for effect in effects {
                if let Effect::Dispatch(command) = effect
                    && let Some(result) = self.dispatcher.dispatch(command)
                {
                    self.core.on_command_result(result);
                }
            }
        }
    }
}

/// How the agent answers a prompt whose turn is running.
#[test]
fn a_git_review_reads_from_the_agents_diff_server_when_it_serves_one() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().git_diff().remote_workspace().build());

    core.open_git_review();

    assert!(
        matches!(review_source(&mut core), Some(ReviewSource::Agent { session_id }) if session_id == "test-session")
    );
}

#[test]
fn a_git_review_of_an_agent_that_serves_no_diff_reads_the_working_tree_it_shares() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().working_dir("/repo").build());

    core.open_git_review();

    assert!(matches!(review_source(&mut core), Some(ReviewSource::Local(path)) if path == Path::new("/repo")));
}

#[test]
fn a_git_review_of_a_remote_agent_that_serves_no_diff_is_unavailable() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().remote_workspace().build());

    core.open_git_review();

    assert!(matches!(review_source(&mut core), Some(ReviewSource::Unavailable(_))));
}

#[derive(Debug, Clone)]
enum Reply {
    Idle(StopReason),
    Answered(Result<(), PromptRejection>),
}

fn core() -> SessionCore {
    SessionCore::new(SessionCoreConfigBuilder::new().build())
}

fn client_info() -> acp::Implementation {
    acp::Implementation::new("test", "1")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn deliver(core: &mut SessionCore, result: CommandResult) {
    core.on_command_result(result);
}

fn answered(request_id: RequestId, result: Result<(), PromptRejection>) -> CommandResult {
    CommandResult::Prompt { request_id, result }
}

/// Submits `text`, returning the request id of the prompt it sent.
fn submit(core: &mut SessionCore, text: &str) -> RequestId {
    core.submit(submission(text));
    let prompts: Vec<_> = core
        .take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(Command::Agent(AgentCommand::Prompt { request_id, text: sent, .. })) if sent == text => {
                Some(request_id)
            }
            _ => None,
        })
        .collect();
    let [request_id] = prompts[..] else { panic!("expected one prompt for {text:?}") };
    request_id
}

/// Submits `text` and starts the agent's turn for it.
fn started_turn(core: &mut SessionCore, text: &str) -> RequestId {
    let request_id = submit(core, text);
    core.on_acp_event(running_notification("test-session").into());
    request_id
}

/// Feeds `replies` to the prompt `request_id`, counting the turns announced.
fn reply(core: &mut SessionCore, request_id: RequestId, replies: impl IntoIterator<Item = Reply>) -> usize {
    for reply in replies {
        match reply {
            Reply::Idle(stop_reason) => core.on_acp_event(idle_notification("test-session", Some(stop_reason)).into()),
            Reply::Answered(result) => deliver(core, answered(request_id, result)),
        }
    }
    core.take_effects().iter().filter(|effect| matches!(effect, Effect::TurnFinished)).count()
}

fn restored(core: &mut SessionCore) -> Option<Submission> {
    core.take_effects().into_iter().find_map(|effect| match effect {
        Effect::RestoreSubmission(submission) => Some(submission),
        _ => None,
    })
}

fn submission(text: &str) -> Submission {
    Submission { text: text.to_string(), ..Submission::default() }
}

fn model(current: &str) -> SessionConfigOption {
    let options = vec![SessionConfigSelectOption::new("fast", "Fast"), SessionConfigSelectOption::new("slow", "Slow")];
    SessionConfigOption::select("model", "Model", current.to_string(), options)
}

fn notices(core: &SessionCore) -> Vec<&str> {
    core.conversation()
        .items()
        .iter()
        .filter_map(|item| match item.content() {
            ConversationContent::Notice(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn texts(core: &SessionCore) -> Vec<String> {
    core.conversation().items().iter().filter_map(|item| item.text().map(std::borrow::Cow::into_owned)).collect()
}

fn review_source(core: &mut SessionCore) -> Option<ReviewSource> {
    core.take_effects().into_iter().find_map(|effect| match effect {
        Effect::Dispatch(Command::GitReview(GitReviewCommand::Open(source))) => Some(source),
        _ => None,
    })
}
