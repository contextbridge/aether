MCP server for unified prompt artifacts -- skills, slash commands, and rules.

This server loads prompt artifacts from explicitly configured directories. It serves Agent Skills through the MCP skills extension (`io.modelcontextprotocol/skills`), and implements user invocable slash commands via the MCP prompts.

# Construction

```rust,ignore
use clap::Parser;
use mcp_servers::{SkillsMcp, SkillsMcpArgs};

let server = SkillsMcp::new(
    "/my/project",
    &["/my/project/.aether/skills".into(), "/my/project/.claude/rules".into()],
);

let args = SkillsMcpArgs::try_parse_from(["--dir", ".aether/skills", "--dir", ".claude/rules"])?;
let server = SkillsMcp::from_args(args, "/my/project".as_ref());
```

# Skills

Each directory under a `--dir` that contains a `SKILL.md` is served at `skill://<skill>/SKILL.md`
through `skills/list`, `skills/get`, and `resources/read`. The server has no tools.

- When two directories hold the same skill name, the later `--dir` wins.
- Skills marked `agent-invocable: false` are not served.
- Symlinks are not followed. A skill containing a symlink is not served.

# Prompts

The server also implements the MCP prompts protocol, listing available user-invocable prompts.

# See also

- [`CodingMcp`](crate::CodingMcp) -- Uses prompt catalogs for read-triggered rules via [`PromptRuleMatcher`](crate::coding::prompt_rule_matcher::PromptRuleMatcher).
