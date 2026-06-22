import { addAbortListener } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, posix } from "node:path";
import { createInterface } from "node:readline";
import { PassThrough } from "node:stream";
import {
  GenericContainer,
  getContainerRuntimeClient,
  type StartedTestContainer,
  Wait,
} from "testcontainers";
import { AetherSdkError } from "@aether-agent/sdk";
import { Image } from "./image.js";
import type { Workspace } from "../workspace.js";

const STARTUP_TIMEOUT_MS = 60_000;

export interface BindMount {
  source: string;
  target: string;
  mode?: "rw" | "ro" | "z" | "Z";
}

export interface ContainerExecOptions {
  command: string[];
  cwd?: string;
  env?: Record<string, string>;
  signal?: AbortSignal;
}

export interface ExecOutput {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface ContainerStreamingOptions {
  command: string[];
  cwd?: string;
  env?: Record<string, string>;
  signal?: AbortSignal;
  onStderr?: (chunk: string) => void;
}

export class Container {
  readonly workspaceRoot = "/workspace";
  readonly cwd: string;

  constructor(
    private readonly container: StartedTestContainer,
    private readonly ephemeralTempdirs: string[],
    cwd: string,
  ) {
    this.cwd = cwd;
  }

  static builder(image: Image): ContainerBuilder {
    return new ContainerBuilder(image);
  }

  async exec(options: ContainerExecOptions): Promise<ExecOutput> {
    const result = await this.container.exec(options.command, {
      workingDir: options.cwd ?? this.cwd,
      env: options.env,
    });
    return {
      exitCode: result.exitCode,
      stdout: result.stdout,
      stderr: result.stderr,
    };
  }

  async execShell(script: string): Promise<ExecOutput> {
    return this.exec({
      command: ["/bin/sh", "-lc", script],
    });
  }

  async *execStreaming(
    options: ContainerStreamingOptions,
  ): AsyncIterable<string> {
    const client = await getContainerRuntimeClient();
    const dockerode = client.container.dockerode;
    const dockerodeContainer = dockerode.getContainer(this.container.getId());
    const env = Object.entries(options.env ?? {}).map(
      ([key, value]) => `${key}=${value}`,
    );
    const exec = await dockerodeContainer.exec({
      Cmd: options.command,
      AttachStdout: true,
      AttachStderr: true,
      WorkingDir: options.cwd ?? this.cwd,
      Env: env.length > 0 ? env : undefined,
    });

    const stream = await exec.start({ hijack: true, stdin: false });
    const stdoutPipe = new PassThrough();
    const stderrPipe = new PassThrough();
    dockerodeContainer.modem.demuxStream(stream, stdoutPipe, stderrPipe);
    stderrPipe.setEncoding("utf8");
    stderrPipe.on("data", (chunk: string) => {
      options.onStderr?.(chunk);
    });

    using abortCleanup = options.signal
      ? addAbortListener(options.signal, () => {
          stream.destroy(new AetherSdkError("aborted", "Aborted by caller"));
        })
      : undefined;
    void abortCleanup;

    try {
      const lines = createInterface({ input: stdoutPipe, crlfDelay: Infinity });
      try {
        for await (const line of lines) {
          yield line;
        }
      } finally {
        lines.close();
      }
    } finally {
      stream.destroy();
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.container.stop().catch(() => {});
    await removeTempdirs(this.ephemeralTempdirs);
  }
}

export class ContainerBuilder {
  private envVars: Record<string, string> = {};
  private mounts: BindMount[] = [];
  private ephemeralMounts: string[] = [];

  constructor(private readonly image: Image) {}

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
    try {
      const ephemeralBindMounts: BindMount[] = [];
      for (const target of this.ephemeralMounts) {
        const source = await mkdtemp(join(tmpdir(), "aether-eval-mount-"));
        ephemeralTempdirs.push(source);
        ephemeralBindMounts.push({ source, target, mode: "rw" });
      }

      const cwd = workspace.relativeCwd
        ? posix.join("/workspace", workspace.relativeCwd)
        : "/workspace";
      const workspaceMount: BindMount = {
        source: workspace.rootPath,
        target: "/workspace",
        mode: "rw",
      };
      const container = await createGenericContainer(this.image);
      const started = await container
        .withEntrypoint(["/bin/sh"])
        .withCommand(["-c", "sleep infinity"])
        .withWorkingDir(cwd)
        .withEnvironment(this.envVars)
        .withBindMounts([
          workspaceMount,
          ...this.mounts,
          ...ephemeralBindMounts,
        ])
        .withWaitStrategy(Wait.forSuccessfulCommand("true"))
        .withStartupTimeout(STARTUP_TIMEOUT_MS)
        .start();

      return new Container(started, ephemeralTempdirs, cwd);
    } catch (err) {
      await removeTempdirs(ephemeralTempdirs);
      throw err;
    }
  }
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

async function removeTempdirs(tempdirs: string[]): Promise<void> {
  await Promise.all(
    tempdirs.map((path) => rm(path, { recursive: true, force: true })),
  );
}
