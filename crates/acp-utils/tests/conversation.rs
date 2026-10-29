use acp_utils::client::AcpEvent;
use acp_utils::conversation::{
    Activity, Conversation, ConversationContent, ItemState, Notice, NoticeLevel, SubAgent, ToolCall, ToolStatus,
    TurnFinished, TurnPhase,
};
use acp_utils::notifications::ContextClearedParams;
use acp_utils::testing::{
    self, announced, idle_notification, idle_state, message_chunk, on_session, plan_notification, running_notification,
    running_state, sub_agent_tool_update,
};
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn message_ids_isolate_replacement_clearing_and_chunks() {
    let mut conversation = conversation();
    conversation.apply_event(&agent_message("a", "first"));
    conversation.apply_event(&agent_message("b", "second"));
    conversation.apply_event(&agent_chunk("a", "!"));
    assert_eq!(text(&conversation, 0), "first!");
    assert_eq!(text(&conversation, 1), "second");

    let revision = conversation.items()[0].revision();
    conversation.apply_event(&agent_message("a", "replacement"));
    assert!(conversation.items()[0].revision() > revision);
    conversation.apply_event(&update(acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("a"))));
    assert_eq!(text(&conversation, 0), "replacement");
    conversation.apply_event(&update(acp::SessionUpdate::AgentMessage(
        acp::AgentMessage::new("a").content(MaybeUndefined::Null),
    )));
    assert_eq!(text(&conversation, 0), "");
    conversation.apply_event(&update(acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("b").content(vec![]))));
    assert_eq!(text(&conversation, 1), "");
}

#[test]
fn streamed_text_merges_into_one_block_and_other_blocks_follow_it() {
    let mut conversation = conversation();
    conversation.apply_event(&agent_chunk("reply", "Hel"));
    let first_revision = conversation.items()[0].revision();
    conversation.apply_event(&agent_chunk("reply", "lo"));
    conversation.apply_event(&agent_chunk("reply", ""));

    assert_eq!(conversation.items().len(), 1);
    assert_eq!(conversation.items()[0].state(), ItemState::Open);
    assert!(conversation.items()[0].revision() > first_revision);
    assert_eq!(conversation.items()[0].content(), &ConversationContent::Assistant(vec!["Hello".into()]));

    let image = acp::ContentBlock::Image(acp::ImageContent::new("data", "image/png"));
    conversation
        .apply_event(&update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(image.clone(), "reply"))));
    conversation.apply_event(&agent_chunk("reply", "after"));
    assert_eq!(
        conversation.items()[0].content(),
        &ConversationContent::Assistant(vec!["Hello".into(), image, "after".into()])
    );
    assert_eq!(text(&conversation, 0), "Hello\n[Image content]\nafter");
}

#[test]
fn remote_user_resources_render_as_references_but_keep_their_blocks() {
    let mut conversation = conversation();
    let resource =
        acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new("file-body-sentinel", "file:///large.rs"),
        )));
    conversation.apply_event(&update(acp::SessionUpdate::UserMessage(
        acp::UserMessage::new("user").content(vec![resource.clone()]),
    )));
    assert_eq!(text(&conversation, 0), "[Resource: file:///large.rs]");
    assert_eq!(conversation.items()[0].content(), &ConversationContent::User(vec![resource.clone()]));

    conversation
        .apply_event(&update(acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(resource, "chunked"))));
    assert_eq!(text(&conversation, 1), "[Resource: file:///large.rs]");
}

#[test]
fn user_messages_appear_once_the_agent_inserts_them_and_are_keyed_by_message_id() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    assert_eq!(conversation.items(), []);

    conversation.apply_event(&user_message("user", "prompt"));
    conversation.apply_event(&user_message("user", "prompt"));
    conversation
        .apply_event(&update(acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(" tail".into(), "user"))));
    assert_eq!(conversation.items().len(), 1);
    assert_eq!(text(&conversation, 0), "prompt tail");
    assert_eq!(conversation.items()[0].message_id(), Some(&acp::MessageId::new("user")));
}

#[test]
fn a_user_message_stays_open_until_its_turn_ends() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&user_message("user", "prompt"));
    assert!(conversation.items()[0].is_open());

    conversation.apply_event(&idle(None));
    assert!(!conversation.items()[0].is_open());
}

