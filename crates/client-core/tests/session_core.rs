use acp_utils::client::AcpEvent;
use acp_utils::conversation::ConversationContent;
use acp_utils::notifications::GitDiffEventPayload;
use acp_utils::testing::{FakeAgent, idle_notification, running_notification, test_connection};
use agent_client_protocol::Channel;
use agent_client_protocol::schema::v2::{
    self as acp, ContentBlock, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, ImageContent,
    NewSessionResponse, SessionConfigOption, SessionConfigSelectOption, SessionId, SessionInfo,
    SetSessionConfigOptionResponse, StopReason,
};
use clankerdiff_client::protocol::server::ServerMessage;
use client_core::command::{
    AgentCommand, Command, CommandResult, FilesystemCommand, GitReviewCommand, PromptRejection,
};
use client_core::composer::attachment::{AttachmentOutcome, PromptAttachment};
use client_core::composer::submission::Submission;
use client_core::request::RequestId;
use client_core::runtime::{CommandDispatcher, NoHost};
use client_core::session::{Connection, Session};
use client_core::session_core::{
    Effect, Elicitation, ForegroundOperation, QueuedPrompt, SessionCore, SessionCoreConfig,
};
use client_core::testing::{FakeExecutor, FakeGit, SessionCoreConfigBuilder};
use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::path::Path;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{LocalSet, spawn_local};
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[test]
fn a_submission_is_sent_as_a_prompt_and_tracked_until_answered() {
    let mut core = core();

    let request_id = sent_prompt(&core.submit(submission("hello")), "hello");

    assert_eq!(core.queued_prompts(), [QueuedPrompt { request_id, submission: submission("hello") }]);
    assert!(core.waiting_for_response());
    assert!(core.on_command_result(prompt_answered(request_id, Ok(()))).is_empty());
    assert_eq!(core.queued_prompts(), []);
}

#[test]
fn prompts_submitted_during_a_turn_queue_behind_it() {
    let mut core = core();
    let first = sent_prompt(&core.submit(submission("first")), "first");
    core.on_acp_event(running_notification("test-session").into());

    sent_prompt(&core.submit(submission("second")), "second");
    assert_eq!(queued_texts(&core), ["first", "second"]);

    core.on_command_result(prompt_answered(first, Ok(())));
    assert_eq!(queued_texts(&core), ["second"]);
    let effects = core.on_acp_event(idle_notification("test-session", Some(StopReason::EndTurn)).into());
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::TurnFinished)), "a prompt is still queued");
}

#[test]
fn attachments_are_read_before_the_prompt_is_sent() {
    let mut core = core();
    let attachment = PromptAttachment { path: "/workspace/notes.md".into(), display_name: "notes.md".into() };
    let with_file = Submission { mentions: vec![attachment.clone()], ..submission("see notes") };

    let effects = core.submit(with_file.clone());
    assert!(matches!(
        &effects[..],
        [Effect::Dispatch(Command::Filesystem(FilesystemCommand::PrepareSubmission { attachments }))]
            if *attachments == [attachment]
    ));
    assert!(matches!(core.foreground(), ForegroundOperation::PreparingPrompt(prepared) if *prepared == with_file));

    let blocks = vec![ContentBlock::from("file contents")];
    let effects = core.on_command_result(prepared(AttachmentOutcome { blocks: blocks.clone(), warnings: Vec::new() }));
    assert!(matches!(
        &effects[..],
        [Effect::Dispatch(Command::Agent(AgentCommand::Prompt { text, content: Some(content), .. }))]
            if text == "see notes" && *content == blocks
    ));
    assert!(core.foreground().is_idle());
}

#[test]
fn media_the_agent_cannot_take_gives_the_submission_back() {
    let mut core = core();
    let photo = PromptAttachment { path: "/workspace/photo.png".into(), display_name: "photo.png".into() };
    let with_photo = Submission { media: vec![photo], ..submission("what is this?") };
    core.submit(with_photo.clone());

    let image = ContentBlock::Image(ImageContent::new("aGk=", "image/png"));
    let effects = core.on_command_result(prepared(AttachmentOutcome { blocks: vec![image], warnings: Vec::new() }));

    assert!(matches!(&effects[0], Effect::RestoreSubmission(restored) if *restored == with_photo));
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
        let request_id = sent_prompt(&core.submit(submission("retry me")), "retry me");

        let effects = core.on_command_result(prompt_answered(request_id, Err(rejection)));

        assert!(matches!(&effects[0], Effect::RestoreSubmission(restored) if *restored == submission("retry me")));
        assert_eq!(core.queued_prompts(), []);
        assert_eq!(notices(&core), expected_notices);
    }
}

