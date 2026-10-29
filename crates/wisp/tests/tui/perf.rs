//! Rendering performance contracts.
//!
//! Each test bounds the *work* a frame does — items rebuilt, bytes re-parsed,
//! cells repainted — rather than timing the clock, so the gate is deterministic
//! on any machine. The `#[ignore]`d benchmark at the bottom prints the
//! per-phase breakdown for profiling instead.

use super::support::*;
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2::{SessionUpdate, ToolCallUpdate};
use serde_json::json;

/// The standard perf terminal: a typical modern window.
const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;

/// Turns in a seeded long-history scenario (~1k conversation items).
const LONG_HISTORY: usize = 200;

type PerfUi = TestUi<InstrumentedBackend>;

fn perf_ui() -> PerfUi {
    TestUi::with_backend(InstrumentedBackend::new(WIDTH, HEIGHT))
}

#[test]
fn settled_session_renders_nothing_per_frame() {
    let mut ui = perf_ui();
    seed_long_history(&mut ui, LONG_HISTORY);
    ui.settle();
    assert!(!ui.app().wants_tick(), "a settled session must not keep the tick loop alive");

    ui.draw();
    ui.render_stats();
    ui.backend_mut().take_stats();

    ui.draw();
    let stats = ui.render_stats();
    let backend = ui.backend_mut().take_stats();

    assert_eq!(stats.frames, 1);
    assert_eq!(stats.item_rebuilds, 0, "a frame with no state change must rebuild nothing");
    assert_eq!(stats.markdown_bytes_parsed, 0);
    assert_eq!(stats.highlight.calls, 0);
    assert_eq!(stats.history_rows_inserted, 0);
    assert_eq!(backend.draws, 1);
    assert_eq!(backend.cells_drawn, 0, "a frame with no state change must repaint no cells");
}

#[test]
fn live_region_stays_bounded_while_streaming() {
    let mut ui = perf_ui();
    seed_long_history(&mut ui, LONG_HISTORY);
    ui.settle();
    ui.render_stats();

    stream_message(&mut ui, StreamContent::CodeBlock, 8 * 1024, 512);

    let stats = ui.render_stats();
    assert!(
        stats.max_live_rows <= usize::from(HEIGHT) * 2,
        "scrollback commits must keep the live region near the viewport height, saw {} rows",
        stats.max_live_rows
    );
}

/// Streaming `total` bytes may re-process at most ~4× that much: per-chunk work
/// must not scale with the message so far, with fixed slack for boundary churn.
fn reprocess_budget(total: usize) -> u64 {
    4 * total as u64 + 64 * 1024
}

#[test]
fn streaming_prose_rerenders_bounded_work() {
    let mut ui = perf_ui();
    seed_long_history(&mut ui, LONG_HISTORY);
    ui.settle();
    ui.render_stats();

    let total = 16 * 1024;
    stream_message(&mut ui, StreamContent::Prose, total, 256);

    let stats = ui.render_stats();
    assert!(
        stats.markdown_bytes_parsed <= reprocess_budget(total),
        "streaming {} bytes re-parsed {} bytes: per-chunk work must not scale with the message so far",
        total,
        stats.markdown_bytes_parsed
    );
}

#[test]
fn streaming_code_block_rerenders_bounded_work() {
    let mut ui = perf_ui();
    seed_long_history(&mut ui, LONG_HISTORY);
    ui.settle();
    ui.render_stats();

    let total = 24 * 1024;
    stream_message(&mut ui, StreamContent::CodeBlock, total, 256);

    let stats = ui.render_stats();
    let budget = reprocess_budget(total);
    assert!(
        stats.markdown_bytes_parsed <= budget,
        "streaming {} bytes re-parsed {} bytes: per-chunk work must not scale with the message so far",
        total,
        stats.markdown_bytes_parsed
    );
    assert!(
        stats.highlight.bytes_highlighted <= budget,
        "streaming {} bytes re-highlighted {} bytes: a growing code block must not re-highlight from the top each chunk",
        total,
        stats.highlight.bytes_highlighted
    );
}