#[test]
fn first_tool_update_creates_and_later_patches_replace_or_clear() {
    let mut conversation = conversation();
    conversation.apply_event(&tool_update(
        acp::ToolCallUpdate::new("tool")
            .title("Bash")
            .raw_input(json!({"command":"first"}))
            .status(acp::ToolCallStatus::Completed),
    ));
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool")));
    assert_eq!(tool(&conversation, 0).title(), "Bash");

    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool").raw_input(json!({"command":"second"}))));
    assert_eq!(tool(&conversation, 0).bash_command(), Some("second"));

    conversation.apply_event(&tool_update(
        acp::ToolCallUpdate::new("tool").title(MaybeUndefined::Null).raw_input(MaybeUndefined::Null),
    ));
    assert_eq!(conversation.items().len(), 1);
    assert_eq!(tool(&conversation, 0).title(), "");
    assert_eq!(tool(&conversation, 0).raw_input(), "");
}

#[test]
fn tool_content_chunks_append_but_updates_replace() {
    let mut conversation = conversation();
    let content = || acp::ToolCallContent::Content(Box::new(acp::Content::new("output")));
    conversation.apply_event(&tool_chunk("tool", content()));
    conversation.apply_event(&tool_chunk("tool", content()));
    assert_eq!(tool(&conversation, 0).content().len(), 2);

    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool").content(vec![content()])));
    assert_eq!(tool(&conversation, 0).content().len(), 1);

    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool").content(MaybeUndefined::Null)));
    assert_eq!(tool(&conversation, 0).content(), []);
}

#[test]
fn tool_kind_status_and_metadata_follow_patch_semantics() {
    let mut conversation = conversation();
    let update: acp::ToolCallUpdate = serde_json::from_value(json!({
        "toolCallId": "tool", "kind": "execute", "rawInput": {"command": "ls"},
        "status": "completed", "_meta": {"display_value": "done"}
    }))
    .unwrap();
    conversation.apply_event(&tool_update(update));
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool")));
    assert_eq!(tool(&conversation, 0).bash_command(), None);
    assert_eq!(tool(&conversation, 0).display_value(), Some("done"));
    assert_eq!(tool(&conversation, 0).status, ToolStatus::Success);

    conversation.apply_event(&tool_update(
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
    let mut conversation = conversation();
    conversation.apply_event(&running());
    let update = acp::ToolCallUpdate::new("read").title("Read file").raw_input(json!({"path": "src/lib.rs"}));
    conversation.apply_event(&tool_update(update.clone()));
    conversation.apply_event(&tool_update(update));
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("edit").title("Edit file")));
    assert_eq!(conversation.items().len(), 2);
    assert!(conversation.items().iter().all(|item| item.state() == ItemState::Open));

    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("read").status(acp::ToolCallStatus::Completed)));
    assert_eq!(conversation.items()[0].state(), ItemState::Sealed);
    assert_eq!(conversation.items()[1].state(), ItemState::Open);

    conversation.apply_event(&idle(None));
    assert_eq!(conversation.items()[1].state(), ItemState::Sealed);
    assert_eq!(tool(&conversation, 1).status, ToolStatus::Success);
}

#[test]
fn diffs_keep_their_patch_and_follow_patch_semantics() {
    let patch =
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-before\n+after\n";
    let change = || acp::DiffChange::modify(acp::AbsolutePath::new("/workspace/src/lib.rs"));
    let mut conversation = conversation();
    conversation.apply_event(&tool_update(
        acp::ToolCallUpdate::new("edit")
            .title("Edit files")
            .content(vec![acp::ToolCallContent::Diff(acp::Diff::patch(patch, vec![change()]))]),
    ));
    let diff = tool(&conversation, 0).diffs().next().unwrap();
    assert_eq!(diff.patch.as_ref().unwrap().text, patch);
    assert_eq!(diff.changes, [change()]);

    conversation.apply_event(&tool_chunk("edit", acp::ToolCallContent::Diff(acp::Diff::new(vec![change()]))));
    assert_eq!(tool(&conversation, 0).diffs().count(), 2);
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("edit")));
    assert_eq!(tool(&conversation, 0).diffs().count(), 2);
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("edit").content(vec![])));
    assert!(tool(&conversation, 0).diffs().next().is_none());
}

