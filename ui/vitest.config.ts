import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// One time zone for every machine, so date formatting is tested the same everywhere.
process.env.TZ = "Europe/Berlin";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    setupFiles: ["src/test-setup.ts", "src/tests/setup.ts"],
    testTimeout: 20_000,
  },
});
