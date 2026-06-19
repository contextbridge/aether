use acp_utils::config_option_id::ConfigOptionId;
use aether_cli::acp::testing::{AcpTestHarness, FakeAgentSwitchingSession};
use agent_client_protocol::schema::{
    ContentBlock, PromptRequest, SetSessionConfigOptionRequest, StopReason, TextContent,
};
use std::future::Future;
use tokio::task::LocalSet;

/// Text each fake agent streams when it runs. Mirrors the harness constants so
/// the *other* agent's view of the shared transcript can be asserted.
const PLANNER_REPLY: &str = "planner reply";
const CODER_REPLY: &str = "coder reply";

#[tokio::test(flavor = "current_thread")]
async fn mode_selection_while_idle_refreshes_mcp_surface_before_next_prompt() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let fake = harness.insert_agent_switching_session().await;
            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;

            select_coder(&harness, &fake).await;

            harness.expect_mcp_server_status(&["coder-mcp"]).await;
            harness.expect_available_commands(&["edit"], &["plan"]).await;
            fake.planner().assert_never_ran();
            fake.coder().assert_never_ran();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn switching_to_agent_without_mcp_sends_empty_status() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let fake = harness.insert_agent_switching_session_with_serverless_coder().await;
            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;

            select_coder(&harness, &fake).await;

            harness.expect_mcp_server_status_exact(&[]).await;
            harness.expect_available_commands(&[], &["plan", "edit"]).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn mode_switch_routes_next_prompt_to_target_agent_and_refreshes_ui_state() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let fake = harness.insert_agent_switching_session().await;
            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;

            select_coder(&harness, &fake).await;
            let prompt = send_prompt(&harness, &fake, "implement it");
            tokio::pin!(prompt);

            harness.expect_mcp_server_status(&["coder-mcp"]).await;
            harness.expect_available_commands(&["edit"], &["plan"]).await;

            let response = prompt.await.expect("prompt succeeds");
            assert_eq!(response.stop_reason, StopReason::EndTurn);
            fake.coder().assert_saw_exactly(&["implement it"]);
            fake.planner().assert_never_ran();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn switch_to_coder_receives_shared_prior_transcript() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let fake = harness.insert_agent_switching_session().await;
            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;

            let first_prompt = send_prompt(&harness, &fake, "make a plan");
            assert_eq!(first_prompt.await.expect("first prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.planner().assert_saw(&["make a plan"]);

            select_coder(&harness, &fake).await;
            let second_prompt = send_prompt(&harness, &fake, "write code");
            tokio::pin!(second_prompt);

            harness.expect_mcp_server_status(&["coder-mcp"]).await;
            harness.expect_available_commands(&["edit"], &["plan"]).await;
            assert_eq!(second_prompt.await.expect("second prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.coder().assert_saw(&["make a plan", PLANNER_REPLY, "write code"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn switching_back_reuses_warm_runtime_and_syncs_latest_transcript() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let fake = harness.insert_agent_switching_session().await;
            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;

            select_coder(&harness, &fake).await;
            let coder_prompt = send_prompt(&harness, &fake, "write code");
            tokio::pin!(coder_prompt);

            harness.expect_mcp_server_status(&["coder-mcp"]).await;
            harness.expect_available_commands(&["edit"], &["plan"]).await;
            assert_eq!(coder_prompt.await.expect("coder prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.coder().assert_saw_exactly(&["write code"]);

            select_planner(&harness, &fake).await;
            let planner_prompt = send_prompt(&harness, &fake, "review code");
            tokio::pin!(planner_prompt);

            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;
            assert_eq!(planner_prompt.await.expect("planner prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.planner().assert_saw(&["write code", CODER_REPLY, "review code"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn mode_change_applies_at_next_prompt_boundary() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let fake = harness.insert_agent_switching_session().await;
            harness.expect_mcp_server_status(&["planner-mcp"]).await;
            harness.expect_available_commands(&["plan"], &["edit"]).await;

            let in_flight = send_prompt(&harness, &fake, "stay planner for this turn");
            assert_eq!(in_flight.await.expect("in-flight prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.planner().assert_saw(&["stay planner for this turn"]);

            select_coder(&harness, &fake).await;
            let next = send_prompt(&harness, &fake, "now coder");
            tokio::pin!(next);

            harness.expect_mcp_server_status(&["coder-mcp"]).await;
            harness.expect_available_commands(&["edit"], &["plan"]).await;
            assert_eq!(next.await.expect("next prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.coder().assert_saw(&["stay planner for this turn", PLANNER_REPLY, "now coder"]);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn loaded_session_restores_last_active_agent_from_control_events() {
    LocalSet::new()
        .run_until(async {
            let harness = AcpTestHarness::start().await;
            harness.append_stored_session("loaded", "2026-05-01T00:00:00Z");
            harness.append_stored_prompt("loaded", "previous request");
            harness.append_agent_switch("loaded", Some("Planner"), Some("Coder"));
            let fake = harness.insert_loaded_agent_switching_session("loaded").await;

            let prompt = send_prompt(&harness, &fake, "continue");
            assert_eq!(prompt.await.expect("prompt succeeds").stop_reason, StopReason::EndTurn);
            fake.coder().assert_saw(&["previous request", "continue"]);
            fake.planner().assert_never_ran();
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn new_session_reports_initial_mcp_server_status() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let _fake = harness.insert_agent_switching_session().await;

            harness.expect_mcp_server_status(&["planner-mcp"]).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn loaded_session_reports_initial_mcp_server_status_for_restored_agent() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            harness.append_stored_session("loaded", "2026-05-01T00:00:00Z");
            harness.append_stored_prompt("loaded", "previous request");
            harness.append_agent_switch("loaded", Some("Planner"), Some("Coder"));
            let _fake = harness.insert_loaded_agent_switching_session("loaded").await;

            harness.expect_mcp_server_status(&["coder-mcp"]).await;
        })
        .await;
}

fn send_prompt(
    harness: &AcpTestHarness,
    fake: &FakeAgentSwitchingSession,
    text: &str,
) -> impl Future<Output = Result<agent_client_protocol::schema::PromptResponse, agent_client_protocol::Error>> + use<> {
    let response = harness.client_cx.send_request(PromptRequest::new(
        fake.session_id().clone(),
        vec![ContentBlock::Text(TextContent::new(text.to_string()))],
    ));
    async move { response.block_task().await }
}

async fn select_coder(harness: &AcpTestHarness, fake: &FakeAgentSwitchingSession) {
    select_mode(harness, fake, "Coder").await;
}

async fn select_planner(harness: &AcpTestHarness, fake: &FakeAgentSwitchingSession) {
    select_mode(harness, fake, "Planner").await;
}

async fn select_mode(harness: &AcpTestHarness, fake: &FakeAgentSwitchingSession, mode: &str) {
    harness
        .client_cx
        .send_request(SetSessionConfigOptionRequest::new(
            fake.session_id().clone(),
            ConfigOptionId::Mode.as_str(),
            mode.to_string(),
        ))
        .block_task()
        .await
        .expect("mode selection succeeds");
}
