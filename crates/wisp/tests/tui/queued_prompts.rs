use super::support::*;
use std::borrow::Cow;
use wisp::command::TerminalCommand;

#[test]
fn a_prompt_sent_during_a_turn_is_queued_until_the_agent_inserts_it() {
    let mut ui = TestUi::with_dimensions(60, 20);
    ui.submit("first");
    ui.acp_event(text_chunk("first reply"));
    ui.take_commands();

    ui.submit("second");

    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::Prompt { text, .. }) if text == "second"));
    assert!(ui.app().composer().is_empty());
    assert_eq!(queued_texts(&ui), ["second"]);
    assert!(ui.viewport_text().contains("queued › second"), "{}", ui.viewport_text());
    assert_eq!(transcript(&ui), ["first", "first reply"]);

    ui.insert_queued_prompts();

    assert!(queued_texts(&ui).is_empty());
    assert!(!ui.viewport_text().contains("queued ›"));
    assert_eq!(transcript(&ui), ["first", "first reply", "second"]);
    assert!(ui.app().waiting_for_response(), "the inserted prompt joins the running turn");
}

#[test]
fn queued_prompts_show_in_order_and_summarize_the_overflow() {
    let mut ui = TestUi::with_dimensions(60, 20);
    ui.submit("first");
    for text in ["one", "two", "three", "four", "five"] {
        ui.submit(text);
    }

    let viewport = ui.viewport_text();

    let row = |needle: &str| viewport.lines().position(|line| line.contains(needle));
    assert!(row("queued › one") < row("queued › two") && row("queued › two") < row("queued › three"), "{viewport}");
    assert!(viewport.contains("+2 more queued"), "{viewport}");
    assert!(!viewport.contains("four"), "{viewport}");
}

#[test]
fn a_multi_line_queued_prompt_shows_its_first_line() {
    let mut ui = TestUi::with_dimensions(60, 20);
    ui.submit("first");
    ui.type_text("line one");
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    ui.type_text("line two");
    ui.key(key(KeyCode::Enter));

    let viewport = ui.viewport_text();

    assert!(viewport.contains("queued › line one …"), "{viewport}");
    assert!(!viewport.contains("line two"), "{viewport}");
}

#[test]
fn prompts_rejected_by_a_cancel_return_to_the_composer_in_order() {
    let mut ui = make_app();
    ui.submit("first");
    ui.submit("second");
    ui.submit("third");
    ui.take_commands();

    ui.key(key(KeyCode::Esc));
    assert!(matches!(ui.next_agent_command(), Some(AgentCommand::Cancel { .. })));
    ui.reject_queued_prompts(&PromptRejection::Cancelled);
    ui.complete_prompt(acp::StopReason::Cancelled);

    assert_eq!(ui.app().composer().text(), "second\nthird");
    assert!(queued_texts(&ui).is_empty());
    assert!(!ui.app().waiting_for_response());
    assert_eq!(transcript(&ui), ["first"], "cancelled prompts never join the conversation and need no notice");
}

#[test]
fn a_rejected_prompt_is_restored_after_the_draft_being_typed() {
    let mut ui = make_app();
    ui.submit("first");
    ui.submit("second");
    ui.type_text("draft");

    ui.reject_queued_prompts(&PromptRejection::Cancelled);

    assert_eq!(ui.app().composer().text(), "draft\nsecond");
}

#[test]
fn a_rejected_prompt_keeps_its_file_mentions() {
    let mut ui = make_app_in("/workspace".into());
    ui.executor_mut().filesystem_mut().write_file("/workspace/context.txt", b"attached context");
    ui.submit("first");
    ui.key(key(KeyCode::Char('@')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Char('c')));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));
    ui.settle_tasks();

    ui.reject_queued_prompts(&PromptRejection::Cancelled);

    assert_eq!(ui.app().composer().text(), "@context.txt ");
    let mentions = ui.app().composer().selected_mentions();
    assert_eq!(mentions.iter().map(|mention| mention.display_name.as_str()).collect::<Vec<_>>(), ["context.txt"]);
}

#[test]
fn a_prompt_the_agent_fails_returns_to_the_composer_with_a_notice() {
    let mut ui = make_app();
    ui.reject_prompts(PromptRejection::Failed("overloaded".into()));

    ui.submit("hello");

    assert_eq!(ui.app().composer().text(), "hello");
    assert!(!ui.app().waiting_for_response());
    assert_eq!(transcript(&ui), ["[wisp] Failed to send prompt: overloaded"]);
}

#[test]
fn the_bell_waits_until_no_prompt_is_queued() {
    let mut ui = make_app();
    ui.submit("first");
    ui.submit("second");
    ui.take_commands();

    ui.complete_prompt(acp::StopReason::EndTurn);
    assert_eq!(bells(&mut ui), 0, "the agent still owes the queued prompt a turn");

    ui.complete_prompt(acp::StopReason::EndTurn);
    assert_eq!(bells(&mut ui), 1);
}

fn queued_texts(ui: &TestUi) -> Vec<&str> {
    ui.app().queued_prompts().iter().map(|prompt| prompt.submission.text.as_str()).collect()
}

fn transcript(ui: &TestUi) -> Vec<String> {
    ui.app().conversation_items().iter().filter_map(|item| item.text().map(Cow::into_owned)).collect()
}

fn bells(ui: &mut TestUi) -> usize {
    ui.take_commands().iter().filter(|command| matches!(command, Command::Terminal(TerminalCommand::RingBell))).count()
}