#[test]
fn a_turn_ending_on_its_own_is_announced_but_a_cancelled_one_is_not() {
    for (stop_reason, announced) in [(StopReason::EndTurn, true), (StopReason::Cancelled, false)] {
        let mut core = core();
        core.on_acp_event(running_notification("test-session").into());

        let effects = core.on_acp_event(idle_notification("test-session", Some(stop_reason)).into());

        assert_eq!(effects.iter().any(|effect| matches!(effect, Effect::TurnFinished)), announced);
        assert!(core.conversation().turn().is_idle());
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

        assert_eq!(finished_turns(&mut core, request_id, replies.clone()), 1, "{replies:?}");
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

        assert_eq!(finished_turns(&mut core, request_id, replies.clone()), 0, "{replies:?}");
    }
}

#[test]
fn a_turn_is_announced_only_once_the_prompt_queued_behind_it_finishes_too() {
    let mut core = core();
    let first = started_turn(&mut core, "first");
    let second = sent_prompt(&core.submit(submission("second")), "second");

    let first_replies = [Reply::Answered(Ok(())), Reply::Idle(StopReason::EndTurn)];
    assert_eq!(finished_turns(&mut core, first, first_replies), 0, "the second prompt is still queued");

    core.on_acp_event(running_notification("test-session").into());
    let second_replies = [Reply::Idle(StopReason::EndTurn), Reply::Answered(Ok(()))];
    assert_eq!(finished_turns(&mut core, second, second_replies), 1);
}

#[test]
fn events_for_another_session_leave_the_conversation_alone() {
    let mut core = core();

    let effects = core.on_acp_event(running_notification("other-session").into());

    assert!(effects.is_empty());
    assert!(core.conversation().turn().is_idle());
}

#[test]
fn a_new_session_starts_empty_and_keeps_the_previous_selections() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().config_options(vec![model("slow")]).build());
    core.notify("old notice");

    let effects = core.new_session();
    assert!(matches!(
        &effects[..],
        [Effect::Dispatch(Command::Agent(AgentCommand::NewSession { cwd }))] if cwd == Path::new(".")
    ));
    assert_eq!(core.conversation().items(), []);
    assert!(matches!(core.foreground(), ForegroundOperation::CreatingSession { .. }));

    let created = NewSessionResponse::new("fresh").config_options(vec![model("fast")]);
    let effects = core.on_command_result::<Infallible>(CommandResult::NewSession(Ok(created)));

    assert!(matches!(effects[0], Effect::SessionCreated));
    assert!(matches!(
        &effects[1..],
        [Effect::Dispatch(Command::Agent(AgentCommand::SetConfigOption { session_id, config_id, value, .. }))]
            if *session_id == SessionId::new("fresh") && config_id == "model" && *value == "slow".into()
    ));
    assert_eq!(core.session().session_id(), &SessionId::new("fresh"));
    assert!(core.foreground().is_idle());
}

#[test]
fn a_new_session_waits_for_the_running_turn() {
    let mut core = core();
    core.on_acp_event(running_notification("test-session").into());

    let effects = core.new_session();

    assert!(matches!(&effects[..], [Effect::Notice(_)]));
    assert_eq!(notices(&core), ["[test] Cannot clear while a prompt or session transition is in progress"]);
    assert!(core.foreground().is_idle());
}

