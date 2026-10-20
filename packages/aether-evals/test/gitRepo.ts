import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { GitFixture, type GitSpec, type Workspace } from "../src/index.js";

export function gitRepo(): GitRepoBuilder {
  return new GitRepoBuilder();
}

export function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", args, { cwd, stdio: "pipe" }).toString().trim();
}

export class GitRepoBuilder implements AsyncDisposable {
  readonly #resources = new AsyncDisposableStack();
  #start: Record<string, string> = {};
  #gold: Record<string, string> = {};
  #spec?: Promise<GitSpec>;

  start(files: Record<string, string>): this {
    this.#start = files;
    return this;
  }

  gold(files: Record<string, string>): this {
    this.#gold = files;
    return this;
  }

  spec(): Promise<GitSpec> {
    this.#spec ??= this.#commit();
    return this.#spec;
  }

  async bundleSpec(bundle?: string): Promise<GitSpec> {
    const path =
      bundle ?? join(await this.tempDir("ws-bundle-"), "repo.bundle");
    await (await this.fixture()).bundle(path);
    return { ...(await this.spec()), source: { bundle: path } };
  }

  async fixture(options: Partial<GitSpec> = {}): Promise<GitFixture> {
    const fixture = await GitFixture.fetch({
      ...(await this.spec()),
      ...options,
    });
    return this.#resources.use(fixture);
  }

  async workspace(options: Partial<GitSpec> = {}): Promise<Workspace> {
    return (await this.checkout(options)).ws;
  }

  async checkout(
    options: Partial<GitSpec> = {},
  ): Promise<{ fixture: GitFixture; ws: Workspace }> {
    const fixture = await this.fixture(options);
    return { fixture, ws: this.#resources.use(await fixture.workspace()) };
  }

  async tempDir(prefix: string, parent = tmpdir()): Promise<string> {
    const path = await mkdtemp(join(parent, prefix));
    this.#resources.defer(() => rm(path, { recursive: true, force: true }));
    return path;
  }

  [Symbol.asyncDispose](): Promise<void> {
    return this.#resources.disposeAsync();
  }

  async #commit(): Promise<GitSpec> {
    const repo = await this.tempDir("ws-repo-");
    git(repo, "init", "--initial-branch", "main");
    git(repo, "config", "user.email", "eval@example.com");
    git(repo, "config", "user.name", "Eval");
    const commit = async (files: Record<string, string>, message: string) => {
      for (const [path, content] of Object.entries(files)) {
        await mkdir(dirname(join(repo, path)), { recursive: true });
        await writeFile(join(repo, path), content);
      }
      git(repo, "add", "--force", ".");
      git(repo, "commit", "--allow-empty", "-m", message);
      return git(repo, "rev-parse", "HEAD");
    };
    const startCommit = await commit(this.#start, "start");
    const goldCommit = await commit(this.#gold, "gold");
    return { source: { url: `file://${repo}` }, startCommit, goldCommit };
  }
}
