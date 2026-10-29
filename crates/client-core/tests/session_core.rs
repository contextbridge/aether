use acp_utils::client::{AcpClientError, AcpEvent};
use acp_utils::conversation::{ConversationContent, Notice, NoticeLevel};
use acp_utils::notifications::{WorkspaceEntry, WorkspaceListResponse, WorkspaceMoveResponse, WorkspaceMoveTarget};
use acp_utils::testing::{
    FakeAgent, idle_notification, message_chunk, on_session, running_notification, select, test_connection, texts,
    user_message,
};
use agent_client_protocol::schema::v2::{
    self as acp, ContentBlock, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, ImageContent, NewSessionResponse,
    SessionConfigOption, SessionId, StopReason,
};
use client_core::command::RequestId;
use client_core::command::{AgentCommand, Command, CommandResult, GitReviewCommand, PromptRejection, ReviewSource};
use client_core::composer::attachment::{AttachmentOutcome, PromptAttachment};
use client_core::composer::submission::Submission;
use client_core::elicitation::ElicitationAnswer;
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::{Effect, ForegroundOperation, SessionCore};
use client_core::session_model::{SessionSetting, WorkspaceAccess};
use client_core::testing::{LiveSession, SessionCoreConfigBuilder};
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;
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
    assert_eq!(
        texts(core.conversation()),
        ["ACP agent does not support image input."],
        "an unsent prompt stays out of the transcript"
    );
    assert_eq!(notices(&core, NoticeLevel::Error), ["ACP agent does not support image input."]);
}

#[test]
fn attachments_are_prepared_for_where_the_agent_runs() {
    for (config, expected) in [
        (SessionCoreConfigBuilder::new(), WorkspaceAccess::Local),
        (SessionCoreConfigBuilder::new().remote_workspace(), WorkspaceAccess::Remote),
    ] {
        let mut core = SessionCore::new(config.build());
        let notes = PromptAttachment { path: "/workspace/notes.md".into(), display_name: "notes.md".into() };

        core.submit(Submission { mentions: vec![notes.clone()], ..submission("read @notes.md") });

        let prepared = core.take_effects().into_iter().find_map(|effect| match effect {
            Effect::Dispatch(Command::PrepareSubmission { attachments, access }) => Some((attachments, access)),
            _ => None,
        });
        assert_eq!(prepared, Some((vec![notes], expected)));
    }
}

