import { addAbortListener } from "node:events";
import { posix } from "node:path";
import { createInterface } from "node:readline";
import { PassThrough, type Readable } from "node:stream";
import { text } from "node:stream/consumers";
import { finished } from "node:stream/promises";
import type Dockerode from "dockerode";
import {
  GenericContainer,
  getContainerRuntimeClient,
  type StartedTestContainer,
  Wait,
} from "testcontainers";
import { abortedError, runCommand, throwIfAborted } from "@aether-agent/sdk";
import { Image } from "./image.js";
import { AetherCommandExitError, AetherEvalError } from "../errors.js";
import { createTempDir, removeDir } from "../fs.js";
import type { Workspace } from "../workspace.js";

const STARTUP_TIMEOUT_MS = 60_000;
const WORKSPACE_ROOT = "/workspace";

export interface BindMount {
  source: string;
  target: string;
  mode?: "rw" | "ro" | "z" | "Z";
}

export interface ContainerExecOptions {
  command: string[];
  cwd?: string;
  env?: Record<string, string>;
  user?: string;
  signal?: AbortSignal;
}

export interface ExecOutput {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface ContainerStreamingOptions extends ContainerExecOptions {
  onStderr?: (chunk: string) => void;
}

export interface ContainerRuntime {
  exec(options: RuntimeExecOptions): Promise<RuntimeExec>;
  stop(): Promise<void>;
}

export interface RuntimeExecOptions {
  command: string[];
  cwd: string;
  env: Record<string, string>;
  user?: string;
}

export interface RuntimeExec {
  stdout: Readable;
  stderr: Readable;
  detach(error?: Error): void;
  exitCode(): Promise<number | undefined>;
}

export class Container {
  readonly workspaceRoot = WORKSPACE_ROOT;
  readonly cwd: string;
  private readonly runtime: ContainerRuntime;
  private readonly user?: string;
  private readonly ephemeralTempdirs: string[];

  constructor(options: {
    runtime: ContainerRuntime;
    cwd: string;
    user?: string;
    ephemeralTempdirs?: string[];
  }) {
    this.runtime = options.runtime;
    this.cwd = options.cwd;
    this.user = options.user;
    this.ephemeralTempdirs = options.ephemeralTempdirs ?? [];
  }

  static builder(image: Image): ContainerBuilder {
    return new ContainerBuilder(image);
  }

  async exec(options: ContainerExecOptions): Promise<ExecOutput> {
    using exec = await this.startExec(options);
    const stdout = await text(exec.stdout);
    const { exitCode, stderr } = await exec.wait();
    return { exitCode, stdout, stderr };
  }

  async execShell(script: string): Promise<ExecOutput> {
    return this.exec({
      command: ["/bin/sh", "-lc", script],
    });
  }

  async *execStreaming(
    options: ContainerStreamingOptions,
  ): AsyncIterable<string> {
    using exec = await this.startExec(options);
    const lines = createInterface({ input: exec.stdout, crlfDelay: Infinity });
    try {
      for await (const line of lines) {
        yield line;
      }
    } finally {
      lines.close();
    }
    const { exitCode, stderr } = await exec.wait();
    if (exitCode !== 0) {
      throw new AetherCommandExitError(exitCode, stderr);
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.runtime.stop().catch(() => {});
    await removeTempdirs(this.ephemeralTempdirs);
  }

  private async startExec(
    options: ContainerStreamingOptions,
  ): Promise<RunningExec> {
    throwIfAborted(options.signal);
    const exec = await this.runtime.exec({
      command: options.command,
      cwd: options.cwd ?? this.cwd,
      env: options.env ?? {},
      user: options.user ?? this.user,
    });

    const drained = Promise.all([
      finished(exec.stdout, { cleanup: true }),
      finished(exec.stderr, { cleanup: true }),
    ]);
    void drained.catch(() => {});
    let stderr = "";
    exec.stderr.setEncoding("utf8");
    exec.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      options.onStderr?.(chunk);
    });

