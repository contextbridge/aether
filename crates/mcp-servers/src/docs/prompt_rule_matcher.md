Matches file reads against glob-triggered prompt rules and injects them as system reminders.

When a file is read through [`CodingMcp`](crate::CodingMcp), the matcher checks rules from the directories configured via `--rules-dir` / [`CodingMcp::with_rules_dirs`](crate::CodingMcp::with_rules_dirs). Any prompt whose `triggers.read` (or flat-rule `paths`/`globs`) matches the file path is returned as additional context.

# How it works

1. Rules declare read triggers as glob patterns in their frontmatter (e.g. `triggers.read: ["*.rs", "src/**/*.ts"]`).
2. On each `read_file` call, [`get_matched_rules`](PromptRuleMatcher::get_matched_rules) tests the file path against all trigger globs.
3. Matched rules fire **once per session** -- subsequent reads of files matching the same rule return an empty result. Call [`clear`](PromptRuleMatcher::clear) to reset.

# See also

- [`CodingMcp`](crate::CodingMcp) -- Integrates this matcher in its `read_file` tool.