#[test]
fn running_tool_ignores_unrelated_input() {
    let mut ui = perf_ui();
    seed_long_history(&mut ui, 20);
    ui.settle();
    ui.acp_event(tool_call("perf-tool", "Editing src/lib.rs"));
    ui.draw();
    ui.render_stats();

    for _ in 0..20 {
        ui.key(key(KeyCode::Char('a')));
        ui.draw();
    }

    let stats = ui.render_stats();
    assert_eq!(stats.item_rebuilds, 0, "input that moves neither the spinner nor the tool must not rebuild it");
    assert_eq!(stats.markdown_bytes_parsed, 0);
}

#[test]
fn thought_streaming_performs_no_conversation_rendering_work() {
    let mut ui = perf_ui();
    seed_long_history(&mut ui, LONG_HISTORY);
    ui.settle();
    ui.render_stats();

    let total = 16 * 1024;
    stream_message(&mut ui, StreamContent::Thought, total, 256);

    let stats = ui.render_stats();
    assert_eq!(
        stats.item_rebuilds, 0,
        "thought chunks must not rebuild transcript items: they drive the progress band, not the transcript"
    );
    assert_eq!(stats.markdown_bytes_parsed, 0, "thought chunks must not be parsed as markdown");
}

/// A message chosen to stress every streaming boundary: prose paragraphs, a
/// fenced Rust block with blank lines and a four-space-indented backtick run
/// inside it (block content to `CommonMark`, so it must not close the block), a
/// fenced Python block whose triple-quoted string spans many lines (highlight
/// state must carry across chunks), a tilde fence containing backtick fences, a
/// setext heading, and a trailing unclosed block.
fn exactness_message() -> String {
    let mut message = String::from("Streaming contract\n===\n\n");
    message.push_str("The paragraph above is a setext heading, so no line of it may be finalized early.\n\n");
    message.push_str("```rust\nfn one() {\n    let s = \"starts here\n");
    for index in 0..40 {
        let _ = writeln!(message, "and continues {index}");
    }
    message.push_str("    ```\n");
    message.push_str("ends here\";\n\nlet two = one();\n}\n```\n\n");
    message.push_str("Between blocks.\n\n```python\ndef f():\n    doc = \"\"\"First line\n");
    for index in 0..40 {
        let _ = writeln!(message, "doc line {index}");
    }
    message.push_str("last line\"\"\"\n    return doc\n```\n\n");
    message.push_str("~~~text\nthis ~~ fence holds ``` backtick ``` lines inside\n~~~\n\n");
    message.push_str("```js\nlet first = 1;\n```\n```text\nsecond\n```\n\n");
    message.push_str("Tail paragraph before the still-open block.\n\n```rust\nfn unclosed(");
    message.push_str(&"x".repeat(600));
    message
}

/// Whatever the renderer does while streaming, the committed conversation must
/// be byte-identical to rendering the same final text in one shot. Guards the
/// incremental path against visual divergence: prefix commits are permanent.
/// Each chunk size lands splits on different structural boundaries.
#[test]
fn streamed_rendering_matches_one_shot() {
    let message = exactness_message();
    let mut one_shot = perf_ui();
    one_shot.acp_event(text_chunk(&message));
    one_shot.draw();
    one_shot.settle();

    for chunk_bytes in [7, 37, 128, 1024] {
        let mut streamed = perf_ui();
        for chunk in chunk_message(&message, chunk_bytes) {
            streamed.acp_event(text_chunk(&chunk));
            streamed.draw();
        }
        streamed.settle();
        assert_eq!(
            streamed.conversation_text(),
            one_shot.conversation_text(),
            "chunk size {chunk_bytes} diverged from one-shot"
        );
    }
}

/// The stronger form of [`streamed_rendering_matches_one_shot`]: the streamed
/// conversation must match a one-shot render of the same partial text at EVERY
/// chunk, because rows committed to native scrollback during streaming are
/// permanent — a divergence that closes by the end still leaves corrupted
/// scrollback behind.
#[test]
fn streamed_markdown_rendering_matches_one_shot_at_every_chunk() {
    assert_matches_one_shot_at_every_chunk(&exactness_message(), text_chunk);
}

