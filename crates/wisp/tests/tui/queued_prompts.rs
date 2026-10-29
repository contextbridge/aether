use super::support::*;

#[test]
fn a_prompt_sent_during_a_turn_is_queued_until_the_agent_inserts_it() {
    let mut ui = TestUi::with_dimensions(60, 20);
    ui.submit("first");
    ui.acp_event(text_chunk("first reply"));

    ui.submit("second");

    let sent: Vec<_> = std::iter::from_fn(|| ui.agent().prompt.try_recv().ok()).collect();
    assert_eq!(sent.iter().filter_map(prompt_text).collect::<Vec<_>>(), ["first", "second"]);
    assert!(ui.app().composer().is_empty());
    assert_eq!(queued_texts(&ui), ["second"]);
    ui.assert_viewport_contains("queued › second");
    assert_eq!(transcript(&ui), ["first", "first reply"]);

    ui.insert_queued_prompts();

    assert_eq!(queued_texts(&ui).len(), 0);
    ui.assert_viewport_not_contains("queued ›");
    assert_eq!(transcript(&ui), ["first", "first reply", "second"]);
    assert!(ui.app().core().waiting_for_response(), "the inserted prompt joins the running turn");
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

    ui.assert_viewport_contains("queued › line one …");
    ui.assert_viewport_not_contains("line two");
}

#[test]
fn prompts_rejected_by_a_cancel_return_to_the_composer_in_order() {
    let mut ui = TestUi::new();
    ui.submit("first");
    ui.submit("second");
    ui.submit("third");

    ui.key(key(KeyCode::Esc));
    assert!(ui.agent().cancel.try_recv().is_ok());
    ui.reject_queued_prompts(&Error::request_cancelled());
    ui.complete_prompt(acp::StopReason::Cancelled);

    assert_eq!(ui.app().composer().text(), "second\nthird");
    assert_eq!(queued_texts(&ui).len(), 0);
    assert!(!ui.app().core().waiting_for_response());
    assert_eq!(transcript(&ui), ["first"], "cancelled prompts never join the conversation and need no notice");
}

#[test]
fn a_rejected_prompt_keeps_its_file_mentions() {
    let mut ui = TestUiBuilder::new().file("context.txt", b"attached context").build();
    ui.submit("first");
    ui.key(key(KeyCode::Char('@')));
    ui.key(key(KeyCode::Char('c')));
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Enter));

    ui.reject_queued_prompts(&Error::request_cancelled());

    assert_eq!(ui.app().composer().text(), "@context.txt ");
    let mentions = ui.app().composer().selected_mentions();
    assert_eq!(mentions.iter().map(|mention| mention.display_name.as_str()).collect::<Vec<_>>(), ["context.txt"]);
}

fn queued_texts(ui: &TestUi) -> Vec<&str> {
    ui.app().core().queued_prompts().iter().map(|prompt| prompt.submission.text.as_str()).collect()
}

fn transcript(ui: &TestUi) -> Vec<String> {
    acp_utils::testing::texts(ui.app().core().conversation())
}
