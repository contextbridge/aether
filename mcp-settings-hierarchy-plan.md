# MCP Settings Hierarchy Plan

## Overview

### Problem Statement

The Wisp/Aether `/settings -> MCP servers` view currently renders every MCP status entry as a flat list. When MCP servers are configured with `proxy: true`, the MCP manager also exposes a virtual `proxy` tool server for tool execution, and the settings UI currently makes proxied servers look like normal direct MCP servers. This is confusing because proxied server tools are accessed through the proxy, but users should care about the real configured servers and whether they are direct or proxied — not about the internal `proxy__call_tool` implementation detail.

### Success Criteria and Acceptance Conditions

- MCP server status entries carry enough metadata to distinguish:
  - direct MCP servers
  - servers nested under a proxy
- The virtual `proxy` implementation server is not included in user-facing MCP server statuses and is not rendered as a row in `/settings -> MCP servers`.
- `/settings -> MCP servers` renders proxied servers under a `Proxied` section instead of as flat peers.
- Direct MCP servers remain visually separate from proxied servers when both are present.
- OAuth authentication behavior still works for proxied nested servers; pressing Enter on an authenticatable nested server emits `AuthenticateServer(<nested server name>)`.
- Existing non-proxied configurations keep the current simple flat rendering unless at least one proxied server exists.
- Status updates preserve grouping after initial connection, failed connection, OAuth-needed, authenticating, and post-auth reconnect states.
- Tests cover rendering, navigation, authentication selection, status metadata serialization, and MCP manager status generation.

## Technical Approach

### Architectural Decisions

1. **Add explicit grouping metadata to real MCP status entries**
   - Extend `McpServerStatusEntry` in `packages/mcp-utils/src/status.rs` with a `group`/`location` field.
   - Use an enum instead of inferring from names:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
     #[serde(tag = "kind", rename_all = "snake_case")]
     pub enum McpServerStatusGroup {
         #[default]
         Direct,
         Proxied { proxy_name: String },
     }
     ```
   - Add builder methods:
     ```rust
     pub fn with_group(mut self, group: McpServerStatusGroup) -> Self
     pub fn as_proxied(mut self, proxy_name: impl Into<String>) -> Self
     ```

2. **Keep the virtual proxy out of user-facing statuses**
   - `McpManager` should continue registering the proxy tool and proxy instructions for agent/tool execution.
   - `McpManager::register_proxy()` should not call `upsert_status(DEFAULT_PROXY_NAME, ...)`.
   - The settings UI should receive statuses only for real configured MCP servers.

3. **Set grouping at the source of truth: `McpManager`**
   - `McpManager` already knows which configured servers are proxied via `proxied_members`.
   - Store the status group on `ServerRecord` so `refresh_status_entries()` can produce stable grouped status entries for all status transitions.

4. **Keep UI grouping contained to `ServerStatusOverlay`**
   - Convert raw `Vec<McpServerStatusEntry>` into render rows inside `packages/wisp/src/components/server_status.rs`.
   - Do not change the root settings menu structure; only the MCP server status pane needs grouped rendering.

5. **Avoid broad TUI changes**
   - `SelectList` has no concept of non-selectable headers.
   - For this specific pane, replace `SelectList<ServerItem>` with a small custom list model that can render headers and skip them during navigation.
   - This avoids adding disabled-row complexity to a generic component.

### Proposed Rendering

When there is no proxied server, preserve the current flat view:

```text
github  ✓ 5 tools
linear  ⚡ needs authentication
```

When at least one proxied server exists, render sections:

```text
Direct
  github  ✓ 5 tools

Proxied
  math  ✓ 3 tools
  linear  ⚡ needs authentication
