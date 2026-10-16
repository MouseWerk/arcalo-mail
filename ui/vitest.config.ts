import { defineConfig } from "vitest/config";

// One time zone for every machine, so date formatting is tested the same everywhere.
process.env.TZ = "Europe/Berlin";

export default defineConfig({
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
    setupFiles: ["src/test-setup.ts"],
    testTimeout: 20_000,
  },
});
