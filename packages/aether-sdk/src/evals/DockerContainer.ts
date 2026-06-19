import { addAbortListener } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { PassThrough } from "node:stream";
import {
  GenericContainer,
  getContainerRuntimeClient,
  type StartedTestContainer,
  Wait,
} from "testcontainers";
import { AetherSdkError } from "../errors.js";
import type { AgentMessage } from "../generated/eval-types.js";
import type { AgentRunResult } from "./Agent.js";
import { DockerImage } from "./DockerImage.js";
import { isTerminalMessage } from "./transcript.js";

const STARTUP_TIMEOUT_MS = 60_000;

export interface BindMount {
  source: string;
  target: string;
  mode?: "rw" | "ro" | "z" | "Z";
}

export interface DockerContainerCreateOptions {
  image: DockerImage;
  env: Record<string, string>;
  bindMounts: BindMount[];
  ephemeralMounts: string[];
}

export interface DockerContainerRunOptions {
  command: string[];
  cwd: string;
  signal?: AbortSignal;
  onMessage?: (message: AgentMessage) => void;
  onStderr?: (chunk: string) => void;
}

export class DockerContainer {
  private constructor(
    private readonly container: StartedTestContainer,
    private readonly ephemeralTempdirs: string[],
  ) {}

  static async create(
    options: DockerContainerCreateOptions,
  ): Promise<DockerContainer> {
    const ephemeralTempdirs: string[] = [];
    try {
      const ephemeralBindMounts: BindMount[] = [];
      for (const target of options.ephemeralMounts) {
        const source = await mkdtemp(join(tmpdir(), "aether-eval-mount-"));
        ephemeralTempdirs.push(source);
        ephemeralBindMounts.push({ source, target, mode: "rw" });
      }

      const container = await createGenericContainer(options.image);
      const started = await container
        .withEntrypoint(["/bin/sh"])
        .withCommand(["-c", "sleep infinity"])
        .withWorkingDir("/workspace")
        .withEnvironment(options.env)
        .withBindMounts([...options.bindMounts, ...ephemeralBindMounts])
        .withWaitStrategy(Wait.forSuccessfulCommand("true"))
        .withStartupTimeout(STARTUP_TIMEOUT_MS)
        .start();

      return new DockerContainer(started, ephemeralTempdirs);
    } catch (err) {
      await removeTempdirs(ephemeralTempdirs);
      throw err;
    }
  }

  async run(options: DockerContainerRunOptions): Promise<AgentRunResult> {
    let stderr = "";
    const client = await getContainerRuntimeClient();
    const dockerode = client.container.dockerode;
    const dockerodeContainer = dockerode.getContainer(this.container.getId());
    const exec = await dockerodeContainer.exec({
      Cmd: options.command,
      AttachStdout: true,
      AttachStderr: true,
      WorkingDir: options.cwd,
    });

    const stream = await exec.start({ hijack: true, stdin: false });
    const stdoutPipe = new PassThrough();
    const stderrPipe = new PassThrough();
    dockerodeContainer.modem.demuxStream(stream, stdoutPipe, stderrPipe);
    stderrPipe.setEncoding("utf8");
    stderrPipe.on("data", (chunk: string) => {
      stderr += chunk;
      options.onStderr?.(chunk);
    });

    using abortCleanup = options.signal
      ? addAbortListener(options.signal, () => {
          stream.destroy(new AetherSdkError("aborted", "Aborted by caller"));
        })
      : undefined;
    void abortCleanup;

    const { transcript, sentTerminal } = await forwardStdoutMessages(
      stdoutPipe,
      options.onMessage,
    );

    stream.destroy();

    if (!sentTerminal) {
      throw new AetherSdkError(
        "eval_command_failed",
        `eval agent command exited without emitting a terminal AgentMessage.\nstderr:\n${stderr}`,
      );
    }

    return { transcript, stderr };
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.container.stop().catch(() => {});
    await removeTempdirs(this.ephemeralTempdirs);
  }
}

async function createGenericContainer(
  image: DockerImage,
): Promise<GenericContainer> {
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

async function forwardStdoutMessages(
  stdout: PassThrough,
  onMessage: ((message: AgentMessage) => void) | undefined,
): Promise<{ transcript: AgentMessage[]; sentTerminal: boolean }> {
  const lines = createInterface({ input: stdout, crlfDelay: Infinity });
  const transcript: AgentMessage[] = [];
  let sentTerminal = false;
  try {
    for await (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      const message = parseAgentMessage(trimmed);
      onMessage?.(message);
      transcript.push(message);
      if (isTerminalMessage(message)) {
        sentTerminal = true;
        break;
      }
    }
  } finally {
    lines.close();
  }
  return { transcript, sentTerminal };
}

async function removeTempdirs(tempdirs: string[]): Promise<void> {
  await Promise.all(
    tempdirs.map((path) => rm(path, { recursive: true, force: true })),
  );
}

function parseAgentMessage(line: string): AgentMessage {
  try {
    return JSON.parse(line) as AgentMessage;
  } catch (err) {
    throw new AetherSdkError(
      "eval_command_failed",
      `eval agent emitted an invalid AgentMessage JSON line: ${line}`,
      err,
    );
  }
}