#[test]
fn resuming_a_session_replays_its_history_into_an_empty_conversation() {
    LocalSet::new().block_on(&runtime(), async {
        let agent = FakeAgent::default()
            .new_session_response(NewSessionResponse::new("current"))
            .replay_message("saved", "replayed answer");
        let mut live = LiveSession::connect(agent).await;
        live.core.notify("from the old session");

        let effects = live.core.resume_session(&SessionId::new("saved"), Path::new("/saved"));
        assert_eq!(live.core.conversation().items(), []);
        let effects = live
            .run_until(effects, |core| core.foreground().is_idle() && !core.conversation().items().is_empty())
            .await;

        assert!(matches!(&effects[..], [Effect::ResumeStarted, Effect::SessionResumed]));
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
        let connection = Connection::connect_to(client_info(), serve(agent), "/saved".into()).await.unwrap();

        let mut live = LiveSession::start(connection.resume_session(SessionId::new("saved")).await.unwrap());
        live.run_until(Vec::new(), |core| !core.conversation().items().is_empty()).await;

        assert_eq!(live.core.session().session_id(), &SessionId::new("saved"));
        assert_eq!(live.core.session().working_dir(), Path::new("/saved"));
        assert_eq!(texts(&live.core), ["replayed answer"]);
        assert!(live.core.conversation().turn().is_idle());
        live.dispatcher.shutdown().await;
    });
}

#[test]
fn a_stale_resume_result_is_ignored() {
    let mut core = core();
    core.resume_session(&SessionId::new("wanted"), Path::new("/workspace"));

    let stale = CommandResult::ResumeSession {
        session_id: SessionId::new("abandoned"),
        result: Ok(acp::ResumeSessionResponse::new()),
    };

    assert!(core.on_command_result::<Infallible>(stale).is_empty());
    assert!(matches!(core.foreground(), ForegroundOperation::ResumingSession { .. }));
}

#[test]
fn a_prompt_runs_a_full_turn_against_the_agent() {
    LocalSet::new().block_on(&runtime(), async {
        let agent =
            FakeAgent::default().new_session_response(NewSessionResponse::new("current")).prompt_reply("hi there");
        let mut live = LiveSession::connect(agent).await;

        let effects = live.core.submit(submission("hello"));
        let effects = live
            .run_until(effects, |core| !core.waiting_for_response() && texts(core).contains(&"hi there".into()))
            .await;

        assert_eq!(effects.iter().filter(|effect| matches!(effect, Effect::TurnFinished)).count(), 1);
        assert_eq!(texts(&live.core), ["hello", "hi there"]);
        assert_eq!(live.core.queued_prompts(), []);
        live.dispatcher.shutdown().await;
    });
}

#[test]
fn a_config_selection_applies_at_once_and_follows_the_agents_reply() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().config_options(vec![model("fast")]).build());

    let effects = core.set_config_option("model", "slow");
    assert_eq!(core.session().config_options()[0].current_value(), Some("slow"));
    let conversation_id = core.conversation().id();
    assert!(matches!(
        &effects[..],
        [Effect::Dispatch(Command::Agent(AgentCommand::SetConfigOption { conversation_id: id, config_id, value, .. }))]
            if *id == conversation_id && config_id == "model" && *value == "slow".into()
    ));

    let reply = SetSessionConfigOptionResponse::new(vec![model("fast")]);
    let effects = core
        .on_command_result::<Infallible>(CommandResult::ConfigOptionsUpdated { conversation_id, result: Ok(reply) });

    assert!(matches!(&effects[..], [Effect::ConfigOptionsChanged]));
    assert_eq!(core.session().config_options()[0].current_value(), Some("fast"));
}

#[test]
fn a_config_reply_for_a_cleared_conversation_is_ignored() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().config_options(vec![model("fast")]).build());
    let conversation_id = core.conversation().id();
    core.on_acp_event(AcpEvent::ContextCleared(acp_utils::notifications::ContextClearedParams {
        session_id: "test-session".into(),
    }));

    let reply = SetSessionConfigOptionResponse::new(vec![model("slow")]);
    let effects = core
        .on_command_result::<Infallible>(CommandResult::ConfigOptionsUpdated { conversation_id, result: Ok(reply) });

    assert!(effects.is_empty());
    assert_eq!(core.session().config_options()[0].current_value(), Some("fast"));
}

