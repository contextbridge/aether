# aether-desktop: Native Agent Orchestrator — Implementation Plan

A native desktop ACP client (Linux + macOS) for running and managing many concurrent Aether agent sessions. It is to the desktop what `wisp` is to the terminal. The UI is inspired by [assistant-ui](https://www.assistant-ui.com/): a left sidebar of threads grouped by project, and a main area with a **Chat** tab and a **Changes** tab. The Changes tab embeds clankerdiff's gpui `DiffViewer` for reviewing the agent's git diff and sending inline comments back as feedback.

Status: **plan only**, not implemented.

---

## 1. Decisions

| # | Topic | Decision |
|---|-------|----------|
| 1 | Process model | **One agent process per thread.** Local: `aether acp` subprocess over stdio. Remote: one websocket connection (one VM) per thread. No server-side multi-session work. |
| 2 | Workspace isolation | **None.** A thread runs in the directory it was created in. Two threads in one project share a checkout, so their Changes tabs show the same diff. This is accepted. |
| 3 | Diff dependency | **Path dependency on `../diff`**, applied to *all* clankerdiff crates and `gpui-pre` via `[patch.crates-io]`, so the workspace has a single copy of every clankerdiff type. |
| 4 | Permissions | **Keep auto-approve** (current `acp-utils` behaviour). Elicitations are the only blocking prompts. |
| 5 | Sidebar | **Live threads plus history**, **grouped by project** (git root of cwd). History is read from `aether-sessions` on disk. |
| 6 | Diff layout | **Tabs: `Chat` / `Changes (N)`** in the main area, with a full-width diff viewer. |
| 7 | Remote | **Pluggable `SessionTarget`** (`Local` / `Remote{url, headers}`). Phase 1 ships Local only. A clankervm provider comes later. |
| 8 | Lifetime | **Agents die with the app.** Reopening shows those threads in history, and they are resumed from the session log. |
| 9 | Code reuse | **Extract a headless client core from wisp** into a new crate that both wisp and desktop are thin views over. |
| 10 | Chat v1 scope | All of: rich rendering, composer controls, @mentions / slash commands / prompt search, elicitations, artifact review, attachments. |
| 11 | Name | Crate dir `crates/desktop`, package and binary `aether-desktop`. |
| 12 | Interaction | **Mouse-first with standard shortcuts.** The Changes tab keeps clankerdiff's own vim-style bindings. |
| 13 | Attention | **Sidebar status dots plus native OS notifications** for unfocused threads that finish a turn or need input. |
| 14 | Packaging | **Out of scope.** Run from source (`just desktop`). `publish = false`, excluded from cargo-dist. |
| 15 | Agents | **Aether only.** Every feature may assume the `_aether/*` extensions are present. |

---

## 2. What already exists (and is reused)

### In this workspace
- **`aether-acp-utils`** provides:
  - `connect_acp_client`, `AcpClient`, `AcpClientHandle`, `AcpEvent`
  - the `Conversation` model: items, turn phase, plan, context usage, tool calls, sub-agents
  - `_aether/*` notification types
  - `WebSocketTransport` (feature `websocket`)
  - `testing::FakeAgent`, a builder with `.capture()`
- **The agent already serves the git diff over ACP.**
  - The client sends clankerdiff `ClientCommand`s as `_aether/git_diff` notifications.
  - The agent's `GitDiffService` (`crates/aether-cli/src/acp/session/git_diff.rs`) runs a clankerdiff `DiffServer` and replies with `_aether/git_diff_event { event: Event<DocumentUpdate> }`.
  - `_aether/git_diff_close` closes it.
  - wisp's `GitReviewRuntime` / `AcpClientTransport` (`crates/wisp/src/runtime/git_review.rs`) plugs this into `clankerdiff-client`, which produces a `ClientState` holding a `DiffSnapshot { document: Arc<DiffDocument>, scope }`.
  - Because the diff comes over ACP, it works the same locally and inside a remote VM, and the host needs no git access.
- **Submitting a review is just a prompt.** `DiffReviewEvent::SubmitReview(submission)` becomes `submission.formatted`, which is sent through the normal prompt path (`App::submit_review` in wisp).
- **`aether-sessions`**: `SessionStore::list() -> Vec<SessionSummary { meta: SessionMeta { session_id, cwd, model, selected_mode, created_at }, title }>`, plus `preview(id)` and `search_prompts`. It is synchronous, plain JSONL, and can be read straight from disk.
- **Resume**: `ResumeSessionRequest::new(id, cwd).replay_from(ReplayFrom::Start)`. History is replayed as ordinary `SessionUpdate`s into `Conversation`.

### In `~/code/diff` (clankerdiff)
- **`clankerdiff-gpui`** (`crates/diff-gpui`):
  - `DiffViewer::with_options(Arc<DiffDocument>, ReviewTheme, DiffViewerOptions)`, with `set_document`, `set_scope`, `set_theme` and `bind_keys`
  - emits `DiffReviewEvent::{SubmitReview, CopyFormattedReview, RepositoryAction, SetScope, Refresh, Cancel}`
  - `MarkdownReviewer`, used here for artifact review
  - `ui` widgets: Button, ListRow, Surface, Modal, EmptyState, Notification, CommentCard, and others, all `RenderOnce` over `UiTheme`
  - bundled fonts via `load_default_fonts`
  - a `testing` module (feature `test-support`) with harness builders on `TestAppContext`
- **`clankerdiff-gpui-desktop`**: bootstrap code to copy (not depend on):
  - `runtime.rs`: tokio runtime as a gpui `Global`, `spawn(cx, fut) -> gpui::Task`
  - `run_loop.rs`: quit semantics, macOS NSApplication stop
  - `menus.rs`
  - `window_chrome.rs`: client-side decorations on Wayland/X11
  - `preferences.rs`
- **`clankerdiff-markdown`**:
  - `MarkdownStream` / `MarkdownStreamUpdate`, an incremental parser built for streaming chat
  - `MarkdownDocument` blocks
  - nothing renders these in gpui yet, so we write the renderer
- **`clankerdiff-syntax`**: tree-sitter highlighting for code blocks, consumed via `StyledText::with_highlights`. See `diff-gpui/src/markdown_viewer.rs` around lines 420-445.
- **`clankerdiff-theme`** / `diff-gpui/src/ui/theme.rs`: `ReviewTheme` maps to `UiTheme`/`UiColors` (canvas, surface, text, accent, positive, destructive), which is enough to theme the whole app.
- **gpui stack**:
  - `gpui-pre =0.3.7`, `gpui-pre-platform =0.3.7` (font-kit, runtime_shaders, wayland, x11), `gpui-base =0.7.0`
  - `gpui-base` provides `Textarea` / `TextareaState` (auto-grow, submit-on-enter), Popover, Tooltip, Toast, Dialog, `v_virtual_list`, Scrollbar, Tabs and Select
  - the vendored `patches/gpui` fixes variable-height `list` scrolling, which the chat transcript needs too

### Remote prior art (`~/code/clankervm`, `~/code/patchwave/clankers`)
- Inside the VM: `aether server --listen 0.0.0.0:8080 --cwd …`, which is ACP over websocket. MCP is only used for the separate tool gateway.
- Host side: `clankervm connect` gets a token through `MicroVmClient::create_auth_token`, then connects with headers `X-aws-proxy-auth: <token>` and `X-aws-proxy-port: 8080`.
- Repos are cloned inside the VM, and changes leave via `git push`. The diff therefore has to come over ACP, which it already does.
- `MicroVmClient` is `pub(crate)` today. It must be made public, or have its token and header logic reimplemented, before a clankervm provider can exist.

---

## 3. Architecture

```
┌──────────────────────────────── aether-desktop (gpui) ────────────────────────────────┐
│ DesktopApp (root view)                                                                │
│  ├─ Sidebar ── ProjectSection* ── ThreadRow* (status dot, title, time)                │
│  └─ ThreadView (selected) ── [Chat] Transcript + Composer │ [Changes] DiffViewer      │
│                                                                                       │
│ ThreadManager (Entity) ── Thread (Entity)* ── SessionCore (pure state, from core)     │
│        │                       │  ▲ AcpEvent / CommandResult                          │
│        │                       ▼  │ Effects → AgentCommand / GitReviewCommand         │
│        │                 ThreadDriver (tokio task: AcpClient + CommandDispatcher)     │
│        ▼                                                                              │
│  Registry (app state on disk) · History (aether-sessions) · Notifier · Connector      │
└───────────────────────────────────────────────────────────────────────────────────────┘
            │ stdio (Local)                         │ websocket (Remote, later)
        aether acp  (one per thread)            aether server in VM (one per thread)
```

### 3.1 New crate: `crates/client-core` (`aether-client-core`)
This crate holds the UI-agnostic client logic extracted from wisp. It has no ratatui, crossterm or gpui dependencies. Its dependencies are `aether-acp-utils`, `clankerdiff-client`, `clankerdiff-core`, `clankerdiff-protocol`, `aether-sessions`, `tokio` and `thiserror`-style error enums.

Public surface:
- **Connection**:
  - `Session::connect`, `connect_to` and `connect_remote_to`. These come from `wisp/src/session/mod.rs`, with a `ClientInfo` parameter in place of the hardcoded `"wisp"`.
  - A `connect_without_new_session` variant, so a thread can be resumed without first creating a throwaway session.
  - Error type `ClientError`.
- **Commands and results**:
  - `AgentCommand`, `GitReviewCommand` and `PromptRejection` (from `command.rs`).
  - A core `CommandResult`. The host-only variants (themes, terminal bell) stay in wisp, in a `HostCommandResult`.
- **Execution**:
  - `TaskSupervisor` and `ReadTask` (`runtime/tasks.rs`).
  - `agent::execute` (`runtime/agent.rs`).
  - `GitReviewRuntime` and `AcpClientTransport` (`runtime/git_review.rs`).
  - `CommandDispatcher`, minus the terminal and theme arms. Hosts add their own commands through a `Host(T)` variant.
- **`SessionCore`**: the per-session state machine. It holds:
  - `SessionModel` and `Conversation`
  - `ForegroundOperation` and `QueuedPrompt`
  - `available_commands: Vec<CommandEntry>`
  - `LocalConfigView` / `LocalConfigOption`
  - `PlanTracker` and `PromptHistory` (made public)

  It has two entry points, `on_acp_event(AcpEvent) -> Vec<Effect>` and `on_command_result(CommandResult) -> Vec<Effect>`. They hold the agnostic halves of `app/acp_reducer.rs` and `app/mod.rs`.

  `Effect` covers:
  - **Outbound:** `Dispatch(Command)`
  - **Host-facing events:**
    - `ShowElicitation(ElicitationResponder, ElicitationKind)`
    - `ShowArtifactReview(..)`
    - `GitReviewState(Arc<ClientState>)`
    - `TurnFinished`, `Notice(..)`
    - `SessionsListed(..)`, `ConnectionClosed`

  Hosts never get mutable access to overlays. They react to effects.
- **Composer logic**:
  - `Submission`
  - `PromptAttachment`, `build_attachments`, `encode_attachment`
  - `index_files`
  - `parse_dropped_file_paths`
  - `media_support_error`
  - `restore_config_selections`
  - the stale-result rule for prompt search, kept as a small `PromptSearchState`
- **Elicitation**: `ElicitationResponder` (answer-once, cancels on drop). `FormField` / `FormFieldKind` parsing from `ElicitationSchema` goes into `acp_utils::elicitation`.
- **`testing` feature**: `FakeExecutor`, `FakeGit` and `FakeFilesystem`, moved from `wisp/src/testing.rs`.

Stays in wisp: `runner.rs`, renderer, all `surfaces/*` views, settings overlay, themes, `GitDiffScreen`, and the bell.

### 3.2 New crate: `crates/desktop` (`aether-desktop`)
`publish = false`, and macOS and Linux only. Proposed layout:

```
crates/desktop/
  Cargo.toml
  src/
    main.rs              // parse CLI (--agent override for dev builds), call aether_desktop::run()
    lib.rs               // pub fn run(); app bootstrap
    app.rs               // pub struct DesktopApp (root view: sidebar + thread view, global actions)
    threads/
      manager.rs         // pub struct ThreadManager: owns Thread entities, selection, spawn/resume/close
      thread.rs          // pub struct Thread: SessionCore + status + driver handle; applies Effects
      driver.rs          // ThreadDriver: tokio task owning AcpClient + CommandDispatcher, mpsc in/out
      status.rs          // pub enum ThreadStatus
    target.rs            // pub enum SessionTarget; pub trait Connector; LocalConnector, WsConnector
    registry.rs          // pub struct Registry: persisted projects + thread metadata
    history.rs           // pub struct History over aether_sessions::SessionStore, grouped by project
    notification.rs      // posts gpui SystemNotifications for threads that want attention
    sidebar/             // Sidebar, ProjectSection, ThreadRow, new-thread / add-project actions
    chat/
      transcript.rs      // virtualized list of ConversationItems (patched gpui `list`)
      markdown.rs        // gpui renderer over clankerdiff_markdown::MarkdownStream
      tool_card.rs       // collapsible tool-call cards, inline diffs, sub-agent tree
      thinking.rs, plan.rs, notice.rs
      composer.rs        // gpui-base Textarea + config selectors + stop + context meter
      mentions.rs        // @file popover, slash commands, prompt search popover
      attachments.rs     // drag-drop / paste images & files
    changes.rs           // Changes tab: DiffViewer wired to GitReviewRuntime
    elicitation.rs       // inline elicitation forms + artifact review (MarkdownReviewer)
    theme.rs             // UiTheme global, follows system appearance
    runtime.rs           // tokio bridge (copied from diff-gpui-desktop)
    menus.rs, run_loop.rs, window_chrome.rs   // copied from diff-gpui-desktop
    testing.rs           // DesktopHarnessBuilder + fakes (feature "testing")
  tests/
```

### 3.3 Threads, processes and data flow
- `Thread` is a gpui `Entity`. It owns:
  - `id: ThreadId` (app-local)
  - `session_id: Option<SessionId>`
  - `target: SessionTarget`
  - `core: SessionCore`
  - `status: ThreadStatus`
  - `unread: bool`
  - `driver: Option<ThreadDriverHandle>`
- `ThreadDriver` runs on the shared tokio runtime (`runtime.rs` Global). It owns the `AcpClient`, the `AcpClientHandle`, the `CommandDispatcher` and the `GitReviewRuntime`.
  - It forwards `AcpEvent`s and `CommandResult`s to the entity over an mpsc channel. The receiving loop is `cx.spawn(async move |this, cx| while let Some(m) = rx.recv().await { this.update(cx, …) })`.
  - It receives `Command`s from the entity over a second mpsc channel.
  - Dropping the handle aborts the task.
- **Effect loop:** `Thread::apply(effects)`:
  - sends `Dispatch` effects to the driver
  - turns host effects into UI state: an elicitation slot, the git state pushed into the Changes view, a status change, a notification
  - calls `cx.notify()`
- **`ThreadStatus`**: `Connecting`, `Idle`, `Running`, `NeedsInput` (pending elicitation), `Errored(String)`, `Disconnected`, `Dormant` (history only, no process).
  - The status comes from `Conversation::turn()` / `activity()` plus the pending elicitation slot.
  - The `unread` flag is set when a turn finishes while the thread is not selected.
- **Process lifetime**:
  - `LocalConnector` spawns `aether acp` with `cwd` and `kill_on_drop(true)`.
  - Quitting drops all drivers, which kills every child.
  - **To verify:** that `aether acp` exits on stdin EOF when the app crashes. On Linux, also set `PR_SET_PDEATHSIG` via `pre_exec` if it doesn't. If that needs `unsafe`, keep it in a small, audited helper.
- **Resume**: clicking a `Dormant` history thread:
  1. `Connector::connect(target)` without creating a new session.
  2. `AgentCommand::ResumeSession { id, cwd }` with replay from start.
  3. The `Conversation` is rebuilt from the replayed updates.
- **Close/stop thread**: drops the driver (kills the process). The thread drops back to `Dormant` in history.

### 3.4 `SessionTarget` and `Connector`
```rust
pub enum SessionTarget {
    Local { cwd: PathBuf, agent: AgentCommandLine },
    Remote { url: Url, headers: Vec<(String, String)>, label: String },
}

pub trait Connector: Send + Sync + 'static {
    fn connect(&self, target: &SessionTarget, mode: ConnectMode)
        -> impl Future<Output = Result<Session, ConnectError>> + Send;
}

pub enum ConnectMode { NewSession, Resume(SessionId), AttachServerSession }
```
- **`LocalConnector`**: an `AcpAgent` process over stdio. The default agent is `aether acp`; `--agent "cargo run -p aether-agent-cli acp"` is supported for dev.
- **`WsConnector`** (phase 8): `tokio_tungstenite::connect_async` with headers, then `WebSocketTransport::new`, then `Session::connect_remote_to`. This reads `RemoteServerInfo` from the initialize metadata, as `aether client` does.
- **Later, `ClankerVmProvider`**: not a `Connector` but a producer of `SessionTarget::Remote`. It launches a VM and mints a token (`X-aws-proxy-auth`, `X-aws-proxy-port`). It also refreshes the token on reconnect, since tokens last about 60 minutes. Its VM lifecycle actions (suspend/resume/stop) appear in the thread's context menu. Prerequisite: make `clankervm`'s `MicroVmClient` public, or extract a small client library.
- Remote threads are recorded in the `Registry`, because their session logs live inside the VM and not in local `aether-sessions`.

### 3.5 Persistence
- **History** (local threads): `aether_sessions::SessionStore::new()`, read directly.
  - Grouped by the git root of `SessionMeta.cwd`, newest first.
  - Title is `SessionSummary.title`, falling back to `"New thread"`.
  - The list refreshes on app focus and whenever a thread finishes a turn.
  - Reads happen on a background thread via `cx.background_spawn`.
- **Registry**: `~/.aether/desktop/state.json`, built on `aether_utils` path and JSON helpers. It stores:
  - `projects: Vec<Project { root, name, collapsed }>`
  - `threads: HashMap<SessionId, ThreadMeta { pinned, archived, custom_title, target: Option<RemoteTarget> }>`
  - window size and last selected thread
- **UI preferences**: theme id and font size live in the same file.

### 3.6 Changes tab
- Opening the tab for the first time on a live thread sends `GitReviewCommand::Open`. That starts the `DiffClient` over `AcpClientTransport` (`_aether/git_diff`).
- `Effect::GitReviewState(state)` triggers `state.snapshot_if_changed(&mut installed)`, then `viewer.set_scope(..)` and `viewer.set_document(snapshot.document.clone(), cx)`. This mirrors `diff-gpui-desktop/src/app.rs`.
- **Tab badge** `Changes (N)`: N is the number of files in the latest snapshot. To keep the badge current, the subscription stays open while the thread is live, not just while the tab is visible. The cost is one `DiffServer` per live thread on the agent side, which is acceptable.
- **Viewer events**:
  - `SubmitReview(sub)` goes to `SessionCore::submit_review(sub.formatted)`, a normal prompt (queued if a turn is running). The UI then switches to the Chat tab.
  - `CopyFormattedReview(s)` copies to the clipboard.
  - `RepositoryAction`, `SetScope` and `Refresh` are sent as `GitReviewCommand::Event` to the agent's `DiffServer`. `Cancel` (Esc) returns to the Chat tab.
- **Shared-cwd consequence**: threads in the same project show the same diff. The header names the project root to make that clear.
- Theme: `ReviewTheme` comes from the app theme, so chat and diff match.

### 3.7 Chat
- **Transcript**:
  - A virtualized list using the patched gpui `list` with variable row heights, one row per `ConversationItem`.
  - It auto-follows the bottom unless the user has scrolled up, and shows a "jump to latest" pill.
  - Rows re-render according to each item's `Revision`.
- **Markdown** (`chat/markdown.rs`):
  - Each assistant message keeps a `MarkdownStream` and feeds it text deltas.
  - `MarkdownStreamUpdate` marks which blocks changed.
  - Blocks render to gpui elements: headings, paragraphs with inline bold/italic/code/links as `StyledText` runs, lists, block quotes, tables and rules.
  - Code blocks use `clankerdiff_syntax::SyntaxHighlighter` and have a copy button.
  - Text is selectable via gpui-base `SelectableText`.
  - This renderer is self-contained and could later move upstream into clankerdiff-gpui.
- **Tool cards**:
  - A collapsible card per `ToolCall` showing status glyph, name, a one-line summary (bash command, file path), duration, and expandable input/output.
  - `ToolCall::diffs()` renders inline through a compact `DiffViewer` or a static patch view.
  - Sub-agents render as a nested tree from `SubAgentState`.
- **Thinking**: a collapsed "Thinking…" block that streams and can be expanded.
- **Plan**: a sticky plan/todo panel above the composer, driven by `PlanTracker`.
- **Notices**: compaction, context cleared, auth/MCP status, and errors.
- **Composer**:
  - gpui-base `TextareaState::auto_grow(1, 12).submit_on_enter(true)`. Enter sends, Shift+Enter adds a newline, ⌘/Ctrl+↩ also sends.
  - Footer: model / mode / reasoning `Select`s driven by `LocalConfigView`, which emit `AgentCommand::SetConfigOption`. Also a Stop button (`CancelSessionNotification`), a context-usage ring (`Conversation::context_usage()`), and a queued-prompt indicator.
  - ↑/↓ on an empty composer walks `PromptHistory`.
- **Mentions and commands**:
  - Typing `@` opens a fuzzy file popover from `index_files(cwd)`, built in the background and cached per project. Choosing a file inserts a mention chip.
  - Typing `/` opens available commands (`available_commands` plus builtins).
  - ⌘/Ctrl+R opens prompt search (`_aether` prompt search, with stale results dropped).
- **Attachments**: drag-drop and paste for images and files, via `parse_dropped_file_paths`, `build_attachments` and `media_support_error`. Thumbnails show in the composer.
- **Elicitations**:
  - Rendered inline at the bottom of the transcript as a card, never as a modal, because many threads may each have one pending.
  - Forms are built from `FormField`s. URL elicitations open the browser.
  - Artifact review elicitations open a full-width `MarkdownReviewer` (clankerdiff-gpui), whose submission becomes an `ArtifactReviewSubmission`.
  - A pending elicitation sets the thread to `NeedsInput`.

### 3.8 Sidebar and attention
- **Layout**:
  - Top: "New thread" button (⌘/Ctrl+N, using the selected project).
  - Then collapsible `ProjectSection`s, each with a `+` on its header.
  - "Add project" opens a folder picker (`cx.prompt_for_paths`).
- **Thread rows**: status dot, title, relative time, and a remote badge where relevant.
  - Context menu: rename, pin, archive, stop. Later, VM actions.
  - Archived threads collapse into an "Archived" group per project.
- **Status dots**:
  - Running: animated pulse
  - Idle: no dot, or an "unread" dot if a turn finished unseen
  - Needs input: amber
  - Errored or disconnected: red
  - Dormant: hollow
- **Shortcuts**: ⌘/Ctrl+K opens the thread switcher (fuzzy, across projects). ⌘/Ctrl+[ and ] cycle threads, and ⌘/Ctrl+1 and 2 switch the Chat/Changes tabs.
- **Notifications**: gpui's built-in `cx.show_system_notification` (XDG over D-Bus on Linux, `UNUserNotificationCenter` on macOS). No `notify-rust` and no `Notifier` trait.
  - `ThreadManager` emits `Attention { thread, event }` when a thread the user isn't watching (unselected, or any thread while the window is inactive) finishes a turn or raises an elicitation. `DesktopApp` posts it, titled like the thread header.
  - The tag is the thread's session id, so a thread's newer notification replaces its older one.
  - Clicking a notification (`cx.on_system_notification_response`) activates the window and selects the live thread with that session.
  - macOS only delivers notifications from an `.app` bundle, so unbundled dev runs (`just desktop`) show none there.

### 3.9 App shell and theme
- Bootstrap follows `diff-gpui-desktop`:
  1. `runtime::init`
  2. `load_default_fonts`
  3. `gpui_base::init`
  4. `DiffViewer::bind_keys`, then the desktop keybindings
  5. menus
  6. `run_loop::install`
  7. `open_window` (1280×840 centred, minimum 640×480)
  8. client-side decorations on Wayland/X11
- **Theme**: a `UiTheme` global built from a `ReviewTheme`.
  - It follows `window.appearance()` / `observe_window_appearance` by default, choosing the paired light or dark built-in theme. A user override is stored in the Registry.
  - This system-appearance behaviour is new; clankerdiff has none today.

---

## 4. Workspace and dependency changes
- **Root `Cargo.toml`**:
  - Add workspace dependencies:
    - `gpui = { package = "gpui-pre", version = "=0.3.7", default-features = false }`
    - `gpui_platform = { package = "gpui-pre-platform", version = "=0.3.7", features = ["font-kit", "runtime_shaders", "wayland", "x11"] }`
    - `gpui-base = "=0.7.0"`
    - `clankerdiff-gpui`, `clankerdiff-markdown`, `clankerdiff-syntax`, `clankerdiff-theme`
  - Add a `[patch.crates-io]` block:
    ```toml
    [patch.crates-io]
    gpui-pre = { path = "../diff/patches/gpui" }
    clankerdiff-core = { path = "../diff/crates/diff-core" }
    clankerdiff-protocol = { path = "../diff/crates/diff-protocol" }
    clankerdiff-client = { path = "../diff/crates/diff-client" }
    clankerdiff-server = { path = "../diff/crates/diff-server" }
    clankerdiff-git = { path = "../diff/crates/diff-git" }
    clankerdiff-ratatui = { path = "../diff/crates/diff-ratatui" }
    # …plus every other clankerdiff crate that is published to crates.io
    ```
    Version requirements must still match: the local diff checkout is at 0.3.3 and aether requires `0.3.2`, which is compatible. Crates that aren't on crates.io, such as `clankerdiff-gpui`, are declared as plain `path` workspace dependencies instead of patches. Cargo warns about patches that are never used.
- **Consequence**: every build of this workspace now needs `../diff` checked out. Update CI to check out `jcarver989/diff` next to aether, and install gpui's Linux system dependencies (wayland, xkbcommon, vulkan/mesa, fontconfig).
- `crates/desktop` is covered by `members = ["crates/*"]`. Exclude it from `dist-workspace.toml` and release-plz.
- **wasm checks**: confirm that `just wasm-check` doesn't pull in gpui (it targets `packages/aether-browser`).
- **justfile**:
  - `desktop` runs `cargo run -p aether-desktop -- --agent 'cargo run -p aether-agent-cli acp'`
  - `desktop-test` runs `cargo nextest run -p aether-desktop -p aether-client-core`
- **Toolchain**: the workspace's 1.99 already exceeds clankerdiff's 1.98. Workspace lints (pedantic, `unreachable_pub`) apply.

---

## 5. Testing strategy
Follows AGENTS.md: fakes over mocks, state assertions, public API only, builders, no timeouts.

- **`aether-client-core`**:
  - Integration tests drive `SessionCore` with `acp_utils::testing::FakeAgent` (new, resume, replay, prompt queueing, config options, elicitation responder, git review flow with `FakeGit`).
  - Tests assert on `Conversation` and `SessionModel` state and on the emitted `Effect`s.
  - wisp's existing tests must stay green after the extraction. That is the regression gate for phase 1.
- **`aether-desktop`**:
  - `DesktopHarnessBuilder` (in `testing.rs`, feature `testing`) wraps `TestAppContext`, following the clankerdiff `DiffViewerHarnessBuilder` pattern: `simulate_keystrokes`, `simulate_input`, `click(debug_selector)`, `bounds`.
  - Builder methods:
    - `.project("/repo")`
    - `.history(sessions)`, a temp-dir `SessionStore::from_path`
    - `.agent(FakeAgent…)`
    - `.selected(thread)`
  - **`FakeConnector`**: implements `Connector` by building an in-memory `FakeAgent` per thread, with no processes. It can script per-thread replies so concurrency is testable.
  - **Notifications**: gpui's `TestPlatform` records posted notifications (`cx.shown_system_notifications()`) and simulates clicks (`DesktopHarness::click_notification`), so there's no hand-written fake.
  - Elements are tagged with `.debug_selector("thread-row-<id>")` and similar.
  - **Key scenarios**:
    - Two threads stream concurrently and each transcript only receives its own updates.
    - An unselected thread's turn finishing sets `unread` and sends exactly one notification.
    - An elicitation sets `NeedsInput`, and answering clears it.
    - Clicking a dormant history thread resumes it and replays the transcript.
    - Changes tab: a git state effect populates the viewer; `SubmitReview` sends a prompt and switches to Chat.
    - Stopping a thread drops the driver and returns it to `Dormant`.
    - Registry round-trip (pinned, archived, projects).
  - Pure logic stays in plain unit-testable functions over public types: status derivation, project grouping, markdown block mapping.
- No screenshot tests in v1.

---

## 6. Phases

Each phase ends with `just fmt`, `just lint`, `just test` and a manual run via `just desktop`.

### Phase 0: workspace plumbing
- Add the `[patch.crates-io]` overrides and the gpui workspace dependencies. Confirm wisp and aether-cli still build and test against the path clankerdiff (0.3.3).
- Make sure `clankerdiff-gpui`'s `test-support` feature is usable from a path dependency.
- CI: check out `../diff` and install the gpui system dependencies.

### Phase 1: extract `aether-client-core` from wisp
- Move the types listed in §3.1. Split `CommandResult` into core and host parts, and replace the overlay mutation in reducers with `Effect`s.
- Rewire wisp on top of it. This is a behaviour-preserving refactor and all wisp tests stay green.
- Add `connect_without_new_session` and the `ClientInfo` parameter.
- Move `FakeExecutor`, `FakeGit` and `FakeFilesystem` into `client-core`'s `testing` feature.

### Phase 2: desktop skeleton and one local thread
- Bootstrap, menus, run loop, window chrome, tokio bridge, theme with system appearance.
- `LocalConnector`, `ThreadDriver`, `Thread`, and a minimal `ThreadManager`.
- Sidebar with projects (add via folder picker) and history read from `aether-sessions`.
- Chat with a plain-text transcript, a composer that sends, and a Stop button.
- Test harness, `FakeConnector`, first integration tests.

### Phase 3: multi-thread orchestration
- Concurrent live threads, `ThreadStatus` derivation, unread tracking, status dots.
- Resume from history (dormant to live), plus stop and close.
- `Registry` persistence: pinned, archived, rename, last selection.
- Native notifications with click to focus, the ⌘K thread switcher (live threads plus saved sessions, fuzzy via `nucleo`), and ⌘[ / ⌘] cycling in sidebar order. Also bound in the `Input` key context, where gpui-base binds those keys to indent. A Threads menu lists all of these. (Done.)
- Verify that process cleanup works when the app crashes. (Done: `aether acp` exits once stdin closes, which the kernel does when the app dies, so no `PR_SET_PDEATHSIG`. Covered by `acp_stdio::an_agent_with_an_open_session_exits_once_its_client_hangs_up`. The `aether-lspd` daemons it starts are shared and shut down on idle by design.)

### Phase 4: Changes tab
- `DiffViewer` embedded under the tabs, `GitReviewRuntime` wiring, the live `Changes (N)` badge.
- Submit review sends a prompt and switches to Chat. Also scope, refresh and repository actions, and copying the formatted review.
- (Done: `src/changes.rs`. A thread opens the review as soon as its session connects and drops it when the connection ends. ⌘/Ctrl+1 and 2 switch tabs, also in the Threads menu. Tests run against `client_core::testing::FakeGitDiffServer`, a `FakeGit`-backed agent-side diff server that `FakeConnector` chains onto every fake agent.)

### Phase 5: rich chat rendering
- gpui markdown renderer over `MarkdownStream`, with syntax-highlighted code blocks and copy buttons.
- Tool cards with inline diffs, the sub-agent tree, thinking blocks, the plan panel, notices.
- Transcript auto-follow and the "jump to latest" pill.
- (Done: `src/chat/{markdown,tool_card,thinking,plan,notice}.rs`. Each reply keeps a `MarkdownStream` that is fed only the newly streamed suffix of its text. Auto-follow uses the patched list's `FollowMode::Tail`. The plan panel shows every entry of `Conversation::plan()` with its progress, and hides once all are done and the turn has ended. It does not use `PlanTracker`'s grace period, which needs a tick loop. Not yet done: selectable transcript text (gpui-base `SelectableText` only takes plain text, not styled runs) and tool-call durations (`ToolCall` has no timestamps). `FakeAgent::prompt_updates` scripts a turn's session updates for tests.)

### Phase 6: composer controls, mentions and commands
- Model, mode and reasoning selectors, the context meter, the queued-prompt indicator, prompt history.
- `@file` mentions, `/` commands, ⌘R prompt search.
- (Done: `src/chat/{composer,completion,footer,prompt_search}.rs` and `src/file_index.rs`.
  - Footer: one menu per select-type session option, single-choice only. Multi-select model lists are not offered yet, and boolean options are not shown.
  - Context meter: a bar and a percentage.
  - Queued prompts: listed above the input.
  - Mentions are gpui-base inline tokens whose id is the file's path, so the submission carries them. The file index is a per-project `Global`, walked in the background and refreshed each time the `@` menu opens.
  - `/` offers only the agent's own commands. wisp's builtins (`clear`, `resume`, `settings`, `move`) map to app controls instead.
  - Prompt history starts empty for each composer and is not seeded from a resumed conversation.
  - Test support: `FakeAgent::prompt_history` answers prompt search, `PromptSearchState::set_query` serves a text-field query, and `DesktopHarness::run_until_painted` waits for an element to be painted.)

### Phase 7: elicitations, artifact review and attachments
- Inline elicitation cards (form and URL) and the `NeedsInput` status. (Done early, after Phase 3: `src/elicitation.rs`. Artifact reviews show a decline-only card until `MarkdownReviewer` lands.)
- Artifact review through `MarkdownReviewer`.
- Drag-drop and paste attachments, with media-support checks.

### Phase 8: remote threads
- `WsConnector` and a "New remote thread" dialog (URL plus headers). Remote threads are stored in the Registry.
- Remote badge and reconnect handling (`Disconnected` status, retry with backoff as `clankervm connect` does).
- Path attachments are disabled for remote threads, matching wisp's current behaviour.

### Later (not planned in detail)
- `ClankerVmProvider`: launch, connect, suspend, resume and stop VMs from the app, with token refresh. Needs `MicroVmClient` exposed from clankervm.
- Packaging: macOS `.app`, signing and notarization; Linux AppImage or `.deb`. Requires moving off the `../diff` path dependency, either by publishing clankerdiff-gpui and upstreaming the gpui `list` patch, or with a git dependency.
- Real permission prompts with per-thread policy (needs `acp-utils` to surface `request_permission` as an event).
- Per-thread git worktrees, to isolate concurrent threads in one repo.
- Non-Aether ACP agents, with features gated on capabilities.
- Moving the gpui markdown renderer upstream into clankerdiff-gpui.

---

## 7. Risks and open items to verify during implementation
1. **Mixed clankerdiff versions.** `[patch.crates-io]` must cover every clankerdiff crate in the dependency graph, or `DiffDocument` / `Event<DocumentUpdate>` types will mismatch between acp-utils and diff-gpui. Check with `cargo tree -d -i clankerdiff-core`.
2. **gpui-pre patch drift.** The vendored `patches/gpui` sits in the diff repo, so aether must track whatever the diff repo pins.
3. **Orphaned agents on crash.** Confirm `aether acp` exits on stdin EOF. Otherwise add a parent-death signal on Linux, and on macOS rely on stdin EOF or a kqueue `NOTE_EXIT` watcher in the agent.
4. **One `DiffServer` per live thread.** Several threads in the same project each run a watcher on the same tree. That's fine at small N; revisit if N gets large.
5. **History freshness.** `SessionMeta` has only `created_at`, with no last-activity timestamp. Sorting by recency may need an `updated_at` added to `aether-sessions` (cheap: the log file's mtime).
6. **Native notification click-through** differs between platforms (D-Bus actions vs macOS). Plan for a best-effort fallback.
7. **Markdown renderer effort.** clankerdiff has no gpui streaming markdown renderer, so this is the largest new UI component.