#[test]
fn a_turn_runs_from_a_running_state_to_the_next_idle_state() {
    let mut conversation = conversation();
    assert_eq!(conversation.turn(), TurnPhase::Idle);

    conversation.apply_event(&running());
    assert_eq!(conversation.turn(), TurnPhase::Running);
    conversation.apply_event(&running());
    assert_eq!(conversation.turn(), TurnPhase::Running);

    let finished = conversation.apply_event(&idle(Some(acp::StopReason::EndTurn)));
    assert_eq!(finished, Some(TurnFinished { stop_reason: Some(acp::StopReason::EndTurn) }));
    assert_eq!(conversation.turn(), TurnPhase::Idle);
    assert_eq!(conversation.apply_event(&idle(None)), None, "a second idle ends nothing");
}

#[test]
fn a_cancelled_turn_keeps_its_output_and_cancels_running_tools() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool").title("Bash")));
    conversation.apply_event(&agent_chunk("reply", "final output"));

    let finished = conversation.apply_event(&idle(Some(acp::StopReason::Cancelled)));

    assert_eq!(finished, Some(TurnFinished { stop_reason: Some(acp::StopReason::Cancelled) }));
    assert_eq!(tool(&conversation, 0).status, ToolStatus::Cancelled);
    assert_eq!(text(&conversation, 1), "final output");
}

#[test]
fn sub_agent_tool_calls_merge_upserts_like_top_level_tools() {
    let mut conversation = with_sub_agent(spawn());
    assert!(sub_agent(&conversation).tool_calls().next().is_none(), "a started sub-agent shows before its tools");

    let mut display = serde_json::Map::new();
    display.insert("display_value".into(), "ls (exit 0)".into());
    for update in [
        acp::ToolCallUpdate::new("bash").title("Bash").name("coding__bash").status(acp::ToolCallStatus::InProgress),
        acp::ToolCallUpdate::new("bash").raw_input(json!({"command": "ls"})),
        acp::ToolCallUpdate::new("bash").title("Ran").status(acp::ToolCallStatus::Completed).meta(display),
        acp::ToolCallUpdate::new("grep").title("Grep").name("coding__grep").status(acp::ToolCallStatus::InProgress),
    ] {
        conversation.apply_event(&sub_agent_tool_update("child", update));
    }

    let calls: Vec<_> = sub_agent(&conversation).tool_calls().collect();
    let [bash, grep] = calls.as_slice() else { panic!("two sub-agent tools") };
    assert_eq!((bash.title(), bash.status), ("Ran", ToolStatus::Success));
    assert_eq!((bash.bash_command(), bash.display_value()), (Some("ls"), Some("ls (exit 0)")));
    assert_eq!((grep.title(), grep.status), ("Grep", ToolStatus::Running));

    conversation.apply_event(&idle(Some(acp::StopReason::Cancelled)));

    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Cancelled));
    let statuses: Vec<_> = sub_agent(&conversation).tool_calls().map(|call| call.status).collect();
    assert_eq!(statuses, [ToolStatus::Success, ToolStatus::Cancelled]);
}

#[test]
fn sub_agent_sessions_are_tracked_once_announced_under_a_known_tool() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&sub_agent_started("spawn", "orphan"));
    assert!(!tracks(&conversation, "orphan"), "a sub-agent of an unknown tool is not tracked");

    conversation.apply_event(&tool_update(spawn()));
    conversation.apply_event(&sub_agent_started("spawn", "child"));
    assert!(tracks(&conversation, "child"));
    assert_eq!(sub_agent(&conversation).title(), Some("explorer"));

    let finished = conversation.apply_event(&idle_notification("child", Some(acp::StopReason::EndTurn)).into());
    assert_eq!(finished, None, "a sub-agent going idle does not finish the parent's turn");
    assert_eq!(conversation.turn(), TurnPhase::Running);
    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Success));
}

#[test]
fn sub_agent_stop_reasons_set_its_status() {
    for (stop_reason, expected) in [
        (acp::StopReason::EndTurn, ToolStatus::Success),
        (acp::StopReason::Cancelled, ToolStatus::Cancelled),
        (acp::ErrorStopReason::new().into(), ToolStatus::Failed),
        (acp::StopReason::Refusal, ToolStatus::Failed),
    ] {
        let mut conversation = with_sub_agent(spawn());
        conversation.apply_event(&announce("spawn", acp::SubagentUpdate::new("child").state(idle_state(stop_reason))));

        assert_eq!(sub_agent(&conversation).status(), Some(expected));
    }
}

