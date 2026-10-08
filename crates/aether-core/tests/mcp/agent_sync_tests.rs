use aether_core::{
    core::{AgentDeps, agent},
    events::{AgentEvent, ToolEvent},
    mcp::mcp,
    testing::{TestScenario, test_agent},
};
use llm::testing::{FakeLlmProvider, llm_response};
use llm::{ChatMessage, Context, LlmResponse};
use mcp_utils::model::{ElicitationCapability, UrlElicitationCapability};
use mcp_utils::testing::{FakeMcpServer, FakeTool, fake_mcp};
use std::error::Error;
use tokio::sync::mpsc;

#[tokio::test]
async fn an_agent_following_a_gateway_still_stops_once_its_input_closes() {
    let runtime = mcp("/workspace").with_servers(vec![fake_mcp("math", FakeMcpServer::new())]).spawn().unwrap();
    let (agent_tx, _agent_rx, handle) =
        agent(FakeLlmProvider::new(Vec::new())).mcp(runtime.gateway().clone()).spawn().await.unwrap();

    drop(agent_tx);

    handle.await_completion().await;
}

#[tokio::test]
async fn spawned_mcp_client_advertises_the_configured_elicitation_support() {
    let server = FakeMcpServer::new();
    let state = server.state();
    let (sink, _host) = mpsc::channel(1);
    let runtime = mcp("/workspace")
        .with_servers(vec![fake_mcp("capability-capture", server)])
        .with_agent_deps(
            AgentDeps::default()
                .with_mcp_elicitation(Some(ElicitationCapability::new().with_url(UrlElicitationCapability::new()))),
        )
        .with_elicitations(sink)
        .spawn()
        .unwrap();

    runtime.gateway().ready().await;

    let capabilities = state.client_capabilities().expect("client capabilities were discovered");
    let elicitation = capabilities.elicitation.expect("elicitation is advertised");
    assert!(elicitation.form.is_none());
    assert!(elicitation.url.is_some());
}

#[tokio::test]
async fn the_agent_follows_the_gateway_catalog_as_it_changes() -> Result<(), Box<dyn Error>> {
    let server = FakeMcpServer::new();
    let (adding, clearing) = (server.state(), server.state());
    let result = test_agent()
        .fake_mcp_server("dynamic", server)
        .llm_responses(&[response(), response(), response()])
        .scenario(
            TestScenario::new()
                .user_text("initially")
                .wait_for_turn_end()
                .perform(move || {
                    tokio::spawn(async move { adding.add_tool_and_notify(FakeTool::new("added_later")).await });
                })
                .wait_for(|event| defines(event, |tools| tools.contains(&"dynamic__added_later")))
                .user_text("after adding a tool")
                .wait_for_turn_end()
                .perform(move || {
                    tokio::spawn(async move { clearing.clear_tools_and_notify().await });
                })
                .wait_for(|event| defines(event, |tools| tools.is_empty()))
                .user_text("after clearing the tools")
                .wait_for_turn_end(),
        )
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    assert!(tool_names(&contexts[0]).contains(&"dynamic__add_numbers".to_string()));
    assert!(system_prompt(&contexts[0]).contains("A fake MCP server for testing"));
    assert!(tool_names(&contexts[1]).contains(&"dynamic__added_later".to_string()));
    assert!(tool_names(&contexts[2]).is_empty());
    assert!(!system_prompt(&contexts[2]).contains("A fake MCP server for testing"));
    Ok(())
}

fn defines(event: &AgentEvent, check: impl Fn(&[&str]) -> bool) -> bool {
    let AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools }) = event else { return false };
    check(&tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>())
}

fn tool_names(context: &Context) -> Vec<String> {
    context.tools().iter().map(|tool| tool.name.clone()).collect()
}

fn system_prompt(context: &Context) -> String {
    match context.messages().first() {
        Some(ChatMessage::System { content, .. }) => content.clone(),
        _ => String::new(),
    }
}

fn response() -> Vec<LlmResponse> {
    llm_response().text(&["done"]).build()
}
