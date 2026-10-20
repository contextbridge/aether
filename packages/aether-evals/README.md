<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [@aether-agent/evals](#aether-agentevals)
  - [Install](#install)
  - [Writing evals with vitest](#writing-evals-with-vitest)
    - [Grading a run with an LLM judge](#grading-a-run-with-an-llm-judge)
    - [Evaluating against a merged change](#evaluating-against-a-merged-change)
    - [Running the image as built](#running-the-image-as-built)

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
  
  // The agent sees only this environment; nothing is forwarded from the host.
  const agent = new DockerAgent({
    container,
    command: ["node", "/app/eval-agent.js"],
    env: { ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY! },
  });

  const trace = await Transcript.fromStream(agent.run(task));
  expect(trace.events.at(-1)).toMatchObject({
    category: "turn",
    event: { type: "ended" },
  });
  
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
    env: { ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY! },
  });
  
  const trace = await Transcript.fromStream(agent.run(task));
  const grader = judge({
    instructions: "Grade strictly using only the provided transcript and files.",
    task: "Change the first line of notes.txt from alpha to beta",
    context: {
      transcript: trace.events,
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

### Evaluating against a merged change

`GitFixture.fetch` fetches a start commit and a gold commit, such as the change a human merged, and
computes `fixture.referenceDiff` between them. That history is fetched in full, blobs included, so
fetch one fixture per task and reuse it across trials, or save it with `fixture.bundle(path)` and
fetch from `{ bundle: path }` next time.

`fixture.workspace()` checks out a fresh workspace at the start commit, on `main` (or `branch`) with no
remote, so the agent can't find the gold commit in git.

```ts
await using fixture = await GitFixture.fetch({
  source: { bundle: "fixtures/repo.bundle" }, // or { url: "https://github.com/example/repo" }
  startCommit,
  goldCommit,
  diffExclude: ["*pnpm-lock.yaml"], // left out of both diffs
});

for (let trial = 0; trial < 3; trial++) {
  await using workspace = await fixture.workspace();

  // ... run the agent ...

  // Commits, staged and unstaged edits, and untracked files since the start commit. Rejects if git
  // can't read the repository.
  const agentDiff = await fixture.agentDiff(workspace);
  const referenceDiff = fixture.referenceDiff;
}
```

### Running the image as built

By default a container replaces the image's entrypoint and runs commands as the image's user. For an
image built to run an agent in production:

```ts
await using container = await Container.builder(Image.parse("my-agent:latest"))
  .withPrivileged(true)
  // Keep an entrypoint that starts services, such as a Docker daemon, and wait for them.
  .withImageEntrypoint(true)
  .withReadyCommand("docker info")
  // Run execs and agents as the image's developer user. On Linux, the user is granted access to the
  // workspace and ephemeral mounts; mounts added with withMount keep their host permissions.
  .withUser("1000:1000")
  .start(workspace);

const controller = new AbortController();
for await (const event of agent.run(task, { signal: controller.signal })) {
  // ...
}
const tests = await container.exec({
  command: ["pnpm", "test"],
  signal: AbortSignal.timeout(600_000),
});
```

Aborting an exec rejects it immediately. Docker can't kill an exec'd process, so the command runs
until it exits or the container stops. Git inside the container treats `/workspace` as a safe
directory, since bind mounts are usually owned by another user.

These container options and abort signals are not yet in the Rust `aether-evals` crate.
