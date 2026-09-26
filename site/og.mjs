// Render og.html to ../docs/assets/og.png (1200x630), the link-preview image
// the landing page's og:image points at. Uses the system Edge (Chrome with
// OG_BROWSER=chrome) so no Playwright browser download is needed.
import { chromium } from "playwright-core";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const browser = await chromium.launch({ channel: process.env.OG_BROWSER || "msedge" });
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } });
await page.goto(pathToFileURL(path.join(here, "og.html")).href);
await page.evaluate(() => document.fonts.ready);
await page.screenshot({ path: path.join(here, "../docs/assets/og.png") });
await browser.close();