#[test]
fn a_rejected_prompt_gives_its_submission_back() {
    for (rejection, expected_errors) in [
        (PromptRejection::Cancelled, Vec::new()),
        (PromptRejection::Failed("offline".into()), vec!["Failed to send prompt: offline"]),
    ] {
        let mut core = core();
        let request_id = submit(&mut core, "retry me");

        deliver(&mut core, answered(request_id, Err(rejection)));

        assert_eq!(restored(&mut core), Some(submission("retry me")));
        assert_eq!(core.queued_prompts(), []);
        assert_eq!(notices(&core, NoticeLevel::Error), expected_errors);
        assert_eq!(notices(&core, NoticeLevel::Info), Vec::<&str>::new());
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
fn resuming_a_session_on_connecting_or_later_replays_its_history_into_an_empty_conversation() {
    for on_connect in [true, false] {
        LocalSet::new().block_on(&runtime(), async {
            let agent = FakeAgent::new("current").replay_message("saved", "replayed answer");
            let (cwd, request) =
                if on_connect { ("/saved", Some(SessionId::new("saved"))) } else { ("/workspace", None) };
            let mut live = LiveSession::connect(agent, cwd, request).await.unwrap();
            if !on_connect {
                live.core.notify("from the old session");
                live.core.resume_session(&SessionId::new("saved"), Path::new("/saved"));
            }
            live.run_until(|core| core.foreground().is_idle() && !core.conversation().items().is_empty()).await;

            assert_eq!(live.core.session().session_id(), &SessionId::new("saved"));
            assert_eq!(live.core.session().working_dir(), Path::new("/saved"));
            assert_eq!(texts(live.core.conversation()), ["replayed answer"]);
            assert!(live.core.conversation().turn().is_idle());
            live.shutdown().await;
        });
    }
}

#[test]
fn a_git_review_reads_the_diff_the_agent_serves_or_else_the_working_tree_it_shares() {
    let served = review_source(&SessionCoreConfigBuilder::new().git_diff().remote_workspace());
    let shared = review_source(&SessionCoreConfigBuilder::new().working_dir("/repo"));
    let remote = review_source(&SessionCoreConfigBuilder::new().remote_workspace());

    assert!(matches!(served, Some(ReviewSource::Agent { session_id }) if session_id == SessionId::new("test-session")));
    assert!(matches!(shared, Some(ReviewSource::Local(path)) if path == Path::new("/repo")));
    assert!(matches!(remote, Some(ReviewSource::Unavailable(_))));
}

#[test]
fn a_cancelled_turn_keeps_its_final_output_and_ignores_a_stale_prompt_result() {
    let mut core = core();
    let request_id = started_turn(&mut core, "hello");

    deliver(&mut core, CommandResult::Notified(Ok(())));
    assert!(core.waiting_for_response(), "cancelling is not finishing");
    core.on_acp_event(on_session("test-session", message_chunk("reply", "final output")));
    reply(&mut core, request_id, [Reply::Idle(StopReason::Cancelled), Reply::Answered(Ok(()))]);
    assert!(!core.waiting_for_response());

    deliver(&mut core, answered(RequestId::next(), Ok(())));

    assert!(!core.waiting_for_response(), "a result for no queued prompt changes nothing");
    assert_eq!(texts(core.conversation()).iter().filter(|text| *text == "final output").count(), 1);
}

#[test]
fn a_config_result_for_a_replaced_conversation_is_ignored() {
    let mut core = core();
    let stale = core.conversation().id();
    core.resume_session(&SessionId::new("restored"), Path::new("/workspace"));
    let resumed = acp::ResumeSessionResponse::new().config_options(vec![model("slow")]);
    deliver(&mut core, CommandResult::ResumeSession { session_id: SessionId::new("restored"), result: Ok(resumed) });

    let result = Ok(acp::SetSessionConfigOptionResponse::new(vec![]));
    deliver(&mut core, CommandResult::ConfigOptionsUpdated { conversation_id: stale, result });

    assert_eq!(core.session().config_options(), [LocalConfigOption::from_acp(model("slow"))]);
}

#[test]
fn selecting_a_value_announces_the_new_setting_before_asking_the_agent_to_apply_it() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().config_options(vec![model("slow")]).build());

    core.set_config_option("model", "fast");

    assert!(matches!(
        core.take_effects()[..],
        [
            Effect::SessionSettingsChanged(SessionSetting::ConfigOptions),
            Effect::Dispatch(Command::Agent(AgentCommand::SetConfigOption { .. }))
        ]
    ));
    assert_eq!(core.session().config_options()[0].current_value(), Some("fast"));
}

#[test]
fn a_replayed_session_cannot_replace_a_live_turn() {
    let mut core = core();
    submit(&mut core, "current");

    let replayed = Ok(acp::ResumeSessionResponse::new());
    deliver(&mut core, CommandResult::ResumeSession { session_id: SessionId::new("test-session"), result: replayed });

    assert!(core.waiting_for_response());
    assert_eq!(core.queued_prompts().len(), 1);
}

#[test]
fn answering_the_question_the_agent_waits_on_sends_the_answer() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let answer = ask(&mut core).await;
        let (id, _) = core.elicitation().expect("the question waits");

        core.answer_elicitation(id, ElicitationAnswer::Decline);

        assert!(core.elicitation().is_none());
        assert!(matches!(answer.await.unwrap().action, ElicitationAction::Decline));
    });
}

#[test]
fn a_question_no_client_can_answer_is_declined_without_asking_the_user() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let params: CreateElicitationRequest = serde_json::from_value(serde_json::json!({
            "mode": "form",
            "sessionId": "test-session",
            "message": "Mixed schema",
            "requestedSchema": {
                "type": "object",
                "properties": { "name": { "type": "string" }, "future": { "type": "future_widget" } },
                "required": ["future"]
            }
        }))
        .unwrap();
        let (event, answer) = elicitation_event(params).await;

        core.on_acp_event(event);

        assert!(core.elicitation().is_none());
        assert!(core.take_effects().is_empty());
        assert!(matches!(answer.await.unwrap().action, ElicitationAction::Decline));
    });
}

#[test]
fn a_new_question_replaces_the_one_before_and_cancels_it() {
    LocalSet::new().block_on(&runtime(), async {
        let mut core = core();
        let first = ask(&mut core).await;
        let (first_id, _) = core.elicitation().unwrap();

        let _second = ask(&mut core).await;
        core.answer_elicitation(first_id, ElicitationAnswer::Decline);

        assert!(matches!(first.await.unwrap().action, ElicitationAction::Cancel));
        assert!(core.elicitation().is_some_and(|(id, _)| id != first_id), "a late answer leaves the new question");
    });
}

