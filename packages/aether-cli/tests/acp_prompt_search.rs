use acp_utils::notifications::{PromptSearchParams, PromptSearchResponse};
use aether_cli::acp::testing::AcpTestHarness;
use std::future::Future;
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn prompt_search_request_finds_user_prompt_history() {
    with_harness(|harness| async move {
        harness.append_stored_session("s1", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("s1", "hello world");

        let response = search(&harness, "hello").await;

        assert_eq!(response.results.len(), 1);
        let hit = &response.results[0];
        assert_eq!(hit.prompt, "hello world");
        assert_eq!(&hit.prompt[hit.match_start..hit.match_end], "hello");
        assert_eq!(hit.session_id, "s1");
        assert!(!response.truncated);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_search_request_searches_user_text_only() {
    with_harness(|harness| async move {
        harness.append_stored_session("agent", "2026-05-01T00:00:00Z");
        harness.append_stored_agent_text("agent", "hello from agent");
        harness.append_stored_session("media", "2026-05-02T00:00:00Z");
        harness.append_stored_user_blocks(
            "media",
            vec![llm::ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() }],
        );
        harness.append_stored_session("multi", "2026-05-03T00:00:00Z");
        harness.append_stored_user_blocks(
            "multi",
            vec![llm::ContentBlock::text("first block"), llm::ContentBlock::text("second block")],
        );

        assert!(search(&harness, "agent").await.results.is_empty());
        assert!(search(&harness, "aW1n").await.results.is_empty());

        let multi = search(&harness, "second").await;
        assert_eq!(multi.results.len(), 1);
        assert_eq!(multi.results[0].prompt, "first block\nsecond block");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_search_request_uses_literal_smart_case_unicode_matching() {
    with_harness(|harness| async move {
        harness.append_stored_session("lower", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("lower", "hello world");
        harness.append_stored_session("upper", "2026-05-02T00:00:00Z");
        harness.append_stored_prompt("upper", "HELLO world");
        harness.append_stored_session("literal", "2026-05-03T00:00:00Z");
        harness.append_stored_prompt("literal", "hello.world");
        harness.append_stored_session("unicode", "2026-05-04T00:00:00Z");
        harness.append_stored_prompt("unicode", "café hello");

        let literal = search(&harness, "hello.world").await;
        assert_eq!(literal.results.len(), 1);
        assert_eq!(literal.results[0].prompt, "hello.world");

        let lower = search(&harness, "hello").await;
        assert!(lower.results.iter().any(|hit| hit.prompt == "hello world"));
        assert!(lower.results.iter().any(|hit| hit.prompt == "HELLO world"));

        let upper = search(&harness, "Hello").await;
        assert!(upper.results.is_empty());

        let unicode = search(&harness, "fé").await;
        assert_eq!(unicode.results.len(), 1);
        let hit = &unicode.results[0];
        assert_eq!(&hit.prompt[hit.match_start..hit.match_end], "fé");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_search_request_limits_results_and_prompt_history() {
    with_harness(|harness| async move {
        harness.append_stored_session("s1", "2026-06-01T00:00:00Z");
        for i in 0..105 {
            harness.append_stored_prompt("s1", &format!("match #{i}"));
        }

        let matches = search(&harness, "match").await;
        assert_eq!(matches.results.len(), 20);
        assert!(matches.truncated);
        assert!(matches.results.iter().any(|hit| hit.prompt == "match #104"));

        let old = search(&harness, "match #0").await;
        assert!(old.results.is_empty(), "oldest prompt outside the history window should be ignored");
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn prompt_search_request_with_empty_query_returns_no_results() {
    with_harness(|harness| async move {
        harness.append_stored_session("s1", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("s1", "cached alpha");

        let response = search(&harness, "").await;
        assert!(response.results.is_empty());
        assert!(!response.truncated);
    })
    .await;
}

async fn with_harness<F, Fut>(body: F)
where
    F: FnOnce(AcpTestHarness) -> Fut,
    Fut: Future<Output = ()>,
{
    LocalSet::new()
        .run_until(async move {
            let harness = AcpTestHarness::start().await;
            body(harness).await;
        })
        .await;
}

async fn search(harness: &AcpTestHarness, query: &str) -> PromptSearchResponse {
    harness
        .client_cx
        .send_request(PromptSearchParams { query: query.to_string(), limit: None })
        .block_task()
        .await
        .expect("prompt search succeeds")
}
