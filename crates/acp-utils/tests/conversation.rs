use acp_utils::conversation::{
    ActivityPhase, Conversation, ConversationContent, ItemState, ToolCall, ToolStatus, TurnFinished, TurnPhase,
};
use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams, SubAgentToolRequest};
use acp_utils::testing::{idle_notification, plan_notification, running_notification};
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn message_ids_isolate_replacement_clearing_and_chunks() {
    let mut conversation = Conversation::new();
    conversation.apply_update(&agent_message("a", "first"));
    conversation.apply_update(&agent_message("b", "second"));
    conversation.apply_update(&agent_chunk("a", "!"));
    assert_eq!(text(&conversation, 0), "first!");
    assert_eq!(text(&conversation, 1), "second");

    let revision = conversation.items()[0].revision();
    conversation.apply_update(&agent_message("a", "replacement"));
    assert!(conversation.items()[0].revision() > revision);
    conversation.apply_update(&acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("a")));
    assert_eq!(text(&conversation, 0), "replacement");
    conversation
        .apply_update(&acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("a").content(MaybeUndefined::Null)));
    assert_eq!(text(&conversation, 0), "");
    conversation.apply_update(&acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("b").content(vec![])));
    assert_eq!(text(&conversation, 1), "");
}

#[test]
fn streamed_text_merges_into_one_block_and_other_blocks_follow_it() {
    let mut conversation = Conversation::new();
    conversation.apply_update(&agent_chunk("reply", "Hel"));
    let first_revision = conversation.items()[0].revision();
    conversation.apply_update(&agent_chunk("reply", "lo"));
    conversation.apply_update(&agent_chunk("reply", ""));

    assert_eq!(conversation.items().len(), 1);
    assert_eq!(conversation.items()[0].state(), ItemState::Open);
    assert!(conversation.items()[0].revision() > first_revision);
    assert_eq!(conversation.items()[0].content(), &ConversationContent::Assistant(vec!["Hello".into()]));

    let image = acp::ContentBlock::Image(acp::ImageContent::new("data", "image/png"));
    conversation.apply_update(&acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(image.clone(), "reply")));
    conversation.apply_update(&agent_chunk("reply", "after"));
    assert_eq!(
        conversation.items()[0].content(),
        &ConversationContent::Assistant(vec!["Hello".into(), image, "after".into()])
    );
    assert_eq!(text(&conversation, 0), "Hello\n[Image content]\nafter");
}

#[test]
fn remote_user_resources_render_as_references_but_keep_their_blocks() {
    let mut conversation = Conversation::new();
    let resource =
        acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new("file-body-sentinel", "file:///large.rs"),
        )));
    conversation
        .apply_update(&acp::SessionUpdate::UserMessage(acp::UserMessage::new("user").content(vec![resource.clone()])));
    assert_eq!(text(&conversation, 0), "[Resource: file:///large.rs]");
    assert_eq!(conversation.items()[0].content(), &ConversationContent::User(vec![resource.clone()]));

    conversation.apply_update(&acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(resource, "chunked")));
    assert_eq!(text(&conversation, 1), "[Resource: file:///large.rs]");
}

#[test]
fn user_ack_adopts_only_the_explicit_optimistic_item() {
    let mut conversation = Conversation::new();
    conversation.append_user_content(vec!["same".into()]);
    conversation.append_pending_user_content(vec!["same".into()]);
    conversation.apply_update(&user_message("user", "expanded prompt"));
    conversation.apply_update(&user_message("user", "expanded prompt"));
    assert_eq!(conversation.items().len(), 2);
    assert_eq!(text(&conversation, 0), "same");
    assert_eq!(text(&conversation, 1), "same");

    conversation
        .apply_update(&acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new("expanded tail".into(), "user")));
    assert_eq!(text(&conversation, 1), "same");
    assert_eq!(conversation.items()[1].message_id(), Some(&acp::MessageId::new("user")));

    conversation.clear();
    conversation.apply_update(&user_message("user", "replayed"));
    assert_eq!(conversation.items().len(), 1);
    assert_eq!(text(&conversation, 0), "replayed");
}