#[test]
fn cancelling_the_turn_or_losing_the_connection_withdraws_the_question() {
    for withdraw in [SessionCore::cancel as fn(&mut SessionCore), |core| core.on_acp_event(AcpEvent::ConnectionClosed)]
    {
        LocalSet::new().block_on(&runtime(), async {
            let mut core = core();
            let answer = ask(&mut core).await;

            withdraw(&mut core);

            assert!(core.elicitation().is_none());
            assert!(core.take_effects().iter().any(|effect| matches!(effect, Effect::ElicitationDismissed)));
            assert!(matches!(answer.await.unwrap().action, ElicitationAction::Cancel));
        });
    }
}

#[test]
fn a_workspace_move_cannot_start_while_a_prompt_runs_or_another_move_is_under_way() {
    let mut running = workspace_core();
    submit(&mut running, "hello");
    let mut listing = workspace_core();
    listing.begin_workspace_move();
    assert_eq!(agent_commands(&mut listing).len(), 1);

    for mut core in [running, listing] {
        let before = format!("{:?}", core.foreground());

        core.begin_workspace_move();

        assert_eq!(agent_commands(&mut core).len(), 0, "no second listing is asked for");
        assert_eq!(format!("{:?}", core.foreground()), before);
        assert_eq!(
            notices(&core, NoticeLevel::Info),
            ["Cannot move workspace while a prompt is running or another move is in progress"]
        );
    }
}

#[test]
fn a_failed_session_transition_reports_why_and_leaves_the_session_usable() {
    type Start = fn(&mut SessionCore);
    type Failure = fn() -> CommandResult;
    let cases: [(Start, Failure, &str); 6] = [
        (SessionCore::new_session, || CommandResult::NewSession(Err(offline())), "Failed to create new session"),
        (SessionCore::list_sessions, || CommandResult::SessionsListed(Err(offline())), "Failed to list sessions"),
        (
            |core| core.resume_session(&SessionId::new("saved"), Path::new("/saved")),
            || CommandResult::ResumeSession { session_id: SessionId::new("saved"), result: Err(offline()) },
            "Failed to resume session",
        ),
        (
            SessionCore::begin_workspace_move,
            || CommandResult::WorkspacesListed(Err(offline())),
            "Failed to list workspaces",
        ),
        (pick_other_workspace, || CommandResult::WorkspaceMoved(Err(offline())), "Workspace move failed"),
        (
            |core| move_to(core, "/other"),
            || CommandResult::ResumeSession { session_id: SessionId::new("test-session"), result: Err(offline()) },
            "Failed to resume session",
        ),
    ];
    for (start, failure, expected) in cases {
        let mut core = workspace_core();
        start(&mut core);

        deliver(&mut core, failure());

        assert!(core.foreground().is_idle(), "{expected}");
        assert_eq!(notices(&core, NoticeLevel::Error), [format!("{expected}: ACP protocol error: offline")]);
        submit(&mut core, "retry");
    }
}

#[test]
fn a_moved_session_reloads_in_its_new_workspace_before_it_takes_new_work() {
    let mut core = workspace_core();
    core.notify("from before the move");

    move_to(&mut core, "/other");

    assert!(matches!(core.foreground(), ForegroundOperation::ResumingSession { moved: true, .. }));
    assert_eq!(core.session().working_dir(), Path::new("/other"));
    assert_eq!(core.conversation().items(), [], "the old transcript is replaced by the replay");
    assert!(agent_commands(&mut core).iter().any(|command| matches!(
        command,
        AgentCommand::ResumeSession { session_id, cwd } if session_id.0.as_ref() == "test-session" && cwd == Path::new("/other")
    )));

    core.new_session();
    core.submit(submission("too early"));
    let effects = core.take_effects();
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Dispatch(Command::Agent(_)))));
    assert!(effects.iter().any(|effect| matches!(effect, Effect::RestoreSubmission(_))));

    core.on_acp_event(on_session("test-session", user_message("replayed", vec!["replayed message".into()])));
    let result = Ok(acp::ResumeSessionResponse::new());
    deliver(&mut core, CommandResult::ResumeSession { session_id: SessionId::new("test-session"), result });

    assert!(core.foreground().is_idle());
    assert_eq!(
        notices(&core, NoticeLevel::Info),
        ["Cannot clear while a prompt or session transition is in progress", "Moved to /other"]
    );
    assert!(core.conversation().items().iter().any(|item| item.text().is_some_and(|text| text == "replayed message")));

    core.resume_session(&SessionId::new("another"), Path::new("/tmp"));
    assert_eq!(core.conversation().items(), [], "resuming elsewhere leaves the move behind");
}