    const abortCleanup = options.signal
      ? addAbortListener(options.signal, () => exec.detach(abortedError()))
      : undefined;

    return {
      stdout: exec.stdout,
      async wait() {
        await drained;
        const exitCode = await exec.exitCode();
        if (exitCode === undefined) {
          throw new AetherEvalError(
            "command_exit_missing",
            `command completed without an exit code.\nstderr:\n${stderr}`,
          );
        }
        return { exitCode, stderr };
      },
      [Symbol.dispose]() {
        abortCleanup?.[Symbol.dispose]();
        exec.detach();
      },
    };
  }
}

export class ContainerBuilder {
  private envVars: Record<string, string> = {};
  private mounts: BindMount[] = [];
  private ephemeralMounts: string[] = [];
  private privileged = false;
  private user?: string;
  private imageEntrypoint = false;
  private readyCommand = "true";

  constructor(private readonly image: Image) {}

  /**
   * Enables Docker privileged mode.
   */
  withPrivileged(privileged: boolean): this {
    this.privileged = privileged;
    return this;
  }

  withUser(user: string): this {
    this.user = user;
    return this;
  }

  withImageEntrypoint(imageEntrypoint: boolean): this {
    this.imageEntrypoint = imageEntrypoint;
    return this;
  }

  withReadyCommand(command: string): this {
    this.readyCommand = command;
    return this;
  }

  withEnvVar(key: string, value: string): this {
    this.envVars[key] = value;
    return this;
  }

  withEnvVars(envVars: Record<string, string>): this {
    this.envVars = { ...this.envVars, ...envVars };
    return this;
  }

  withMount(mount: BindMount): this {
    this.mounts.push(mount);
    return this;
  }

  withEphemeralMount(target: string): this {
    this.ephemeralMounts.push(target);
    return this;
  }

  async start(workspace: Workspace): Promise<Container> {
    const ephemeralTempdirs: string[] = [];
    let container: Container | undefined;
    try {
      const ephemeralBindMounts: BindMount[] = [];
      for (const target of this.ephemeralMounts) {
        const source = await createTempDir("aether-eval-mount-");
        ephemeralTempdirs.push(source);
        ephemeralBindMounts.push({ source, target, mode: "rw" });
      }

      const cwd = workspace.relativeCwd
        ? posix.join(WORKSPACE_ROOT, workspace.relativeCwd)
        : WORKSPACE_ROOT;
      const client = await getContainerRuntimeClient();
      const generic = await createGenericContainer(this.image);
      if (this.privileged) {
        generic.withPrivilegedMode();
      }
      if (this.imageEntrypoint) {
        generic.withCommand(["sleep", "infinity"]);
      } else {
        generic
          .withEntrypoint(["/bin/sh"])
          .withCommand(["-c", "sleep infinity"]);
      }
      const started = await generic
        .withWorkingDir(cwd)
        .withEnvironment(withSafeWorkspace(this.envVars))
        .withBindMounts([
          { source: workspace.rootPath, target: WORKSPACE_ROOT, mode: "rw" },
          ...this.mounts,
          ...ephemeralBindMounts,
        ])
        .withWaitStrategy(Wait.forSuccessfulCommand(this.readyCommand))
        .withStartupTimeout(STARTUP_TIMEOUT_MS)
        .start();

      container = new Container({
        runtime: dockerRuntime(
          started,
          client.container.dockerode.getContainer(started.getId()),
        ),
        cwd,
        user: this.user,
        ephemeralTempdirs,
      });
      if (this.user) {
        await grantAccess(container, this.user, [
          workspace.rootPath,
          ...ephemeralTempdirs,
        ]);
      }
      return container;
    } catch (err) {
      await (container
        ? container[Symbol.asyncDispose]()
        : removeTempdirs(ephemeralTempdirs));
      throw err;
    }
  }
}

interface RunningExec extends Disposable {
  stdout: Readable;
  wait(): Promise<{ exitCode: number; stderr: string }>;
}

