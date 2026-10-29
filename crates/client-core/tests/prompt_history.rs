use client_core::composer::prompt_history::PromptHistory;

#[test]
fn submitting_ends_recall_so_the_next_recall_starts_from_the_newest_prompt() {
    let mut history = PromptHistory::default();
    history.push("first");
    history.push("second");
    assert_eq!(history.older("draft"), Some("second"));
    assert_eq!(history.older("draft"), Some("first"));

    history.push("first");

    assert!(!history.is_recalling());
    assert_eq!(history.older(""), Some("first"));
    assert_eq!(history.older(""), Some("second"));
}