#[test]
fn an_elicitation_is_handed_to_the_host() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let request = form_elicitation("Pick a branch");

        let (event, _response) = elicitation_event(request.clone()).await;
        let effects = core.on_acp_event(event);

        assert!(matches!(
            &effects[..],
            [Effect::ShowElicitation { elicitation: Elicitation::Request(params), .. }] if **params == request
        ));
    });
}

#[test]
fn an_elicitation_the_host_drops_unanswered_is_cancelled() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let (event, response) = elicitation_event(form_elicitation("Pick a branch")).await;

        drop(core.on_acp_event(event));

        assert!(matches!(response.await.unwrap().action, ElicitationAction::Cancel));
    });
}

#[test]
fn the_host_answers_an_elicitation_through_its_responder() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let (event, response) = elicitation_event(form_elicitation("Pick a branch")).await;
        let mut effects = core.on_acp_event(event);
        let Some(Effect::ShowElicitation { responder, .. }) = effects.pop() else { panic!("expected an elicitation") };

        responder
            .accept(Some(BTreeMap::from([("branch".to_string(), ElicitationContentValue::String("main".into()))])));

        let ElicitationAction::Accept(accept) = response.await.unwrap().action else { panic!("expected accept") };
        assert_eq!(accept.content.unwrap()["branch"], ElicitationContentValue::String("main".into()));
    });
}

#[test]
fn the_host_declines_an_elicitation_through_its_responder() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let (event, response) = elicitation_event(form_elicitation("Pick a branch")).await;
        let mut effects = core.on_acp_event(event);
        let Some(Effect::ShowElicitation { responder, .. }) = effects.pop() else { panic!("expected an elicitation") };

        responder.decline();

        assert!(matches!(response.await.unwrap().action, ElicitationAction::Decline));
    });
}

#[test]
fn an_artifact_review_elicitation_is_recognised() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let meta = ArtifactReviewElicitationMeta::new(Some("docs/plan.md".into()), "Review the plan", "# Plan");
        let request = form_elicitation("Review the plan").meta(meta.to_meta());

        let (event, _response) = elicitation_event(request).await;
        let effects = core.on_acp_event(event);

        assert!(matches!(
            &effects[..],
            [Effect::ShowElicitation { elicitation: Elicitation::ArtifactReview(reviewed), .. }] if *reviewed == meta
        ));
    });
}

#[test]
fn git_diff_events_are_forwarded_to_the_review_client() {
    let mut core = core();
    let event = AcpEvent::GitDiffEvent(GitDiffEventPayload {
        session_id: "test-session".into(),
        event: ServerMessage::Health { error: None },
    });

    let effects = core.on_acp_event(event);

    assert!(matches!(
        &effects[..],
        [Effect::Dispatch(Command::GitReview(GitReviewCommand::Forward(ServerMessage::Health { error: None })))]
    ));
}

#[test]
fn an_open_git_review_publishes_the_working_tree_diff() {
    let mut git = FakeGit::new("/workspace");
    git.add_file("src/lib.rs", "pub fn answer() -> u32 { 42 }\n");
    let mut executor = FakeExecutor::<Infallible>::with_git(git);
    let mut core = core();
    let mut published = Vec::new();

    executor.record(commands(core.open_git_review()));
    executor.settle(|result| {
        let (commands, effects) = split(core.on_command_result::<Infallible>(result));
        published.extend(effects.into_iter().filter_map(|effect| match effect {
            Effect::GitReviewState(state) => Some(state),
            _ => None,
        }));
        commands
    });

    let snapshot = published.last().and_then(|state| state.snapshot.clone()).expect("a published diff");
    let paths: Vec<_> = snapshot.document.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["src/lib.rs"]);
    assert!(executor.git_review_active());
    assert!(matches!(&core.close_git_review()[..], [Effect::Dispatch(Command::GitReview(GitReviewCommand::Close))]));
}

#[test]
fn a_review_is_sent_as_a_prompt() {
    let mut core = core();

    let effects = core.submit_review("Rename `foo` to `bar`.");

    assert!(matches!(&effects[..], [Effect::Notice(_), Effect::Dispatch(_), Effect::ReviewSubmitted]));
    sent_prompt(&effects, "Rename `foo` to `bar`.");
    assert_eq!(queued_texts(&core), ["Rename `foo` to `bar`."]);
    assert_eq!(notices(&core), ["[test] Submitted review of working tree diff.\nRename `foo` to `bar`."]);
}