#[test]
fn losing_the_connection_ends_a_workspace_pick() {
    let mut core = workspace_core();
    core.begin_workspace_move();
    deliver(&mut core, CommandResult::WorkspacesListed(Ok(WorkspaceListResponse { workspaces: other_workspace() })));
    assert!(matches!(core.foreground(), ForegroundOperation::PickingWorkspace));

    core.on_acp_event(AcpEvent::ConnectionClosed);

    assert!(core.foreground().is_idle());
}

#[derive(Debug, Clone)]
enum Reply {
    Idle(StopReason),
    Answered(Result<(), PromptRejection>),
}

fn core() -> SessionCore {
    SessionCore::new(SessionCoreConfigBuilder::new().build())
}

fn workspace_core() -> SessionCore {
    SessionCore::new(SessionCoreConfigBuilder::new().workspace_move().build())
}

fn pick_other_workspace(core: &mut SessionCore) {
    core.begin_workspace_move();
    deliver(core, CommandResult::WorkspacesListed(Ok(WorkspaceListResponse { workspaces: other_workspace() })));
    core.move_workspace(WorkspaceMoveTarget::Existing { path: "/other".into() });
}

fn move_to(core: &mut SessionCore, new_cwd: &str) {
    pick_other_workspace(core);
    deliver(core, CommandResult::WorkspaceMoved(Ok(WorkspaceMoveResponse { new_cwd: PathBuf::from(new_cwd) })));
}

fn other_workspace() -> Vec<WorkspaceEntry> {
    vec![
        WorkspaceEntry { path: "/workspace".into(), is_current: true },
        WorkspaceEntry { path: "/other".into(), is_current: false },
    ]
}

fn offline() -> AcpClientError {
    AcpClientError::Protocol(agent_client_protocol::Error::new(-32603, "offline"))
}

fn agent_commands(core: &mut SessionCore) -> Vec<AgentCommand> {
    core.take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(Command::Agent(command)) => Some(command),
            _ => None,
        })
        .collect()
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

fn submit(core: &mut SessionCore, text: &str) -> RequestId {
    core.submit(submission(text));
    let prompts: Vec<_> = core
        .take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(Command::Agent(AgentCommand::Prompt { request_id, content, .. }))
                if matches!(content.first(), Some(ContentBlock::Text(sent)) if sent.text == text) =>
            {
                Some(request_id)
            }
            _ => None,
        })
        .collect();
    let [request_id] = prompts[..] else { panic!("expected one prompt for {text:?}") };
    request_id
}

fn started_turn(core: &mut SessionCore, text: &str) -> RequestId {
    let request_id = submit(core, text);
    core.on_acp_event(running_notification("test-session").into());
    request_id
}

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
    select("model", "Model", current, &[("fast", "Fast"), ("slow", "Slow")])
}

fn notices(core: &SessionCore, level: NoticeLevel) -> Vec<&str> {
    core.conversation()
        .items()
        .iter()
        .filter_map(|item| match item.content() {
            ConversationContent::Notice(Notice { level: shown, text }) if *shown == level => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn review_source(config: &SessionCoreConfigBuilder) -> Option<ReviewSource> {
    let mut core = SessionCore::new(config.build());
    core.open_git_review();
    core.take_effects().into_iter().find_map(|effect| match effect {
        Effect::Dispatch(Command::GitReview(GitReviewCommand::Open(source))) => Some(source),
        _ => None,
    })
}

async fn ask(core: &mut SessionCore) -> oneshot::Receiver<CreateElicitationResponse> {
    let params = CreateElicitationRequest::new(
        ElicitationFormMode::new(ElicitationSessionScope::new("test-session"), ElicitationSchema::new()),
        "Continue?",
    );
    let (event, answer) = elicitation_event(params).await;
    core.on_acp_event(event);
    assert!(core.take_effects().iter().any(|effect| matches!(effect, Effect::ElicitationRequested)));
    answer
}

async fn elicitation_event(
    params: CreateElicitationRequest,
) -> (AcpEvent, oneshot::Receiver<CreateElicitationResponse>) {
    let (cx, mut peer) = test_connection().await;
    let (responder, answer) = peer.fake_elicitation(&cx).await;
    (AcpEvent::ElicitationRequest { params: Box::new(params), responder }, answer)
}
