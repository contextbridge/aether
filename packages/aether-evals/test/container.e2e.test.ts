import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

import { Container, Image, Workspace } from "../src/index.js";
import { gitRepo } from "./gitRepo.js";

describe.skipIf(!process.env.AETHER_EVALS_E2E)(
  "Container (e2e, requires Docker)",
  () => {
    it.each([
      ["1000:1000", "1000"],
      ["nobody", "65534"],
    ])(
      "runs execs as %s, with write access to the workspace",
      async (user, uid) => {
        await using workspace = await Workspace.empty();
        await using container = await Container.builder(Image.parse("alpine:3"))
          .withUser(user)
          .start(workspace);

        const id = await container.exec({ command: ["id", "-u"] });
        const write = await container.exec({
          command: ["/bin/sh", "-c", "mkdir dir && echo agent > dir/out.txt"],
        });

        expect(id.stdout.trim()).toBe(uid);
        expect(write.exitCode).toBe(0);
        expect(await readFile(workspace.join("dir/out.txt"), "utf8")).toBe(
          "agent\n",
        );
      },
    );

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
      await using repo = gitRepo().start({ "app.txt": "start\n" });
      const { startCommit } = await repo.spec();
      const workspace = await repo.workspace();
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
