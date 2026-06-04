import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

const root = path.dirname(fileURLToPath(import.meta.url));

// Headless tests for the panel: the framework-agnostic core (ws client + state
// store, *.test.ts) and the React UI components (@testing-library/react,
// *.test.tsx). happy-dom supplies `location` / timers for the WS client tests
// and the DOM for component rendering; the WS socket is injected via a
// constructor seam and Monaco is module-mocked with a textarea double (no real
// network, no real editor). `@vitejs/plugin-react` gives the JSX transform; the
// same `@/` and `@/components/*` aliases as vite.config.ts so imports match the
// app. setup.ts registers jest-dom matchers + Radix pointer/scroll polyfills.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    setupFiles: ["src/test/setup.ts"],
    globals: false,
  },
  resolve: {
    // Mirror vite.config.ts: only the vendored shadcn/ui + AI Elements subtrees
    // resolve to the root `components/`; panel components + lib/state resolve
    // under `src/`.
    alias: [
      {
        find: /^@\/components\/(ui|ai-elements)\/(.*)$/,
        replacement: path.resolve(root, "components") + "/$1/$2",
      },
      { find: "@", replacement: path.resolve(root, "src") },
    ],
  },
});