#[test]
fn streamed_thought_preserves_content_without_rendering_it() {
    let mut message = String::new();
    for index in 0..60 {
        let _ = writeln!(message, "Considering step {index} of the plan before acting.");
    }
    message.push_str("Ready to start.");
    let mut ui = perf_ui();
    ui.submit("think");
    let mut seen = String::new();
    for chunk in chunk_message(&message, 61) {
        seen.push_str(&chunk);
        ui.acp_event(thought_chunk(&chunk));
        ui.draw();

        assert_eq!(
            ui.app().core().conversation().items().last().and_then(ConversationItem::text).as_deref(),
            Some(seen.as_str())
        );
        ui.assert_history_not_contains("Considering step");
        ui.assert_viewport_contains(seen.split_whitespace().last().unwrap());
        assert!(ui.app().progress_indicator().is_active(), "streaming thought must keep the progress band active");
    }
}

fn assert_matches_one_shot_at_every_chunk(message: &str, stream: impl Fn(&str) -> AcpEvent) {
    let mut streamed = perf_ui();
    let mut seen = String::new();
    for chunk in chunk_message(message, 61) {
        seen.push_str(&chunk);
        streamed.acp_event(stream(&chunk));
        streamed.draw();

        let mut reference = perf_ui();
        reference.acp_event(stream(&seen));
        reference.draw();

        assert_eq!(
            streamed.conversation_text(),
            reference.conversation_text(),
            "streamed render diverged from one-shot after {} bytes",
            seen.len()
        );
    }
}

#[test]
#[ignore = "benchmark: run with `cargo nextest run -p aether-wisp --features testing perf -- --ignored --nocapture`"]
fn rendering_work_breakdown() {
    println!();
    println!(
        "{:<44} {:>6} {:>8} {:>10} {:>12} {:>10} {:>9} {:>9} {:>9} {:>10} {:>8}",
        "scenario",
        "frames",
        "rebuilds",
        "md bytes",
        "hl miss/bytes",
        "max live",
        "layout",
        "live",
        "draw",
        "rebuild",
        "cells"
    );

    bench("seed 200-turn history", |_| {}, |ui| seed_long_history(ui, LONG_HISTORY));
    bench(
        "settle + 20 no-op frames",
        |ui| seed_long_history(ui, LONG_HISTORY),
        |ui| {
            ui.settle();
            for _ in 0..20 {
                ui.draw();
            }
        },
    );
    bench(
        "stream prose 32KB/256B, long history",
        |ui| {
            seed_long_history(ui, LONG_HISTORY);
            ui.settle();
        },
        |ui| stream_message(ui, StreamContent::Prose, 32 * 1024, 256),
    );
    bench(
        "stream thought 16KB/256B, long history",
        |ui| {
            seed_long_history(ui, LONG_HISTORY);
            ui.settle();
        },
        |ui| {
            stream_message(ui, StreamContent::Thought, 16 * 1024, 256);
        },
    );
    bench(
        "stream code 24KB/256B, long history",
        |ui| {
            seed_long_history(ui, LONG_HISTORY);
            ui.settle();
        },
        |ui| stream_message(ui, StreamContent::CodeBlock, 24 * 1024, 256),
    );
    bench("stream code 24KB/256B, empty", TestUi::settle, |ui| {
        stream_message(ui, StreamContent::CodeBlock, 24 * 1024, 256);
    });
    bench(
        "running tool + 50 ticks, long history",
        |ui| {
            seed_long_history(ui, LONG_HISTORY);
            ui.settle();
            ui.acp_event(tool_call("bench-tool", "Editing src/lib.rs"));
            ui.draw();
        },
        |ui| {
            let mut now = Instant::now();
            for _ in 0..50 {
                ui.tick(now);
                now += Duration::from_millis(100);
                ui.draw();
            }
        },
    );
    bench(
        "20 keypresses, settled long history",
        |ui| {
            seed_long_history(ui, LONG_HISTORY);
            ui.settle();
        },
        |ui| {
            for _ in 0..20 {
                ui.key(key(KeyCode::Char('a')));
                ui.draw();
            }
        },
    );
}

