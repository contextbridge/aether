# Proposal: rebuild around a first-class deferred-tool catalog
  
  ## Recommendation
  
  Preserve the prototype’s user experience—deferred tools callable from real Bash—but do not merge or
  incrementally clean up the branch as-is.
  
  Instead:
  
  1. Refactor main first around a narrow, session-scoped MCP catalog/router API.
  2. Keep real Bash and a thin aether mcp subprocess over a Unix socket if arbitrary shell composition
  is a hard requirement.
  3. Make the gateway and CLI mere adapters over the router, rather than owners of discovery, filtering,
  lifecycle, or agent synchronization.
  4. Do not have the Bash tool parse shell commands.
  5. Do not adopt Brush yet. Run a small feasibility spike only after the MCP refactor, and only if
  replacing system Bash is independently desirable.
  
  The process boundary is not the fundamental smell. If this must work:
  
  aether mcp linear list_issues --args '{"state":"open"}' |
    jq -r '.[].identifier'
  
  then a real Bash subprocess needs some way to call back into Aether. IPC is the natural boundary. The
  smell is that the branch forced this adapter through too much of the MCP manager, runtime, agent,
  config, and synchronization architecture.
  
  ───────────────
  
  ## What the branch demonstrates
  
  The prototype has a good product direction:
  
  - Deferred tool schemas stay out of the initial model context.
  - Discovery is progressive:
    - aether mcp --help
    - aether mcp <server> --help
    - aether mcp <server> <tool> --help
  - Invocation works inside real Bash, including pipes, redirects, &&, and command substitution.
  - MCP credentials stay in the owning Aether process.
  - The current branch exposes only an active-session socket through AETHER_MCP_IPC_SOCKET.
  - Existing direct/deferred partitioning and tool filters remain authoritative.
  
  But its implementation cost is too high:
  
  - 75 files changed
  - +3,050 / -1,719 lines
  - Production code alone: approximately +2,529 / -1,312
  - 10 files added, 2 deleted, 63 modified
  
  The largest changes are telling:
  
  ┌───────────────────────────────────────┬─────────────┐
  │ Area                                  │       Churn │
  ├───────────────────────────────────────┼─────────────┤
  │ mcp-utils/src/client/manager.rs       │ +586 / -232 │
  │ aether-core/src/mcp/run_mcp_task.rs   │  +337 / -50 │
  │ aether-core/src/mcp/mcp_builder.rs    │  +239 / -52 │
  │ aether-cli/src/mcp_command.rs         │        +236 │
  │ aether-core/src/mcp/command_client.rs │        +164 │
  │ Bash implementation                   │  +124 / -33 │
  └───────────────────────────────────────┴─────────────┘
  
  A shell-facing adapter should not require broad changes to the agent lifecycle and MCP catalog
  synchronization.
  
  ───────────────
  
  # Why it became so complex
  
  ## 1. McpManager owns too many concerns
  
  On main, McpManager in crates/mcp-utils/src/client/manager.rs simultaneously owns:
  
  - Server connection lifecycle
  - OAuth state
  - Server ordering
  - Tool catalog storage
  - Direct/proxied exposure policy
  - Tool routing
  - Tool filtering assumptions
  - Generated discovery files
  - Synthetic proxy tool generation
  - Server instructions
  - Status projection
  - Event publication
  
  The branch then added:
  
  - Deferred server discovery
  - Deferred tool discovery
  - Deferred execution routing
  - Catalog refresh operations
  - Catalog snapshots and deltas
  - Progressive-discovery instructions
  
  That turned a shell adapter into a manager rewrite.
  
  ## 2. In-memory MCP servers are constructed too early
  
  This is the most important structural issue.
  
  On main, McpConfig::into_servers eagerly invokes an in-memory ServerFactory:
  
  config parsing
    -> server_factory(args, input).await
    -> Box<dyn DynService>
    -> McpTransport::InMemory
    -> later: McpBuilder::spawn()
  
  The runtime command channel/router does not exist when the coding server is built. Therefore the
  coding Bash implementation cannot simply receive a session-scoped MCP handle.
  
  The prototype works around that with:
  
  - Shared mutable BashEnvironment
  - A callback to extend it after gateway binding
  - PATH injection
  - Socket environment injection
  - Gateway lifecycle attached later
  
  This is architectural friction caused by eager construction.
  
  It also explains the existing limitation that in-memory McpServers cannot be cloned:
  McpTransport::InMemory stores an already-created boxed service rather than a reusable service
  specification.
  
  ## 3. The manager channel leaks as an enum
  
  McpCommand is both:
  
  - The private actor protocol used by run_mcp_task
  - The public capability surface needed by agents, prompts, CLI gateways, and runtimes
  
  Adding a consumer therefore means adding enum variants, oneshot response plumbing, dispatch branches,
  error translation, and lifecycle handling.
  
  The branch’s McpCommandClient is directionally correct, but it arrived after the actor protocol had
  already leaked through much of the codebase.
  
  ## 4. Agent synchronization is owned by each host
  
  On main, MCP events are interpreted in ACP/runtime-specific code to:
  
  - Update tools
  - Update instructions
  - Maintain current snapshots
  - Forward statuses and elicitation
  
  The branch sensibly tries to centralize this in McpSession::connect_agent, but that becomes entangled
  with the deferred gateway change and expands the feature’s blast radius.
  
  This should be an independent refactor.
  
  ## 5. Discovery re-fetches and mutates the catalog
  
  The branch calls list_tools again while servicing deferred discovery and then feeds the result back
  into McpManager through an internal event channel.
  
  That creates:
  
  - Background discovery operations
  - Manager self-events
  - Catalog replacement states
  - Before/after catalog snapshots
  - Delta calculation
  - Additional concurrency tests
  
  This is unnecessary for the initial design. The manager already obtains and caches tool definitions
  when a server connects. Progressive discovery should read that catalog. Dynamic tool-list refresh
  should be one general MCP capability shared by direct and deferred tools, not something introduced
  specifically by the shell gateway.
  
  ## 6. The CLI grammar is doing too much
  
  aether mcp supports:
  
  - key=value
  - --key value
  - --args '<JSON object>'
  - JSON on stdin
  - JSON type coercion
  - Duplicate detection
  - Mutually exclusive input modes
  - Dynamic help
  
  This is pleasant, but it adds a substantial parser and test surface before the architecture is stable.
  
  ───────────────
  
  # Target architecture
  
  The core should have one authoritative session-scoped MCP capability:
  
                    ┌───────────────────────┐
                    │ MCP connections/OAuth │
                    │     McpManager        │
                    └───────────┬───────────┘
                                │ owns
                    ┌───────────▼───────────┐
                    │      ToolCatalog      │
                    │ definitions/exposure  │
                    │ filter/status/instr.  │
                    └───────────┬───────────┘
                                │ exposed through
                    ┌───────────▼───────────┐
                    │       McpHandle       │
                    │ snapshot/list/call    │
                    │ prompt/auth/cancel    │
                    └──────┬────────┬───────┘
                           │        │
                 direct agent      deferred adapters
                   tool calls       ├─ gateway over UDS
                                    ├─ `aether mcp`
                                    └─ future embedded shell
  
  ## 1. ToolCatalog
  
  Extract a pure catalog projection from McpManager.
  
  Conceptually:
  
  pub struct ToolCatalog {
      servers: Vec<ServerCatalogEntry>,
  }
  
  pub struct ServerCatalogEntry {
      name: String,
      description: String,
      instructions: Option<String>,
      status: McpServerStatus,
      tools: Vec<CatalogTool>,
  }
  
  pub struct CatalogTool {
      namespaced_name: String,
      local_name: String,
      definition: ToolDefinition,
      exposure: ToolExposure,
      allowed: bool,
  }
  
  It should answer, without I/O:
  
  - Which tools are model-visible?

  - Which tools are deferred?
  - Which deferred servers are discoverable?
  - What is a tool’s complete schema?
  - What instructions should be shown to the model?
  - Is a route permitted by exposure and the agent’s tool filter?
  
  The manager remains responsible for connections and replacing catalog entries when the server reports
  a change.
  
  ### Important rule
  
  There must be one catalog. Do not maintain:
  
  - One list for model-visible tools
  - Another generated filesystem catalog
  - Another gateway list
  - Another agent snapshot with independent filtering
  
  Those should all be projections of the same state.
  
  ## 2. McpHandle
  
  Expose a typed facade over the manager actor. The actor enum remains private.
  
  For example:
  
  #[derive(Clone)]
  pub struct McpHandle {
      tx: Sender<ManagerRequest>,
  }
  
  impl McpHandle {
      pub async fn snapshot(&self) -> Result<McpSnapshot, McpHandleError>;
      pub async fn list_deferred_servers(&self) -> Result<Vec<DeferredServer>, McpHandleError>;
      pub async fn list_deferred_tools(
          &self,
          server: &str,
      ) -> Result<Vec<ToolSummary>, McpHandleError>;
      pub async fn describe_deferred_tool(
          &self,
          server: &str,
          tool: &str,
      ) -> Result<ToolDefinition, McpHandleError>;
      pub async fn call(
          &self,
          route: ToolRoute,
          arguments: Map<String, Value>,
          options: CallOptions,
      ) -> Result<ToolCallStream, McpHandleError>;
  }
  
  ToolRoute should encode intent:
  
  pub enum ToolRoute {
      ModelVisible { server: String, tool: String },
      Deferred { server: String, tool: String },
  }
  
  That prevents the gateway from accidentally invoking direct-only tools and keeps policy enforcement in
  one place.
  
  The handle should also own consistent handling of:
  
  - Cancellation
  - Timeout
  - MCP Task completion
  - Tool filters
  - Tool exposure
  - Trace metadata
  - Error normalization
  
  The CLI and agent should not construct manager command variants directly.
  
  ## 3. Lazy in-memory server construction
  
  Change the in-memory transport from:
  
  InMemory {
      server: Box<dyn DynService<RoleServer>>,
  }
  
  to something like:
  
  InMemory {
      spec: InMemoryServerSpec,
  }
  
  Then instantiate the server during McpBuilder::spawn, after runtime services exist:
  
  pub struct RuntimeServices {
      pub mcp: McpHandle,
      pub root_dir: PathBuf,
      pub agent_deps: AgentDeps,
  }
  
  The factory becomes conceptually:
  
  Fn(InMemoryServerSpec, RuntimeServices) -> Future<DynService>
  
  This gives the coding server a normal McpHandle, eliminating the environment-extension callback hack.
  
  It also potentially makes in-memory server configurations cloneable because the transport stores a
  reusable spec/factory reference rather than a consumed boxed service.
  
  ## 4. Centralized session/agent synchronization
  
  Extract MCP-to-agent synchronization independently of the gateway feature:
  
  pub struct McpSession {
      handle: McpHandle,
      events: Receiver<McpEvent>,
      runtime: McpRuntime,
  }
  
  A session should offer one operation for connecting catalog updates to an agent. ACP, headless, TUI,
  and subagents should not each interpret ToolDefinitionsChanged and ServerInstructionsUpdated.
  
  This is useful regardless of deferred tools and should land before the feature.
  
  ## 5. Thin adapters
  
  Once the above exists, deferred invocation surfaces become small adapters.
  
  ### Gateway adapter
  
  Responsibilities only:
  
  - Bind a private local endpoint
  - Convert MCP tools/list to McpHandle catalog queries
  - Convert MCP tools/call to McpHandle::call
  - Disconnect/cancel when the client disappears
  - Own and clean up the socket
  
  It should not own:
  
  - Tool filtering
  - Exposure policy
  - Catalog refreshing
  - Agent synchronization
  - Server instructions
  - OAuth
  - Tool result semantics
  
  The branch’s reuse of rmcp for the gateway is reasonable. Avoid inventing another request/response
  protocol.
  
  ### CLI adapter
  
  Responsibilities only:
  
  - Parse list, describe, or call
  - Connect to the inherited session endpoint
  - Print JSON
  - Map usage/runtime errors to exit codes
  
  A smaller initial grammar would be preferable:
  
  aether mcp list
  aether mcp list linear
  aether mcp describe linear create_issue
  aether mcp call linear create_issue --json '{"title":"Bug"}'
  printf '%s' '{"title":"Bug"}' | aether mcp call linear create_issue
  
  I would initially omit key=value, --key value, and heuristic JSON coercion. Agents already generate
  JSON reliably, and jq composition remains available.
  
  Aliases can later provide the prototype’s friendlier syntax if telemetry shows it matters.
  
  ───────────────
  
  # Bash architecture options
  
  ## Option A: exact whole-command interception
  
  The coding Bash tool recognizes only commands whose entire parsed input is an aether mcp invocation
  and dispatches directly through McpHandle.
  
  Example supported:
  
  aether mcp call linear list_issues --json '{}'
  
  Examples not supported:
  
  aether mcp call linear list_issues --json '{}' | jq .
  cd /tmp && aether mcp call linear list_issues --json '{}'
  x=$(aether mcp call linear list_issues --json '{}')
  
  ### Assessment
  
  - Lowest implementation cost
  - No IPC
  - No process startup
  - No PATH manipulation
  - But substantially worse than the prototype’s UX
  - Easy for the model to accidentally leave the supported subset
  
  This is viable only if arbitrary shell composition is explicitly not required. It must fail closed and
  document the restricted grammar; it must not silently fall back or partially interpret commands.
  
  ## Option B: parse arbitrary shell commands in the Bash tool
  
  Reject this.
  
  Correctly handling:
  
  - Quotes
  - Escapes
  - Variables
  - Command substitution
  - Pipelines
  - Redirection
  - Subshells
  - Functions
  - Loops
  - Here documents
  - &&/||
  - Background jobs
  
  means implementing or embedding a shell parser and execution model. A starts-with check is not
  architecture; it is a growing collection of incorrect special cases.
  
  ## Option C: real Bash plus UDS helper
  
  This is my recommendation if composition is required.
  
  coding__bash
    -> bash -c 'aether mcp ... | jq ...'
         -> small `aether mcp` process
              -> private session socket
                   -> gateway adapter
                        -> McpHandle
  
  After the core refactor, this should require changes mainly in:
  
  - Gateway adapter
  - CLI command
  - Bash environment/PATH wiring
  - Session composition
  
  It should not require manager, agent, ACP, headless, and subagent rewrites.
  
  ### Security and policy
  
  The current branch relies on a socket inside a private 0700 runtime directory. That is reasonable for
  a same-user local capability, but the final design should explicitly decide whether it also needs a
  per-session bearer token.
  
  More importantly:
  
  - McpHandle::call(ToolRoute::Deferred) must re-check exposure and the agent’s tool filter.
  - The gateway must never offer direct-only tools.
  - Credentials remain only in the manager.
  - Socket paths and any token must not appear in logs or tool results.
  - Disconnecting the CLI must cancel the routed call.
  - The Bash permission decision is the outer human-approval boundary.
  - Inner MCP invocations should still emit structured audit/telemetry records, even though the model
  sees only the containing Bash tool call.
  
  ## Option D: abandon Bash invocation and expose a normal discovery tool
  
  A much smaller design is:
  
  deferred__list_servers
  deferred__list_tools
  deferred__describe_tool
  deferred__call_tool
  
  This keeps schemas progressive without any shell bridge.
  
  It would be the best option if the only requirement is reducing initial context size. It loses
  ordinary shell composition, but it also preserves structured tool calls, UI rendering, annotations,
  and cancellation more naturally.
  
  I would reconsider whether aether mcp is essential or merely aesthetically preferable. If pipeline
  composition is not being used in real sessions, this is the simplest long-term architecture.
  
  ───────────────
  
  # Brush assessment
  
  Brush is technically promising:
  
  - Rust-native, embeddable via brush_core::Shell
  - Bash/POSIX syntax
  - Pipelines, redirections, substitutions, control flow
  - Shell::run_string and run_dash_c_command
  - Custom builtin registration through ShellBuilder::builtin
  - Configurable working directory, environment, file descriptors, and process cleanup
  
  This would allow an in-process aether or mcp builtin to participate naturally in pipelines without
  IPC.
  
  However, I would not adopt it for this feature now.
  
  ## Key concerns
  
  ### 1. Custom builtins cannot easily capture session state
  
  Brush’s Registration stores a CommandExecuteFunc, and that type is a plain function pointer:
  
  fn(
      ExecutionContext<'_, SE>,
      Vec<CommandArg>,
  ) -> BoxFuture<'_, Result<ExecutionResult, Error>>
  
  It is not a closure and cannot capture a session-specific McpHandle.
  
  ShellExtensions currently exposes error formatting, not arbitrary application state. Injecting a
  router would therefore require:
  
  - A global/session registry,
  - Encoding a handle in shell state,
  - Forking Brush,
  - Or an upstream extension point.
  
  That recreates the exact state-injection problem we are trying to remove.
  
  ### 2. It changes Bash semantics
  
  Brush reports strong compatibility, but its compatibility reference still documents:
  
  - Roughly 125 known test failures
  - Partial signal traps
  - Unsupported select
  - Unsupported wait -n
  - Unreliable $!
  - Missing disown and logout
  - IFS, printf, arithmetic, and alias edge cases
  
  The coding Bash tool currently means actual bash -c. Changing that is a product behavior change, not
  an implementation detail.
  
  ### 3. Larger dependency and maintenance surface
  
  brush-core brings a full shell interpreter and a substantial dependency graph. Its MSRV is 1.88, which
  is compatible with this repository’s 1.97 toolchain, so compiler version is not the blocker. The
  blocker is becoming responsible for shell compatibility and process behavior.
  
  ## Appropriate Brush spike
  
  After the MCP handle refactor, a time-boxed spike could answer:
  
  1. Can a session-specific Arc<McpHandle> be injected without global mutable state or a fork?
  2. Can stdout/stderr and pipelines preserve current BashOutput behavior?
  3. Do timeout and cancellation kill all external process descendants?
  4. Do the repository’s existing Bash integration tests pass unchanged?
  5. Do common agent-generated commands behave identically to system Bash?
  6. Is build-time and binary-size growth acceptable?
  
  Unless all six pass, keep real Bash.
  
  ───────────────
  
  # Phased implementation plan
  
  ## Phase 1: behavior-preserving MCP refactor
  
  No deferred-tool UX changes yet.
  
  1. Introduce McpHandle.
  2. Keep the actor command enum private.
  3. Extract catalog projection from McpManager.
  4. Move filtering/exposure authorization into catalog/router methods.
  5. Make in-memory service construction lazy.
  6. Centralize MCP-to-agent synchronization in McpSession.
  7. Preserve the existing proxy behavior while doing this.
  
  This phase should ideally be net-neutral or net-negative in code, especially after simplifying the
  existing 337-line tool_proxy.rs.
  
  ## Phase 2: replace proxy with deferred catalog semantics
  
  1. Rename the internal concept from Proxied to Deferred.
  2. Stop generating proxy__call_tool.
  3. Stop writing tool definitions to $AETHER_HOME/tool-proxy.
  4. Project deferred server/tool summaries from the in-memory catalog.
  5. Expose list, describe, and deferred call through McpHandle.
  6. Apply the same tool filter at discovery and execution.
  
  Do not re-fetch tools/list from every server for each help request. Use the catalog populated at
  connection time.
  
  Dynamic notifications/tools/list_changed handling should be a separate general capability that
  refreshes both direct and deferred projections.
  
  ## Phase 3: add the selected adapter
  
  If shell composition is required:
  
  1. Add the small local gateway.
  2. Add the minimal aether mcp list|describe|call CLI.
  3. Inject its endpoint into the coding Bash environment at lazy server construction time.
  4. Keep true bash -c.
  5. Add structured auditing for nested MCP calls.
  
  If composition is not required, skip the gateway and expose normal progressive MCP tools instead.
  
  ## Phase 4: optional ergonomic syntax
  
  Only after the architecture is stable:
  
  - key=value
  - --key value
  - Typed coercion
  - Dynamic completion
  - Human-terminal standalone mode, if independently valuable
  
  These are UX enhancements, not core architecture.
  
  ## Phase 5: optional Brush spike
  
  Do not block the deferred-tools work on it.
  
  ───────────────
  
  # Acceptance criteria
  
  I would use explicit complexity and behavior gates.
  
  ## Architecture
  
  - Gateway and CLI depend only on McpHandle, not McpManager.
  - Coding server construction receives runtime services directly; no mutable environment-extension
  callback.
  - Agent code never sees the manager actor command enum.
  - One catalog drives model-visible tools, deferred discovery, instructions, status, and authorization.
  - No feature-specific catalog refresh event loop.
  
  ## Complexity budget
  
  The final deferred Bash feature, after the refactor, should target:
  
  - Fewer than roughly 10 production files changed
  - A few hundred new production lines, not thousands
  - No broad ACP/headless/subagent changes
  - No second tool catalog
  - No custom IPC protocol if rmcp already provides the required framing
  
  The foundational refactor can touch more files, but it must be independently justified and leave
  simpler abstractions behind.
  

  ## Public-API integration tests
  
  Tests should cover:
  
  1. Deferred tools absent from initial model tool definitions.
  2. Deferred server/tool discovery from the cached catalog.
  3. Direct tools rejected by the deferred route.
  4. Agent allow/deny filters enforced in discovery and execution.
  5. Complete input schema returned by describe.
  6. JSON call results composable with jq.
  7. Pipes, &&, redirects, and command substitution through real Bash.
  8. CLI disconnect cancels an active tool call.
  9. MCP Task/deferred result completion works through the gateway.
  10. OAuth credentials are never exported.
  11. Gateway disappears and its socket is removed when the session drops.
  12. Catalog updates affect both direct and deferred projections.
  13. Bash permission behavior remains unchanged.
  14. Nested MCP calls produce audit/telemetry events.
  
  ───────────────
  
  # Final decision
  
  The best architecture is:
  
    A first-class, in-memory MCP catalog/router with thin presentation adapters.
  
  For the prototype’s full UX, retain:
  
    Real Bash + a very small aether mcp process + private UDS + McpHandle.
  
  Reject:
  
  - Ad hoc Bash parsing
  - Starts-with interception presented as full shell support
  - Making the gateway own catalog refresh or policy
  - Adopting Brush as part of this feature before proving state injection and Bash compatibility
  
  The branch’s IPC idea is sound. The excessive complexity is evidence that main lacks a clean runtime
  capability boundary—not evidence that IPC itself is wrong.
  
  No files were modified; the working tree remains clean on main.