function dockerRuntime(
  started: StartedTestContainer,
  docker: Dockerode.Container,
): ContainerRuntime {
  return {
    async exec({ command, cwd, env, user }) {
      const vars = Object.entries(env).map(([key, value]) => `${key}=${value}`);
      const exec = await docker.exec({
        Cmd: command,
        AttachStdout: true,
        AttachStderr: true,
        WorkingDir: cwd,
        Env: vars.length > 0 ? vars : undefined,
        User: user,
      });
      const stream = await exec.start({ hijack: true, stdin: false });
      const stdout = new PassThrough();
      const stderr = new PassThrough();
      void finished(stream, {
        readable: true,
        writable: false,
        cleanup: true,
      }).then(
        () => {
          stdout.end();
          stderr.end();
        },
        (error: Error) => {
          stdout.destroy(error);
          stderr.destroy(error);
        },
      );
      docker.modem.demuxStream(stream, stdout, stderr);

      return {
        stdout,
        stderr,
        detach(error) {
          stream.destroy(error);
          stdout.destroy(error);
          stderr.destroy(error);
        },
        async exitCode() {
          const status = await exec.inspect();
          return status.Running ? undefined : (status.ExitCode ?? undefined);
        },
      };
    },
    async stop() {
      await started.stop();
    },
  };
}

async function createGenericContainer(image: Image): Promise<GenericContainer> {
  if (!image.build) {
    return new GenericContainer(image.toString());
  }

  let builder = GenericContainer.fromDockerfile(
    image.build.context,
    image.build.dockerfile,
  );

  if (image.build.buildArgs) {
    builder = builder.withBuildArgs(image.build.buildArgs);
  }

  if (image.build.cache !== undefined) {
    builder = builder.withCache(image.build.cache);
  }

  if (image.build.buildkit) {
    builder = builder.withBuildkit();
  }

  if (image.build.platform) {
    builder = builder.withPlatform(image.build.platform);
  }

  if (image.build.target) {
    builder = builder.withTarget(image.build.target);
  }

  return await builder.build(image.toString(), {
    deleteOnExit: image.build.deleteOnExit ?? false,
  });
}

function withSafeWorkspace(
  env: Record<string, string>,
): Record<string, string> {
  const index = Number(env.GIT_CONFIG_COUNT ?? 0);
  return {
    ...env,
    GIT_CONFIG_COUNT: String(index + 1),
    [`GIT_CONFIG_KEY_${index}`]: "safe.directory",
    [`GIT_CONFIG_VALUE_${index}`]: WORKSPACE_ROOT,
  };
}

async function grantAccess(
  container: Container,
  user: string,
  paths: string[],
): Promise<void> {
  const hostUid = process.getuid?.();
  if (process.platform !== "linux" || hostUid === undefined) return;

  const uid = await containerUid(container, user);
  if (uid === hostUid) return;

  const acl = `u:${uid}:rwX,d:u:${uid}:rwx,d:u:${hostUid}:rwx`;
  await runCommand("setfacl", ["-R", "-m", acl, ...paths], {
    cwd: process.cwd(),
    env: process.env,
    spawnFailedMessage: `Failed to run setfacl to grant UID ${uid} access to the workspace (is the acl package installed?)`,
    exitedErrorCode: "execution_failed",
    exitedMessage: ({ exitCode, stderr }) =>
      `setfacl exited with code ${exitCode}.\n${stderr}`,
  });
}

async function containerUid(
  container: Container,
  user: string,
): Promise<number> {
  const [name = ""] = user.split(":");
  if (/^\d+$/.test(name)) return Number(name);

  const id = await container.exec({ command: ["id", "-u"], user, cwd: "/" });
  if (id.exitCode !== 0) {
    throw new AetherEvalError(
      "configuration_error",
      `could not resolve the container user's UID:\n${id.stderr}`,
    );
  }
  return Number(id.stdout.trim());
}

async function removeTempdirs(tempdirs: string[]): Promise<void> {
  await Promise.all(tempdirs.map(removeDir));
}
