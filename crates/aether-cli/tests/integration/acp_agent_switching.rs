use acp_utils::config_option_id::ConfigOptionId;
use aether_cli::acp::testing::{AcpTestHarness, FakeAgentSwitchingSession};
use agent_client_protocol::schema::v2::{
    CloseSessionRequest, ContentBlock, PromptRequest, PromptResponse, SetSessionConfigOptionRequest, StopReason,
    TextContent,
};
use std::future::Future;

/// Text each fake agent streams when it runs. Mirrors the harness constants so
/// the *other* agent's view of the shared transcript can be asserted.
const PLANNER_REPLY: &str = "planner reply";
const CODER_REPLY: &str = "coder reply";

#[tokio::test(flavor = "current_thread")]
async fn mode_selection_while_idle_refreshes_mcp_surface_before_next_prompt() {
    AcpTestHarness::run(|mut harness| async move {
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
    AcpTestHarness::run(|mut harness| async move {
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
    AcpTestHarness::run(|mut harness| async move {
        let fake = harness.insert_agent_switching_session().await;
        harness.expect_mcp_server_status(&["planner-mcp"]).await;
        harness.expect_available_commands(&["plan"], &["edit"]).await;

        select_coder(&harness, &fake).await;
        let prompt = send_prompt(&harness, &fake, "implement it");
        tokio::pin!(prompt);

        harness.expect_mcp_server_status(&["coder-mcp"]).await;
        harness.expect_available_commands(&["edit"], &["plan"]).await;

        let response = prompt.await.expect("prompt succeeds");
        assert_eq!(serde_json::to_value(response).unwrap(), serde_json::json!({}));
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.coder().assert_saw_exactly(&["implement it"]);
        fake.planner().assert_never_ran();
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn switch_to_coder_receives_shared_prior_transcript() {
    AcpTestHarness::run(|mut harness| async move {
        let fake = harness.insert_agent_switching_session().await;
        harness.expect_mcp_server_status(&["planner-mcp"]).await;
        harness.expect_available_commands(&["plan"], &["edit"]).await;

        let first_prompt = send_prompt(&harness, &fake, "make a plan");
        first_prompt.await.expect("first prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.planner().assert_saw(&["make a plan"]);

        select_coder(&harness, &fake).await;
        let second_prompt = send_prompt(&harness, &fake, "write code");
        tokio::pin!(second_prompt);

        harness.expect_mcp_server_status(&["coder-mcp"]).await;
        harness.expect_available_commands(&["edit"], &["plan"]).await;
        second_prompt.await.expect("second prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.coder().assert_saw(&["make a plan", PLANNER_REPLY, "write code"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn switching_back_reuses_warm_runtime_and_syncs_latest_transcript() {
    AcpTestHarness::run(|mut harness| async move {
        let fake = harness.insert_agent_switching_session().await;
        harness.expect_mcp_server_status(&["planner-mcp"]).await;
        harness.expect_available_commands(&["plan"], &["edit"]).await;

        select_coder(&harness, &fake).await;
        let coder_prompt = send_prompt(&harness, &fake, "write code");
        tokio::pin!(coder_prompt);

        harness.expect_mcp_server_status(&["coder-mcp"]).await;
        harness.expect_available_commands(&["edit"], &["plan"]).await;
        coder_prompt.await.expect("coder prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.coder().assert_saw_exactly(&["write code"]);

        assert_eq!(harness.live_runtime_count(), 2);
        select_planner(&harness, &fake).await;
        assert_eq!(harness.live_runtime_count(), 2, "switching back keeps the warm runtime");
        let planner_prompt = send_prompt(&harness, &fake, "review code");
        tokio::pin!(planner_prompt);

        harness.expect_mcp_server_status(&["planner-mcp"]).await;
        harness.expect_available_commands(&["plan"], &["edit"]).await;
        planner_prompt.await.expect("planner prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.planner().assert_saw(&["write code", CODER_REPLY, "review code"]);
        harness.shutdown().await;
        assert_eq!(harness.live_runtime_count(), 0, "shutdown joins all cached runtimes");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn mode_change_applies_at_next_prompt_boundary() {
    AcpTestHarness::run(|mut harness| async move {
        let fake = harness.insert_agent_switching_session().await;
        harness.expect_mcp_server_status(&["planner-mcp"]).await;
        harness.expect_available_commands(&["plan"], &["edit"]).await;

        let in_flight = send_prompt(&harness, &fake, "stay planner for this turn");
        in_flight.await.expect("prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.planner().assert_saw(&["stay planner for this turn"]);

        select_coder(&harness, &fake).await;
        let next = send_prompt(&harness, &fake, "now coder");
        tokio::pin!(next);

        harness.expect_mcp_server_status(&["coder-mcp"]).await;
        harness.expect_available_commands(&["edit"], &["plan"]).await;
        next.await.expect("next prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.coder().assert_saw(&["stay planner for this turn", PLANNER_REPLY, "now coder"]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn loaded_session_restores_last_active_agent_from_control_events() {
    AcpTestHarness::run(|mut harness| async move {
        harness.append_stored_session("loaded", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("loaded", "previous request");
        harness.append_agent_switch("loaded", Some("Planner"), Some("Coder"));
        let fake = harness.insert_loaded_agent_switching_session("loaded").await;

        let prompt = send_prompt(&harness, &fake, "continue");
        prompt.await.expect("prompt accepted");
        harness.expect_idle(fake.session_id(), StopReason::EndTurn).await;
        fake.coder().assert_saw(&["previous request", "continue"]);
        fake.planner().assert_never_ran();
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn new_session_reports_initial_mcp_server_status() {
    AcpTestHarness::run(|mut harness| async move {
        let _fake = harness.insert_agent_switching_session().await;

        harness.expect_mcp_server_status(&["planner-mcp"]).await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn loaded_session_reports_initial_mcp_server_status_for_restored_agent() {
    AcpTestHarness::run(|mut harness| async move {
        harness.append_stored_session("loaded", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("loaded", "previous request");
        harness.append_agent_switch("loaded", Some("Planner"), Some("Coder"));
        let _fake = harness.insert_loaded_agent_switching_session("loaded").await;

        harness.expect_mcp_server_status(&["coder-mcp"]).await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn close_interrupts_runtime_startup_during_mode_switch() {
    AcpTestHarness::run(|harness| async move {
        let fake = harness.insert_agent_switching_session().await;
        let mut pending = harness.pause_next_runtime();
        let switching = harness
            .client_cx
            .send_request(SetSessionConfigOptionRequest::new(
                fake.session_id().clone(),
                ConfigOptionId::Mode.as_str(),
                "Coder",
            ))
            .block_task();
        pending.wait_until_started().await;
        harness
            .client_cx
            .send_request(CloseSessionRequest::new(fake.session_id().clone()))
            .block_task()
            .await
            .expect("close interrupts startup");
        assert!(switching.await.is_err());
        assert_eq!(harness.live_runtime_count(), 0);
    })
    .await;
}

fn send_prompt(
    harness: &AcpTestHarness,
    fake: &FakeAgentSwitchingSession,
    text: &str,
) -> impl Future<Output = Result<PromptResponse, agent_client_protocol::Error>> + use<> {
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
            mode,
        ))
        .block_task()
        .await
        .expect("mode selection succeeds");
}
