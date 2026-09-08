import { syncPublicSources } from "./publicSources.js";

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  if (args.some((arg) => arg !== "--only=eudtools") || args.length > 1) {
    throw new Error("Usage: npm run sync-public -- [--only=eudtools]");
  }
  const summaries = await syncPublicSources(
    undefined, args.length ? "eudtools" : "all"
  );
  for (const summary of summaries) {
    console.error(
      `${summary.outputFile}: rows=${summary.rows} commit=${summary.commit}`
    );
  }
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
