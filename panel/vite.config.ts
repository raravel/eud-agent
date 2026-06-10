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
    // Order matters (most specific first). The vendored shadcn/ui + AI Elements
    // source lives at the panel root `components/{ui,ai-elements}/` (the shadcn
    // contract dirs); panel-specific components live at `src/components/`
    // (features/03 ## Implementation). Route ONLY the two vendored subtrees to
    // the root `components/`; everything else under `@/` (incl.
    // `@/components/<PanelComponent>`, `@/lib/*`, `@/state/*`) resolves to src/.
    alias: [
      {
        find: /^@\/components\/(ui|ai-elements)\/(.*)$/,
        replacement: path.resolve(root, "components") + "/$1/$2",
      },
      { find: "@", replacement: path.resolve(root, "src") },
    ],
  },
  build: {
    // No source maps: keeps the build CDN-free and avoids shipping map URLs.
    sourcemap: false,
  },
  server: {
    // tauri.conf.json devUrl points here; strictPort so `cargo tauri dev`
    // never silently attaches to a different port's stale server.
    port: 5173,
    strictPort: true,
  },
});
