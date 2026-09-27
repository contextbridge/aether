import { createRequire } from "node:module";
import { resolve } from "node:path";
import { generateTypes } from "@aether-agent/schema-codegen";

await generateTypes({
  cargo: ["-p", "aether-browser", "--example", "typescript_schema"],
  output: resolve(import.meta.dirname, "../pkg/types.d.ts"),
  external: {
    from: "@agentclientprotocol/sdk/experimental/v2",
    schema: createRequire(import.meta.url).resolve(
      "@agentclientprotocol/sdk/schema/v2/schema.unstable.json",
    ),
  },
  tsTypesFrom: "./browser.js",
});
