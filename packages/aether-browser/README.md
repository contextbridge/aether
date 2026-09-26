# @aether-agent/browser

Browser ACP client for `aether server`, compiled to WebAssembly from [`crates/acp-wasm`](https://github.com/contextbridge/aether/tree/main/crates/acp-wasm). It connects over WebSocket (`ws://127.0.0.1:8765` by default), speaks ACP v2, and reduces each session into the same conversation model the `wisp` TUI renders.

This package is not published yet. Build it with [`wasm-pack`](https://github.com/wasm-bindgen/wasm-pack):

```bash
pnpm browser:build
```

The build writes the module and its generated `.d.ts` to `pkg/`. The declarations import their request, response and notification types from `@agentclientprotocol/sdk/experimental/v2`, a peer dependency.

```ts
import init, { AetherClient } from "@aether-agent/browser";

await init();
const client = await AetherClient.connect("ws://127.0.0.1:8765", (event) => {
  if (event.type === "conversation_changed") console.log(event.conversation.items);
});
const { sessionId } = await client.newSession({ cwd: "/workspace/project" });
await client.prompt({ sessionId, prompt: [{ type: "text", text: "hello" }] });
await client.disconnect();
```

See the [crate README](https://github.com/contextbridge/aether/tree/main/crates/acp-wasm#readme) for the full API, events, the conversation model, error codes and connection behavior. `pnpm typecheck` compiles `test/usage.ts` against the built declarations.

[`packages/aether-browser-playground`](../aether-browser-playground) is a React + assistant-ui UI built on this package. `just playground` serves it against a running `aether server`.
