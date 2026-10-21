use aether_desktop::cli::Cli;
use clap::Parser;

#[test]
fn agent_defaults_to_aether_acp() {
    let cli = Cli::parse_from(["aether-desktop"]);
    assert_eq!(cli.agent, "aether acp");
}

#[test]
fn agent_accepts_a_full_command_line() {
    let cli = Cli::parse_from(["aether-desktop", "--agent", "cargo run -p aether-agent-cli acp"]);
    assert_eq!(cli.agent, "cargo run -p aether-agent-cli acp");
}