/// Runs one benchmark scenario: `prepare` builds it up unmeasured, then `measure`
/// runs with the counters reset and the row is printed.
fn bench(
    name: &str,
    prepare: impl FnOnce(&mut TestUi<InstrumentedBackend>),
    measure: impl FnOnce(&mut TestUi<InstrumentedBackend>),
) {
    let mut ui = perf_ui();
    prepare(&mut ui);
    ui.draw();
    ui.render_stats();
    ui.backend_mut().take_stats();

    measure(&mut ui);

    let stats = ui.render_stats();
    let backend = ui.backend_mut().take_stats();
    println!(
        "{:<44} {:>6} {:>8} {:>10} {:>5}/{:<6} {:>10} {:>8.2}ms {:>8.2}ms {:>8.2}ms {:>9.2}ms {:>8}",
        name,
        stats.frames,
        stats.item_rebuilds,
        stats.markdown_bytes_parsed,
        stats.highlight.cache_misses,
        stats.highlight.bytes_highlighted,
        stats.max_live_rows,
        ms(stats.ns_layout),
        ms(stats.ns_live),
        ms(stats.ns_draw),
        ms(stats.ns_item_rebuild),
        backend.cells_drawn,
    );
}

#[expect(clippy::cast_precision_loss)]
fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1e6
}

/// Seeds a deterministic long conversation: `turns` completed turns of
/// user messages, thoughts, markdown prose with fenced code, bash and edit
/// tool calls with diffs, and a sub-agent tree every eighth turn — drawing
/// after each turn so scrollback commits exactly like a live session.
fn seed_long_history(ui: &mut PerfUi, turns: usize) {
    for turn in 0..turns {
        let prompt = format!("Turn {turn}: reconcile the writer path in module_{turn} and add a regression test.");
        ui.submit(&prompt);
        ui.acp_event(session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
            acp::ContentBlock::Text(acp::TextContent::new(format!(
                "Reading module_{turn} to find the torn-update window before touching any call site."
            ))),
            format!("seed-thought-{turn}"),
        ))));
        ui.acp_event(text_chunk_with_id(&format!("seed-response-{turn}"), SEED_PROSE));
        ui.acp_event(text_chunk_with_id(&format!("seed-response-{turn}"), SEED_CODE_BLOCK));
        let bash = format!("seed-bash-{turn}");
        ui.acp_event(seed_bash_tool(&bash));
        ui.acp_event(tool_completed(&bash));
        let edit = format!("seed-edit-{turn}");
        ui.acp_event(seed_edit_tool(&edit, turn));
        ui.acp_event(tool_completed_with_diff_contents(&edit, SEED_DIFF_BEFORE, SEED_DIFF_AFTER));
        if turn % 8 == 0 {
            seed_sub_agent_tree(ui, turn);
        }
        ui.acp_event(text_chunk_with_id(&format!("seed-closing-{turn}"), SEED_CLOSING));
        ui.complete_prompt(acp::StopReason::EndTurn);
        ui.draw();
    }
}

/// Streams one assistant message of `total_bytes` in `chunk_bytes` chunks,
/// drawing after every chunk the way the event loop draws after every
/// wakeup. The message stays one open item while it streams.
fn stream_message(ui: &mut PerfUi, content: StreamContent, total_bytes: usize, chunk_bytes: usize) {
    let thought = matches!(content, StreamContent::Thought);
    let message = match content {
        StreamContent::Prose => prose_message(total_bytes),
        StreamContent::CodeBlock => code_block_message(total_bytes),
        StreamContent::Thought => thought_message(total_bytes),
    };
    for chunk in chunk_message(&message, chunk_bytes.max(1)) {
        if thought {
            ui.acp_event(thought_chunk(&chunk));
        } else {
            ui.acp_event(text_chunk(&chunk));
        }
        ui.draw();
    }
}

fn seed_sub_agent_tree(ui: &mut PerfUi, turn: usize) {
    let parent = format!("seed-spawn-{turn}");
    ui.acp_event(seed_spawn_tool(&parent));
    ui.acp_event(tool_completed(&parent));
    for agent in ["explorer", "fixer"] {
        let session = format!("{parent}-{agent}");
        let grep = format!("{session}-grep");
        ui.acp_event(sub_agent_started(&parent, &session, agent));
        ui.acp_event(sub_agent_tool_update(
            session.as_str(),
            ToolCallUpdate::new(grep.clone())
                .title("Grep")
                .name("coding__grep")
                .raw_input(json!({ "pattern": "torn update" })),
        ));
        ui.acp_event(sub_agent_tool_update(
            session.as_str(),
            ToolCallUpdate::new(grep).status(acp::ToolCallStatus::Completed),
        ));
        ui.acp_event(sub_agent_finished(&parent, &session, agent, acp::StopReason::EndTurn));
    }
}

