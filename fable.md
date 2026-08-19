TL;DR

The prototype's direction is right — a real aether mcp subprocess talking to the host over a Unix socket beats both command interception and an embedded shell. What made it 3,000 lines is not the architecture but three things: (1) about a third of the diff is unrelated work bundled in, (2) the McpManager actor fuses connection lifecycle with catalog reads and tool calls, so every new consumer must be threaded through the command loop with oneshot replies, background tasks, and event-sync machinery, and (3) the socket is created too late in construction, forcing mutable shared state (BashEnvironment) and callbacks to inject the env var. My proposal: refactor main to split the manager into a control plane (actor) and a data plane (shared read-only catalog snapshot), after which this feature is ~500 lines and the manager itself gets simpler than it is on main today.

Where the 3,050 lines actually went

┌──────────────────────────────────────────────────────────────────────┬────────┬───────────────────┐
│                                Bucket                                │ ~Lines │      Verdict      │
├──────────────────────────────────────────────────────────────────────┼────────┼───────────────────┤
│ Essential mechanism (UDS transport + gateway + aether mcp CLI +      │        │ Keep the idea,    │
│ tests)                                                               │ ~950   │ shrink the        │
│                                                                      │        │ implementation    │
├──────────────────────────────────────────────────────────────────────┼────────┼───────────────────┤
│ Actor tax: 3 new McpCommand variants, oneshot plumbing,              │        │ Symptom of the    │
│ background_operations, de-async'd list_prompts/get_prompt,           │ ~800   │ real smell —      │
│ McpManagerEvent::ToolCatalogChanged,                                 │        │ should not exist  │
│ CatalogReplacement/CatalogSnapshot/emit_catalog_delta                │        │                   │
├──────────────────────────────────────────────────────────────────────┼────────┼───────────────────┤
│ Construction-order tax: BashEnvironment (Arc<RwLock<Vec<…>>>),       │        │                   │
│ extend_environment callback, with_progressive_discovery closure      │ ~200   │ Should not exist  │
│ wiring                                                               │        │                   │
├──────────────────────────────────────────────────────────────────────┼────────┼───────────────────┤
│ Bundled unrelated work: ToolFilter feature + settings + wisp UI,     │        │ Real              │
│ agent-sync refactor (synchronize_agent moved into mcp_builder), bash │ ~550   │ improvements,     │
│  process-group/killpg hardening                                      │        │ wrong PR          │
├──────────────────────────────────────────────────────────────────────┼────────┼───────────────────┤
│ Rename churn: proxy → deferTools across config, docs, website, tests │ ~550   │ Mechanical, split │
│                                                                      │        │  into its own PR  │
└──────────────────────────────────────────────────────────────────────┴────────┴───────────────────┘

Root-cause diagnosis

1. The manager actor is the bottleneck for everything. On main, McpManager lives inside run_mcp_task and all access goes through McpCommand. The gateway is just a second reader of the catalog and a second caller of tools — but because the only door is the command channel, the prototype had to add ListDeferredServers/ListDeferredTools/ExecuteDeferredTool variants, build McpCommandClient (a second facade over the same manager, 164 lines), spawn everything as background operations so a slow list_tools doesn't stall the loop (there's a whole test asserting this), and then sync results back into the actor's cached state via McpManagerEvent. That's ~1,000 lines of ceremony around what is conceptually two reads and one call.

The telling detail: call_tool(client, params, options) is already a free function taking an Arc<RunningService>. Tool calls never needed the actor — only resolving the client did.

2. Late socket binding forced shared mutable state. The coding server factory (which owns the Bash tool) is registered before the gateway socket exists, so the prototype invented a mutable BashEnvironment plus an extend_environment callback so the socket path could be injected after the fact. That's pure construction-ordering debt: the socket path doesn't depend on anything — bind it first (or derive a session-scoped path up front) and the env becomes a plain immutable list.

3. Freshness solved in the wrong direction. ToolCatalogChanged events exist so the manager's cached tool list stays fresh enough to validate deferred calls against. But whether a tool is deferred/allowed is a pure function of the exposure rules and the name — and the MCP server itself is the authority on whether a tool exists. Forward the call and let the server reject unknown tools; the entire event-sync loop disappears.

Alternatives considered

Bash tool parses/intercepts aether mcp commands. Tempting because CodingMcp is in-process — the trivial case needs no IPC at all. But the feature's own instructions advertise pipelines (aether mcp linear list_issues | jq …), and interception can't handle pipes, $(…), &&, or scripts the model writes and runs with bash script.sh without reimplementing shell semantics. Partial shell parsing is a compatibility minefield with confusing failure modes. Reject.