#[test]
fn a_sub_agent_follows_its_latest_report_from_either_session() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&tool_update(spawn()));
    conversation.apply_event(&announce("spawn", acp::SubagentUpdate::new("child").state(idle_state(None))));
    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Success), "a child may be announced already idle");

    conversation.apply_event(&idle_notification("child", Some(acp::StopReason::Cancelled)).into());
    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Cancelled));

    let patch = acp::SubagentUpdate::new("child").title("reviewer").state(running_state());
    conversation.apply_event(&update(acp::SessionUpdate::SubagentUpdate(patch)));
    let agent = sub_agent(&conversation);
    assert_eq!(
        (agent.title(), agent.status()),
        (Some("reviewer"), Some(ToolStatus::Running)),
        "only the first names its tool"
    );
}

#[test]
fn a_withdrawn_sub_agent_report_leaves_its_work_unconfirmed() {
    let mut conversation = with_sub_agent(spawn());
    conversation.apply_event(&announce("spawn", acp::SubagentUpdate::new("child").state(idle_state(None))));

    conversation.apply_event(&announce("spawn", acp::SubagentUpdate::new("child").state(MaybeUndefined::Null)));

    assert_eq!(sub_agent(&conversation).status(), None, "a cleared report is not a finished outcome");
}

#[test]
fn a_sub_agent_in_an_unknown_state_is_not_presented_as_running() {
    let mut conversation = with_sub_agent(background_spawn());

    conversation.apply_event(&on_session(
        "child",
        acp::SessionUpdate::StateUpdate(acp::StateUpdate::Unknown(acp::UnknownStateUpdate::new())),
    ));

    assert_eq!(sub_agent(&conversation).status(), None);
    assert!(!conversation.any_running());
}

#[test]
fn losing_the_connection_unconfirms_running_sub_agents() {
    let mut conversation = with_sub_agent(background_spawn());
    conversation.apply_event(&idle(Some(acp::StopReason::EndTurn)));

    conversation.apply_event(&AcpEvent::ConnectionClosed);

    assert_eq!(sub_agent(&conversation).status(), None);
    assert!(!conversation.any_running());
    assert_eq!(conversation.items()[0].state(), ItemState::Open, "a fresh report after reconnecting may still land");
}

#[test]
fn resuming_unconfirms_replayed_sub_agent_work_until_a_fresh_report() {
    let mut conversation = with_sub_agent(background_spawn());
    conversation.apply_event(&sub_agent_started("spawn", "finished"));
    conversation.apply_event(&idle_notification("finished", Some(acp::StopReason::EndTurn)).into());
    conversation.apply_event(&idle(Some(acp::StopReason::EndTurn)));

    conversation.unconfirm_sub_agents();

    let statuses: Vec<_> = tool(&conversation, 0).sub_agents.iter().map(SubAgent::status).collect();
    assert_eq!(statuses, [None, Some(ToolStatus::Success)], "recorded outcomes stay visible");
    assert_eq!(conversation.items()[0].state(), ItemState::Open);

    conversation.apply_event(&running_notification("child").into());
    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Running));
}

#[test]
fn a_sub_agent_announced_without_state_keeps_its_spawn_open_past_the_turn() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&tool_update(background_spawn()));
    conversation.apply_event(&announce("spawn", acp::SubagentUpdate::new("child")));

    conversation.apply_event(&idle(Some(acp::StopReason::EndTurn)));

    assert_eq!(sub_agent(&conversation).status(), None, "announcing a child does not imply it is running");
    assert_eq!(conversation.items()[0].state(), ItemState::Open);
}

#[test]
fn events_for_other_sessions_are_ignored() {
    let mut conversation = conversation();
    let revision = conversation.revision();

    conversation.apply_event(&agent_message_on("other", "a", "elsewhere"));
    conversation.apply_event(&running_notification("other").into());

    assert_eq!((conversation.items(), conversation.turn()), ([].as_slice(), TurnPhase::Idle));
    assert_eq!(conversation.revision(), revision);
}

#[test]
fn reset_starts_over_as_another_sessions_conversation() {
    let mut conversation = conversation();
    conversation.apply_event(&agent_message("a", "before"));

    conversation.reset("next");
    conversation.apply_event(&agent_message("b", "stale"));
    conversation.apply_event(&agent_message_on("next", "c", "after"));

    assert_eq!(conversation.session_id(), &acp::SessionId::new("next"));
    assert_eq!(conversation.items().len(), 1);
    assert_eq!(text(&conversation, 0), "after");
}

