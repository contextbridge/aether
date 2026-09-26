# aether-acp-wasm

Browser ACP client for `aether server`, compiled to WebAssembly and packaged as `@aether-agent/browser`. It runs the same ACP v2 client that `wisp` and `aether client` use (from `aether-acp-utils`) over a browser WebSocket, reduces each session with the same conversation model `wisp` renders, and exposes both behind a `wasm-bindgen` API typed against the ACP v2 TypeScript types.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quickstart](#quickstart)
- [API](#api)
- [Events](#events)
- [Conversations](#conversations)
- [Errors](#errors)
- [Connection behavior](#connection-behavior)
- [Development](#development)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Quickstart

1. Start the server (see the [remote agent docs](https://aether-agent.io/aether/running/remote/)):

   ```bash
   aether server --cwd /workspace/project --agent Build
   ```

2. Build the npm package (requires [`wasm-pack`](https://github.com/wasm-bindgen/wasm-pack)); the output lands in `packages/aether-browser/pkg`:

   ```bash
   pnpm browser:build
   ```

3. Connect from the page, then resume the server's live session or start one in its working directory:

   ```ts
   import init, { AetherClient, type AetherClientEvent } from "@aether-agent/browser";

   await init();
   const client = await AetherClient.connect("ws://127.0.0.1:8765", (event: AetherClientEvent) => {
     switch (event.type) {
       case "conversation_changed":
         console.log(event.conversation.turn, event.conversation.items);
         break;
       case "elicitation_request":
         event.respond({ action: "decline" });
         break;
       case "connection_closed":
         console.log("disconnected");
         break;
     }
   });

   type RemoteServerInfo = { cwd: string; sessionId: string | null };
   const aether = client.initializeResponse._meta?.["contextbridge/aether"] as { remote: RemoteServerInfo };
   const { cwd, sessionId: liveSession } = aether.remote;

   let sessionId: string;
   if (liveSession) {
     // Replayed history arrives as session_update events before this resolves.
     await client.resumeSession({ sessionId: liveSession, cwd }, true);
     sessionId = liveSession;
   } else {
     sessionId = (await client.newSession({ cwd })).sessionId;
   }

   await client.prompt({ sessionId, prompt: [{ type: "text", text: "Summarize this repository" }] });
   ```

## API

| Member | Description |
|--------|-------------|
| `AetherClient.connect(url, onEvent, options?)` | Opens the socket and completes ACP initialization. `onEvent` receives every event from then on. `options.protocols` lists WebSocket subprotocols to offer, as in `new WebSocket(url, protocols)`. |
| `initializeResponse` | The agent's `InitializeResponse`: info, capabilities, auth methods, and `aether server`'s `RemoteServerInfo` under `_meta["contextbridge/aether"].remote`. |
| `newSession(request)` | `session/new`. |
| `resumeSession(request, replay)` | `session/resume`. Starts the session's conversation over; with `replay`, history arrives as `session_update` events, and rebuilds the conversation, before the promise resolves. |
| `prompt(request)` | `session/prompt`. Echoes the prompt into the session's conversation, then resolves once the agent accepts it; the turn streams as `session_update` events and ends with an idle state update. Throws `turn_in_progress` unless the session's turn is `idle`. |
| `conversation(sessionId)` | The session's current `Conversation`, or `undefined` for a session this client has not seen. |
| `cancel(sessionId)` | `session/cancel`. |
| `disconnect()` | Closes the connection without sending `session/cancel` or `session/close`. Resolves after `connection_closed` is emitted. |
| `free()` | Releases the client; dropping the last reference also closes the connection. |

## Events

`onEvent` is called in order with an `AetherClientEvent`:

- `{ type: "session_update", notification }` -- an ACP `session/update`
- `{ type: "elicitation_request", request, respond }` -- a form or URL elicitation; call `respond(response)` once to answer it
- `{ type: "context_cleared" | "sub_agent_progress" | "auth_methods_updated" | "mcp_notification" | "git_diff_event", params }` -- Aether extension notifications
- `{ type: "connection_closed", close }` -- the connection ended; no further events follow. When the server or network closed the socket, `close` holds the WebSocket close `code` and `reason`. It is `null` when this client closed it, as `disconnect()` or a binary frame does
- `{ type: "conversation_changed", sessionId, conversation }` -- follows any event, `prompt()` or `resumeSession()` call that changes a session's conversation

Permission requests are approved automatically, as in `wisp`.

## Conversations

Each session's updates, the client's own prompts, and Aether's sub-agent progress are reduced into a `Conversation` by `acp_utils::conversation`, the model `wisp` renders, so a browser UI inherits its rules:

- `items` -- messages, tool calls and client notices in order. A message keeps its content blocks, with adjacent streamed text merged into one block; a replayed or corrected message replaces its item by `messageId` rather than appending another. A tool call is the merge of all its updates, plus the tree of sub-agents it spawned. An item is `open` until its turn ends or, for a tool call, until it and its sub-agents finish; `sealed` items no longer change.
- `turn` -- `idle`, `submitting`, `running`, or `completed_before_acceptance` when the agent reached idle before answering the prompt request. A `running` state while idle adopts a turn already in progress, such as the live turn of a session this client just attached to. When a turn is cancelled or its prompt fails, its still-running tool calls end with an `error` status.
- `activity` -- what the agent is doing (`thinking`, `responding`, `requires_action`, `working`) and the reasoning it is streaming while it thinks. Activity after a turn ends is ignored.
- `plan`, `contextUsage`, `compacting` -- the latest plan, context-window usage, and whether the agent is compacting its context.

Snapshots are immutable and share structure: an item that did not change since the previous snapshot is the same object, so UI frameworks can skip it. Aether's `_aether/context_cleared` notification clears every conversation.

## Errors

Methods throw (or their promises reject with) an `Error` whose `code` is:

- `connect_failed` -- the socket could not open, or it closed before initialization completed. If the socket closed, the error's `close` holds the WebSocket close `code` and `reason`; a socket that never opened reports `1006`
- `protocol` -- the agent rejected a request, or the connection is closed
- `invalid_argument` -- a request (or elicitation response) does not match its ACP type; a malformed elicitation response also answers the agent with `invalid_params`
- `turn_in_progress` -- `prompt()` was called while the session's turn was not `idle`

## Connection behavior

- `aether server` accepts **one client at a time**. Browsers do not expose the HTTP status of a rejected WebSocket handshake, so its `409 Conflict` surfaces as `connect_failed`.
- Pages served over `https://` can only open `wss://` sockets. The raw server has no TLS or authentication, so put an authenticating TLS gateway in front of it. Browsers cannot add custom headers to a WebSocket handshake, so the gateway must authenticate with cookies, the URL, or subprotocols offered through `options.protocols` (such as the `lambda-microvms.*` auth subprotocols of a Lambda microVM).
- The endpoint must select one of the offered subprotocols, or the browser fails the handshake. `aether server` selects none, so offer `protocols` only to a gateway that does.
- There is **no automatic reconnect**. After `connection_closed`, connect again and resume the session with `replay` to catch up. Detaching does not stop a running turn; the server cancels pending elicitations when the client disconnects.
- An elicitation that is never answered keeps its `respond` function alive until the page unloads.
- Text frames carry JSON-RPC messages. Binary frames fail the connection, and the browser answers the server's keepalive pings.

## Development

```bash
just wasm-check   # clippy for wasm32-unknown-unknown
just wasm-test    # wasm tests in Node (>= 22) against a fake agent, then build and type-check the npm package
```

`just wasm-test` needs `wasm-pack`. On the host target this crate compiles to nothing; its code only builds for `wasm32-unknown-unknown`.

## License

MIT
