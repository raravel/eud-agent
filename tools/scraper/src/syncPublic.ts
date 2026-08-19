import { syncPublicSources } from "./publicSources.js";

async function main(): Promise<void> {
  const summaries = await syncPublicSources();
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
