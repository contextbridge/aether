use crate::support::{LiveSession, deliver, notices, offline, receive, run_local};
use acp_utils::client::AcpEvent;
use acp_utils::conversation::NoticeLevel;
use acp_utils::notifications::{
    AuthMethodsUpdatedParams, WorkspaceEntry, WorkspaceListResponse, WorkspaceMoveResponse, WorkspaceMoveTarget,
};
use acp_utils::testing::{
    FakeAgent, idle_notification, message_chunk, on_session, running_notification, select, test_connection, texts,
    user_message,
};
use agent_client_protocol::schema::v2::{
    self as acp, ContentBlock, CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationFormMode, ElicitationSchema, ElicitationSessionScope, ImageContent, NewSessionResponse,
    SessionConfigOption, SessionId, SessionUpdate, StopReason,
};
use clankerdiff_core::RepoPath;
use client_core::command::RequestId;
use client_core::command::{AgentCommand, Command, CommandResult, GitReviewCommand, PromptRejection, ReviewSource};
use client_core::composer::attachment::{AttachmentOutcome, PromptAttachment};
use client_core::composer::file_listing::FileListing;
use client_core::composer::submission::Submission;
use client_core::elicitation::ElicitationAnswer;
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::{Effect, ForegroundOperation, SessionCore};
use client_core::session_model::{SessionSetting, WorkspaceAccess};
use client_core::testing::{FakeGit, SessionCoreConfigBuilder};
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;

#[test]
fn media_the_agent_cannot_take_gives_the_submission_back() {
    let mut core = core();
    let photo = PromptAttachment { path: "/workspace/photo.png".into(), display_name: "photo.png".into() };
    let with_photo = Submission { media: vec![photo], ..Submission::from("what is this?") };
    core.submit(with_photo.clone());

    deliver(&mut core, prepared(vec![image()]));

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

        core.submit(Submission { mentions: vec![notes.clone()], ..Submission::from("read @notes.md") });

        let prepared = core.take_effects().into_iter().find_map(|effect| match effect {
            Effect::Dispatch(Command::PrepareSubmission { attachments, access }) => Some((attachments, access)),
            _ => None,
        });
        assert_eq!(prepared, Some((vec![notes], expected)));
    }
}

