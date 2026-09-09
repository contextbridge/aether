#!/usr/bin/env node

import { text } from "node:stream/consumers";
import { setImmediate } from "node:timers/promises";
import { fileURLToPath } from "node:url";

export async function writeFakeOutput(stdout) {
  stdout = stdout.replaceAll("$PID", String(process.pid));
  if (process.env.FAKE_ECHO_STDIN) stdout += await text(process.stdin);
  const bytes = Buffer.from(stdout);
  const chunkSize = Number(process.env.FAKE_CHUNK_SIZE ?? bytes.length) || 1;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    await new Promise((resolve) =>
      process.stdout.write(bytes.subarray(offset, offset + chunkSize), resolve),
    );
    await setImmediate();
  }
  if (process.env.FAKE_STDERR) process.stderr.write(process.env.FAKE_STDERR);
  if (process.env.FAKE_CLOSE_STDOUT) {
    await new Promise((resolve) => process.stdout.end(resolve));
  }
  if (process.env.FAKE_HOLD) {
    setInterval(() => {}, 2 ** 31 - 1);
    await new Promise(() => {});
  }
  process.exit(Number(process.env.FAKE_EXIT_CODE ?? 0));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await writeFakeOutput(process.env.FAKE_STDOUT ?? "");
}
