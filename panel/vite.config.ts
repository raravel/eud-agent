import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const root = path.dirname(fileURLToPath(import.meta.url));

// The panel is served by the local Python server from a single origin and
// hosted in WebView2 — zero runtime CDN. `base: "./"` keeps every emitted
// asset reference relative so the built dist/index.html points only at local
// bundled assets (incl. Monaco workers).
export default defineConfig({
  base: "./",
  plugins: [react(), tailwindcss()],
  resolve: {
    // More specific aliases first: vendored shadcn/ui + AI Elements source
    // lives at the panel root `components/` (the contract dir), while app code
    // resolves under `src/`.
    alias: [
      {
        find: /^@\/components\/(.*)$/,
        replacement: path.resolve(root, "components") + "/$1",
      },
      { find: "@", replacement: path.resolve(root, "src") },
    ],
  },
  build: {
    // No source maps: keeps the build CDN-free and avoids shipping map URLs.
    sourcemap: false,
  },
});
