import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const DOCS_DIR = path.resolve(import.meta.dirname, "../src/docs");
const TEMP_EXAMPLE = path.resolve(import.meta.dirname, ".doc-example.ts");
const TEMP_CONFIG = path.resolve(
  import.meta.dirname,
  ".doc-example.tsconfig.json",
);
const TSC = path.resolve(import.meta.dirname, "../node_modules/.bin/tsc");

const docFiles = readdirSync(DOCS_DIR)
  .filter((name) => name.endsWith(".md"))
  .sort();

describe("documentation examples", () => {
  it.each(docFiles)(
    "typechecks the TypeScript snippets in %s",
    (file: string) => {
      const markdown = readFileSync(path.join(DOCS_DIR, file), "utf8");
      const snippets = extractTypeScriptSnippets(markdown);

      try {
        for (const snippet of snippets) {
          assertTypechecks(
            snippet.replaceAll('"@aether-agent/sdk"', '"../src/index.js"'),
          );
        }
      } finally {
        rmSync(TEMP_EXAMPLE, { force: true });
        rmSync(TEMP_CONFIG, { force: true });
      }
    },
  );
});

function extractTypeScriptSnippets(markdown: string): string[] {
  // `\b` so `tsx` is not treated as `ts`; `[^\n]*` keeps the info string on the
  // fence line so the first code line (often the import) is not swallowed.
  return Array.from(
    markdown.matchAll(/```(?:ts|typescript)\b[^\n]*\n([\s\S]*?)```/g),
    (match) => match[1] ?? "",
  );
}

function assertTypechecks(source: string) {
  writeFileSync(TEMP_EXAMPLE, source);
  writeFileSync(
    TEMP_CONFIG,
    JSON.stringify({
      extends: "../tsconfig.json",
      compilerOptions: { noEmit: true, rootDir: ".." },
      files: [path.basename(TEMP_EXAMPLE)],
    }),
  );

  expect(() => execFileSync(TSC, ["--project", TEMP_CONFIG])).not.toThrow();
}
