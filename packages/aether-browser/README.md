# @aether-agent/browser

A ACP client for `aether server`, that runs in the browser. It is written in Rust and compiles to WebAssembly. 

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quickstart](#quickstart)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Quickstart

1. Start the server (see the [remote agent docs](https://aether-agent.io/aether/running/remote/)):

   ```bash
   aether server --cwd /workspace/project --agent Build
   ```

2. Install the package:

   ```bash
   pnpm add @aether-agent/browser @agentclientprotocol/sdk
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
         event.elicitation.respond({ action: "decline" });
         break;
       case "connection_closed":
         console.log("disconnected");
         break;
     }
   });

   const { cwd, sessionId } = client.remote!;

   if (sessionId) {
     // Replayed history arrives as session_update events before this resolves.
     await client.resumeSession({ sessionId, cwd, replayFrom: { type: "start" } });
   } else {
     await client.newSession({ cwd });
   }

   await client.prompt([{ type: "text", text: "Summarize this repository" }]);
   ```

## License

MIT