#[test]
fn a_review_submitted_during_a_turn_queues_behind_it() {
    let mut core = core();
    started_turn(&mut core, "first");

    let effects = core.submit_review("Rename `foo` to `bar`.");

    assert!(matches!(&effects[..], [Effect::Notice(_), Effect::Dispatch(_), Effect::ReviewSubmitted]));
    sent_prompt(&effects, "Rename `foo` to `bar`.");
    assert_eq!(queued_texts(&core), ["first", "Rename `foo` to `bar`."]);
}

#[test]
fn a_review_submitted_during_a_session_transition_is_not_sent() {
    let mut core = core();
    core.resume_session(&SessionId::new("saved"), Path::new("/saved"));

    let effects = core.submit_review("Rename `foo` to `bar`.");

    assert!(matches!(&effects[..], [Effect::Notice(_)]));
    assert_eq!(core.queued_prompts(), []);
    assert_eq!(notices(&core), ["[test] Cannot submit review while a prompt or session transition is in progress"]);
}

#[test]
fn listed_sessions_leave_out_the_current_one() {
    let mut core = core();
    let listed = acp::ListSessionsResponse::new(vec![
        SessionInfo::new("test-session", "/workspace"),
        SessionInfo::new("older", "/workspace"),
    ]);

    let effects = core.on_command_result::<Infallible>(CommandResult::SessionsListed(Ok(listed)));

    assert!(matches!(
        &effects[..],
        [Effect::SessionsListed(sessions)] if sessions.iter().map(|s| s.session_id.0.as_ref()).eq(["older"])
    ));
}

#[test]
fn a_closed_connection_ends_the_turn_and_the_foreground_operation() {
    let mut core = core();
    core.begin_workspace_move();
    assert!(matches!(core.foreground(), ForegroundOperation::ListingWorkspaces));
    core.on_acp_event(running_notification("test-session").into());

    let effects = core.on_acp_event(AcpEvent::ConnectionClosed);

    assert!(matches!(&effects[..], [Effect::ConnectionClosed]));
    assert!(core.foreground().is_idle());
    assert!(core.conversation().turn().is_idle());
}

/// A core attached to a real ACP connection, with every effect's command run
/// through the dispatcher and every agent event fed back in.
struct LiveSession {
    core: SessionCore,
    dispatcher: CommandDispatcher<NoHost>,
    events: mpsc::UnboundedReceiver<AcpEvent>,
}

impl LiveSession {
    async fn connect(agent: FakeAgent) -> Self {
        Self::start(
            Connection::connect_to(client_info(), serve(agent), "/workspace".into())
                .await
                .unwrap()
                .new_session()
                .await
                .unwrap(),
        )
    }

    fn start(session: Session) -> Self {
        let Session { client, response, working_dir, workspace_status, workspace_access } = session;
        let config = SessionCoreConfig {
            initialize_response: client.initialize_response,
            session_response: response,
            working_dir,
            workspace_status,
            workspace_access,
            client_name: "test".into(),
        };
        Self {
            core: SessionCore::new(config),
            dispatcher: CommandDispatcher::new(client.handle, NoHost),
            events: client.event_rx,
        }
    }

    /// Runs `effects` and everything they lead to until `done` holds, returning
    /// the effects other than dispatches, in order.
    async fn run_until(&mut self, effects: Vec<Effect>, done: impl Fn(&SessionCore) -> bool) -> Vec<Effect> {
        let mut pending = VecDeque::from(effects);
        let mut observed = Vec::new();
        loop {
            while let Some(effect) = pending.pop_front() {
                match effect {
                    Effect::Dispatch(command) => {
                        if let Some(result) = self.dispatcher.dispatch(command) {
                            pending.extend(self.core.on_command_result(result));
                        }
                    }
                    effect => observed.push(effect),
                }
            }
            if done(&self.core) {
                return observed;
            }
            let has_pending_tasks = self.dispatcher.has_pending_tasks();
            tokio::select! {
                Some(result) = self.dispatcher.next_result(), if has_pending_tasks => {
                    pending.extend(self.core.on_command_result(result));
                }
                Some(event) = self.events.recv() => pending.extend(self.core.on_acp_event(event)),
            }
        }
    }
}