#[test]
fn optimistic_prompt_stays_open_until_its_turn_ends() {
    let mut conversation = Conversation::new();
    conversation.append_pending_user_content(vec!["prompt".into()]);
    conversation.start_prompt();
    assert!(conversation.items()[0].is_open());

    conversation.apply_update(&user_message("ack", "expanded"));
    assert!(conversation.items()[0].is_open());

    conversation.accept_prompt();
    conversation.apply_update(&idle(None));
    assert!(!conversation.items()[0].is_open());
}

#[test]
fn first_tool_update_creates_and_later_patches_replace_or_clear() {
    let mut conversation = Conversation::new();
    conversation.apply_update(&tool_update(
        acp::ToolCallUpdate::new("tool")
            .title("Bash")
            .raw_input(json!({"command":"first"}))
            .status(acp::ToolCallStatus::Completed),
    ));
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool")));
    assert_eq!(tool(&conversation, 0).title(), "Bash");

    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool").raw_input(json!({"command":"second"}))));
    assert_eq!(tool(&conversation, 0).bash_command().as_deref(), Some("second"));

    conversation.apply_update(&tool_update(
        acp::ToolCallUpdate::new("tool").title(MaybeUndefined::Null).raw_input(MaybeUndefined::Null),
    ));
    assert_eq!(conversation.items().len(), 1);
    assert!(tool(&conversation, 0).title().is_empty());
    assert!(tool(&conversation, 0).raw_input().is_empty());
}

#[test]
fn tool_content_chunks_append_but_updates_replace() {
    let mut conversation = Conversation::new();
    let content = || acp::ToolCallContent::Content(Box::new(acp::Content::new("output")));
    conversation.apply_update(&tool_chunk("tool", content()));
    conversation.apply_update(&tool_chunk("tool", content()));
    assert_eq!(tool(&conversation, 0).content().len(), 2);

    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool").content(vec![content()])));
    assert_eq!(tool(&conversation, 0).content().len(), 1);

    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool").content(MaybeUndefined::Null)));
    assert!(tool(&conversation, 0).content().is_empty());
}

#[test]
fn tool_kind_status_and_metadata_follow_patch_semantics() {
    let mut conversation = Conversation::new();
    let update: acp::ToolCallUpdate = serde_json::from_value(json!({
        "toolCallId": "tool", "kind": "execute", "rawInput": {"command": "ls"},
        "status": "completed", "_meta": {"display_value": "done"}
    }))
    .unwrap();
    conversation.apply_update(&tool_update(update));
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool")));
    assert_eq!(tool(&conversation, 0).bash_command(), None);
    assert_eq!(tool(&conversation, 0).display_value(), Some("done"));
    assert_eq!(tool(&conversation, 0).status, ToolStatus::Success);

    conversation.apply_update(&tool_update(
        acp::ToolCallUpdate::new("tool")
            .kind(MaybeUndefined::Null)
            .status(MaybeUndefined::Null)
            .meta(MaybeUndefined::Null),
    ));
    assert_eq!(tool(&conversation, 0).bash_command(), None);
    assert_eq!(tool(&conversation, 0).display_value(), None);
    assert_eq!(tool(&conversation, 0).status, ToolStatus::Running);
}

#[test]
fn tool_items_seal_at_a_terminal_status_or_the_end_of_the_turn() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    let update = acp::ToolCallUpdate::new("read").title("Read file").raw_input(json!({"path": "src/lib.rs"}));
    conversation.apply_update(&tool_update(update.clone()));
    conversation.apply_update(&tool_update(update));
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("edit").title("Edit file")));
    assert_eq!(conversation.items().len(), 2);
    assert!(conversation.items().iter().all(|item| item.state() == ItemState::Open));

    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("read").status(acp::ToolCallStatus::Completed)));
    assert_eq!(conversation.items()[0].state(), ItemState::Sealed);
    assert_eq!(conversation.items()[1].state(), ItemState::Open);

    conversation.apply_update(&idle(None));
    assert_eq!(conversation.items()[1].state(), ItemState::Sealed);
    assert_eq!(tool(&conversation, 1).status, ToolStatus::Success);
}

