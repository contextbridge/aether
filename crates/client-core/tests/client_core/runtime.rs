use crate::support::{TestError, next_document, next_review_state, run_local};
use acp_utils::client::{AcpClientError, AcpEvent, connect_acp_client};
use acp_utils::testing::{Channel, FakeAgent, initialize_request};
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_remote::Invocation;
use client_core::command::{AgentCommand, Command, CommandResult, GitReviewCommand, ReviewSource};
use client_core::runtime::{Input, SessionRuntime};
use client_core::session_model::initial_workspace_status;
use client_core::testing::{FakeGit, git_diff_agent};
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;
use utils::testing::TestWorkspace;

#[test]
fn a_closed_agent_connection_fails_commands_and_is_reported_once() -> Result<(), TestError> {
    run_local(async {
        let mut runtime = disconnected_runtime().await?;

        runtime.dispatch(Command::Agent(AgentCommand::Cancel { session_id: SessionId::new("session") }));

        assert!(matches!(runtime.next().await, Input::Result(CommandResult::Notified(Err(_)))));
        assert!(matches!(runtime.next().await, Input::Event(AcpEvent::ConnectionClosed)));
        runtime.shutdown().await;
        Ok(())
    })
}

#[test]
fn workspace_status_falls_back_to_the_session_path_without_an_agent() -> Result<(), TestError> {
    run_local(async {
        let mut runtime = disconnected_runtime().await?;
        let first = TempDir::new()?;
        let second = TempDir::new()?;

        for cwd in [first.path(), second.path()] {
            runtime.dispatch(Command::Agent(AgentCommand::FetchWorkspaceStatus {
                session_id: SessionId::new("s"),
                cwd: cwd.to_path_buf(),
            }));
        }

        let mut resolved = Vec::new();
        while resolved.len() < 2 {
            if let Input::Result(CommandResult::WorkspaceResolved { cwd, status }) = runtime.next().await {
                assert_eq!(status, initial_workspace_status(&cwd));
                resolved.push(cwd);
            }
        }
        assert!(resolved.contains(&second.path().to_path_buf()));
        Ok(())
    })
}

#[test]
fn a_git_review_reads_the_diff_the_agent_serves() -> Result<(), TestError> {
    run_local(async {
        let git = FakeGit::new("/repo").with_untracked("changed.txt", "edited\n");
        let (agent_end, client_end) = Channel::duplex();
        tokio::task::spawn_local(git_diff_agent(FakeAgent::default(), git).connect_to(agent_end));
        let client = connect_acp_client(client_end, initialize_request()).await?;
        let mut runtime = SessionRuntime::new(client.handle, client.event_rx);

        runtime.dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Agent {
            session_id: SessionId::new("s1"),
        })));

        let document = next_document(&mut runtime).await;
        assert_eq!(document.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["changed.txt"]);
        runtime.shutdown().await;
        Ok(())
    })
}

#[test]
fn a_git_review_of_a_local_working_tree_reads_it_through_a_diff_server_of_its_own() -> Result<(), TestError> {
    run_local(async {
        let repository = TestWorkspace::new().git_init().file("changed.txt", "edited\n");
        let mut runtime = disconnected_runtime().await?;

        runtime.dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Local(repository.root().into()))));

        let document = next_document(&mut runtime).await;
        assert_eq!(document.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["changed.txt"]);
        runtime.shutdown().await;
        Ok(())
    })
}

#[test]
fn a_git_review_over_stdio_reads_the_diff_its_command_serves() -> Result<(), TestError> {
    run_local(async {
        let repository = TestWorkspace::new().git_init().file("changed.txt", "edited\n");
        let mut runtime = disconnected_runtime().await?;

        let args = vec![repository.root().to_string_lossy().into_owned()];
        runtime.dispatch(open_over_stdio(env!("CARGO_BIN_EXE_stdio-diff-server"), args, repository.root()));

        let document = next_document(&mut runtime).await;
        assert_eq!(document.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(), ["changed.txt"]);
        runtime.shutdown().await;
        Ok(())
    })
}

#[test]
fn a_git_review_over_stdio_whose_command_cannot_start_or_exits_fails_naming_it() -> Result<(), TestError> {
    run_local(async {
        let dir = TempDir::new()?;
        for (program, named) in
            [("aether-test-no-such-diff-server", "aether-test-no-such-diff-server"), ("true", "`true`")]
        {
            let mut runtime = disconnected_runtime().await?;

            runtime.dispatch(open_over_stdio(program, Vec::new(), dir.path()));

            let state = next_review_state(&mut runtime).await;
            assert_eq!(state.label(), "failed");
            assert!(state.status().is_some_and(|status| status.contains(named)), "{:?}", state.status());
        }
        Ok(())
    })
}

#[test]
fn a_git_review_outside_a_repository_fails_with_the_reason() -> Result<(), TestError> {
    run_local(async {
        let outside = TempDir::new()?;
        let mut runtime = disconnected_runtime().await?;

        runtime.dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Local(outside.path().into()))));

        assert_eq!(next_review_state(&mut runtime).await.label(), "failed");
        Ok(())
    })
}

#[test]
fn an_unavailable_git_review_fails_with_the_reason() -> Result<(), TestError> {
    run_local(async {
        let mut runtime = disconnected_runtime().await?;

        runtime
            .dispatch(Command::GitReview(GitReviewCommand::Open(ReviewSource::Unavailable("no diff here".to_owned()))));

        let state = next_review_state(&mut runtime).await;
        assert_eq!(state.label(), "failed");
        assert!(state.status().is_some_and(|status| status.contains("no diff here")));
        Ok(())
    })
}

async fn disconnected_runtime() -> Result<SessionRuntime, AcpClientError> {
    let client = FakeAgent::default().build().await?;
    client.handle.disconnect().await;
    Ok(SessionRuntime::new(client.handle, client.event_rx))
}

fn open_over_stdio(program: &str, args: Vec<String>, dir: &Path) -> Command {
    let invocation = Invocation { program: program.to_owned(), args, dir: dir.into(), env: BTreeMap::new() };
    Command::GitReview(GitReviewCommand::Open(ReviewSource::Stdio(invocation)))
}
