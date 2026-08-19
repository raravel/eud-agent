import { build } from "esbuild";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const upstreamRoot = process.env.EPSCRIPT_LSP_SOURCE;
if (!upstreamRoot || !existsSync(path.join(upstreamRoot, "analyzer.ts"))) {
  throw new Error(
    "EPSCRIPT_LSP_SOURCE must point to the pinned packages/server/src directory",
  );
}

const output = process.env.EPSCRIPT_LSP_OUTPUT
  ? path.resolve(process.env.EPSCRIPT_LSP_OUTPUT)
  : path.resolve(packageRoot, "../../vendor/epscript-lsp-agent/adapter.cjs");

await build({
  absWorkingDir: packageRoot,
  entryPoints: ["src/agent-adapter.ts"],
  outfile: output,
  bundle: true,
  platform: "node",
  nodePaths: [path.join(packageRoot, "node_modules")],
  format: "cjs",
  target: "node18",
  minify: true,
  legalComments: "none",
  charset: "utf8",
  sourcemap: false,
  logLevel: "info",
  plugins: [
    {
      name: "pinned-epscript-lsp-source",
      setup(buildApi) {
        buildApi.onResolve({ filter: /^#upstream\// }, (args) => ({
          path: `${path.join(upstreamRoot, args.path.slice("#upstream/".length))}.ts`,
        }));
      },
    },
  ],
});
