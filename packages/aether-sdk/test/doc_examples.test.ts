import { readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const DOCS_DIR = path.resolve(import.meta.dirname, "../src/docs");
const TEMP_EXAMPLE = path.resolve(import.meta.dirname, ".doc-example.ts");

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

  const configPath = ts.findConfigFile(
    import.meta.dirname,
    ts.sys.fileExists,
    "tsconfig.json",
  );
  if (!configPath) throw new Error("tsconfig.json not found");

  const config = ts.readConfigFile(configPath, ts.sys.readFile);
  if (config.error)
    throw new Error(
      ts.flattenDiagnosticMessageText(config.error.messageText, "\n"),
    );

  const parsed = ts.parseJsonConfigFileContent(
    config.config,
    ts.sys,
    path.dirname(configPath),
    undefined,
    configPath,
  );
  // Typecheck only: emit layout (outDir/rootDir) would reject the out-of-tree temp file.
  const program = ts.createProgram({
    rootNames: [TEMP_EXAMPLE],
    options: { ...parsed.options, noEmit: true },
  });
  const diagnostics = ts.getPreEmitDiagnostics(program);

  expect(formatDiagnostics(diagnostics)).toEqual("");
}

function formatDiagnostics(diagnostics: readonly ts.Diagnostic[]) {
  return diagnostics
    .map((diagnostic) =>
      ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
    )
    .join("\n");
}