#[test]
fn diffs_keep_their_patch_and_follow_patch_semantics() {
    let patch =
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-before\n+after\n";
    let change = || acp::DiffChange::modify(acp::AbsolutePath::new("/workspace/src/lib.rs"));
    let mut conversation = Conversation::new();
    conversation.apply_update(&tool_update(
        acp::ToolCallUpdate::new("edit")
            .title("Edit files")
            .content(vec![acp::ToolCallContent::Diff(acp::Diff::patch(patch, vec![change()]))]),
    ));
    let diff = tool(&conversation, 0).diffs().next().unwrap();
    assert_eq!(diff.patch.as_ref().unwrap().text, patch);
    assert_eq!(diff.changes, [change()]);

    conversation.apply_update(&tool_chunk("edit", acp::ToolCallContent::Diff(acp::Diff::new(vec![change()]))));
    assert_eq!(tool(&conversation, 0).diffs().count(), 2);
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("edit")));
    assert_eq!(tool(&conversation, 0).diffs().count(), 2);
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("edit").content(vec![])));
    assert!(tool(&conversation, 0).diffs().next().is_none());
}

#[test]
fn a_prompt_accepted_before_idle_finishes_on_idle() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    assert_eq!(conversation.turn(), TurnPhase::Submitting);
    assert!(conversation.waiting_for_response());

    conversation.accept_prompt();
    conversation.apply_update(&running());
    assert_eq!(conversation.turn(), TurnPhase::Running);

    let finished = conversation.apply_update(&idle(Some(acp::StopReason::EndTurn)));
    assert_eq!(finished, Some(TurnFinished { stop_reason: Some(acp::StopReason::EndTurn) }));
    assert_eq!(conversation.turn(), TurnPhase::Idle);
    assert_eq!(conversation.apply_update(&idle(None)), None, "a second idle ends nothing");
}

#[test]
fn a_turn_that_finishes_before_acceptance_stays_owed_until_the_response() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    assert!(conversation.apply_update(&idle(None)).is_some());
    assert_eq!(conversation.turn(), TurnPhase::CompletedBeforeAcceptance);
    assert!(!conversation.waiting_for_response());
    assert!(!conversation.turn().is_idle(), "the prompt's response is still owed");

    conversation.clear();
    assert_eq!(conversation.turn(), TurnPhase::CompletedBeforeAcceptance, "a clear does not settle the prompt");

    conversation.accept_prompt();
    assert!(conversation.turn().is_idle());
}

#[test]
fn a_rejected_prompt_fails_its_running_tools() {
    for idle_first in [false, true] {
        let mut conversation = Conversation::new();
        conversation.start_prompt();
        conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool").title("Bash")));
        if idle_first {
            conversation.apply_update(&idle(None));
        }

        conversation.reject_prompt("overloaded");

        assert!(conversation.turn().is_idle());
        let expected = if idle_first { ToolStatus::Success } else { ToolStatus::Error("failed: overloaded".into()) };
        assert_eq!(tool(&conversation, 0).status, expected);
    }
}

#[test]
fn a_cancelled_turn_keeps_its_output_and_cancels_running_tools() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool").title("Bash")));
    conversation.apply_update(&agent_chunk("reply", "final output"));

    let finished = conversation.apply_update(&idle(Some(acp::StopReason::Cancelled)));

    assert_eq!(finished, Some(TurnFinished { stop_reason: Some(acp::StopReason::Cancelled) }));
    assert_eq!(tool(&conversation, 0).status, ToolStatus::Error("cancelled".into()));
    assert_eq!(text(&conversation, 1), "final output");
    conversation.accept_prompt();
    assert!(conversation.turn().is_idle(), "a late acceptance does not restart the turn");
}

#[test]
fn a_running_state_while_idle_adopts_a_turn_started_elsewhere() {
    let mut conversation = Conversation::new();
    conversation.apply_update(&running());
    assert_eq!(conversation.turn(), TurnPhase::Running);
    assert_eq!(conversation.activity().phase(), ActivityPhase::Responding);

    assert!(conversation.apply_update(&idle(None)).is_some());
    assert!(conversation.turn().is_idle());
    assert_eq!(conversation.activity().phase(), ActivityPhase::Idle);
}

