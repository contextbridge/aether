use agent_client_protocol::schema as acp;
use tui::testing::{TestTerminal, assert_buffer_eq};

use super::common::*;

#[tokio::test]
async fn test_agent_message_text_chunks() {
    let renderer = render(vec![text_chunk("Hello"), text_chunk(" World"), prompt_done()]);

    let expected = expected_with_prompt(&[&p("Hello World")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_agent_thought_chunks() {
    let renderer = render(vec![thought_chunk("Plan"), thought_chunk(" this"), prompt_done()]);

    let expected = expected_with_prompt(&[&p("Plan this")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_agent_message_chunks_stream_before_prompt_done() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("Hello"),
        ))))
        .unwrap();
    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(" World"),
        ))))
        .unwrap();

    let expected = expected_with_prompt(&[&p("Hello World")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_thought_and_text_chunks_stream_before_prompt_done() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("Thinking"),
        ))))
        .unwrap();
    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("Done"),
        ))))
        .unwrap();

    let expected = expected_with_prompt(&[&p("Thinking"), "", &p("Done")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_text_and_thought_chunks_stream_in_arrival_order() {
    let terminal = TestTerminal::new(TEST_WIDTH, 40);
    let mut renderer = Renderer::new(terminal, TEST_AGENT.to_string(), &[], (TEST_WIDTH, 40));
    renderer.initial_render().unwrap();

    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("A"),
        ))))
        .unwrap();
    renderer
        .on_session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("B"),
        ))))
        .unwrap();
    renderer
        .on_session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("C"),
        ))))
        .unwrap();

    let expected = expected_with_prompt(&[&p("A"), "", &p("B"), "", &p("C")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_thought_prefix_resets_after_non_thought_boundary() {
    let renderer = render(vec![thought_chunk("Plan"), text_chunk("Answer"), thought_chunk("Refine"), prompt_done()]);

    let expected = expected_with_prompt(&[&p("Plan"), "", &p("Answer"), "", &p("Refine")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_multiline_thought_prefixes_only_first_line() {
    let renderer = render(vec![thought_chunk("line one\nline two"), prompt_done()]);

    let expected = expected_with_prompt(&[&p("line one"), &p("line two")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}
