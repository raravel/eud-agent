// Render og.html / og-en.html to ../docs/assets/og.png / og-en.png (1200x630),
// the link-preview images the Korean and English pages' og:image point at. Uses the system Edge (Chrome with
// OG_BROWSER=chrome) so no Playwright browser download is needed.
import { chromium } from "playwright-core";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const browser = await chromium.launch({ channel: process.env.OG_BROWSER || "msedge" });
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } });
// `node og.mjs` renders both cards; `node og.mjs en` (or `ko`) renders one.
const cards = [
  ["ko", "og.html", "og.png"],
  ["en", "og-en.html", "og-en.png"],
].filter(([lang]) => !process.argv[2] || process.argv[2] === lang);
for (const [, source, output] of cards) {
  await page.goto(pathToFileURL(path.join(here, source)).href);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: path.join(here, "../docs/assets", output) });
}
await browser.close();
