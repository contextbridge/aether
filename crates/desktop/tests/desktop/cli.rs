use aether_desktop::agents::AgentCommand;
use aether_desktop::cli::Cli;
use clap::Parser;

#[test]
fn the_builtin_agent_defaults_to_aether_acp() {
    let cli = Cli::parse_from(["aether-desktop"]);
    assert_eq!(cli.agent, AgentCommand::new("aether").args(["acp"]));
}

#[test]
fn the_builtin_agent_accepts_a_full_command_line() {
    let cli = Cli::parse_from(["aether-desktop", "--agent", "cargo run -p aether-agent-cli acp"]);
    assert_eq!(cli.agent, AgentCommand::new("cargo").args(["run", "-p", "aether-agent-cli", "acp"]));
}

#[test]
fn an_agent_command_line_that_cannot_be_split_is_rejected() {
    assert!(Cli::try_parse_from(["aether-desktop", "--agent", "aether 'acp"]).is_err());
}
