use aether_cli::acp::{
    map_agent_event_to_session_notification, map_replayed_agent_event, try_extract_plan_notification,
};
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent};
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use llm::{ToolCallRequest, ToolCallResult};
use mcp_utils::display_meta::{FileDiff, PlanMeta, PlanMetaEntry, PlanMetaStatus, ToolDisplayMeta, ToolResultMeta};
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn tool_upserts_preserve_omitted_null_and_replacement_fields() {
    let mut tool = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("{\"path\":\"a\"}") }));
    assert_eq!(tool.title.value().map(String::as_str), Some("Read file"));
    assert_eq!(tool.raw_input, MaybeUndefined::Value(json!({"path": "a"})));
    let omitted = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("invalid json") }));
    assert!(omitted.raw_input.is_undefined());
    assert!(serde_json::to_value(&omitted).unwrap().get("rawInput").is_none());
    tool.apply_update(omitted);
    assert_eq!(tool.raw_input, MaybeUndefined::Value(json!({"path": "a"})));

    let clear =
        mapped_tool(&AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id: "tool".into(), chunk: "null".into() }));
    assert!(clear.raw_input.is_null());
    assert_eq!(serde_json::to_value(&clear).unwrap()["rawInput"], json!(null));
    tool.apply_update(clear);
    assert!(tool.raw_input.is_null());
    tool.apply_update(mapped_tool(&AgentEvent::Tool(ToolEvent::CallUpdate {
        tool_call_id: "tool".into(),
        chunk: "{\"other\":1}".into(),
    })));
    assert_eq!(tool.raw_input, MaybeUndefined::Value(json!({"other": 1})));
    assert_eq!(tool.meta.value().unwrap()["aetherToolName"], "coding__read_file");

    let progress = AgentEvent::Tool(ToolEvent::Progress {
        request: request("{}"),
        progress: 1.0,
        total: Some(2.0),
        message: None,
    });
    tool.apply_update(mapped_tool(&progress));
    let result = mapped_tool(&result_event(None));
    let expected = result.content.clone();
    tool.apply_update(result);
    assert_eq!(tool.content, expected, "result content replaces rather than appends progress");
    assert_eq!(tool.content.value().unwrap().len(), 1);
}

#[test]
fn metadata_replacements_preserve_tool_identity_and_clear_old_display_values() {
    let mut tool = mapped_tool(&AgentEvent::Tool(ToolEvent::Call { request: request("{}") }));
    for value in ["a.rs", ""] {
        tool.apply_update(mapped_tool(&AgentEvent::Tool(ToolEvent::DisplayUpdate {
            request: request("{}"),
            meta: ToolDisplayMeta::new("Read", value).into(),
        })));
        assert_eq!(tool.meta.value().unwrap()["aetherToolName"], "coding__read_file");
        assert_eq!(
            tool.meta.value().unwrap().get("display_value").cloned(),
            if value.is_empty() { None } else { Some(json!(value)) }
        );
    }
    tool.apply_update(mapped_tool(&result_event(Some(ToolDisplayMeta::new("Read", "done").into()))));
    assert_eq!(tool.meta.value().unwrap()["aetherToolName"], "coding__read_file");
    assert_eq!(tool.meta.value().unwrap()["display_value"], "done");
}

#[test]
fn live_chunks_append_and_replayed_messages_replace_under_stable_ids() {
    for thought in [false, true] {
        let mut text = String::new();
        for chunk in ["hello", " world"] {
            let event = message(thought, chunk, false);
            assert!(map_replayed_agent_event("session".into(), &event).is_none());
            let update = map_agent_event_to_session_notification("session".into(), &event).unwrap().update;
            let chunk = match update {
                acp::SessionUpdate::AgentMessageChunk(c) | acp::SessionUpdate::AgentThoughtChunk(c) => c,
                other => panic!("expected chunk: {other:?}"),
            };
            assert_eq!(chunk.message_id, acp::MessageId::new("message"));
            let acp::ContentBlock::Text(content) = chunk.content else { panic!("expected text") };
            text.push_str(&content.text);
        }
        let complete = message(thought, "hello world", true);
        assert!(map_agent_event_to_session_notification("session".into(), &complete).is_none());
        let replay = map_replayed_agent_event("session".into(), &complete).unwrap().update;
        let (id, content) = match replay {
            acp::SessionUpdate::AgentMessage(m) => (m.message_id, m.content),
            acp::SessionUpdate::AgentThought(m) => (m.message_id, m.content),
            other => panic!("expected snapshot: {other:?}"),
        };
        assert_eq!(id, acp::MessageId::new("message"));
        assert_eq!(content, MaybeUndefined::Value(vec![acp::ContentBlock::Text(acp::TextContent::new(text))]));
    }
}

#[test]
fn whole_user_messages_preserve_identity_and_multimodal_content() {
    use aether_cli::acp::{map_acp_to_content_blocks, map_user_message};
    let blocks = vec![
        acp::ContentBlock::Text(acp::TextContent::new("hello")),
        acp::ContentBlock::Image(acp::ImageContent::new("aGVsbG8=", "image/png")),
        acp::ContentBlock::Audio(acp::AudioContent::new("aGVsbG8=", "audio/wav")),
    ];
    let llm_blocks = map_acp_to_content_blocks(blocks.clone());
    let message = map_user_message("user-message".into(), &llm_blocks);
    assert_eq!(message.message_id, acp::MessageId::new("user-message"));
    assert_eq!(message.content, MaybeUndefined::Value(blocks));
    let empty = map_user_message("user-message".into(), &[]);
    assert_eq!(empty.content, MaybeUndefined::Value(vec![]));
}

