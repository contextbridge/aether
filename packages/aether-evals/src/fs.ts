import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export function createTempDir(prefix: string): Promise<string> {
  return mkdtemp(join(tmpdir(), prefix));
}

export async function withTempDir<T>(
  prefix: string,
  fn: (path: string) => Promise<T>,
): Promise<T> {
  const path = await createTempDir(prefix);
  try {
    return await fn(path);
  } finally {
    await removeDir(path);
  }
}

export function removeDir(path: string): Promise<void> {
  return rm(path, { recursive: true, force: true });
}

export async function pathExists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}