```

Rules:

- Never render a `proxy  ✓ 1 tool` row. The proxy is an internal implementation detail and adds no user value.
- Show section headers only when at least one `Proxied` entry exists.
- If there are proxied entries but no direct entries, omit the empty `Direct` section.
- Proxied server rows are indented under the `Proxied` header.
- Only server rows are selectable; headers and blank spacer rows are not selectable.
- Existing status symbols and colors remain unchanged.

### Key Technical Considerations and Trade-offs

- **Data model vs. UI inference**: Adding explicit `McpServerStatusGroup` is slightly more code, but avoids brittle UI guesses based on `name == "proxy"` or list ordering.
- **Removing the proxy status row simplifies the UI**: The UI only needs to group real servers as direct/proxied; it does not need a proxy-parent row or `Proxy` status variant.
- **Protocol serialization**: `McpServerStatusEntry` is serialized through ACP notifications. Add/update serde tests so grouped status entries round-trip correctly.
- **Multiple proxies**: The current manager uses one `DEFAULT_PROXY_NAME`, but `Proxied { proxy_name }` keeps enough metadata to group by proxy if multiple proxies are introduced later. For the current UI, all proxied entries can be shown under a single `Proxied` header.
- **Selection preservation**: Current `update_entries()` preserves selected index. With headers, preserve selection by selected server name where possible; fall back to the first selectable server row.

## Implementation Steps

1. **Write failing tests for status grouping metadata**
   - In `packages/acp-utils/src/notifications.rs` tests, extend `mcp_server_status_entry_serde_roundtrip` or add a new test:
     ```rust
     #[test]
     fn mcp_server_status_entry_proxied_group_serde_roundtrip() { ... }
     ```
   - Assert JSON includes the proxied group and deserializes to the same entry.

2. **Extend `McpServerStatusEntry` with grouping metadata**
   - Modify `packages/mcp-utils/src/status.rs`:
     - Add `McpServerStatusGroup` enum above `McpServerStatusEntry` with only `Direct` and `Proxied { proxy_name }` variants.
     - Add `pub group: McpServerStatusGroup` to `McpServerStatusEntry`.
     - Update `McpServerStatusEntry::new()` to default to `McpServerStatusGroup::Direct`.
     - Add builder helpers for grouped/proxied rows.

3. **Track grouping in `McpManager` records**
   - Modify `ServerRecord` in `packages/mcp-utils/src/client/manager.rs`:
     ```rust
     struct ServerRecord {
         connection: Option<McpServerConnection>,
         status: McpServerStatus,
         reauth_config: Option<StreamableHttpClientTransportConfig>,
         group: McpServerStatusGroup,
     }
     ```
   - Update constructors:
     ```rust
     fn new(status, reauth_config, group) -> Self
     fn connected(connection, tool_count, reauth_config, group) -> Self
     ```
   - Update `status_entry()` to call `.with_group(self.group.clone())`.

4. **Pass group information through all MCP manager state transitions**
   - In `add_mcps()`:
     - Determine `is_proxied` from `proxied_members.contains(&name)` for successes and failures.
     - Pass `McpServerStatusGroup::Proxied { proxy_name: DEFAULT_PROXY_NAME.to_string() }` for proxied nested servers.
     - Pass `McpServerStatusGroup::Direct` for non-proxied servers.
   - In `register_connection()` / `apply_connected()`:
     - Replace the `is_proxied: bool` parameter with `group: McpServerStatusGroup` and derive whether tools should be hidden behind the proxy from it:
       ```rust
       let is_proxied = matches!(group, McpServerStatusGroup::Proxied { .. });
       ```
   - In `register_proxy()`:
     - Continue adding `proxy__call_tool` and `self.proxy = Some(...)`.
     - Remove the `upsert_status(DEFAULT_PROXY_NAME, McpServerStatus::Connected { tool_count: 1 }, None)` call.
   - In `authenticate_server_task()` and `apply_connection_attempt()`:
     - Preserve the existing record group during `Authenticating` and reconnect.
   - In `mark_failed()` / `upsert_status()`:
     - Do not reset an existing record’s group unless a new group is explicitly provided.

5. **Write failing MCP manager tests for grouped statuses and hidden proxy status**
   - Add or update tests in `packages/mcp-utils/src/client/manager.rs` test module, or add coverage in `packages/aether-core/tests/mcp/tool_proxy_tests.rs`.
   - Test mixed direct + proxied configuration:
     - direct server `direct`
     - proxied server `math`
   - Assert `manager.server_statuses()` or `McpSpawnResult.server_statuses` contains:
     - `direct` with `Direct`
     - `math` with `Proxied { proxy_name: "proxy" }`
     - no entry named `proxy`
   - Add a failure/OAuth-needed case if existing test helpers make this straightforward.

6. **Refactor `ServerStatusOverlay` row model**
   - In `packages/wisp/src/components/server_status.rs`, replace `ServerItem`/`SelectList<ServerItem>` with a local row model:
     ```rust
     enum ServerStatusRow {
         Header(String),
         Spacer,
         Server { entry: McpServerStatusEntry, indent: usize },
     }
     ```
   - Add helper functions:
     ```rust
     fn build_rows(entries: Vec<McpServerStatusEntry>) -> Vec<ServerStatusRow>
     fn has_proxied_entries(entries: &[McpServerStatusEntry]) -> bool
     fn first_selectable_row(rows: &[ServerStatusRow]) -> Option<usize>
     fn next_selectable_row(rows: &[ServerStatusRow], current: usize, direction: isize) -> Option<usize>
     fn render_server_entry(entry: &McpServerStatusEntry, selected: bool, indent: usize, ctx: &ViewContext) -> Line
     ```
   - Preserve the existing status detail text and status-dependent styles.

7. **Implement grouped row construction**
   - If there are no proxied entries, build one flat `Server` row per entry with `indent = 0`.
   - If there are proxied entries:
     - Add `Header("Direct")` and direct server rows only when direct entries exist.
     - Add a `Spacer` between sections only when both sections exist.
     - Add `Header("Proxied")`.
     - Add proxied server rows with `indent = 1`.
   - Sort/order within each group should preserve the input order from `McpManager`.

8. **Implement custom event handling for server rows**
   - `Esc` emits `ServerStatusMessage::Close`.
   - `Up`/`Down` moves to previous/next selectable server row and wraps around.
   - `Enter` on a selectable server row emits `Authenticate(name)` only if `entry.can_authenticate()`.
   - Headers and spacers cannot be selected.
   - Mouse scroll can mirror Up/Down behavior if current code behavior should be preserved.

9. **Preserve selected server across updates**
   - In `ServerStatusOverlay::update_entries()`:
     - Capture the selected server name before rebuilding rows.
     - Rebuild rows from the new entries.
     - If the same server still exists, select its new row.
     - Otherwise select the first server row.

10. **Add Wisp component tests for hierarchical rendering**
    - In `packages/wisp/src/components/server_status.rs` tests:
      - `renders_flat_entries_when_no_proxy_exists`
      - `renders_direct_and_proxied_sections_when_proxy_exists`
      - `does_not_render_proxy_status_row`
      - `navigation_skips_headers_and_spacers`
      - `enter_on_proxied_oauth_server_emits_nested_server_name`
      - `update_entries_preserves_selection_by_server_name`
    - In `packages/wisp/tests/component_tests/settings_overlay.rs`, update or add an integration-style render test that opens the MCP server pane and asserts the visible hierarchy.

11. **Run validation**
    - First run targeted tests after adding tests and before implementation to confirm they fail.
    - After implementation:
      - `cargo test -p mcp-utils status` or targeted manager tests
      - `cargo test -p acp-utils notifications`
      - `cargo test -p wisp server_status settings_overlay`
      - `just test` if targeted tests pass
      - `just fmt`
      - `just lint` if time permits

## Testing Plan

### Unit Tests Required

- `packages/mcp-utils/src/status.rs` / `packages/acp-utils/src/notifications.rs`
  - Direct entries default to `McpServerStatusGroup::Direct`.
  - Proxied entries serialize and deserialize with `proxy_name`.

- `packages/mcp-utils/src/client/manager.rs`
  - Direct servers produce `Direct` status entries.
  - Proxied nested servers produce `Proxied { proxy_name: "proxy" }` entries.
  - The virtual proxy does not produce a user-facing status entry.
  - Authentication state transitions preserve the nested server’s group.

- `packages/wisp/src/components/server_status.rs`
  - Flat rendering remains unchanged without proxied entries.
  - Hierarchical rendering shows `Direct` and `Proxied` headers with nested indentation.
  - The string `proxy  ✓ 1 tool` never appears in rendered MCP server settings.
  - Headers/spacers are not selectable.
  - Enter on nested OAuth server authenticates the nested server name, not `proxy`.
  - Selection is preserved by server name after updates.

### Integration Tests Needed

- `packages/wisp/tests/component_tests/settings_overlay.rs`
  - Open `/settings -> MCP servers` with mixed direct/proxied statuses and assert the rendered pane contains the hierarchy and omits any `proxy` row.

- Existing app-level settings tests should continue passing without changes except for expected output if they assert exact text around MCP server rows.

### Edge Cases to Verify

- No MCP servers configured still shows `(no MCP servers configured)`.
- Only direct servers: no new headers, existing flat appearance.
- Only proxied servers: show `Proxied` section without empty `Direct` section.
- A nested proxied server failed or needs OAuth.
- A proxied server starts OAuth authentication and later reconnects; it remains under `Proxied`.
- A previously selected nested server disappears after status update; selection moves to the first available server row.
- Narrow terminal rendering does not panic.

## Files to Modify/Create

| Path | Change | Status |
| --- | --- | --- |
| `/home/josh/code/aether/packages/mcp-utils/src/status.rs` | Add `McpServerStatusGroup`; add `group` field and builder methods to `McpServerStatusEntry`. | Modified |
| `/home/josh/code/aether/packages/mcp-utils/src/client/manager.rs` | Store status group on `ServerRecord`; set `Direct` and `Proxied` groups during connection, failure, OAuth, and reconnect flows; stop adding a user-facing `proxy` status entry; add manager tests. | Modified |
| `/home/josh/code/aether/packages/acp-utils/src/notifications.rs` | Update/add serialization tests for grouped MCP status entries. | Modified |
| `/home/josh/code/aether/packages/wisp/src/components/server_status.rs` | Replace flat `SelectList` model with header-aware custom rows; render direct/proxied sections; update navigation/authentication logic; add component tests. | Modified |
| `/home/josh/code/aether/packages/wisp/tests/component_tests/settings_overlay.rs` | Add mixed direct/proxied render coverage; assert no internal proxy row is rendered; update any impacted assertions. | Modified |
| `/home/josh/code/aether/packages/aether-core/tests/mcp/tool_proxy_tests.rs` | Optionally add spawn-level assertions that proxied MCP statuses include grouping metadata and omit the internal proxy row. | Modified |

## Additional Notes

- Avoid broad changes to `tui::SelectList`; the header-skipping behavior is only needed in this one overlay.
- Do not render internal proxy implementation details in user-facing settings. Users should see real configured servers grouped by whether they are direct or proxied.
- Keep the top-level MCP summary (`server_status_summary`) status-focused for now (`connected`, `needs auth`, etc.). Removing the proxy status row will also make this summary more accurate because it will count only real MCP servers.
- Follow the repository testing convention: add failing tests first, then implement.
