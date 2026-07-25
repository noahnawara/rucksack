// Renders the share cards from the site's own built stylesheet, so the design
// tokens, fonts, and pass geometry cannot drift from the shipped page.
//
//   npm run build && node scripts/build-social-card.mjs
//
// Outputs:
//   public/rucksack-ai-coding-agent-phone-hotspot.jpg  1200x630  site og:image
//   ../assets/social/github-social-preview.png         1280x640  GitHub preview

import { chromium } from "@playwright/test";
import { readdir, readFile, writeFile, mkdir } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const siteDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const distDir = join(siteDir, "dist");

const stylesheet = (await readdir(join(distDir, "assets"))).find(
  (name) => name.startsWith("index-") && name.endsWith(".css"),
);
if (!stylesheet) {
  throw new Error("no built stylesheet found — run `npm run build` first");
}

const template = (
  await readFile(join(siteDir, "scripts", "social-card.html"), "utf8")
).replace("CARD_STYLESHEET", stylesheet);

const types = {
  ".css": "text/css",
  ".woff2": "font/woff2",
  ".js": "text/javascript",
  ".html": "text/html",
};

const server = createServer(async (request, response) => {
  if (request.url === "/") {
    response.writeHead(200, { "content-type": "text/html" });
    response.end(template);
    return;
  }
  try {
    const body = await readFile(join(distDir, request.url.replace(/^\//, "")));
    response.writeHead(200, {
      "content-type": types[extname(request.url)] ?? "application/octet-stream",
    });
    response.end(body);
  } catch {
    response.writeHead(404).end();
  }
});

await new Promise((resolve) => server.listen(0, resolve));
const origin = `http://localhost:${server.address().port}`;

const targets = [
  {
    width: 1200,
    height: 630,
    path: join(siteDir, "public", "rucksack-ai-coding-agent-phone-hotspot.jpg"),
    options: { type: "jpeg", quality: 92 },
  },
  {
    width: 1280,
    height: 640,
    path: join(siteDir, "..", "assets", "social", "github-social-preview.png"),
    options: { type: "png" },
  },
];

const browser = await chromium.launch();
for (const target of targets) {
  const page = await browser.newPage({
    viewport: { width: target.width, height: target.height },
    deviceScaleFactor: 1,
  });
  await page.goto(origin, { waitUntil: "networkidle" });
  await page.addStyleTag({
    content: `:root { --card-width: ${target.width}px; --card-height: ${target.height}px; }`,
  });
  await page.evaluate(() => document.fonts.ready);
  await mkdir(dirname(target.path), { recursive: true });
  const shot = await page.screenshot(target.options);
  await writeFile(target.path, shot);
  await page.close();
  process.stdout.write(`wrote ${target.path} (${target.width}x${target.height})\n`);
}

await browser.close();
server.close();
