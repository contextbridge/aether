//! Ensures the JSON examples embedded in the settings doc comments stay valid.
//! The same `.md` files are rendered into the auto-generated settings reference,
//! so a passing test guarantees the published examples actually parse.

use aether_doctest::assert_json_examples;
use aether_project::{AetherSettings, AgentConfig, McpSourceSpec};

#[test]
fn aether_settings_examples_parse() {
    assert_json_examples::<AetherSettings>("aether_settings.md", include_str!("../src/docs/aether_settings.md"));
}

#[test]
fn agent_config_examples_parse() {
    assert_json_examples::<AgentConfig>("agent_config.md", include_str!("../src/docs/agent_config.md"));
}

#[test]
fn mcp_source_spec_examples_parse() {
    assert_json_examples::<McpSourceSpec>("mcp_source_spec.md", include_str!("../src/docs/mcp_source_spec.md"));
}
