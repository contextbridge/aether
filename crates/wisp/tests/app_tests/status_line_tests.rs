use agent_client_protocol::schema as acp;
use tui::KeyCode;

use super::common::*;

#[tokio::test]
async fn test_status_line_shows_workspace_on_left() -> TestResult {
    let renderer = RendererTest::new().size((TEST_WIDTH, 24)).build()?;

    let lines = renderer.writer().get_lines();
    let status_line = lines
        .iter()
        .find(|line| line.contains(TEST_WORKSPACE_DIR) && line.contains(TEST_GIT_REF) && line.contains(TEST_AGENT))
        .unwrap_or_else(|| panic!("Status line should show workspace and agent.\nBuffer:\n{}", lines.join("\n")));
    assert!(status_line.find(TEST_WORKSPACE_DIR).unwrap() < status_line.find(TEST_AGENT).unwrap());
    Ok(())
}

#[tokio::test]
async fn test_status_line_shows_agent_name() -> TestResult {
    let renderer = RendererTest::new().size((80, 24)).agent_name("claude-code").build()?;

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("claude-code")),
        "Status line should show agent name.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_status_line_shows_model_from_config_options() -> TestResult {
    let config_options = vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "openrouter:gpt-4o",
            vec![acp::SessionConfigSelectOption::new("openrouter:gpt-4o", "OpenRouter / GPT-4o")],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ];

    let renderer =
        RendererTest::new().size((80, 24)).agent_name("aether-acp").config_options(&config_options).build()?;

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("aether-acp") && l.contains("OpenRouter / GPT-4o")),
        "Status line should show agent name and model.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_status_line_updates_on_config_option_update() -> TestResult {
    let config_options = vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "openrouter:gpt-4o",
            vec![acp::SessionConfigSelectOption::new("openrouter:gpt-4o", "OpenRouter / GPT-4o")],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ];

    let mut renderer =
        RendererTest::new().size((80, 24)).agent_name("aether-acp").config_options(&config_options).build()?;

    // Send a ConfigOptionUpdate with a new model
    let new_config_options = vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "ollama:llama3",
            vec![acp::SessionConfigSelectOption::new("ollama:llama3", "Ollama / llama3")],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ];

    renderer
        .on_session_update(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(new_config_options)))?;

    let lines = renderer.writer().get_lines();
    assert!(
        lines.iter().any(|l| l.contains("Ollama / llama3")),
        "Status line should show updated model.\nBuffer:\n{}",
        lines.join("\n")
    );
    assert!(
        !lines.iter().any(|l| l.contains("GPT-4o")),
        "Status line should no longer show old model.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_available_commands_update_is_forwarded() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    renderer.on_session_update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
        vec![acp::AvailableCommand::new("search", "Search code")],
    )))?;

    // Open the command picker with /
    press(&mut renderer, KeyCode::Char('/')).await?;

    let names = command_picker_visible_names(renderer.writer());
    assert!(names.iter().any(|n| n == "search"), "Command picker should show 'search' command. Got: {names:?}");
    Ok(())
}
