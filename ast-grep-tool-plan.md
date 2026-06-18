# Add `ast_grep` Tool to the Coding MCP Server

## Overview

### Problem statement

The built-in `coding` MCP server currently has:

- `grep` for regex/text search.
- `find` for file discovery.
- LSP tools for symbol-aware navigation.

It does not have a structural search tool for finding syntax patterns like `fn $NAME(...) { $$$BODY }`, `console.log($$$ARGS)`, or `useEffect($$$ARGS)` without relying on brittle regexes or requiring a live LSP server.

Add a read-only `ast_grep` MCP tool to the existing `coding` MCP server. The tool should use ast-grep's Rust library crates in-process rather than shelling out to the `sg` / `ast-grep` CLI.

### Success criteria and acceptance conditions

- A new `ast_grep` MCP tool is available from the existing `coding` server.
- The tool performs read-only AST structural search over a file or directory.
- The implementation uses Rust crates (`ast-grep-core` and `ast-grep-language`) and does **not** require an external `ast-grep` binary to be installed.
- The tool supports:
  - Required ast-grep `pattern` string.
  - Required `language` string using `ast_grep_language::SupportLang` aliases such as `rs`, `rust`, `ts`, `tsx`, `py`, `js`.
  - Optional `path`, defaulting to the coding server workspace root.
  - Optional `glob` file filter.
  - Optional context line controls.
  - Optional `headLimit` result limit.
- Results include deterministic structured output with file path, 1-based line/column range, byte range, matched text, and metavariable captures.
- Directory searches respect `.gitignore`, skip hidden files by default, do not follow symlinks, and search only files matching the selected language unless the path is an explicit file.
- Tool annotations mark the tool as read-only and closed-world:
  - `read_only_hint = true`
  - `open_world_hint = false`
- Existing coding tools continue to compile and pass tests.
- Unit and integration tests cover the public tool behavior.

## Technical Approach

### Architectural decision

Add the tool to the existing `coding` MCP server, not a new MCP server.

Rationale:

- `coding` already owns code search, file traversal, workspace root resolution, and MCP tool registration.
- `ast_grep` sits naturally between `grep` and LSP: structural source search that is more precise than regex but does not require an LSP server.
- A new MCP server would add configuration and discovery overhead without a strong boundary benefit.

### Library vs shell-out decision

Use ast-grep's Rust libraries in-process:

- `ast-grep-core` for `Pattern::try_new` and matching APIs.
- `ast-grep-language` for `SupportLang`, language aliases, parser integration, and language-aware extension filtering.

Do not shell out to the CLI.

Rationale:

- No external binary install requirement.
- Avoids shell escaping and command injection risk.
- Produces typed structured MCP output directly.
- Fits the current `grep` / `find` pattern of in-process Rust implementations.
- Avoids version drift between the packaged MCP server and a user's installed `ast-grep` CLI.

### MVP scope

Implement read-only structural search only. Do **not** implement rewriting in this change.

A future rewrite tool should be separate, e.g. `ast_rewrite` or `ast_grep_rewrite`, with non-read-only MCP annotations and explicit file safety semantics.

### Input schema

Create `AstGrepInput` in `packages/mcp-servers/src/coding/tools/ast_grep/mod.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepInput {
    /// ast-grep pattern code, e.g. "fn $NAME() { $$$BODY }"
    pub pattern: String,

    /// Language alias understood by ast-grep, e.g. "rs", "rust", "ts", "tsx", "py", "js".
    pub language: String,

    /// File or directory to search. Defaults to the coding server workspace root.
    pub path: Option<String>,

    /// Optional glob filter, e.g. "**/*.rs" or "*.{ts,tsx}".
    pub glob: Option<String>,

    /// Lines before each match.
    pub context_before: Option<u32>,

    /// Lines after each match.
    pub context_after: Option<u32>,

    /// Lines before and after each match. Overrides contextBefore/contextAfter.
    pub context_around: Option<u32>,

    /// Maximum number of matches to return.
    pub head_limit: Option<usize>,
}
```

Use `language` rather than `type` to make it clear this is an AST parser selection, not a ripgrep file type filter.