#[test]
fn a_sub_agent_repeating_its_idle_state_changes_nothing() {
    let mut conversation = with_sub_agent(background_spawn());
    conversation.apply_event(&sub_agent_started("spawn", "second"));

    let announced_idle =
        announce("spawn", acp::SubagentUpdate::new("child").state(idle_state(acp::StopReason::EndTurn)));
    assert!(changes(&mut conversation, &announced_idle));
    assert!(!changes(&mut conversation, &idle_notification("child", Some(acp::StopReason::EndTurn)).into()));
    assert!(!changes(&mut conversation, &announced_idle));

    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Success));
    assert_eq!(conversation.items()[0].state(), ItemState::Open, "the second sub-agent keeps the spawn open");
}

#[test]
fn nested_sub_agents_attach_to_the_tool_call_that_spawned_them() {
    let mut conversation = with_sub_agent(spawn());
    conversation.apply_event(&sub_agent_tool_update("child", acp::ToolCallUpdate::new("child-spawn")));
    conversation.apply_event(
        &announced("child", "child-spawn", acp::SubagentUpdate::new("grandchild").state(running_state())).into(),
    );
    conversation.apply_event(&sub_agent_tool_update("grandchild", acp::ToolCallUpdate::new("grep").title("Grep")));

    let grandchild = &first_tool_call(sub_agent(&conversation)).sub_agents[0];
    assert_eq!(*grandchild.session_id(), acp::SessionId::new("grandchild"));
    assert_eq!(first_tool_call(grandchild).title(), "Grep");
    assert!(tracks(&conversation, "grandchild"));

    conversation
        .apply_event(&announced("child", "unknown", acp::SubagentUpdate::new("orphan").state(running_state())).into());
    assert!(!tracks(&conversation, "orphan"), "a nested sub-agent of an unknown tool is not tracked");
}

#[test]
fn a_sub_agent_keeps_its_own_transcript() {
    let mut conversation = with_sub_agent(spawn());
    conversation.apply_event(&on_session(
        "child",
        acp::SessionUpdate::SessionMessage(
            acp::SessionMessage::new("task")
                .sender_session_id(acp::SessionId::new("session"))
                .content(vec!["Find the bug".into()]),
        ),
    ));
    conversation.apply_event(&on_session(
        "child",
        acp::SessionUpdate::AgentMessage(acp::AgentMessage::new("reply").content(vec!["Found it".into()])),
    ));
    conversation.apply_event(&sub_agent_tool_call("child", "grep"));

    let items = sub_agent(&conversation).items();
    let transcript: Vec<_> = items.iter().map(|item| item.text().map(std::borrow::Cow::into_owned)).collect();
    assert_eq!(transcript, [Some("Find the bug".to_string()), Some("Found it".to_string()), None]);
    assert_eq!(conversation.items().len(), 1, "the sub-agent's transcript stays out of the parent's");
}

#[test]
fn a_sub_agent_context_clear_leaves_the_parent_alone() {
    let mut conversation = with_sub_agent(spawn());
    conversation.apply_event(&sub_agent_tool_call("child", "grep"));

    conversation.apply_event(&AcpEvent::ContextCleared(ContextClearedParams { session_id: "child".into() }));
    assert_eq!(conversation.items().len(), 1);
    assert_eq!(sub_agent(&conversation).items(), [], "the child's own transcript is cleared");
    assert!(tracks(&conversation, "child"));

    conversation.apply_event(&AcpEvent::ContextCleared(ContextClearedParams { session_id: "session".into() }));
    assert_eq!(conversation.items(), []);
    assert!(!tracks(&conversation, "child"));
}

#[test]
fn background_sub_agents_outlive_the_turn_that_spawned_them() {
    let mut conversation = with_sub_agent(background_spawn());
    conversation.apply_event(&idle(Some(acp::StopReason::EndTurn)));

    assert_eq!(conversation.items()[0].state(), ItemState::Open, "the spawn stays live for its running sub-agent");
    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Running));

    conversation.apply_event(&sub_agent_tool_call("child", "grep"));
    conversation.apply_event(&idle_notification("child", Some(acp::StopReason::EndTurn)).into());
    conversation.apply_event(&sub_agent_tool_update(
        "child",
        acp::ToolCallUpdate::new("grep").status(acp::ToolCallStatus::Completed),
    ));

    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Success));
    assert_eq!(sub_agent(&conversation).tool_calls().count(), 1);
    assert_eq!(conversation.items()[0].state(), ItemState::Sealed);
}

