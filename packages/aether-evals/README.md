<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [@aether-agent/evals](#aether-agentevals)
  - [Install](#install)
  - [Writing evals with vitest](#writing-evals-with-vitest)
    - [Grading a run with an LLM judge](#grading-a-run-with-an-llm-judge)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

# @aether-agent/evals

This package facilitates writing evals for Aether agents. 

## Install

```sh
npm install --save-dev @aether-agent/evals
```

## Writing evals with vitest

`@aether-agent/evals` runs a single Dockerized eval per test:

```ts
import { test, expect } from "vitest";
import {
  Transcript,
  DockerAgent,
  Container,
  Image,
  Task,
  Workspace,
} from "@aether-agent/evals";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

test("agent edits notes.txt", async () => {
  await using workspace = await Workspace.fromFiles({ "notes.txt": "alpha\nalpha\n" });
  const task = new Task(
    "Change the first line of notes.txt from alpha to beta"
  );
  
  await using container = await Container.builder(
    Image.parse("aether-sandbox:latest")
  ).start(workspace);
  
  const agent = new DockerAgent({
    container,
    command: ["node", "/app/eval-agent.js"]
  });

  const trace = await Transcript.fromStream(agent.run(task));
  expect(trace.messages.at(-1)?.type).toBe("done");
  
  // Assert against the retained workspace and the recorded tool calls.
  expect(await readFile(join(workspace.path, "notes.txt"), "utf8")).toBe(
    "beta\nalpha\n",
  );
  
  const tests = await container.exec({ command: ["pnpm", "test"] });
  expect(tests.exitCode).toBe(0);
  
  // Workspace is removed when `workspace` goes out of scope; run container exec assertions first.
});
```

### Grading a run with an LLM judge

```ts
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test, expect } from "vitest";
import {
  Transcript,
  DockerAgent,
  Container,
  Image,
  generate,
  judge,
  Task,
  Workspace,
} from "@aether-agent/evals";

test("agent makes a maintainer-quality edit", async () => {
  await using workspace = await Workspace.fromFiles({ "notes.txt": "alpha\nalpha\n" });
  const task = new Task(
    "Change the first line of notes.txt from alpha to beta",
  );
  
  await using container = await Container.builder(
    Image.parse("aether-sandbox:latest"),
  ).start(workspace);
  
  const agent = new DockerAgent({
    container,
    command: ["node", "/app/eval-agent.js"],
  });
  
  const trace = await Transcript.fromStream(agent.run(task));
  const grader = judge({
    instructions: "Grade strictly using only the provided transcript and files.",
    task: "Change the first line of notes.txt from alpha to beta",
    context: {
      transcript: trace.messages,
      files: {
        "notes.txt": await readFile(join(workspace.path, "notes.txt"), "utf8"),
      },
    },
    criteria: [
      {
        id: "correct-edit",
        description: "The final notes.txt content is exactly 'beta\\nalpha\\n'.",
        blocking: true,
        threshold: 1,
        weight: 3,
      },
      {
        id: "minimal-change",
        description: "No unrelated files or lines were changed.",
        blocking: true,
        threshold: 0.8,
        weight: 2,
      },
    ],
  });

  const response = await generate(grader.prompt, {
    model: "anthropic:claude-sonnet-4-5", // or set AETHER_LLM_MODEL
    temperature: 0,
    schema: grader.schema,
  });
  
  const summary = grader.summarize(response);
  expect(summary.passed, summary.reason).toBe(true);
});
```
