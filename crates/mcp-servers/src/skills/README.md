# SkillsMcp

Slash commands and reusable skill prompts. Skills teach the agent domain-specific knowledge; commands trigger multi-step workflows.

**Flags:** `--dir <path>` (repeatable prompt directories)

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Directory Structure](#directory-structure)
- [Skills over MCP](#skills-over-mcp)
  - [Rules](#rules)
- [Writing a Command](#writing-a-command)
- [Writing a Skill](#writing-a-skill)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Directory Structure

```text
~/.aether/
├── commands/           # Slash commands (markdown files)
│   ├── commit.md
│   ├── review-branch.md
│   └── ...
└── skills/             # Skill directories (multi-file support)
    ├── rust/
    │   ├── SKILL.md        # Primary skill content (required)
    │   ├── traits.md       # Auxiliary file
    │   └── references/
    │       └── REF.md      # Nested auxiliary file
    └── ...
```

## Skills over MCP

Skills are served through the [MCP skills extension](https://modelcontextprotocol.io/extensions/skills/overview) (`io.modelcontextprotocol/skills`), so any host that supports the extension can use them. The server has no tools.

| Method | Description |
|--------|-------------|
| `skills/list` | Every `agent-invocable` skill, with its frontmatter and a manifest (`uri`, `digest`, `size`) of each file it ships. Re-reads the directories, so edits are picked up. |
| `skills/get` | The entry for one skill by its `SKILL.md` URI. |
| `resources/read` | The bytes of one skill file, exactly as on disk. |

A skill is served at `skill://<skill>/SKILL.md`. For example `--dir .aether/skills` serves `.aether/skills/rust` at `skill://rust/SKILL.md`.

Commands are exposed as **MCP Prompts** (via `list_prompts` / `get_prompt`). This is what powers `/slash-commands` in the TUI.

### Rules

- A skill is a directory with a `SKILL.md`. Its frontmatter must have a `name` equal to the directory name (lowercase letters, numbers, and hyphens) and a `description`. A flat `.md` file is not a skill; it can still be a slash command or a read-triggered rule.
- When two `--dir`s hold a skill of the same name, the later one wins, as for slash commands and rules.
- Skill content is served verbatim. Shell interpolation (`` !`cmd` ``) runs only in slash commands.
- Symlinks are not followed. A symlinked skill directory, or a skill containing a symlink, is not served.
- Hidden files (names starting with `.`) are not served. A skill may ship at most 512 files and 16 MiB.

## Writing a Command

Create a markdown file in `commands/` with YAML frontmatter:

```markdown
---
description: Generate a commit message for staged changes
argument-hint: [optional args]
allowed-tools:
  - Bash
  - Read
  - Grep
---

Your prompt template here. Use $1, $2, etc. for positional arguments.
```

| Frontmatter Field | Description |
|-------------------|-------------|
| `description` | Shown when listing available commands. |
| `argument-hint` | Hint text shown to the user for expected arguments. |
| `allowed-tools` | Restricts which tools the agent can use while executing the command. |

The filename (minus `.md`) becomes the command name: `commands/review-branch.md` -> `/review-branch`.

## Writing a Skill

Create a directory under `skills/` with a `SKILL.md` file:

```markdown
---
name: rust
description: Rust best practices and project conventions
tags: [rust, testing]
agent-invocable: true
---

# Rust Coding Guidelines

See [traits](./traits.md) for trait conventions.
See [error-handling](./error-handling.md) for error patterns.
```

Skills support multi-file content. The `SKILL.md` is the primary file. Supporting files are listed in the skill's manifest and loaded on demand.
