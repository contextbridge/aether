use aether_cli::acp::testing::{AcpTestHarness, FakeAgentSwitchingSession};
use agent_client_protocol::schema::v2::{AvailableCommand, AvailableCommandInput, PromptRequest, StopReason};
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::{Prompt, PromptArgument};
use mcp_utils::testing::FakeMcpServer;

#[tokio::test(flavor = "current_thread")]
async fn slash_command_arguments_fill_the_prompt_template() {
    AcpTestHarness::run(|mut harness| async move {
        harness.add_mcp_server(ServerSpec::in_process(
            "docs",
            FakeMcpServer::new().with_prompt("greet", "{ARGUMENTS}|{1}|{3}"),
        ));
        let session = harness.insert_agent_switching_session().await;

        send(&mut harness, &session, "/greet do a  thing").await;

        session.planner().assert_saw(&["do a  thing|do|thing"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_slash_commands_reach_the_agent_unchanged() {
    AcpTestHarness::run(|mut harness| async move {
        let session = harness.insert_agent_switching_session().await;

        send(&mut harness, &session, "/missing").await;

        session.planner().assert_saw(&["/missing"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn commands_keep_the_names_their_servers_gave_them() {
    AcpTestHarness::run(|mut harness| async move {
        harness.add_mcp_server(ServerSpec::in_process(
            "docs",
            FakeMcpServer::new().with_prompt("release__notes", "notes for {1}"),
        ));
        harness.add_mcp_server(ServerSpec::in_process(
            "deep__docs",
            FakeMcpServer::new().with_prompt("outline", "outline for {1}"),
        ));
        let session = harness.insert_agent_switching_session().await;

        harness.expect_available_commands(&["release__notes", "outline"], &["notes", "deep__docs__outline"]).await;
        send(&mut harness, &session, "/release__notes v2").await;
        send(&mut harness, &session, "/outline v3").await;

        session.planner().assert_saw(&["notes for v2", "outline for v3"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_first_server_offering_a_prompt_name_owns_the_command() {
    AcpTestHarness::run(|mut harness| async move {
        harness
            .add_mcp_server(ServerSpec::in_process("second", FakeMcpServer::new().with_prompt("plan", "second plan")));
        let session = harness.insert_agent_switching_session().await;

        let commands = harness.next_available_commands().await;
        send(&mut harness, &session, "/plan").await;

        assert_eq!(commands.iter().filter(|command| command.name == "plan").count(), 1);
        session.planner().assert_saw(&["expanded plan"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn commands_carry_the_prompt_description_and_argument_hint() {
    AcpTestHarness::run(|mut harness| async move {
        let argument = PromptArgument::new("ARGUMENTS").with_description("[query]").with_required(false);
        let search = Prompt::new("search", Some("Search code"), Some(vec![argument]));
        harness.add_mcp_server(ServerSpec::in_process(
            "code",
            FakeMcpServer::new().with_prompt_definition(search, "search"),
        ));
        harness.insert_agent_switching_session().await;

        let commands = harness.next_available_commands().await;

        assert_eq!(command(&commands, "search").description, "Search code");
        assert_eq!(hint(&commands, "search"), "[query]");
        assert_eq!(hint(&commands, "plan"), "optional arguments");
    })
    .await;
}

async fn send(harness: &mut AcpTestHarness, session: &FakeAgentSwitchingSession, text: &str) {
    let id = session.session_id().clone();
    harness.client_cx.send_request(PromptRequest::new(id.clone(), vec![text.into()])).block_task().await.unwrap();
    harness.expect_idle(&id, StopReason::EndTurn).await;
}

fn command<'a>(commands: &'a [AvailableCommand], name: &str) -> &'a AvailableCommand {
    commands.iter().find(|command| command.name == name).unwrap_or_else(|| panic!("expected /{name} in {commands:?}"))
}

fn hint(commands: &[AvailableCommand], name: &str) -> String {
    match &command(commands, name).input {
        Some(AvailableCommandInput::Text(input)) => input.hint.clone(),
        other => panic!("expected /{name} to take text input, got {other:?}"),
    }
}
