use tui::testing::assert_buffer_eq;
use tui::{BRAILLE_FRAMES, ViewContext};

use super::common::*;

fn spinner_on_row(lines: &[String], row: usize) -> char {
    lines[row].chars().find(|character| BRAILLE_FRAMES.contains(character)).expect("spinner")
}

#[tokio::test]
async fn test_compaction_overrides_progress_indicator_and_restores_it_afterward() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;
    renderer.on_context_compaction(true)?;

    let lines = renderer.writer().get_lines();
    let row = lines.iter().position(|line| line.contains("Compacting context...")).expect("compaction message");
    assert!(lines[row].contains("esc to interrupt"));
    let spinner = spinner_on_row(&lines, row);
    let spinner_style = renderer.writer().style_of_text(row, &spinner.to_string()).expect("spinner style");
    assert_eq!(spinner_style.fg, Some(ViewContext::new((TEST_WIDTH, 40)).theme.warning()));

    renderer.on_context_compaction(false)?;
    let lines = renderer.writer().get_lines();
    assert!(!lines.iter().any(|line| line.contains("Compacting context...")));
    let row = lines
        .iter()
        .position(|line| line.contains("esc to interrupt") && !line.contains("Compacting context..."))
        .expect("normal progress message");
    let spinner = spinner_on_row(&lines, row);
    let spinner_style = renderer.writer().style_of_text(row, &spinner.to_string()).expect("spinner style");
    assert_eq!(spinner_style.fg, Some(ViewContext::new((TEST_WIDTH, 40)).theme.info()));

    Ok(())
}

#[tokio::test]
async fn test_prompt_done_clears_compaction_indicator() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;
    renderer.on_context_compaction(true)?;
    renderer.on_prompt_done()?;

    assert!(!renderer.writer().get_lines().iter().any(|line| line.contains("Compacting context...")));
    Ok(())
}

#[tokio::test]
async fn test_context_clear_resets_compaction_indicator() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;
    renderer.on_context_compaction(true)?;
    renderer.on_context_cleared()?;

    assert!(!renderer.writer().get_lines().iter().any(|line| line.contains("Compacting context...")));
    Ok(())
}

#[tokio::test]
async fn test_spinner_visible_after_prompt_submit() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;

    let lines = renderer.writer().get_lines();
    let has_interrupt = lines.iter().any(|l| l.contains("esc to interrupt"));
    assert!(has_interrupt, "Progress indicator should be visible after prompt submit.\nBuffer:\n{}", lines.join("\n"));
    Ok(())
}

#[tokio::test]
async fn test_spinner_persists_on_session_update() -> TestResult {
    use agent_client_protocol::schema as acp;

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;

    renderer.on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("Hi")),
    )))?;

    let lines = renderer.writer().get_lines();
    let has_interrupt = lines.iter().any(|l| l.contains("esc to interrupt"));
    assert!(
        has_interrupt,
        "Progress indicator should persist while waiting for response.\nBuffer:\n{}",
        lines.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn test_spinner_disappears_on_prompt_done() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;

    renderer.on_prompt_done()?;

    let lines = renderer.writer().get_lines();
    let has_braille = lines.iter().any(|l| "⠒⠮⠷⢷⡾⣯⣽⣿⣭⢯".chars().any(|c| l.contains(c)));
    assert!(!has_braille, "Spinner should disappear after prompt done.\nBuffer:\n{}", lines.join("\n"));
    Ok(())
}

#[tokio::test]
async fn test_spinner_not_visible_on_initial_render() -> TestResult {
    let renderer = RendererTest::new().size((80, 24)).build()?;

    let expected = expected_prompt(80, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
    Ok(())
}

#[tokio::test]
async fn test_on_tick_advances_animation() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "Hello").await?;
    press(&mut renderer, Enter).await?;

    let lines_before: Vec<String> = renderer.writer().get_lines();

    renderer.on_tick().await?;

    let lines_after: Vec<String> = renderer.writer().get_lines();

    assert_ne!(lines_before, lines_after, "on_tick should advance the animation and produce a different frame");
    Ok(())
}

#[tokio::test]
async fn test_on_tick_noop_when_not_waiting() -> TestResult {
    let mut renderer = RendererTest::new().size((80, 24)).build()?;

    let lines_before: Vec<String> = renderer.writer().get_lines();

    renderer.on_tick().await?;

    let lines_after: Vec<String> = renderer.writer().get_lines();

    assert_eq!(lines_before, lines_after, "on_tick should be a no-op when not waiting for response");
    Ok(())
}
