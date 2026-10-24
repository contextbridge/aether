use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use client_core::composer::prompt_search::PromptSearchState;

#[test]
fn setting_the_query_replaces_it_and_waits_for_results() {
    let mut state = PromptSearchState::default();
    state.push_char('x');

    assert_eq!(state.set_query("fix build"), "fix build");

    assert_eq!(state.query(), "fix build");
    assert!(state.is_loading());
}

#[test]
fn a_blank_query_waits_for_nothing() {
    let mut state = PromptSearchState::default();
    state.set_query("fix");

    state.set_query("  ");

    assert!(state.is_blank());
    assert!(!state.is_loading());
}

#[test]
fn results_for_a_query_set_since_are_stale() {
    let mut state = PromptSearchState::default();
    state.set_query("fi");
    state.set_query("fix");

    assert!(!state.on_results(response("fi", &["fit"])));
    assert!(state.on_results(response("fix", &["fix it"])));

    assert_eq!(prompts(&state), ["fix it"]);
    assert!(!state.is_loading());
}

#[test]
fn a_blank_query_has_no_results() {
    let mut state = PromptSearchState::default();
    state.set_query("fix");
    state.on_results(response("fix", &["fix it"]));

    state.set_query("");

    assert_eq!(prompts(&state), Vec::<&str>::new());
}

#[test]
fn a_failed_search_for_the_query_drops_the_results_it_replaces() {
    let mut state = PromptSearchState::default();
    state.set_query("fix");
    state.on_results(response("fix", &["fix it"]));
    state.push_char('e');

    assert!(!state.on_failed("fix", "stale".to_owned()));
    assert!(state.on_failed("fixe", "agent unavailable".to_owned()));

    assert_eq!(prompts(&state), Vec::<&str>::new());
    assert_eq!(state.error(), Some("agent unavailable"));
}

fn prompts(state: &PromptSearchState) -> Vec<&str> {
    state.results().iter().map(|result| result.prompt.as_str()).collect()
}

fn response(query: &str, prompts: &[&str]) -> PromptSearchResponse {
    let results = prompts
        .iter()
        .map(|prompt| PromptSearchResult {
            session_id: "session".to_owned(),
            cwd: "/".into(),
            session_created_at: "2026-01-01T00:00:00Z".to_owned(),
            prompt: (*prompt).to_owned(),
            match_start: 0,
            match_end: query.len(),
        })
        .collect();
    PromptSearchResponse { query: query.to_owned(), results, truncated: false }
}
