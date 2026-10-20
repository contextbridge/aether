import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { Container, Image, Workspace } from "../src/index.js";

describe.skipIf(!process.env.AETHER_EVALS_E2E)(
  "Container (e2e, requires Docker)",
  () => {
    it("runs execs as the configured user, with write access to the workspace", async () => {
      await using workspace = await Workspace.empty();
      await using container = await Container.builder(Image.parse("alpine:3"))
        .withUser("1000:1000")
        .start(workspace);

      const id = await container.exec({ command: ["id", "-u"] });
      const write = await container.exec({
        command: ["/bin/sh", "-c", "mkdir dir && echo agent > dir/out.txt"],
      });

      expect(id.stdout.trim()).toBe("1000");
      expect(write.exitCode).toBe(0);
      expect(await readFile(workspace.join("dir/out.txt"), "utf8")).toBe(
        "agent\n",
      );
    });

    it("keeps the image entrypoint and waits for the ready command", async () => {
      const image = Image.fromDockerfile("aether-evals-entrypoint:test", {
        context: new URL("fixtures/entrypoint", import.meta.url).pathname,
      });
      await using workspace = await Workspace.empty();
      await using container = await Container.builder(image)
        .withImageEntrypoint(true)
        .withReadyCommand("test -f /tmp/service-ready")
        .start(workspace);

      const ready = await container.exec({
        command: ["test", "-f", "/tmp/service-ready"],
      });

      expect(ready.exitCode).toBe(0);
    });

    it("lets git use the workspace repository as another user", async () => {
      const startCommit = await commitRepo();
      await using workspace = await Workspace.fromGitRepo({
        url: `file://${repo}`,
        startCommit,
        goldCommit: startCommit,
      });
      await using container = await Container.builder(
        Image.parse("alpine/git:latest"),
      )
        .withUser("1000:1000")
        .start(workspace);

      const head = await container.exec({
        command: ["git", "rev-parse", "HEAD"],
      });

      expect(head.stderr).toBe("");
      expect(head.stdout.trim()).toBe(startCommit);
    });
  },
);

let repo: string;

afterEach(async () => {
  if (repo) await rm(repo, { recursive: true, force: true });
});

async function commitRepo(): Promise<string> {
  repo = await mkdtemp(join(tmpdir(), "container-e2e-repo-"));
  const git = (...args: string[]) =>
    execFileSync("git", args, { cwd: repo, stdio: "pipe" }).toString().trim();
  git("init", "--initial-branch", "main");
  await writeFile(join(repo, "app.txt"), "start\n");
  git("add", ".");
  git(
    "-c",
    "user.email=eval@example.com",
    "-c",
    "user.name=Eval",
    "commit",
    "-m",
    "start",
  );
  return git("rev-parse", "HEAD");
}
