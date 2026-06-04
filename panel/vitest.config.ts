import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const root = path.dirname(fileURLToPath(import.meta.url));

// Headless unit tests for the framework-agnostic core (ws client + state
// store). happy-dom supplies `location` / timers for the WS client tests; the
// socket itself is injected via a constructor seam (no real network). The same
// `@/` and `@/components/*` aliases as vite.config.ts so imports match the app.
export default defineConfig({
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
    globals: false,
  },
  resolve: {
    alias: [
      {
        find: /^@\/components\/(.*)$/,
        replacement: path.resolve(root, "components") + "/$1",
      },
      { find: "@", replacement: path.resolve(root, "src") },
    ],
  },
});
