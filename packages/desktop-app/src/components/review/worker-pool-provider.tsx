import {
  WorkerPoolContextProvider,
  type WorkerInitializationRenderOptions,
  type WorkerPoolOptions,
} from "@pierre/diffs/react";
import WorkerUrl from "@pierre/diffs/worker/worker.js?worker&url";
import type { PropsWithChildren } from "react";

const poolOptions: WorkerPoolOptions = {
  poolSize: Math.min(
    Math.max(1, (globalThis.navigator?.hardwareConcurrency ?? 2) - 1),
    3,
  ),
  totalASTLRUCacheSize: 100,
  workerFactory: () => new Worker(WorkerUrl, { type: "module" }),
};

const highlighterOptions: WorkerInitializationRenderOptions = {
  theme: "ayu-dark",
  langs: [
    "cpp",
    "css",
    "go",
    "javascript",
    "json",
    "markdown",
    "python",
    "rust",
    "sh",
    "tsx",
    "typescript",
    "yaml",
  ],
  preferredHighlighter: "shiki-wasm",
};

export function DiffWorkerPoolProvider({ children }: PropsWithChildren) {
  if (typeof Worker === "undefined") return children;

  return (
    <WorkerPoolContextProvider
      poolOptions={poolOptions}
      highlighterOptions={highlighterOptions}
    >
      {children}
    </WorkerPoolContextProvider>
  );
}
