# aether-project

Project-local settings and agent catalog resolution for the Aether AI agent framework. Reads `.aether/settings.json` to discover agents, prompts, and MCP server configurations.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Key Types](#key-types)
- [Usage](#usage)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Key Types

- **`AetherSettings`** -- Parsed project settings from `.aether/settings.json`
- **`AgentCatalog`** -- Resolved catalog of project agents with their prompts, models, and tool filters
- **`PromptCatalog`** -- Collection of project prompt files
- **`SettingsError`** -- Settings validation errors

## Usage

`.aether/settings.json` supports top-level `prompts` and `mcps` as typed defaults. An agent without local `prompts` inherits top-level `prompts`; an agent without local `mcps` inherits top-level `mcps`. Agent-local `prompts` or `mcps` replace the corresponding top-level defaults for that agent.

```rust,no_run
use aether_project::{AetherSettings, AgentCatalog};
use std::path::Path;

let project_root = Path::new(".");
let settings = AetherSettings::load_default(project_root).unwrap();
let catalog = if settings.agents.is_empty() {
    AgentCatalog::empty(project_root.to_path_buf())
} else {
    AgentCatalog::from_settings(project_root, settings).unwrap()
};

println!("Project root: {:?}", catalog.project_root());

for agent in catalog.all() {
    println!("Agent: {} (model: {})", agent.name, agent.model);
}
```

## License

MIT
