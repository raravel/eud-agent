// Release preflight, run once before any platform builds.
//
// The app version lives in three files that must agree with the tag, and the
// managed FFmpeg distribution is pinned by checksum per platform. Both were
// checked by an inline PowerShell step that only ran on the Windows runner and
// only understood the single-archive manifest schema. This is the same check,
// written once, runnable on every runner — and locally, with `node
// .github/scripts/verify-release.mjs v1.0.0`.
import { readFileSync, existsSync, appendFileSync } from "node:fs";

const MANIFEST_SCHEMA = "eud-managed-ffmpeg/2";
/** Every platform the release ships a managed audio converter for. */
const REQUIRED_FFMPEG_PLATFORMS = ["windows-x86_64", "macos-aarch64", "macos-x86_64"];
const SHA256 = /^[0-9a-f]{64}$/;

const problems = [];
const fail = (message) => problems.push(message);

/** `v1.2.3` — the only shape the release workflow accepts. */
function releaseVersion(tag) {
  const match = /^v(\d+\.\d+\.\d+)$/.exec(tag ?? "");
  if (!match) {
    fail(`Release tag must be v<semver>; got ${JSON.stringify(tag ?? "")}.`);
    return null;
  }
  return match[1];
}

/** The three committed versions, each with where it came from. */
function committedVersions() {
  const found = [];
  const tauri = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).version;
  if (typeof tauri === "string") found.push(["src-tauri/tauri.conf.json", tauri]);
  else fail("src-tauri/tauri.conf.json has no version.");

  const cargo = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync("src-tauri/Cargo.toml", "utf8"));
  if (cargo) found.push(["src-tauri/Cargo.toml", cargo[1]]);
  else fail("Could not read the version from src-tauri/Cargo.toml.");

  // Newline-agnostic: the lock file is checked out with CRLF on Windows.
  const locked = /\[\[package\]\]\s+name\s*=\s*"eud-agent"\s+version\s*=\s*"([^"]+)"/.exec(
    readFileSync("Cargo.lock", "utf8"),
  );
  if (locked) found.push(["Cargo.lock", locked[1]]);
  else fail("Could not read the eud-agent version from Cargo.lock.");

  return found;
}

/**
 * The managed audio converter is downloaded at runtime and admitted only when
 * it matches these pins, so a manifest that lost a platform or a checksum
 * ships an app whose audio tools can never install.
 */
function verifyFfmpegManifest() {
  const manifest = JSON.parse(readFileSync("vendor/ffmpeg/manifest.json", "utf8"));
  if (manifest.schema !== MANIFEST_SCHEMA) {
    fail(`Managed FFmpeg manifest schema must be ${MANIFEST_SCHEMA}; got ${manifest.schema}.`);
    return;
  }
  if (!existsSync("vendor/ffmpeg/LICENSE.txt")) {
    fail("Managed FFmpeg GPL license resource is missing.");
  }
  for (const platform of REQUIRED_FFMPEG_PLATFORMS) {
    const entry = manifest.platforms?.[platform];
    if (!entry) {
      fail(`Managed FFmpeg manifest has no ${platform} platform.`);
      continue;
    }
    const archives = Array.isArray(entry.archives) ? entry.archives : [];
    if (archives.length === 0) {
      fail(`Managed FFmpeg ${platform} has no archives.`);
      continue;
    }
    const tools = new Set();
    for (const archive of archives) {
      if (!SHA256.test(archive.sha256 ?? "")) {
        fail(`Managed FFmpeg ${platform} archive SHA-256 is invalid: ${archive.url}`);
      }
      if (!Number.isInteger(archive.bytes) || archive.bytes <= 0) {
        fail(`Managed FFmpeg ${platform} archive byte size is invalid: ${archive.url}`);
      }
      for (const member of archive.members ?? []) {
        if (!SHA256.test(member.sha256 ?? "")) {
          fail(`Managed FFmpeg ${platform} member pin is invalid: ${member.name}`);
        }
        // `ffmpeg.exe` on Windows, `ffmpeg` elsewhere.
        tools.add(String(member.name ?? "").replace(/\.exe$/, ""));
      }
    }
    for (const tool of ["ffmpeg", "ffprobe"]) {
      if (!tools.has(tool)) fail(`Managed FFmpeg ${platform} does not pin ${tool}.`);
    }
  }
}

const tag = process.argv[2];
const version = releaseVersion(tag);
for (const [source, actual] of committedVersions()) {
  if (version && actual !== version) {
    fail(`Release tag ${tag} does not match ${source} version ${actual}.`);
  }
}
verifyFfmpegManifest();

if (problems.length > 0) {
  for (const problem of problems) console.error(`error: ${problem}`);
  process.exit(1);
}

console.log(`Release ${tag} verified: version ${version}, managed FFmpeg pins intact.`);
if (process.env.GITHUB_OUTPUT) {
  appendFileSync(process.env.GITHUB_OUTPUT, `tag=${tag}\nversion=${version}\n`);
}