/// The shape of message [`stream_message`] streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamContent {
    /// Flowing paragraphs, so finalization sealing runs as blank lines land and
    /// the open item stays small.
    Prose,
    /// One fenced Rust code block. No blank line outside the fence can finalize
    /// it while it streams, so the open item grows to the whole message — the
    /// shape that keeps re-rendering everything received so far.
    CodeBlock,
    /// A thinking stream: line-per-sentence thought chunks, which drive the
    /// progress band's activity rather than appending to the conversation.
    Thought,
}

fn chunk_message(message: &str, chunk_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let mut end = rest.len().min(chunk_bytes);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    chunks
}

const SEED_PROSE: &str = "\
Examining the request. The module guards its invariants behind a shared handle,
so the fix has to land on the writer side rather than at each call site. I will
rework the boundary so retries cannot observe a torn update, then cover the
regression with a test that fails on the current code.

";

const SEED_CODE_BLOCK: &str = "\
```rust
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::with_capacity(incoming.len());
    for delta in incoming {
        if !state.accepts(&delta) {
            continue;
        }
        state.apply(&delta);
        applied.push(delta);
    }
    state.commit(applied)
}
```

";

const SEED_CLOSING: &str = "\
Done — the writer now retries atomically and the regression test covers the
torn window.

";

const SEED_DIFF_BEFORE: &str = "\
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::new();
    for delta in incoming {
        state.apply(&delta);
    }
    state.commit(Vec::new())
}
";

const SEED_DIFF_AFTER: &str = "\
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::with_capacity(incoming.len());
    for delta in incoming {
        state.apply(&delta);
        applied.push(delta);
    }
    state.commit(applied)
}
";

fn seed_bash_tool(id: &str) -> AcpEvent {
    let mut tool_call = ToolCallUpdate::new(id.to_string()).title(format!("Run {id}"));
    tool_call.name = MaybeUndefined::Value("bash".into());
    tool_call.raw_input = MaybeUndefined::Value(json!({ "command": "cargo test --module writer" }));
    session_update(SessionUpdate::ToolCallUpdate(tool_call))
}

fn seed_edit_tool(id: &str, turn: usize) -> AcpEvent {
    session_update(SessionUpdate::ToolCallUpdate(
        ToolCallUpdate::new(id.to_string()).title(format!("Editing src/module_{turn}.rs")),
    ))
}

fn seed_spawn_tool(id: &str) -> AcpEvent {
    let mut tool_call = ToolCallUpdate::new(id.to_string()).title(format!("Spawning sub-agents ({id})"));
    tool_call.name = MaybeUndefined::Value("spawn_subagent".into());
    session_update(SessionUpdate::ToolCallUpdate(tool_call))
}

fn prose_message(total_bytes: usize) -> String {
    let mut message = String::new();
    let mut sentence = 0;
    while message.len() < total_bytes {
        for _ in 0..4 {
            let _ =
                write!(message, "Sentence {sentence} carries ordinary words so wrapping and parsing do real work. ");
            sentence += 1;
        }
        message.push_str("\n\n");
    }
    message
}

fn code_block_message(total_bytes: usize) -> String {
    let mut message = String::from("```rust\n");
    let mut line = 0;
    while message.len() < total_bytes {
        let _ = writeln!(message, "let value_{line} = state.reconcile(incoming[{line}]).expect(\"delta accepted\");");
        line += 1;
    }
    message.push_str("```\n");
    message
}

fn thought_message(total_bytes: usize) -> String {
    let mut message = String::new();
    let mut step = 0;
    while message.len() < total_bytes {
        let _ = writeln!(message, "Considering step {step} of the plan before acting on it.");
        step += 1;
    }
    message
}
