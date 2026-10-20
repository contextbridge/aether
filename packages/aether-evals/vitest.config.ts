import { configDefaults, defineConfig } from "vitest/config";

const E2E_TESTS = "test/**/*.e2e.test.ts";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "unit",
          include: ["test/**/*.test.ts"],
          exclude: [...configDefaults.exclude, E2E_TESTS],
          testTimeout: 10_000,
        },
      },
      {
        test: {
          name: "e2e",
          include: [E2E_TESTS],
          testTimeout: 0,
        },
      },
    ],
  },
});