#[test]
fn cancelling_while_idle_stops_background_sub_agents() {
    let mut conversation = with_sub_agent(background_spawn());
    conversation.apply_event(&idle(Some(acp::StopReason::EndTurn)));

    let finished = conversation.apply_event(&idle(Some(acp::StopReason::Cancelled)));

    assert_eq!(finished, None, "no turn was running");
    assert_eq!(sub_agent(&conversation).status(), Some(ToolStatus::Cancelled));
    assert_eq!(conversation.items()[0].state(), ItemState::Sealed);
}

#[test]
fn a_running_state_while_idle_starts_a_turn_that_thinks_until_it_responds() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    assert_eq!(conversation.turn(), TurnPhase::Running);
    assert_eq!(conversation.activity(), Activity::Thinking);

    conversation.apply_event(&update(acp::SessionUpdate::StateUpdate(acp::StateUpdate::RequiresAction(
        acp::RequiresActionStateUpdate::new(),
    ))));
    conversation.apply_event(&running());
    assert_eq!(conversation.activity(), Activity::Responding, "running again resumes a turn already underway");

    assert!(conversation.apply_event(&idle(None)).is_some());
    assert!(conversation.turn().is_idle());
    assert_eq!(conversation.activity(), Activity::Idle);
}

#[test]
fn replayed_history_does_not_start_activity() {
    let mut conversation = conversation();
    conversation.apply_event(&idle(None));
    conversation.apply_event(&agent_message("history", "saved response"));
    conversation.apply_event(&agent_chunk("history", " tail"));

    assert!(conversation.turn().is_idle());
    assert_eq!(conversation.activity(), Activity::Idle);
    assert_eq!(text(&conversation, 0), "saved response tail");
}

#[test]
fn thoughts_are_items_keyed_by_message_id_and_drive_thinking() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&thought_chunk("thought", "first\n"));
    conversation.apply_event(&thought_chunk("thought", "second"));
    assert_eq!(conversation.activity(), Activity::Thinking);
    assert_eq!(conversation.items()[0].content(), &ConversationContent::Thought(vec!["first\nsecond".into()]));

    conversation.apply_event(&thought_chunk("next", "fresh"));
    assert_eq!(text(&conversation, 1), "fresh", "a new thought is a new item");

    conversation.apply_event(&agent_chunk("reply", "answer"));
    assert_eq!(conversation.activity(), Activity::Responding);
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool")));
    assert_eq!(conversation.activity(), Activity::Working);

    conversation.apply_event(&update(acp::SessionUpdate::AgentThought(
        acp::AgentThought::new("next").content(MaybeUndefined::Null),
    )));
    assert_eq!(text(&conversation, 1), "", "an upsert replaces the thought");
    conversation.apply_event(&update(acp::SessionUpdate::AgentThought(
        acp::AgentThought::new("unseen").content(MaybeUndefined::Null),
    )));
    assert_eq!(conversation.items().len(), 4, "clearing a thought that never arrived adds nothing");
}

#[test]
fn activity_after_the_turn_ends_is_ignored_until_the_next_turn() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("spawn").title("spawn_subagent")));
    conversation.apply_event(&idle(None));

    conversation.apply_event(&thought_chunk("late", "stray thought"));
    conversation.apply_event(&sub_agent_started("spawn", "late-child"));
    conversation.apply_event(&compaction(acp::CompactionStatus::InProgress));

    assert_eq!(conversation.activity(), Activity::Idle);
    assert_eq!(tool(&conversation, 0).sub_agents, []);

    conversation.apply_event(&running());
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("next-spawn").title("spawn_subagent")));
    conversation.apply_event(&sub_agent_started("next-spawn", "child"));
    conversation.apply_event(&sub_agent_tool_call("child", "call"));
    assert_eq!(tool(&conversation, 0).sub_agents, [], "a sealed tool never changes");
    let next_spawn = tool(&conversation, conversation.items().len() - 1);
    assert_eq!(first_tool_call(&next_spawn.sub_agents[0]).id(), "call");
}

#[test]
fn compaction_is_tracked_until_it_completes_or_the_turn_ends() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&compaction(acp::CompactionStatus::InProgress));
    assert_eq!(conversation.activity(), Activity::Compacting);
    conversation.apply_event(&compaction(acp::CompactionStatus::Completed));
    assert_ne!(conversation.activity(), Activity::Compacting);

    conversation.apply_event(&compaction(acp::CompactionStatus::InProgress));
    conversation.apply_event(&idle(None));
    assert_ne!(conversation.activity(), Activity::Compacting);
}

