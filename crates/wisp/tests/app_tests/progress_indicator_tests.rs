use tui::testing::{TestTerminal, assert_buffer_eq};

use super::common::*;

#[tokio::test]
async fn test_spinner_visible_after_prompt_submit() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "Hello").await;
    press_enter(&mut renderer).await;

    let lines = renderer.writer().get_lines();
    let has_interrupt = lines.iter().any(|l| l.contains("esc to interrupt"));
    assert!(has_interrupt, "Progress indicator should be visible after prompt submit.\nBuffer:\n{}", lines.join("\n"));
}

#[tokio::test]
async fn test_spinner_persists_on_session_update() {
    use agent_client_protocol::schema as acp;

    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "Hello").await;
    press_enter(&mut renderer).await;

    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("Hi"),
        ))))
        .unwrap();

    let lines = renderer.writer().get_lines();
    let has_interrupt = lines.iter().any(|l| l.contains("esc to interrupt"));
    assert!(
        has_interrupt,
        "Progress indicator should persist while waiting for response.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_spinner_disappears_on_prompt_done() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "Hello").await;
    press_enter(&mut renderer).await;

    renderer.on_prompt_done().unwrap();

    let lines = renderer.writer().get_lines();
    let has_braille = lines.iter().any(|l| "⠒⠮⠷⢷⡾⣯⣽⣿⣭⢯".chars().any(|c| l.contains(c)));
    assert!(!has_braille, "Spinner should disappear after prompt done.\nBuffer:\n{}", lines.join("\n"));
}

#[tokio::test]
async fn test_spinner_not_visible_on_initial_render() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));

    renderer.initial_render().unwrap();

    let expected = expected_prompt(80, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_on_tick_advances_animation() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));

    renderer.initial_render().unwrap();

    type_string(&mut renderer, "Hello").await;
    press_enter(&mut renderer).await;

    let lines_before: Vec<String> = renderer.writer().get_lines();

    renderer.on_tick().await.unwrap();

    let lines_after: Vec<String> = renderer.writer().get_lines();

    assert_ne!(lines_before, lines_after, "on_tick should advance the animation and produce a different frame");
}

#[tokio::test]
async fn test_on_tick_noop_when_not_waiting() {
    let terminal = TestTerminal::new(80, 24);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (80, 24));

    renderer.initial_render().unwrap();

    let lines_before: Vec<String> = renderer.writer().get_lines();

    renderer.on_tick().await.unwrap();

    let lines_after: Vec<String> = renderer.writer().get_lines();

    assert_eq!(lines_before, lines_after, "on_tick should be a no-op when not waiting for response");
}
