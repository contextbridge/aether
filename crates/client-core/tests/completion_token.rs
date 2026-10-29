use client_core::composer::completion_token::{CompletionToken, Trigger};

#[test]
fn a_slash_at_the_start_of_the_prompt_names_a_command() {
    let token = CompletionToken::at("/rev", 4).unwrap();

    assert_eq!(token, CompletionToken { trigger: Trigger::Command, range: 0..4 });
    assert_eq!(token.query("/rev"), "rev");
}

#[test]
fn a_slash_later_in_the_prompt_is_not_a_command() {
    assert_eq!(CompletionToken::at("fix /rev", 8), None);
}

#[test]
fn a_command_may_contain_slashes() {
    let token = CompletionToken::at("/git/log", 8).unwrap();

    assert_eq!(token.query("/git/log"), "git/log");
}

#[test]
fn an_at_sign_that_starts_a_word_mentions_a_file() {
    let token = CompletionToken::at("fix @src/ma", 11).unwrap();

    assert_eq!(token, CompletionToken { trigger: Trigger::File, range: 4..11 });
    assert_eq!(token.query("fix @src/ma"), "src/ma");
}

#[test]
fn an_at_sign_inside_a_word_is_not_a_mention() {
    assert_eq!(CompletionToken::at("me@example", 10), None);
}

#[test]
fn a_mention_ends_at_whitespace() {
    assert_eq!(CompletionToken::at("@src fix", 8), None);
}

#[test]
fn only_the_text_before_the_cursor_counts() {
    let token = CompletionToken::at("@src fix", 4).unwrap();

    assert_eq!(token.query("@src fix"), "src");
}

#[test]
fn a_cursor_off_a_char_boundary_has_no_token() {
    assert_eq!(CompletionToken::at("@é", 2), None);
}