#[test]
fn plan_and_usage_updates_replace_the_previous_ones() {
    let mut conversation = conversation();
    let entry =
        |content: &str| acp::PlanEntry::new(content, acp::PlanEntryPriority::Medium, acp::PlanEntryStatus::Pending);
    conversation.apply_event(&plan_notification("session", "a", vec![entry("one")]).into());
    conversation.apply_event(&plan_notification("session", "b", vec![entry("two")]).into());
    conversation.apply_event(&update(acp::SessionUpdate::PlanUpdate(acp::PlanUpdate::new(
        acp::PlanUpdateContent::Other(acp::OtherPlanUpdateContent::new("future", "b", BTreeMap::new())),
    ))));
    let plan = conversation.plan().unwrap();
    assert_eq!(plan.plan_id, acp::PlanId::new("b"));
    assert_eq!(plan.entries, [entry("two")]);

    conversation.apply_event(&update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(10, 100))));
    conversation.apply_event(&update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(20, 100))));
    assert_eq!(conversation.context_usage().map(|usage| usage.used), Some(20));
}

#[test]
fn clear_replaces_identity_and_resets_everything() {
    let mut conversation = conversation();
    let previous_id = conversation.id();
    conversation.apply_event(&running());
    conversation.apply_event(&user_message("user", "before"));
    conversation.apply_event(&update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(10, 100))));
    conversation.apply_event(&plan_notification("session", "a", vec![]).into());

    conversation.clear();

    assert_ne!(conversation.id(), previous_id);
    assert_eq!(conversation.items(), []);
    assert!(conversation.turn().is_idle(), "a running turn ends with the conversation");
    assert!(conversation.plan().is_none());
    assert!(conversation.context_usage().is_none());
}

#[test]
fn a_closed_connection_ends_the_turn() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&thought_chunk("thought", "pondering"));

    conversation.apply_event(&AcpEvent::ConnectionClosed);

    assert!(conversation.turn().is_idle());
    assert_eq!(conversation.activity(), Activity::Idle);
}

#[test]
fn notices_are_distinct_from_user_content() {
    let mut conversation = conversation();
    conversation.apply_event(&user_message("user", "prompt"));
    conversation.append_notice(NoticeLevel::Info, "Context cleared");

    assert!(matches!(conversation.items()[0].content(), ConversationContent::User(_)));
    assert_eq!(
        conversation.items()[1].content(),
        &ConversationContent::Notice(Notice { level: NoticeLevel::Info, text: "Context cleared".into() })
    );
}

#[test]
fn items_serialize_with_their_kind_and_protocol_content() {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&agent_chunk("reply", "hello"));
    conversation.apply_event(&tool_update(acp::ToolCallUpdate::new("tool").title("Bash")));
    conversation.apply_event(&idle(Some(acp::StopReason::Cancelled)));
    conversation.append_notice(NoticeLevel::Error, "note");

    let items = serde_json::to_value(conversation.items()).unwrap();

    assert_eq!(
        items,
        json!([
            {
                "id": 0, "messageId": "reply", "revision": 8, "state": "sealed",
                "kind": "assistant", "content": [{"type": "text", "text": "hello"}]
            },
            {
                "id": 1, "messageId": null, "revision": 8, "state": "sealed", "kind": "tool",
                "content": {
                    "status": "cancelled",
                    "subAgents": [],
                    "toolCall": {"toolCallId": "tool", "title": "Bash"}
                }
            },
            {"id": 2, "messageId": null, "revision": 9, "state": "sealed", "kind": "notice",
             "content": {"level": "error", "text": "note"}}
        ])
    );
    assert_eq!(serde_json::to_value(conversation.activity()).unwrap(), json!("idle"));
    assert_eq!(serde_json::to_value(conversation.turn()).unwrap(), json!("idle"));
}

#[test]
fn revision_advances_only_when_something_observable_changes() {
    let mut conversation = conversation();
    let commands = update(acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(vec![])));
    let usage = update(acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(10, 100)));

    assert!(!changes(&mut conversation, &commands));
    assert!(changes(&mut conversation, &agent_chunk("reply", "hello")));
    assert!(!changes(&mut conversation, &agent_chunk("reply", "")));
    assert!(changes(&mut conversation, &usage));
    assert!(!changes(&mut conversation, &usage));
    assert!(changes(&mut conversation, &running()));
    assert!(changes(&mut conversation, &tool_update(acp::ToolCallUpdate::new("tool"))));
    assert!(!changes(&mut conversation, &tool_update(acp::ToolCallUpdate::new("tool"))));

    let revision = conversation.revision();
    conversation.apply_event(&sub_agent_started("unknown", "child"));
    assert_eq!(conversation.revision(), revision, "a sub-agent of an unknown tool changes nothing");

    conversation.clear();
    assert!(conversation.revision() > revision, "a clear advances the revision with the new identity");
}