#[test]
fn replayed_history_does_not_start_activity() {
    let mut conversation = Conversation::new();
    conversation.apply_update(&idle(None));
    conversation.apply_update(&agent_message("history", "saved response"));
    conversation.apply_update(&agent_chunk("history", " tail"));

    assert!(conversation.turn().is_idle());
    assert_eq!(conversation.activity().phase(), ActivityPhase::Idle);
    assert_eq!(text(&conversation, 0), "saved response tail");
}

#[test]
fn thoughts_stream_while_thinking_and_clear_when_the_agent_moves_on() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    conversation.apply_update(&thought_chunk("thought", "first\n"));
    conversation.apply_update(&thought_chunk("thought", "second"));
    assert_eq!(conversation.activity().phase(), ActivityPhase::Thinking);
    assert_eq!(conversation.activity().thought(), "first\nsecond");

    conversation.apply_update(&thought_chunk("next", "fresh"));
    assert_eq!(conversation.activity().thought(), "fresh", "a new thought replaces the previous one");

    conversation.apply_update(&agent_chunk("reply", "answer"));
    assert_eq!(conversation.activity().phase(), ActivityPhase::Responding);
    assert_eq!(conversation.activity().thought(), "");

    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool")));
    assert_eq!(conversation.activity().phase(), ActivityPhase::Working);
}

#[test]
fn activity_after_the_turn_ends_is_ignored_until_the_next_prompt() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("spawn").title("spawn_subagent")));
    conversation.apply_update(&idle(None));

    conversation.apply_update(&thought_chunk("late", "stray thought"));
    conversation.apply_sub_agent_progress(&sub_agent_tool_call("spawn", "late-call"));
    conversation.apply_update(&compaction(acp::CompactionStatus::InProgress));

    assert_eq!(conversation.activity().thought(), "");
    assert!(tool(&conversation, 0).sub_agents.is_empty());
    assert!(!conversation.is_compacting());

    conversation.start_prompt();
    conversation.apply_sub_agent_progress(&sub_agent_tool_call("spawn", "call"));
    assert_eq!(tool(&conversation, 0).sub_agents[0].tool_calls[0].id, "call");
}

#[test]
fn compaction_is_tracked_until_it_completes_or_the_turn_ends() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    conversation.apply_update(&compaction(acp::CompactionStatus::InProgress));
    assert!(conversation.is_compacting());
    conversation.apply_update(&compaction(acp::CompactionStatus::Completed));
    assert!(!conversation.is_compacting());

    conversation.apply_update(&compaction(acp::CompactionStatus::InProgress));
    conversation.apply_update(&idle(None));
    assert!(!conversation.is_compacting());
}

#[test]
fn plan_and_usage_updates_replace_the_previous_ones() {
    let mut conversation = Conversation::new();
    let entry =
        |content: &str| acp::PlanEntry::new(content, acp::PlanEntryPriority::Medium, acp::PlanEntryStatus::Pending);
    conversation.apply_update(&plan_notification("session", "a", vec![entry("one")]).update);
    conversation.apply_update(&plan_notification("session", "b", vec![entry("two")]).update);
    conversation.apply_update(&acp::SessionUpdate::PlanUpdate(acp::PlanUpdate::new(acp::PlanUpdateContent::Other(
        acp::OtherPlanUpdateContent::new("future", "b", BTreeMap::new()),
    ))));
    let plan = conversation.plan().unwrap();
    assert_eq!(plan.plan_id, acp::PlanId::new("b"));
    assert_eq!(plan.entries, [entry("two")]);

    conversation.apply_update(&acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(10, 100)));
    conversation.apply_update(&acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(20, 100)));
    assert_eq!(conversation.context_usage().map(|usage| usage.used), Some(20));
}

#[test]
fn clear_replaces_identity_and_resets_everything() {
    let mut conversation = Conversation::new();
    let previous_id = conversation.id();
    conversation.start_prompt();
    conversation.accept_prompt();
    conversation.append_user_content(vec!["before".into()]);
    conversation.apply_update(&acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(10, 100)));
    conversation.apply_update(&plan_notification("session", "a", vec![]).update);

    conversation.clear();

    assert_ne!(conversation.id(), previous_id);
    assert!(conversation.items().is_empty());
    assert!(conversation.turn().is_idle(), "a running turn ends with the conversation");
    assert!(conversation.plan().is_none());
    assert!(conversation.context_usage().is_none());
}

