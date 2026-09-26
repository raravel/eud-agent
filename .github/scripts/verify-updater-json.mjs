// Release postflight: every platform that built is in the updater manifest.
//
// tauri-action writes `latest.json` by fetching the release's existing asset,
// reusing its `platforms`, and adding its own entry. That is a read-modify-write,
// so two jobs publishing at the same instant can both read the same state and the
// later upload silently drops the earlier platform — an app that never offers
// updates to those users, with nothing red to show for it.
//
// Serializing the matrix to avoid that costs the whole release the wall-clock of
// every build run back to back. Detecting it costs one short job, and the fix is
// to re-run the job that lost: it then reads the entries the others already wrote.
//
// Usage: node .github/scripts/verify-updater-json.mjs <latest.json> <version>
import { readFileSync } from "node:fs";

/** Tauri updater platform keys for the targets this release's matrix builds. */
const REQUIRED_PLATFORMS = ["windows-x86_64", "darwin-aarch64"];

const [, , manifestPath, expectedVersion] = process.argv;
const problems = [];
const fail = (message) => problems.push(message);

let manifest;
try {
  manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
} catch (error) {
  console.error(`error: ${manifestPath} could not be read as JSON: ${error.message}`);
  console.error(
    "The release published no updater manifest. Re-run the publish jobs for this tag.",
  );
  process.exit(1);
}

if (expectedVersion && manifest.version !== expectedVersion) {
  fail(`Updater manifest version is ${manifest.version}, expected ${expectedVersion}.`);
}

const platforms = manifest.platforms ?? {};
for (const platform of REQUIRED_PLATFORMS) {
  const entry = platforms[platform];
  if (!entry) {
    fail(
      `Updater manifest is missing ${platform}. Its publish job either failed or lost the ` +
        `latest.json merge; re-run that job to add it back.`,
    );
    continue;
  }
  if (typeof entry.url !== "string" || entry.url.length === 0) {
    fail(`Updater manifest ${platform} has no download url.`);
  }
  if (typeof entry.signature !== "string" || entry.signature.length === 0) {
    fail(`Updater manifest ${platform} has no signature.`);
  }
}

for (const platform of Object.keys(platforms)) {
  if (!REQUIRED_PLATFORMS.includes(platform)) {
    console.log(`note: updater manifest also carries ${platform}.`);
  }
}

if (problems.length > 0) {
  for (const problem of problems) console.error(`error: ${problem}`);
  process.exit(1);
}

console.log(
  `Updater manifest for ${manifest.version} carries every platform: ${REQUIRED_PLATFORMS.join(", ")}.`,
);
