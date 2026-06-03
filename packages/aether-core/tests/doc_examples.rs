//! Ensures the JSON examples embedded in config doc comments stay valid.
//! These `.md` files are rendered into the auto-generated settings reference.

use aether_core::agent_spec::ToolFilter;
use aether_core::core::PromptSource;
use aether_doctest::assert_json_examples;

#[test]
fn prompt_source_examples_parse() {
    assert_json_examples::<PromptSource>("prompt_source.md", include_str!("../src/docs/prompt_source.md"));
}

#[test]
fn tool_filter_examples_parse() {
    assert_json_examples::<ToolFilter>("tool_filter.md", include_str!("../src/docs/tool_filter.md"));
}