#[test]
fn a_closed_connection_ends_the_turn() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    conversation.apply_update(&thought_chunk("thought", "pondering"));

    conversation.connection_closed();

    assert!(conversation.turn().is_idle());
    assert_eq!(conversation.activity().phase(), ActivityPhase::Idle);
}

#[test]
fn notices_are_distinct_from_user_content() {
    let mut conversation = Conversation::new();
    conversation.append_user_content(vec!["prompt".into()]);
    conversation.append_notice("Context cleared");

    assert!(matches!(conversation.items()[0].content(), ConversationContent::User(_)));
    assert_eq!(conversation.items()[1].content(), &ConversationContent::Notice("Context cleared".into()));
}

#[test]
fn items_serialize_with_their_kind_and_protocol_content() {
    let mut conversation = Conversation::new();
    conversation.start_prompt();
    conversation.accept_prompt();
    conversation.apply_update(&agent_chunk("reply", "hello"));
    conversation.apply_update(&tool_update(acp::ToolCallUpdate::new("tool").title("Bash")));
    conversation.apply_update(&idle(Some(acp::StopReason::Cancelled)));
    conversation.append_notice("note");

    let items = serde_json::to_value(conversation.items()).unwrap();

    assert_eq!(
        items,
        json!([
            {
                "id": 0, "messageId": "reply", "revision": 2, "state": "sealed",
                "kind": "assistant", "content": [{"type": "text", "text": "hello"}]
            },
            {
                "id": 1, "messageId": null, "revision": 2, "state": "sealed", "kind": "tool",
                "content": {
                    "status": {"error": "cancelled"},
                    "subAgents": [],
                    "toolCall": {"toolCallId": "tool", "title": "Bash"}
                }
            },
            {"id": 2, "messageId": null, "revision": 0, "state": "sealed", "kind": "notice", "content": "note"}
        ])
    );
    assert_eq!(serde_json::to_value(conversation.activity()).unwrap(), json!({"phase": "idle", "thought": ""}));
    assert_eq!(serde_json::to_value(conversation.turn()).unwrap(), json!("idle"));
}

fn text(conversation: &Conversation, index: usize) -> String {
    conversation.items()[index].text().expect("a message or notice").into_owned()
}

fn tool(conversation: &Conversation, index: usize) -> &ToolCall {
    let ConversationContent::Tool(tool) = conversation.items()[index].content() else { panic!("tool item") };
    tool
}

fn agent_message(id: &str, text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessage(acp::AgentMessage::new(id).content(vec![text.into()]))
}

fn agent_chunk(id: &str, text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(text.into(), id))
}

fn thought_chunk(id: &str, text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(text.into(), id))
}

fn user_message(id: &str, text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::UserMessage(acp::UserMessage::new(id).content(vec![text.into()]))
}

fn tool_update(update: acp::ToolCallUpdate) -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCallUpdate(update)
}

fn tool_chunk(id: &str, content: acp::ToolCallContent) -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCallContentChunk(acp::ToolCallContentChunk::new(id, content))
}

fn compaction(status: acp::CompactionStatus) -> acp::SessionUpdate {
    acp::SessionUpdate::CompactionUpdate(acp::CompactionUpdate::new("compaction", status))
}

fn running() -> acp::SessionUpdate {
    running_notification("session").update
}

fn idle(stop_reason: Option<acp::StopReason>) -> acp::SessionUpdate {
    idle_notification("session", stop_reason).update
}

fn sub_agent_tool_call(parent_tool_id: &str, id: &str) -> SubAgentProgressParams {
    SubAgentProgressParams {
        parent_tool_id: parent_tool_id.to_string(),
        task_id: "task".to_string(),
        agent_name: "explorer".to_string(),
        event: SubAgentEvent::ToolCall {
            request: SubAgentToolRequest { id: id.to_string(), name: "grep".to_string(), arguments: "{}".to_string() },
        },
    }
}