#[test]
fn a_prompt_sends_its_text_before_its_attachments_and_leaves_blank_text_out() {
    let photo = PromptAttachment { path: "/workspace/photo.png".into(), display_name: "photo.png".into() };
    for (text, expected) in
        [("describe this", vec![ContentBlock::from("describe this"), image()]), (" \n ", vec![image()])]
    {
        let images = acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new());
        let mut core = SessionCore::new(SessionCoreConfigBuilder::new().prompt_capabilities(images).build());
        core.submit(Submission { media: vec![photo.clone()], ..Submission::from(text) });

        deliver(&mut core, prepared(vec![image()]));

        let sent: Vec<_> = agent_commands(&mut core)
            .into_iter()
            .filter_map(|command| match command {
                AgentCommand::Prompt { content, .. } => Some(content),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [expected], "{text:?}");
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

        assert_eq!(restored(&mut core), Some(Submission::from("retry me")));
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

    receive(&mut core, running_notification("test-session"));
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

    assert_eq!(core.session_id(), &SessionId::new("fresh"));
    assert!(core.foreground().is_idle());
    assert!(core.take_effects().iter().any(|effect| matches!(
        effect,
        Effect::Dispatch(Command::Agent(AgentCommand::SetConfigOption { session_id, config_id, value, .. }))
            if *session_id == SessionId::new("fresh") && config_id == "model" && *value == "slow".into()
    )));
}

#[test]
fn resuming_a_session_on_connecting_or_later_replays_its_history_into_an_empty_conversation() {
    for on_connect in [true, false] {
        run_local(async {
            let (agent, mut requests) = FakeAgent::new("current").replay_message("saved", "replayed answer").capture();
            let (cwd, request) =
                if on_connect { ("/saved", Some(SessionId::new("saved"))) } else { ("/workspace", None) };
            let mut live = LiveSession::connect(agent, cwd, request).await.unwrap();
            if !on_connect {
                live.core.notify("from the old session");
                live.core.resume_session(&SessionId::new("saved"), Path::new("/saved"));
            }
            live.run_until(|core| core.foreground().is_idle() && !core.conversation().items().is_empty()).await;

            let created = std::iter::from_fn(|| requests.new_session.try_recv().ok()).count();
            assert_eq!(created, usize::from(!on_connect), "resuming never creates a session");
            assert_eq!(live.core.session_id(), &SessionId::new("saved"));
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
fn a_git_review_lists_the_files_of_the_repository_it_reads_as_they_change() {
    run_local(async {
        let git = FakeGit::new("/repo").with_committed("src/lib.rs", "pub fn a() {}\n").with_untracked("notes.md", "n");
        let mut live = LiveSession::reviewing(git.clone()).await.unwrap();
        assert_eq!(live.core.files(), FileListing::Pending);

        live.core.ensure_git_review();
        live.run_until(|core| core.files() != FileListing::Pending).await;

        let FileListing::Listed(files) = live.core.files() else { panic!("listed: {:?}", live.core.files()) };
        assert_eq!(files.root, Path::new("/repo"));
        assert_eq!(files.paths.iter().map(RepoPath::as_str).collect::<Vec<_>>(), ["notes.md", "src/lib.rs"]);
        let mention = files.attachment(&files.paths[1]);
        assert_eq!(
            (mention.path.as_path(), mention.display_name.as_str()),
            (Path::new("/repo/src/lib.rs"), "src/lib.rs")
        );

        git.add_file("src/new.rs", "pub fn b() {}\n");
        live.run_until(|core| matches!(core.files(), FileListing::Listed(files) if files.paths.len() == 3)).await;
        live.shutdown().await;
    });
}

#[test]
fn files_outside_a_repository_fail_with_the_reason() {
    run_local(async {
        let mut live = LiveSession::reviewing(FakeGit::not_a_repository("/repo")).await.unwrap();

        live.core.ensure_git_review();
        live.run_until(|core| core.files() != FileListing::Pending).await;

        assert!(
            matches!(live.core.files(), FileListing::Failed(reason) if reason.contains("not inside a Git worktree")),
            "{:?}",
            live.core.files()
        );
        live.shutdown().await;
    });
}

#[test]
fn a_cancelled_turn_keeps_its_final_output_and_ignores_a_stale_prompt_result() {
    let mut core = core();
    let request_id = started_turn(&mut core, "hello");

    deliver(&mut core, CommandResult::Notified(Ok(())));
    assert!(core.waiting_for_response(), "cancelling is not finishing");
    receive(&mut core, on_session("test-session", message_chunk("reply", "final output")));
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
fn a_resume_result_for_a_resume_no_longer_under_way_is_ignored() {
    let mut abandoned = core();
    abandoned.resume_session(&SessionId::new("wanted"), Path::new("/workspace"));
    let mut prompting = core();
    submit(&mut prompting, "current");

    for (mut core, resumed) in [(abandoned, "abandoned"), (prompting, "test-session")] {
        let foreground = format!("{:?}", core.foreground());
        let queued = core.queued_prompts().len();

        let result = Ok(acp::ResumeSessionResponse::new().config_options(vec![model("slow")]));
        deliver(&mut core, CommandResult::ResumeSession { session_id: SessionId::new(resumed), result });

        assert_eq!(format!("{:?}", core.foreground()), foreground, "{resumed}");
        assert_eq!(core.queued_prompts().len(), queued, "{resumed}");
        assert!(core.session().config_options().is_empty(), "{resumed}");
    }
}

#[test]
fn an_agent_config_update_replaces_the_session_selections() {
    let mode = select("mode", "Mode", "plan", &[("plan", "Plan"), ("code", "Code")]);
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().config_options(vec![model("slow"), mode]).build());
    assert_eq!(selections(&core), [("model", "slow"), ("mode", "plan")]);

    let update = SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![model("fast")]));
    receive(&mut core, on_session("test-session", update));

    assert_eq!(selections(&core), [("model", "fast")]);
    assert!(matches!(core.take_effects()[..], [Effect::SessionSettingsChanged(SessionSetting::ConfigOptions)]));
}

#[test]
fn an_auth_methods_update_replaces_the_advertised_methods() {
    let method = |id: &str| acp::AuthMethod::Agent(acp::AuthMethodAgent::new(id.to_owned(), id.to_owned()));
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().auth_methods(vec![method("initial")]).build());

    receive(
        &mut core,
        AcpEvent::AuthMethodsUpdated(AuthMethodsUpdatedParams { auth_methods: vec![method("updated")] }),
    );

    let methods: Vec<_> = core.session().auth_methods().iter().map(|method| method.method_id().0.as_ref()).collect();
    assert_eq!(methods, ["updated"]);
    assert!(matches!(core.take_effects()[..], [Effect::SessionSettingsChanged(SessionSetting::AuthMethods)]));
}

#[test]
fn answering_the_question_the_agent_waits_on_sends_the_answer() {
    run_local(async {
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
    run_local(async {
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

        receive(&mut core, event);

        assert!(core.elicitation().is_none());
        assert!(core.take_effects().is_empty());
        assert!(matches!(answer.await.unwrap().action, ElicitationAction::Decline));
    });
}

#[test]
fn a_new_question_replaces_the_one_before_and_cancels_it() {
    run_local(async {
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
    for withdraw in [SessionCore::cancel as fn(&mut SessionCore), |core| receive(core, AcpEvent::ConnectionClosed)] {
        run_local(async {
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
    core.submit(Submission::from("too early"));
    let effects = core.take_effects();
    assert!(!effects.iter().any(|effect| matches!(effect, Effect::Dispatch(Command::Agent(_)))));
    assert!(effects.iter().any(|effect| matches!(effect, Effect::RestoreSubmission(_))));

    receive(&mut core, on_session("test-session", user_message("replayed", vec!["replayed message".into()])));
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

    receive(&mut core, AcpEvent::ConnectionClosed);

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

fn agent_commands(core: &mut SessionCore) -> Vec<AgentCommand> {
    core.take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(Command::Agent(command)) => Some(command),
            _ => None,
        })
        .collect()
}

fn answered(request_id: RequestId, result: Result<(), PromptRejection>) -> CommandResult {
    CommandResult::Prompt { request_id, result }
}

fn submit(core: &mut SessionCore, text: &str) -> RequestId {
    core.submit(Submission::from(text));
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
    receive(core, running_notification("test-session"));
    request_id
}

fn reply(core: &mut SessionCore, request_id: RequestId, replies: impl IntoIterator<Item = Reply>) -> usize {
    for reply in replies {
        match reply {
            Reply::Idle(stop_reason) => receive(core, idle_notification("test-session", Some(stop_reason))),
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

fn prepared(blocks: Vec<ContentBlock>) -> CommandResult {
    CommandResult::SubmissionPrepared(AttachmentOutcome { blocks, warnings: vec![] })
}

fn image() -> ContentBlock {
    ContentBlock::Image(ImageContent::new("aGk=", "image/png"))
}

fn model(current: &str) -> SessionConfigOption {
    select("model", "Model", current, &[("fast", "Fast"), ("slow", "Slow")])
}

fn selections(core: &SessionCore) -> Vec<(&str, &str)> {
    core.session()
        .config_options()
        .iter()
        .filter_map(|option| Some((option.id.as_str(), option.current_value()?)))
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
    receive(core, event);
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
