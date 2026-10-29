use acp_utils::client::AcpClientError;
use acp_utils::notifications::PromptSearchResponse;
use acp_utils::testing::{prompt_search_hit, prompt_search_response};
use agent_client_protocol::Error;
use client_core::command::{AgentCommand, Command, CommandResult};
use client_core::composer::prompt_search::PromptSearchStatus;
use client_core::session_core::{Effect, SessionCore};
use client_core::testing::SessionCoreConfigBuilder;

#[test]
fn a_query_is_sent_to_the_agent_and_waits_for_its_answer() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().prompt_search().build());

    core.search_prompts("fix build");

    assert_eq!(searched(&mut core), ["fix build"]);
    assert_eq!(core.prompt_search().query(), "fix build");
    assert!(matches!(core.prompt_search().status(), PromptSearchStatus::Searching));
}

#[test]
fn a_blank_query_is_not_sent_and_has_no_results() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().prompt_search().build());
    core.search_prompts("fix");
    answer(&mut core, "fix", Ok(response("fix", &["fix it"])));
    core.take_effects();

    core.search_prompts("  ");

    assert_eq!(searched(&mut core), Vec::<String>::new());
    assert_eq!(prompts(&core), Vec::<&str>::new());
    assert!(matches!(core.prompt_search().status(), PromptSearchStatus::Blank));
}

#[test]
fn an_answer_to_a_query_edited_away_from_is_dropped() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().prompt_search().build());
    core.search_prompts("fi");
    core.search_prompts("fix");
    core.take_effects();

    answer(&mut core, "fi", Ok(response("fi", &["fit"])));
    assert!(!updated(&mut core));
    answer(&mut core, "fix", Ok(response("fix", &["fix it"])));
    assert!(updated(&mut core));

    assert_eq!(prompts(&core), ["fix it"]);
    assert!(matches!(core.prompt_search().status(), PromptSearchStatus::Results));
}

#[test]
fn a_failed_search_for_the_query_drops_the_results_it_replaces() {
    let mut core = SessionCore::new(SessionCoreConfigBuilder::new().prompt_search().build());
    core.search_prompts("fix");
    answer(&mut core, "fix", Ok(response("fix", &["fix it"])));
    core.search_prompts("fixe");

    answer(&mut core, "fix", Err(AcpClientError::Protocol(Error::internal_error())));
    assert_eq!(prompts(&core), ["fix it"]);
    answer(&mut core, "fixe", Err(AcpClientError::Protocol(Error::internal_error())));

    assert_eq!(prompts(&core), Vec::<&str>::new());
    assert!(matches!(core.prompt_search().status(), PromptSearchStatus::Failed(_)));
}

fn answer(core: &mut SessionCore, query: &str, result: Result<PromptSearchResponse, AcpClientError>) {
    core.on_command_result(CommandResult::PromptSearchResults { query: query.to_owned(), result });
}

fn searched(core: &mut SessionCore) -> Vec<String> {
    core.take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(Command::Agent(AgentCommand::SearchPrompts(params))) => Some(params.query),
            _ => None,
        })
        .collect()
}

fn updated(core: &mut SessionCore) -> bool {
    core.take_effects().iter().any(|effect| matches!(effect, Effect::PromptSearchUpdated))
}

fn prompts(core: &SessionCore) -> Vec<&str> {
    core.prompt_search().results().iter().map(|result| result.prompt.as_str()).collect()
}

fn response(query: &str, prompts: &[&str]) -> PromptSearchResponse {
    prompt_search_response(query, prompts.iter().map(|prompt| prompt_search_hit(prompt, 0, query.len())).collect())
}
