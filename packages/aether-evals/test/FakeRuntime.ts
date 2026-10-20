import { PassThrough } from "node:stream";

import {
  Container,
  type ContainerRuntime,
  type RuntimeExec,
  type RuntimeExecOptions,
} from "../src/index.js";

export interface FakeOutput {
  stdout?: string;
  stderr?: string;
  exitCode?: number;
  hang?: boolean;
}

export class FakeRuntime implements ContainerRuntime {
  readonly execs: RuntimeExecOptions[] = [];

  constructor(private readonly output: FakeOutput = {}) {}

  container(options: { user?: string } = {}): Container {
    return new Container({ runtime: this, cwd: "/workspace", ...options });
  }

  async exec(options: RuntimeExecOptions): Promise<RuntimeExec> {
    this.execs.push(options);
    const {
      stdout = "",
      stderr = "",
      exitCode = 0,
      hang = false,
    } = this.output;
    const out = new PassThrough();
    const err = new PassThrough();
    if (!hang) {
      out.end(stdout);
      err.end(stderr);
    }
    return {
      stdout: out,
      stderr: err,
      detach(error) {
        out.destroy(error);
        err.destroy(error);
      },
      async exitCode() {
        return hang ? undefined : exitCode;
      },
    };
  }

  async stop(): Promise<void> {}
}