Embedded interpreter (brush) with an aether builtin. Genuinely elegant: builtins compose natively with pipes, no IPC for the common path. But it swaps the engine under your most-used tool for a beta-maturity interpreter to serve a discovery feature, and it still doesn't eliminate IPC — the moment the model runs bash script.sh containing aether mcp calls, you're in a real bash child that needs a real binary and a socket anyway. So you'd carry both mechanisms. Reject for now; worth a future spike only if you want brush for other reasons (Windows, sandboxing, session state).

Keep main's file-based tool proxy. The thing being replaced. The CLI UX is strictly better (live help trees, no stale JSON dirs, shell composability), so the branch's direction stands.

Subprocess + UDS speaking real MCP (the prototype's transport). Correct choice. rmcp gives you serve/client for free, and a nice emergent property: anything — including a subagent process — can mount the gateway socket as an ordinary MCP server. Keep MCP-over-UDS; don't invent an ad-hoc JSON protocol.

Proposal

Phase 0 — unbundle

Land as independent PRs against main: the proxy → deferTools rename, the bash process-group/kill-on-drop hardening, ToolFilter, and the agent-sync consolidation (synchronize_agent). Each is a good change; together they're half the diff and all the review noise.

Phase 1 — refactor main: split control plane from data plane

This is the "make main easier to build on" refactor, valuable independent of this feature.

- The actor keeps only connection lifecycle: connect, reconnect, OAuth, shutdown, status transitions.
- It publishes a catalog snapshot via watch::channel<McpCatalog>, where the catalog holds, per server: name, description, exposure rules, cached tool definitions, and the Arc<RunningService> client handle. Cheap to clone, readable by anyone, no channel round-trip.
- Consumers go direct: the agent resolves a client from the snapshot and uses the existing call_tool free function (cancellation and progress events already live there, not in the actor). list_prompts, get_prompt, and server statuses become snapshot reads plus direct client calls.
- End state: McpCommand shrinks to roughly AuthenticateServer and shutdown; run_mcp_task loses ExecuteTool, ListPrompts, GetPrompt, the background_operations JoinSet, and all the non-blocking gymnastics. Main gets net simpler.

One design note to handle deliberately: snapshots holding Arc clients can keep a connection alive past shutdown_server. Publish the removal in a new snapshot and treat connection close as last-Arc-drop (or an explicit close on the running service) so readers fail fast rather than talking to a zombie.

Phase 2 — the feature, now small

- Bind the socket first. Create the UDS endpoint (keep the prototype's 0700 runtime-dir and guard — that part is good) at session construction, before any server factory runs. The env injection becomes a plain Vec<(OsString, OsString)> (socket path + PATH with the aether binary prepended) passed into CodingMcp at build time. BashEnvironment's lock and the callback wiring are deleted.
- Gateway = a ~100-line stateless ServerHandler over the watch::Receiver<McpCatalog>: list_tools filters the snapshot to deferred+allowed tools; call_tool checks the name against exposure rules, resolves the Arc client, and awaits call_tool directly. Each UDS connection is already its own task, so a slow tool call blocks only its own caller — no shared loop, no background-op machinery, no "does discovery block dispatch" test scaffolding. For freshness, forward list_tools for deferred servers to the live server; nothing syncs back into the manager.
- Keep the CLI, slim the arg parsing. The four input styles (key=value, --key value, --args, stdin) are the biggest chunk of mcp_command.rs and carry a correctness footgun: values are speculatively parsed as JSON, so version=1.10 silently becomes the number 1.1 while zip=01234 stays a string. Support JSON only — one positional JSON object or stdin — and the parsing code roughly halves while becoming predictable. Models are excellent at emitting JSON; the key=value sugar buys little.
- Instructions render from the same snapshot (the deferred-server list), essentially as the prototype does.

Expected outcome

Phase 1 is roughly a wash or net-negative in lines (the actor sheds more than the snapshot adds). Phase 2 lands at ~500–700 lines including tests, versus ~1,750 of essential-path code in the prototype — and with no new event types, no second manager facade, and no shared mutable state. The gateway also falls out as a general capability: any external process gets a real MCP endpoint into the session, which subagents can reuse for free.

If you want, I can turn Phase 1 into a concrete refactor plan against the current manager.rs/run_mcp_task.rs (what moves where, in what order, and which tests carry over).
