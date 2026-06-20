// Render docs/diagrams/loop.html -> docs/loop.png at 2x.
// Run from the video-assets-staging dir so `playwright` resolves:
//   cd video-assets-staging && node ../docs/diagrams/render.mjs
import { chromium } from "playwright";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const htmlPath = resolve(here, "loop.html");
const outPath = resolve(here, "..", "loop.png");

const browser = await chromium.launch();
const ctx = await browser.newContext({
  viewport: { width: 1580, height: 1060 },
  deviceScaleFactor: 2,
});
const page = await ctx.newPage();
await page.goto("file://" + htmlPath);
await page.waitForFunction("window.__ready === true");
await page.waitForTimeout(400); // let rough.js draw + fonts settle
const el = await page.$("#stage");
await el.screenshot({ path: outPath, omitBackground: true });
await browser.close();
console.log("wrote", outPath);