#[test]
fn plan_snapshots_replace_entries_with_a_fixed_session_scoped_id() {
    let statuses =
        [PlanMetaStatus::Pending, PlanMetaStatus::InProgress, PlanMetaStatus::Completed, PlanMetaStatus::Cancelled];
    let mut id = None;
    for session in ["one", "two"] {
        for entries in
            [statuses.iter().map(|status| PlanMetaEntry { content: "task".into(), status: *status }).collect(), vec![]]
        {
            let meta = ToolResultMeta::with_plan(ToolDisplayMeta::new("Todo", ""), PlanMeta { entries });
            let notification = try_extract_plan_notification(session.into(), Some(&meta)).unwrap();
            assert_eq!(notification.session_id, acp::SessionId::new(session));
            let acp::SessionUpdate::PlanUpdate(update) = notification.update else { panic!("expected plan update") };
            let acp::PlanUpdateContent::Items(plan) = update.plan else { panic!("expected items") };
            assert_eq!(id.get_or_insert(plan.plan_id.clone()), &plan.plan_id);
            let wire = serde_json::to_value(&plan).unwrap();
            if plan.entries.is_empty() {
                assert_eq!(wire["entries"], json!([]));
            } else {
                assert_eq!(
                    wire["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| entry["status"].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    ["pending", "in_progress", "completed", "_aether_cancelled"]
                );
            }
        }
    }
}

#[test]
fn diff_changes_and_git_patches_produce_the_same_file_state() {
    let cases = [
        (None, Some("new\n"), "add"),
        (None, Some(""), "add"),
        (Some("old\n"), None, "delete"),
        (Some(""), None, "delete"),
        (Some("old\n"), Some("new\n"), "modify"),
        (Some("old"), Some("new"), "modify"),
        (Some("old\n"), Some(""), "modify"),
        (Some(""), Some("new\n"), "modify"),
        (Some("a\r\nb\r\n"), Some("c\r\n"), "modify"),
        (Some("old\n"), Some("new"), "modify"),
    ];
    for (old, new, operation) in cases {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap().join("file with \"quotes\"\t雪.txt");
        if let Some(old) = old {
            std::fs::write(&path, old).unwrap();
        }
        let diff = mapped_diff(FileDiff {
            path: path.to_string_lossy().into_owned(),
            old_text: old.map(str::to_owned),
            new_text: new.map(str::to_owned),
        });
        let change = serde_json::to_value(&diff.changes[0]).unwrap();
        assert_eq!(change["operation"], operation);
        assert_eq!(change["path"], path.to_string_lossy().as_ref());
        assert_eq!(change["fileType"], "text");
        assert_eq!(change["mimeType"], "text/plain");
        let git_patch = diff.patch.unwrap();
        assert_eq!(git_patch.format, acp::DiffPatchFormat::GitPatch);
        let mut child = Command::new("git")
            .args(["apply", "--unsafe-paths", "-p0", "-"])
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(git_patch.text.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{operation}: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            git_patch.text
        );
        assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), new);
    }
}

#[test]
fn unchanged_diff_has_no_patch_and_invalid_snapshots_are_not_emitted() {
    let diff = mapped_diff(FileDiff {
        path: "/tmp/file".into(),
        old_text: Some("same".into()),
        new_text: Some("same".into()),
    });
    assert!(diff.patch.is_none());
    for diff in [
        FileDiff { path: "relative.txt".into(), old_text: None, new_text: Some("new".into()) },
        FileDiff { path: "/tmp/file".into(), old_text: None, new_text: None },
        FileDiff { path: "/dev/null".into(), old_text: None, new_text: Some("new".into()) },
        FileDiff { path: "/tmp/file\0".into(), old_text: None, new_text: Some("new".into()) },
        FileDiff { path: "/tmp/file".into(), old_text: Some("old\0".into()), new_text: Some("new".into()) },
        FileDiff { path: "/tmp/file".into(), old_text: None, new_text: Some("new\0".into()) },
    ] {
        let tool =
            mapped_tool(&result_event(Some(ToolResultMeta::with_file_diff(ToolDisplayMeta::new("Edit", ""), diff))));
        assert_eq!(tool.content.value().unwrap().len(), 1);
    }
}

fn request(arguments: &str) -> ToolCallRequest {
    ToolCallRequest { id: "tool".into(), name: "coding__read_file".into(), arguments: arguments.into() }
}

fn result_event(result_meta: Option<ToolResultMeta>) -> AgentEvent {
    AgentEvent::Tool(ToolEvent::Result {
        result: ToolCallResult {
            id: "tool".into(),
            name: "coding__read_file".into(),
            arguments: "{}".into(),
            result: "done".into(),
        },
        result_meta,
    })
}

fn mapped_tool(event: &AgentEvent) -> acp::ToolCallUpdate {
    let notification = map_agent_event_to_session_notification("session".into(), event).unwrap();
    assert_eq!(serde_json::to_value(&notification).unwrap()["update"]["sessionUpdate"], "tool_call_update");
    let acp::SessionUpdate::ToolCallUpdate(tool) = notification.update else { panic!("expected tool upsert") };
    tool
}

fn mapped_diff(diff: FileDiff) -> acp::Diff {
    let tool = mapped_tool(&result_event(Some(ToolResultMeta::with_file_diff(ToolDisplayMeta::new("Edit", ""), diff))));
    tool.content
        .take()
        .unwrap()
        .into_iter()
        .find_map(|content| match content {
            acp::ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        })
        .unwrap()
}

fn message(thought: bool, chunk: &str, is_complete: bool) -> AgentEvent {
    AgentEvent::Message(if thought {
        MessageEvent::Thought { message_id: "message".into(), chunk: chunk.into(), is_complete }
    } else {
        MessageEvent::Text { message_id: "message".into(), chunk: chunk.into(), is_complete }
    })
}