### Output schema

Create output structs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepOutput {
    pub matches: Vec<AstGrepMatch>,
    pub count: usize,
    pub truncated: bool,
    pub language: String,
    pub search_path: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepMatch {
    pub file: String,
    pub range: AstGrepRange,
    pub text: String,
    pub captures: Vec<AstGrepCapture>,
    pub before_context: Option<Vec<String>>,
    pub after_context: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepRange {
    /// 1-based line number.
    pub start_line: usize,
    /// 1-based character column.
    pub start_column: usize,
    /// 1-based line number.
    pub end_line: usize,
    /// 1-based character column.
    pub end_column: usize,
    /// 0-based byte offset.
    pub start_byte: usize,
    /// 0-based exclusive byte offset.
    pub end_byte: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AstGrepCapture {
    pub name: String,
    pub text: String,
}
```

Use 1-based line/column values for user-facing consistency with `read_file` and `grep`. Keep byte offsets 0-based because they are source offsets.

### Matching behavior

- Parse `language` once:

```rust
let lang: SupportLang = args.language.parse()
    .map_err(|_| AstGrepError::UnsupportedLanguage(args.language.clone()))?;
```

- Compile the pattern once with non-panicking API:

```rust
use ast_grep_core::Pattern;

let pattern = Pattern::try_new(&args.pattern, lang)
    .map_err(|e| AstGrepError::InvalidPattern(e.to_string()))?;
```

Do not use `Pattern::new`, because it unwraps and can panic on invalid user input.

- For each source file:

```rust
use ast_grep_language::LanguageExt;

let source = std::fs::read_to_string(path)?;
let root = lang.ast_grep(&source);
for node_match in root.root().find_all(pattern.clone()) {
    // build AstGrepMatch
}
```

- Convert positions:

```rust
let start = node_match.start_pos();
let end = node_match.end_pos();
let range = node_match.range();

AstGrepRange {
    start_line: start.line() + 1,
    start_column: start.column(&*node_match) + 1,
    end_line: end.line() + 1,
    end_column: end.column(&*node_match) + 1,
    start_byte: range.start,
    end_byte: range.end,
}
```

- Convert captures deterministically:

```rust
let mut captures: Vec<AstGrepCapture> =
    std::collections::HashMap::<String, String>::from(node_match.get_env().clone())
        .into_iter()
        .map(|(name, text)| AstGrepCapture { name, text })
        .collect();
captures.sort_by(|a, b| a.name.cmp(&b.name));
```

### File traversal behavior

For explicit file paths:

- Validate the path exists.
- Apply `glob` if provided.
- Parse the file using the provided `language`, even if the file extension does not match that language. This lets advanced users search extensionless files or generated fixtures.

For directory paths:

- Use `ignore::WalkBuilder` sequentially for deterministic ordering and simplicity.
- Configure it like `find` / `grep`:

```rust
WalkBuilder::new(search_path)
    .hidden(false)
    .git_ignore(true)
    .follow_links(false)
```

- Collect matching files into `Vec<PathBuf>`, sort by path, then parse in order.
- Include only regular files where:
  - Optional `glob` matches, and
  - The file extension maps to the selected `SupportLang`.

Use the `Language` trait implementation for extension inference:

```rust
use ast_grep_language::{Language, SupportLang};

fn file_matches_language(path: &Path, lang: SupportLang) -> bool {
    <SupportLang as Language>::from_path(path) == Some(lang)
}
```

### Error handling

Add `AstGrepError` to `packages/mcp-servers/src/coding/error.rs` and wrap it in `CodingError`:

```rust
#[derive(Debug, Error)]
pub enum AstGrepError {
    #[error("Search path does not exist: {0}")]
    PathNotFound(String),

    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlobPattern { pattern: String, reason: String },

    #[error("Failed to build glob set: {0}")]
    GlobSetBuildFailed(String),

    #[error("Unsupported ast-grep language: {0}")]
    UnsupportedLanguage(String),

    #[error("Invalid ast-grep pattern: {0}")]
    InvalidPattern(String),

    #[error("Failed to read file '{path}': {reason}")]
    ReadFailed { path: String, reason: String },
}
```

For directory walks, skip individual unreadable/non-UTF-8 files to behave like search tools that tolerate noisy trees. For an explicit file path, return `ReadFailed`.

### Trade-offs

- Sequential directory processing is simpler and deterministic. If performance becomes an issue, a later change can parallelize after preserving stable ordering and limit semantics.
- `language` is required. Inference is attractive, but the pattern itself is language-specific, so requiring a parser makes the tool less surprising.
- Rule-file support via `ast-grep-config` is intentionally excluded from the MVP. Add it later as a separate tool or mode if needed.
- Rewriting is excluded from the MVP to avoid mixing read-only search with mutation safety design.

## Implementation Steps

1. **Add dependencies**

   Update workspace dependencies in `/Users/josh/code/aether-2/Cargo.toml`:

   ```toml
   ast-grep-core = "0.43"
   ast-grep-language = "0.43"
   ```

   Update `/Users/josh/code/aether-2/packages/mcp-servers/Cargo.toml`:

   - Add optional dependencies:

     ```toml
     ast-grep-core = { workspace = true, optional = true }
     ast-grep-language = { workspace = true, optional = true }
     ```

   - Add both to the `coding` feature:

     ```toml
     "dep:ast-grep-core",
     "dep:ast-grep-language",
     ```

2. **Create the `ast_grep` tool module**

   Add directory:

   ```text
   /Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools/ast_grep/
   ```

   Add files:

   ```text
   mod.rs
   description.md
   ```

   In `mod.rs`:

   - Define `AstGrepInput`.
   - Define `AstGrepOutput`, `AstGrepMatch`, `AstGrepRange`, and `AstGrepCapture`.
   - Implement:

     ```rust
     pub async fn perform_ast_grep(args: AstGrepInput) -> Result<AstGrepOutput, AstGrepError>
     ```

   - Keep private helpers below public structs/functions:

     ```rust
     fn build_glob_set(glob: Option<&str>) -> Result<Option<globset::GlobSet>, AstGrepError>
     fn collect_files(search_path: &Path, lang: SupportLang, glob: Option<&globset::GlobSet>) -> Vec<PathBuf>
     fn file_matches_filters(path: &Path, lang: SupportLang, glob: Option<&globset::GlobSet>) -> bool
     fn search_file(path: &Path, lang: SupportLang, pattern: &Pattern, args: &AstGrepInput) -> Result<Vec<AstGrepMatch>, AstGrepError>
     fn build_match(path: &Path, source: &str, node_match: NodeMatchLike, context_before: usize, context_after: usize) -> AstGrepMatch
     fn context_for_match(source: &str, start_line: usize, end_line: usize, before: usize, after: usize) -> (Option<Vec<String>>, Option<Vec<String>>)
     ```

   The concrete `NodeMatchLike` should be the real ast-grep node match type inferred by the helper signature. If Rust lifetime/generic syntax becomes cumbersome, inline match construction inside `search_file` and keep only simple helpers for context/captures.

3. **Implement `perform_ast_grep` control flow**

   Pseudo-code:

   ```rust
   pub async fn perform_ast_grep(mut args: AstGrepInput) -> Result<AstGrepOutput, AstGrepError> {
       if args.path.as_deref().is_some_and(|p| p.trim().is_empty()) {
           args.path = None;
       }

       let lang: SupportLang = args.language.parse()
           .map_err(|_| AstGrepError::UnsupportedLanguage(args.language.clone()))?;
       let pattern = Pattern::try_new(&args.pattern, lang)
           .map_err(|e| AstGrepError::InvalidPattern(e.to_string()))?;
       let glob_set = build_glob_set(args.glob.as_deref())?;

       let search_path = args.path.as_deref().unwrap_or(".");
       let path = Path::new(search_path);
       if !path.exists() {
           return Err(AstGrepError::PathNotFound(search_path.to_string()));
       }

       let (context_before, context_after) = context_counts(&args);
       let limit = args.head_limit.unwrap_or(usize::MAX);
       let mut matches = Vec::new();
       let mut truncated = false;

       let files = if path.is_file() {
           if glob_set.as_ref().is_some_and(|glob| !glob.is_match(path)) {
               Vec::new()
           } else {
               vec![path.to_path_buf()]
           }
       } else {
           collect_files(path, lang, glob_set.as_ref())
       };

       for file in files {
           let file_matches = search_file(&file, lang, &pattern, context_before, context_after, limit - matches.len())?;
           matches.extend(file_matches);
           if matches.len() >= limit {
               matches.truncate(limit);
               truncated = true;
               break;
           }
       }

       let count = matches.len();
       let meta = ToolDisplayMeta::new(
           "AST grep",
           format!("'{}' in {} ({count} matches)", args.pattern, basename(search_path)),
       );

       Ok(AstGrepOutput {
           matches,
           count,
           truncated,
           language: lang.to_string(),
           search_path: search_path.to_string(),
           meta: Some(meta.into()),
       })
   }
   ```

   Avoid comments in code unless explaining a non-obvious why.

4. **Register the module**

   Update `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools/mod.rs`:

   ```rust
   pub mod ast_grep;
   ```

5. **Add error integration**

   Update `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/error.rs`:

   - Add `AstGrep(#[from] AstGrepError)` variant to `CodingError` near `Grep` and `Find`.
   - Add the `AstGrepError` enum near `GrepError`.

6. **Expose tool types through `CodingTools`**

   Update `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools_trait.rs`:

   - Import:

     ```rust
     use super::tools::ast_grep::{AstGrepInput, AstGrepOutput, perform_ast_grep};
     ```

   - Add a default trait method near `grep` and `find`:

     ```rust
     /// Search code using ast-grep structural patterns.
     fn ast_grep(&self, args: AstGrepInput) -> impl Future<Output = Result<AstGrepOutput, CodingError>> + Send {
         async move { perform_ast_grep(args).await.map_err(CodingError::from) }
     }
     ```

   Update `/Users/josh/code/aether-2/packages/mcp-servers/src/docs/coding_tools.md` to list `ast_grep` under provided methods.

7. **Add default implementation**

   Update `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/default_tools.rs`:

   - Import `AstGrepInput`, `AstGrepOutput`, and `perform_ast_grep`.
   - Add:

     ```rust
     async fn ast_grep(&self, args: AstGrepInput) -> Result<AstGrepOutput, CodingError> {
         perform_ast_grep(args).await.map_err(CodingError::from)
     }
     ```

8. **Add the MCP tool method**

   Update `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/mod.rs`:

   - Import:

     ```rust
     use tools::ast_grep::{AstGrepInput, AstGrepOutput, perform_ast_grep};
     ```

     If `perform_ast_grep` is not used directly in `mod.rs`, do not import it there.

   - Add quick reference line in `build_instructions()`:

     ```text
     - **Structural code patterns** (AST search): `ast_grep`
     ```

   - Add tool method next to `grep` / `find`:

     ```rust
     #[doc = include_str!("tools/ast_grep/description.md")]
     #[tool(annotations(read_only_hint = true, open_world_hint = false))]
     pub async fn ast_grep(
         &self,
         request: Parameters<AstGrepInput>,
         context: RequestContext<RoleServer>,
     ) -> Result<Json<AstGrepOutput>, String> {
         let Parameters(mut args) = request;
         let root = self.primary_workspace_root().await;
         let normalized_path = resolve_dir(&root, args.path.as_deref());
         args.path = Some(normalized_path.to_string_lossy().to_string());
         notify_preview(&context, ToolDisplayMeta::new("AST grep", format!("'{}'", args.pattern))).await;
         self.tools.ast_grep(args).await.into_mcp()
     }
     ```

9. **Write tool description**

   Add `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools/ast_grep/description.md`:

   Include:

   - Short description: structural AST search using ast-grep patterns.
   - Guidance: use `ast_grep` for syntax shapes, `grep` for raw text/log/TODO searches, LSP for definitions/references/types.
   - Usage examples:

     ```json
     {"language": "rs", "pattern": "fn $NAME($$$ARGS) { $$$BODY }", "glob": "**/*.rs"}
     {"language": "ts", "pattern": "console.log($$$ARGS)", "path": "src", "headLimit": 20}
     {"language": "tsx", "pattern": "useEffect($$$ARGS)", "contextAround": 2}
     {"language": "py", "pattern": "def $NAME($$$ARGS): $$$BODY"}
     ```

   - Parameter explanations.
   - Mention line/column ranges are 1-based and byte ranges are 0-based.

10. **Update user-facing docs**

   Update:

   - `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/README.md`
     - Add `ast_grep` to the Search table.
     - Add it to the table of contents only if the section structure changes.
   - `/Users/josh/code/aether-2/packages/mcp-servers/src/docs/coding_mcp.md`
     - Add `ast_grep` under Shell & search.
   - `/Users/josh/code/aether-2/packages/mcp-servers/README.md`
     - Update the `coding` feature/server description from `grep, find` to include `ast_grep` or “structural search”.

11. **Add unit tests for the tool module**

   In `packages/mcp-servers/src/coding/tools/ast_grep/mod.rs`, add tests similar to `grep` and `find` module tests:

   - `finds_rust_function_pattern`
     - Create temp dir with `lib.rs` containing multiple functions.
     - Search `language: "rs"`, pattern `fn $NAME() {}` or `fn $NAME($$$ARGS) { $$$BODY }`.
     - Assert matches include expected function text.
   - `returns_metavariable_captures`
     - Pattern `fn $NAME() {}`.
     - Assert capture `NAME` is `foo` for `fn foo() {}`.
   - `filters_directory_by_language`
     - Include `.rs` and `.py` files with text that would otherwise match.
     - Search `language: "rs"`.
     - Assert only `.rs` files are returned.
   - `applies_glob_filter`
     - Include `src/lib.rs` and `tests/lib.rs`.
     - Use `glob: "src/**/*.rs"` or equivalent.
     - Assert only `src` file is searched.
   - `invalid_language_returns_error`
     - Use `language: "not-a-language"`.
     - Assert `AstGrepError::UnsupportedLanguage`.
   - `invalid_pattern_returns_error`
     - Use empty pattern or a known multiple-root pattern such as `"12  3344"` for a language where it errors.
     - Assert `AstGrepError::InvalidPattern`.
   - `head_limit_truncates_results`
     - Produce several matches.
     - Use `headLimit: 1`.
     - Assert `count == 1` and `truncated == true`.
   - `range_is_one_based_and_byte_offsets_are_zero_based`
     - Use a small source where expected range is obvious.
     - Assert start line/column are 1-based.

12. **Add MCP integration test**

   Update `/Users/josh/code/aether-2/packages/mcp-servers/tests/coding_mcp_tools.rs` or add a new integration test file if the existing file is getting too broad.

   Add test:

   ```rust
   #[tokio::test]
   async fn test_ast_grep_uses_workspace_root_when_no_path_given() -> Result<(), Box<dyn std::error::Error>> {
       let temp = tempfile::tempdir()?;
       let workspace = temp.path().to_path_buf();
       std::fs::write(workspace.join("lib.rs"), "fn target() {}\nfn other() {}\n")?;

       let server_service = CodingMcp::new().with_root_dir(workspace.clone());
       let client_info = ClientInfo::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"));
       let (_server_handle, client) = connect(server_service, client_info).await?;

       let result = client.call_tool(
           CallToolRequestParams::new("ast_grep").with_arguments(
               serde_json::json!({
                   "language": "rs",
                   "pattern": "fn $NAME() {}"
               }).as_object().unwrap().clone(),
           ),
       ).await?;

       let text_content = result.content.first().and_then(|c| c.as_text()).ok_or("Expected text content")?;
       let parsed: serde_json::Value = serde_json::from_str(&text_content.text)?;
       assert_eq!(parsed["searchPath"].as_str().unwrap(), workspace.to_str().unwrap());
       assert!(parsed["matches"].as_array().unwrap().iter().any(|m| {
           m["file"].as_str().unwrap().ends_with("lib.rs") &&
           m["captures"].as_array().unwrap().iter().any(|c| c["name"] == "NAME" && c["text"] == "target")
       }));

       Ok(())
   }
   ```

   Also consider a small integration assertion that the tool appears in `list_tools` if there is an existing pattern for that in tests.

13. **Run validation**

   Preferred quick checks:

   - Use LSP diagnostics for workspace or affected files first.
   - Then run targeted tests if needed:

     ```bash
     just test -p aether-mcp-servers ast_grep
     ```

   If the project’s `just test` does not accept package/test filters, use the repository’s standard `just test`.

## Testing Plan

### Unit tests required

Add unit tests in `packages/mcp-servers/src/coding/tools/ast_grep/mod.rs` for:

- Basic Rust structural match.
- TypeScript or TSX structural match if compilation time remains acceptable.
- Capture extraction and deterministic capture sorting.
- Glob filtering.
- Language-based extension filtering for directory walks.
- Direct file path behavior.
- Invalid language errors.
- Invalid pattern errors.
- Missing path errors.
- Context line extraction.
- `headLimit` truncation.
- Deterministic path ordering.

### Integration tests needed

Add MCP-level tests for public API behavior:

- `ast_grep` defaults to `CodingMcp::with_root_dir(...)` workspace root when `path` is omitted.
- JSON response shape includes `matches`, `count`, `truncated`, `language`, and `searchPath`.
- At least one match contains expected `file`, `text`, `range`, and `captures`.

### Edge cases to verify

- Empty `path` or whitespace `path` defaults to workspace root.
- `glob` that matches no files returns `matches: []`, `count: 0`, `truncated: false`.
- Unsupported language aliases return a clean tool error, not a panic.
- Invalid ast-grep patterns return a clean tool error, not a panic.
- Non-UTF-8 files in directory searches do not crash the search.
- Explicit non-UTF-8 file path returns a readable error.
- Multi-line matches have correct start/end line values and context after the end line.
- `headLimit: 0` returns no matches and `truncated: true` if there would have been at least one match. If implementing that exactly is awkward, reject `headLimit: 0` with a validation error and document it.

## Files to Modify/Create

| Path | Change | Status |
|---|---|---|
| `/Users/josh/code/aether-2/Cargo.toml` | Add workspace dependencies `ast-grep-core` and `ast-grep-language`. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/Cargo.toml` | Add optional deps and include them in the `coding` feature. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools/ast_grep/mod.rs` | Implement input/output structs, `perform_ast_grep`, helpers, and unit tests. | Add |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools/ast_grep/description.md` | Add MCP tool documentation and examples. | Add |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools/mod.rs` | Export `pub mod ast_grep;`. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/error.rs` | Add `AstGrepError` and `CodingError::AstGrep`. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/tools_trait.rs` | Add imports and default `ast_grep` method. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/default_tools.rs` | Implement `CodingTools::ast_grep` for `DefaultCodingTools`. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/mod.rs` | Import tool types, add quick-reference instruction, and register `ast_grep` MCP method. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/docs/coding_tools.md` | Document `ast_grep` as a provided backend method. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/docs/coding_mcp.md` | Add `ast_grep` to coding server tool docs. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/src/coding/README.md` | Add `ast_grep` to Search tools. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/README.md` | Update high-level coding server description to mention structural search. | Modify |
| `/Users/josh/code/aether-2/packages/mcp-servers/tests/coding_mcp_tools.rs` | Add MCP integration test for `ast_grep` workspace-root behavior and response shape. | Modify |

## Additional Notes

- The tool should be named `ast_grep` to match existing snake_case MCP tool naming.
- Do not add `ast-grep-config` in the MVP. Use it later only if adding YAML rule-file scanning.
- Do not add rewrite support in this change. Rewriting needs separate permissions, read-before-edit/write safety, and likely diagnostic refresh behavior.
- Keep implementation comments sparse and only explain non-obvious “why” decisions.
- If dependency compilation cost is a concern after implementation, measure it before attempting optimization; correctness and tool stability matter more for the first version.
- If directory search performance becomes a problem, add parallel traversal in a follow-up while preserving deterministic output ordering and `headLimit` semantics.