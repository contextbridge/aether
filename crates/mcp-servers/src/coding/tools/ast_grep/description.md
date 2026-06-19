Structural AST search using ast-grep patterns.

Use `ast_grep` when you need to find syntax shapes such as functions, calls, hooks, imports, or language constructs. Use `grep` for raw text, logs, TODOs, and regex searches. Use LSP tools for definitions, references, types, and symbol navigation when a language server is available.

## Usage

```json
{"language": "rs", "pattern": "fn $NAME($$$ARGS) { $$$BODY }", "glob": "**/*.rs"}
{"language": "rs", "pattern": "use $PATH;", "glob": "**/*.rs"}
{"language": "rs", "pattern": "use $CRATE;", "constraints": {"CRATE": "^crossterm"}, "glob": "**/*.rs"}
{"language": "ts", "pattern": "console.log($$$ARGS)", "path": "src", "headLimit": 20}
{"language": "tsx", "pattern": "useEffect($$$ARGS)", "contextAround": 2}
{"language": "py", "pattern": "def $NAME($$$ARGS): $$$BODY"}
```

## Filtering by capture value

Use `constraints` to keep only matches whose captured text matches a regex. For example, this finds every file that imports `crossterm` in 1-toolcall:

```json
{
  "language": "rs",
  "pattern": "use $CRATE;",
  "constraints": {"CRATE": "^crossterm"},
  "glob": "**/*.rs"
}
```

## Parameters

- `pattern` — required ast-grep pattern code.
- `language` — required ast-grep language alias such as `rs`, `rust`, `ts`, `tsx`, `py`, or `js`.
- `path` — file or directory to search. Defaults to the workspace root.
- `glob` — optional file filter such as `**/*.rs` or `*.{ts,tsx}`.
- `constraints` — optional map from metavariable name (without `$`) to a regex the captured text must match. Only matches where every constraint is satisfied are returned. Example: `{"CRATE": "^crossterm"}` with pattern `use $CRATE;`.
- `contextBefore` — lines before each match.
- `contextAfter` — lines after each match.
- `contextAround` — lines before and after each match; overrides `contextBefore` and `contextAfter`.
- `headLimit` — maximum number of matches to return.

Directory searches respect `.gitignore`, skip hidden files, do not follow symlinks, and search only files matching the selected language. Explicit file searches parse the provided file with the selected language even if the extension does not match.

Result line and column ranges are 1-based. Byte ranges are 0-based and end-exclusive.