/// Serves `agent` in memory, returning the client's end of the transport.
fn serve(agent: FakeAgent) -> Channel {
    let (agent_transport, client_transport) = Channel::duplex();
    spawn_local(agent.agent().connect_to(agent_transport));
    client_transport
}

fn client_info() -> acp::Implementation {
    acp::Implementation::new("test", "1")
}

/// How the agent answers a prompt whose turn is running.
#[derive(Debug, Clone)]
enum Reply {
    Idle(StopReason),
    Answered(Result<(), PromptRejection>),
}

fn core() -> SessionCore {
    SessionCore::new(SessionCoreConfigBuilder::new().build())
}

/// Submits `text` and starts the agent's turn for it.
fn started_turn(core: &mut SessionCore, text: &str) -> RequestId {
    let request_id = sent_prompt(&core.submit(submission(text)), text);
    core.on_acp_event(running_notification("test-session").into());
    request_id
}

/// Feeds `replies` to the prompt `request_id`, counting the turns announced.
fn finished_turns(core: &mut SessionCore, request_id: RequestId, replies: impl IntoIterator<Item = Reply>) -> usize {
    replies
        .into_iter()
        .flat_map(|reply| match reply {
            Reply::Idle(stop_reason) => core.on_acp_event(idle_notification("test-session", Some(stop_reason)).into()),
            Reply::Answered(result) => core.on_command_result(prompt_answered(request_id, result)),
        })
        .filter(|effect| matches!(effect, Effect::TurnFinished))
        .count()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn submission(text: &str) -> Submission {
    Submission { text: text.to_string(), ..Submission::default() }
}

fn model(current: &str) -> SessionConfigOption {
    SessionConfigOption::select(
        "model",
        "Model",
        current.to_string(),
        vec![SessionConfigSelectOption::new("fast", "Fast"), SessionConfigSelectOption::new("slow", "Slow")],
    )
}

fn form_elicitation(message: &str) -> CreateElicitationRequest {
    CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), ElicitationSchema::new()),
        message,
    )
}

/// An elicitation from the agent, with the agent's end of its answer.
async fn elicitation_event(
    params: CreateElicitationRequest,
) -> (AcpEvent, oneshot::Receiver<CreateElicitationResponse>) {
    let (cx, mut peer) = test_connection().await;
    let (responder, response) = peer.fake_elicitation(&cx).await;
    (AcpEvent::ElicitationRequest { params: Box::new(params), responder }, response)
}

fn prompt_answered(request_id: RequestId, result: Result<(), PromptRejection>) -> CommandResult<Infallible> {
    CommandResult::Prompt { request_id, result }
}

fn prepared(outcome: AttachmentOutcome) -> CommandResult<Infallible> {
    CommandResult::SubmissionPrepared(outcome)
}

/// The request id of the one prompt `effects` sent, after checking its text.
fn sent_prompt(effects: &[Effect], expected_text: &str) -> RequestId {
    let prompts: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(Command::Agent(AgentCommand::Prompt { request_id, session_id, text, .. })) => {
                assert_eq!(session_id, &SessionId::new("test-session"));
                assert_eq!(text, expected_text);
                Some(*request_id)
            }
            _ => None,
        })
        .collect();
    let [request_id] = prompts[..] else { panic!("expected one prompt in {effects:?}") };
    request_id
}

fn commands(effects: Vec<Effect>) -> Vec<Command<Infallible>> {
    split(effects).0
}

fn split(effects: Vec<Effect>) -> (Vec<Command<Infallible>>, Vec<Effect>) {
    let mut commands = Vec::new();
    let mut others = Vec::new();
    for effect in effects {
        match effect {
            Effect::Dispatch(command) => commands.push(command),
            effect => others.push(effect),
        }
    }
    (commands, others)
}

fn queued_texts(core: &SessionCore) -> Vec<&str> {
    core.queued_prompts().iter().map(|prompt| prompt.submission.text.as_str()).collect()
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
