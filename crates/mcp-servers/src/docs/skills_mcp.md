MCP server for unified prompt artifacts -- skills, slash commands, and rules.

This server loads prompt artifacts from explicitly configured directories. It exposes tools to discover and load Skills, and implements user invocable slash commands via the MCP prompts.

# Construction

```rust,ignore
use mcp_servers::SkillsMcp;

let server = SkillsMcp::new(
    &["/my/project/.aether/skills".into(), "/my/project/.claude/rules".into()],
    "/my/project/.aether/notes".into(),
);

// From CLI args
let server = SkillsMcp::from_args(vec![
    "--dir".into(), "/my/project/.aether/skills".into(),
    "--dir".into(), "/my/project/.claude/rules".into(),
    "--notes-dir".into(), "/my/project/.aether/notes".into(),
]).unwrap();
```

# Tools provided

- **`list_skills`** -- Discover available `agent-invocable` skills (lightweight metadata only).
- **`get_skills`** -- Load skill files by exact name returned from `list_skills`.
- **`save_note`** -- Save a note (creates or updates a file in the notes directory).
- **`search_notes`** -- Search saved notes by keyword.

Recommended flow: `search_notes` -> `list_skills` -> `get_skills`.

# Prompts

The server also implements the MCP prompts protocol, listing available user-invocable prompts.

# See also

- [`CodingMcp`](crate::CodingMcp) -- Uses prompt catalogs for read-triggered rules via [`PromptRuleMatcher`](crate::coding::prompt_rule_matcher::PromptRuleMatcher).
