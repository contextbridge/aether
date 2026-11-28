use clap::{CommandFactory, Parser};
use foxfire_tui::cli::Cli;
use foxfire_tui::{DEFAULT_LOG_DIR, client_info};

#[test]
fn the_terminal_client_identifies_itself_as_foxfire() {
    assert_eq!(Cli::command().get_name(), "foxfire");
    assert_eq!(client_info().name, "foxfire");
    assert_eq!(DEFAULT_LOG_DIR, "/tmp/foxfire-logs");
}

#[test]
fn the_default_agent_is_still_the_aether_server() {
    assert_eq!(Cli::parse_from(["foxfire"]).agent, "aether acp");
}