#[test]
fn item_revisions_come_from_the_conversation_clock() {
    let mut conversation = conversation();
    conversation.apply_event(&agent_chunk("first", "one"));
    conversation.apply_event(&agent_chunk("second", "two"));
    let seen = conversation.revision();

    conversation.apply_event(&agent_chunk("first", " more"));

    let changed: Vec<_> = conversation.items().iter().filter(|item| item.revision() > seen).collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].text().as_deref(), Some("one more"));
    assert_eq!(changed[0].revision(), conversation.revision());
}

fn changes(conversation: &mut Conversation, event: &AcpEvent) -> bool {
    let revision = conversation.revision();
    conversation.apply_event(event);
    conversation.revision() != revision
}

fn text(conversation: &Conversation, index: usize) -> String {
    conversation.items()[index].text().expect("a message or notice").into_owned()
}

fn tool(conversation: &Conversation, index: usize) -> &ToolCall {
    let ConversationContent::Tool(tool) = conversation.items()[index].content() else { panic!("tool item") };
    tool
}

fn agent_message(id: &str, text: &str) -> AcpEvent {
    agent_message_on("session", id, text)
}

fn agent_message_on(session: &str, id: &str, text: &str) -> AcpEvent {
    on_session(session, acp::SessionUpdate::AgentMessage(acp::AgentMessage::new(id).content(vec![text.into()])))
}

fn agent_chunk(id: &str, text: &str) -> AcpEvent {
    update(message_chunk(id, text))
}

fn thought_chunk(id: &str, text: &str) -> AcpEvent {
    update(testing::thought_chunk(id, text))
}

fn user_message(id: &str, text: &str) -> AcpEvent {
    update(testing::user_message(id, vec![text.into()]))
}

fn tool_update(tool_call: acp::ToolCallUpdate) -> AcpEvent {
    update(acp::SessionUpdate::ToolCallUpdate(tool_call))
}

fn tool_chunk(id: &str, content: acp::ToolCallContent) -> AcpEvent {
    update(acp::SessionUpdate::ToolCallContentChunk(acp::ToolCallContentChunk::new(id, content)))
}

fn compaction(status: acp::CompactionStatus) -> AcpEvent {
    update(acp::SessionUpdate::CompactionUpdate(acp::CompactionUpdate::new("compaction", status)))
}

fn running() -> AcpEvent {
    running_notification("session").into()
}

fn idle(stop_reason: Option<acp::StopReason>) -> AcpEvent {
    idle_notification("session", stop_reason).into()
}

fn update(update: acp::SessionUpdate) -> AcpEvent {
    on_session("session", update)
}

fn announce(spawning_tool_id: &str, update: acp::SubagentUpdate) -> AcpEvent {
    announced("session", spawning_tool_id, update.title("explorer")).into()
}

fn sub_agent_started(spawning_tool_id: &str, session: &str) -> AcpEvent {
    announce(spawning_tool_id, acp::SubagentUpdate::new(session).state(running_state()))
}

fn sub_agent_tool_call(session: &str, id: &str) -> AcpEvent {
    sub_agent_tool_update(session, acp::ToolCallUpdate::new(id.to_string()).title("Grep").name("coding__grep"))
}

fn first_tool_call(agent: &SubAgent) -> &ToolCall {
    agent.tool_calls().next().expect("the sub-agent has a tool call")
}

fn sub_agent(conversation: &Conversation) -> &SubAgent {
    &tool(conversation, 0).sub_agents[0]
}

fn conversation() -> Conversation {
    Conversation::new("session")
}

fn tracks(conversation: &Conversation, session: &str) -> bool {
    conversation.tracks(&session.into())
}

fn with_sub_agent(spawn: acp::ToolCallUpdate) -> Conversation {
    let mut conversation = conversation();
    conversation.apply_event(&running());
    conversation.apply_event(&tool_update(spawn));
    conversation.apply_event(&sub_agent_started("spawn", "child"));
    conversation
}

fn spawn() -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new("spawn").name("subagents__spawn_subagent")
}

fn background_spawn() -> acp::ToolCallUpdate {
    spawn().status(acp::ToolCallStatus::Completed)
}
