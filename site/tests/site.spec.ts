import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { fileURLToPath, URL } from "node:url";

const promptPath = fileURLToPath(
  new URL("../src/content/install-agent-prompt.txt", import.meta.url),
);
const readmePath = fileURLToPath(new URL("../../README.md", import.meta.url));
const robotsPath = fileURLToPath(
  new URL("../public/robots.txt", import.meta.url),
);
const sitemapPath = fileURLToPath(
  new URL("../public/sitemap.xml", import.meta.url),
);
const manifestPath = fileURLToPath(
  new URL("../public/site.webmanifest", import.meta.url),
);

type TextLineBounds = {
  readonly height: number;
  readonly width: number;
  readonly x: number;
  readonly y: number;
};

const readCanonicalPrompt = async (): Promise<string> =>
  readFile(promptPath, "utf8");

const measureTextLines = (element: HTMLElement): readonly TextLineBounds[] => {
  const range = document.createRange();
  range.selectNodeContents(element);

  return Array.from(
    range.getClientRects(),
    (bounds: DOMRect): TextLineBounds => {
      return {
        height: bounds.height,
        width: bounds.width,
        x: bounds.x,
        y: bounds.y,
      };
    },
  );
};

test("shows one promise, one distilled commute pass, one action, and the evidence chapters", async ({
  page,
}): Promise<void> => {
  await page.goto("/");

  await expect(page).toHaveTitle(
    "Keep Your Coding Agent Running with the Lid Closed | rucksack",
  );
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
    "href",
    "https://www.rucksack.wtf/",
  );
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "close your laptop. keep your agents running.",
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("figure", {
      name: "example rucksack commute pass",
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "copy setup prompt" }),
  ).toBeVisible();
  await expect(page.locator(".pixel-backpack")).toHaveCount(3);
  await expect(page.locator(".pass-state")).toHaveText("packed");
  await expect(
    page.getByRole("img", { name: "office wifi to phone hotspot" }),
  ).toBeVisible();
  await expect(
    page.getByText("handoff verified", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("lock the Mac, close it, and go.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", {
      level: 2,
      name: "no second environment to set up.",
    }),
  ).toBeVisible();
  await expect(page.getByText("pack → connect hotspot → go")).toBeVisible();
  await expect(
    page.getByRole("heading", {
      level: 2,
      name: "packed means you can leave.",
    }),
  ).toBeVisible();
  // Every row here must be a gate `pack` actually enforces. Two rows once claimed an observed
  // agent task and a phone confirmation, neither of which the CLI has ever done since the lease
  // became host-scoped, so this list is the place that drift shows up first.
  await expect(page.getByText("hotspot reaches the internet")).toBeVisible();
  await expect(
    page.getByText("enough charge to be worth leaving"),
  ).toBeVisible();
  await expect(page.getByText("not already throttling")).toBeVisible();
  await expect(
    page.getByText("closed-lid lease active and bounded"),
  ).toBeVisible();
  await expect(page.getByText("current task observed")).toHaveCount(0);
  await expect(page.getByText("access confirmed by you")).toHaveCount(0);
  await expect(
    page.getByText(
      "permissions stay unchanged. provider-native remotes carry the conversation; rucksack has no code relay.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", {
      level: 2,
      name: "what happens when the lid closes.",
    }),
  ).toBeVisible();
  await expect(
    page.getByText("why does my hotspot drop when I close the lid?", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("why doesn’t caffeinate fix it?", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("will Claude Code keep running if I close my laptop?", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText("why not just run the agent in the cloud?", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "can I control Codex, Claude Code, or Cursor from my phone?",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(page.getByRole("navigation", { name: "project" })).toContainText(
    "security",
  );
  await expect(
    page.getByRole("link", {
      name: "rucksack on GitHub",
    }),
  ).toHaveAttribute("href", "https://github.com/noahnawara/rucksack");
});

test("publishes canonical search, social, and software metadata", async ({
  page,
}): Promise<void> => {
  await page.goto("/");

  await expect(page.locator('meta[name="description"]')).toHaveAttribute(
    "content",
    "Keep Codex, Claude Code, or Cursor running on your MacBook and reachable from your phone through a verified hotspot handoff and bounded closed-lid lease.",
  );
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute(
    "content",
    "index, follow, max-image-preview:large, max-snippet:-1, max-video-preview:-1",
  );
  await expect(page.locator('meta[property="og:url"]')).toHaveAttribute(
    "content",
    "https://www.rucksack.wtf/",
  );
  await expect(page.locator('meta[property="og:image"]')).toHaveAttribute(
    "content",
    "https://www.rucksack.wtf/rucksack-ai-coding-agent-phone-hotspot.jpg",
  );
  await expect(page.locator('meta[property="og:image:width"]')).toHaveAttribute(
    "content",
    "1200",
  );
  await expect(
    page.locator('meta[property="og:image:height"]'),
  ).toHaveAttribute("content", "630");
  await expect(page.locator('meta[name="twitter:card"]')).toHaveAttribute(
    "content",
    "summary_large_image",
  );
  await expect(page.locator('link[rel="manifest"]')).toHaveAttribute(
    "href",
    "/site.webmanifest",
  );

  const structuredDataText = await page
    .locator('script[type="application/ld+json"]')
    .textContent();
  if (structuredDataText === null) {
    throw new Error("The JSON-LD graph is missing");
  }
  const structuredData: unknown = JSON.parse(structuredDataText);
  expect(structuredData).toMatchObject({
    "@context": "https://schema.org",
    "@graph": expect.arrayContaining([
      expect.objectContaining({
        "@type": "WebSite",
        url: "https://www.rucksack.wtf/",
      }),
      expect.objectContaining({
        "@type": "WebPage",
        url: "https://www.rucksack.wtf/",
      }),
      expect.objectContaining({
        "@type": "ImageObject",
        width: 1200,
        height: 630,
      }),
      // `codeRepository` belongs to SoftwareSourceCode, not SoftwareApplication, and
      // schema.org's validator rejects it here. `sameAs` carries the repository link
      // instead, so keep asserting that rather than reintroducing the invalid property.
      expect.objectContaining({
        "@type": "SoftwareApplication",
        operatingSystem: "macOS 14 or later",
        softwareVersion: "0.1.0-alpha.7",
        sameAs: "https://github.com/noahnawara/rucksack",
      }),
    ]),
  });
});

test("publishes crawl discovery files and a valid manifest", async ({
  request,
}): Promise<void> => {
  const expectedRobots = await readFile(robotsPath, "utf8");
  const expectedSitemap = await readFile(sitemapPath, "utf8");
  const manifestText = await readFile(manifestPath, "utf8");
  const manifest: unknown = JSON.parse(manifestText);

  const robotsResponse = await request.get("/robots.txt");
  expect(robotsResponse.ok()).toBe(true);
  expect(await robotsResponse.text()).toBe(expectedRobots);

  const sitemapResponse = await request.get("/sitemap.xml");
  expect(sitemapResponse.ok()).toBe(true);
  expect(await sitemapResponse.text()).toBe(expectedSitemap);

  const socialImageResponse = await request.get(
    "/rucksack-ai-coding-agent-phone-hotspot.jpg",
  );
  expect(socialImageResponse.ok()).toBe(true);
  expect(socialImageResponse.headers()["content-type"]).toContain("image/jpeg");

  for (const iconPath of [
    "/favicon.ico",
    "/apple-touch-icon.png",
    "/icon-192.png",
    "/icon-512.png",
  ] as const) {
    const iconResponse = await request.get(iconPath);
    expect(iconResponse.ok()).toBe(true);
    expect(iconResponse.headers()["content-type"]).toContain("image/");
  }

  expect(manifest).toMatchObject({
    name: "rucksack: Commute Mode for AI coding agents",
    start_url: "/",
    categories: ["productivity", "utilities"],
  });
});

test("renders and copies the canonical prompt byte for byte", async ({
  context,
  page,
}): Promise<void> => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  const canonicalPrompt = await readCanonicalPrompt();

  await expect(page.locator("#install-prompt")).toHaveText(canonicalPrompt);
  await page.getByRole("button", { name: "copy setup prompt" }).click();

  const clipboardText = await page.evaluate(async (): Promise<string> =>
    navigator.clipboard.readText(),
  );
  expect(clipboardText).toBe(canonicalPrompt);
  await expect(page.locator(".copy-text")).toHaveText("setup prompt copied");
  await expect(page.locator("#copy-status")).toHaveText("setup prompt copied.");
  await expect(page.locator("#manual-copy")).not.toBeVisible();
});

test("gives a direct manual-copy handoff when clipboard access fails", async ({
  page,
}): Promise<void> => {
  await page.addInitScript((): void => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (): Promise<never> =>
          Promise.reject(
            new DOMException("Clipboard access was blocked", "NotAllowedError"),
          ),
      },
    });
  });
  await page.goto("/");

  await page.getByRole("button", { name: "copy setup prompt" }).click();

  const status = page.locator("#copy-status");
  await expect(status).toContainText(
    "rucksack stopped copying the setup prompt.",
  );
  await expect(status).toContainText("your browser blocked clipboard access.");
  await expect(status).toContainText("select the prompt below and copy it.");
  await expect(page.locator("#manual-copy")).toBeVisible();
  await expect(page.locator("#install-prompt-details")).toHaveAttribute(
    "open",
    "",
  );
  await expect(page.locator("#install-prompt")).not.toBeEmpty();
  const selectedText = await page.evaluate(
    (): string => window.getSelection()?.toString() ?? "",
  );
  expect(selectedText).toBe((await readCanonicalPrompt()).trimEnd());
});

test("has no automated accessibility violations", async ({
  page,
}): Promise<void> => {
  await page.goto("/");

  const accessibilityScanResults = await new AxeBuilder({ page }).analyze();
  expect(accessibilityScanResults.violations).toEqual([]);
});

test("keeps the custom not-found page out of search and offers recovery", async ({
  page,
}): Promise<void> => {
  await page.goto("/404.html");

  await expect(page).toHaveTitle("route not packed | rucksack");
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute(
    "content",
    "noindex, follow",
  );
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "this page missed the handoff.",
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "back to rucksack" }),
  ).toHaveAttribute("href", "/");

  const accessibilityScanResults = await new AxeBuilder({ page }).analyze();
  expect(accessibilityScanResults.violations).toEqual([]);
});

test("does not create page-level horizontal overflow at narrow and tablet widths", async ({
  page,
}): Promise<void> => {
  for (const width of [320, 800, 959, 960]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/");

    const widths = await page.evaluate(
      (): { readonly clientWidth: number; readonly scrollWidth: number } => ({
        clientWidth: document.documentElement.clientWidth,
        scrollWidth: document.documentElement.scrollWidth,
      }),
    );

    expect(widths.scrollWidth).toBeLessThanOrEqual(widths.clientWidth);
  }
});

test("keeps the mobile action large while the pass stays subordinate", async ({
  page,
}): Promise<void> => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");

  const button = page.locator("[data-copy-prompt]");
  const pass = page.locator("#pass-stage");
  const headline = page.locator("#headline");
  const buttonBounds = await button.boundingBox();
  const passBounds = await pass.boundingBox();
  const headlineBounds = await headline.boundingBox();

  expect(buttonBounds).not.toBeNull();
  expect(passBounds).not.toBeNull();
  expect(headlineBounds).not.toBeNull();
  if (buttonBounds === null || passBounds === null || headlineBounds === null) {
    throw new Error("The mobile hierarchy has missing rendered bounds");
  }

  expect(buttonBounds.height).toBeGreaterThanOrEqual(44);
  expect(buttonBounds.width).toBeGreaterThanOrEqual(300);
  expect(buttonBounds.y + buttonBounds.height).toBeLessThanOrEqual(844);
  expect(passBounds.width).toBeLessThan(headlineBounds.width);
});

test("keeps every pass phrase on one line at 320 pixels", async ({
  page,
}): Promise<void> => {
  await page.setViewportSize({ width: 320, height: 900 });
  await page.goto("/");

  const phraseSelectors = [
    ".pass-title",
    ".pass-state",
    ".route-point",
    ".route-result",
  ] as const;

  const phraseLineCounts = await page.evaluate(
    (selectors: readonly string[]): readonly number[] =>
      selectors.flatMap((selector: string): readonly number[] =>
        Array.from(document.querySelectorAll<HTMLElement>(selector)).map(
          (element: HTMLElement): number => {
            const range = document.createRange();
            range.selectNodeContents(element);
            return range.getClientRects().length;
          },
        ),
      ),
    phraseSelectors,
  );

  expect(phraseLineCounts.length).toBeGreaterThan(0);
  expect(
    phraseLineCounts.every((lineCount: number): boolean => lineCount === 1),
  ).toBe(true);
});

test("keeps wide-screen content on one centered two-column grid", async ({
  page,
}): Promise<void> => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/");

  const hierarchy = await page.evaluate(
    (): {
      readonly headlineFontSize: number;
      readonly headlineLineHeight: number;
      readonly headlineWidth: number;
      readonly introLeft: number;
      readonly outerLeft: number;
      readonly outerRight: number;
      readonly passLeft: number;
      readonly passRight: number;
      readonly passWidth: number;
      readonly routeFontSize: number;
      readonly setupCopyLeft: number;
      readonly setupHeadlineFontSize: number;
      readonly setupHeadlineLineHeight: number;
      readonly setupPathsLeft: number;
      readonly setupPathsRight: number;
    } => {
      const header = document.querySelector<HTMLElement>(".site-header");
      const headline = document.querySelector<HTMLElement>("#headline");
      const intro = document.querySelector<HTMLElement>(".intro");
      const pass = document.querySelector<HTMLElement>("#pass-stage");
      const routeName = document.querySelector<HTMLElement>(".route");
      const setupCopy = document.querySelector<HTMLElement>(".setup-copy");
      const setupHeadline = document.querySelector<HTMLElement>("#setup-title");
      const setupPaths = document.querySelector<HTMLElement>(".setup-paths");
      if (
        header === null ||
        headline === null ||
        intro === null ||
        pass === null ||
        routeName === null ||
        setupCopy === null ||
        setupHeadline === null ||
        setupPaths === null
      ) {
        throw new Error("Desktop hierarchy elements are missing");
      }

      const headerBounds = header.getBoundingClientRect();
      const headlineBounds = headline.getBoundingClientRect();
      const headlineStyle = window.getComputedStyle(headline);
      const introBounds = intro.getBoundingClientRect();
      const passBounds = pass.getBoundingClientRect();
      const setupCopyBounds = setupCopy.getBoundingClientRect();
      const setupHeadlineStyle = window.getComputedStyle(setupHeadline);
      const setupPathsBounds = setupPaths.getBoundingClientRect();
      return {
        headlineFontSize: Number.parseFloat(headlineStyle.fontSize),
        headlineLineHeight: Number.parseFloat(headlineStyle.lineHeight),
        headlineWidth: headlineBounds.width,
        introLeft: introBounds.left,
        outerLeft: headerBounds.left,
        outerRight: window.innerWidth - headerBounds.right,
        passLeft: passBounds.left,
        passRight: passBounds.right,
        passWidth: passBounds.width,
        routeFontSize: Number.parseFloat(
          window.getComputedStyle(routeName).fontSize,
        ),
        setupCopyLeft: setupCopyBounds.left,
        setupHeadlineFontSize: Number.parseFloat(setupHeadlineStyle.fontSize),
        setupHeadlineLineHeight: Number.parseFloat(
          setupHeadlineStyle.lineHeight,
        ),
        setupPathsLeft: setupPathsBounds.left,
        setupPathsRight: setupPathsBounds.right,
      };
    },
  );

  expect(hierarchy.passWidth).toBeLessThan(hierarchy.headlineWidth);
  expect(hierarchy.routeFontSize).toBeLessThan(hierarchy.headlineFontSize);
  expect(hierarchy.headlineLineHeight / hierarchy.headlineFontSize).toBeCloseTo(
    1.2,
    2,
  );
  expect(
    hierarchy.setupHeadlineLineHeight / hierarchy.setupHeadlineFontSize,
  ).toBeCloseTo(1.2, 2);
  expect(hierarchy.outerLeft).toBeGreaterThanOrEqual(150);
  expect(Math.abs(hierarchy.outerLeft - hierarchy.outerRight)).toBeLessThan(1);
  expect(Math.abs(hierarchy.introLeft - hierarchy.setupCopyLeft)).toBeLessThan(
    1,
  );
  expect(Math.abs(hierarchy.passLeft - hierarchy.setupPathsLeft)).toBeLessThan(
    1,
  );
  expect(
    Math.abs(hierarchy.passRight - hierarchy.setupPathsRight),
  ).toBeLessThan(1);
});

test("shows the final pass state without motion when reduced motion is requested", async ({
  page,
}): Promise<void> => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");

  await expect(page.locator("#pass-stage")).not.toHaveClass(/is-running/);
  await expect(page.locator(".pass-state")).toHaveText("packed");
  await expect(
    page.getByText("handoff verified", { exact: true }),
  ).toBeVisible();

  const runningAnimations = await page.evaluate(
    (): number =>
      document
        .getAnimations()
        .filter(
          (animation: Animation): boolean => animation.playState === "running",
        ).length,
  );
  expect(runningAnimations).toBe(0);
});

test("keeps the hero glyphs fixed when the display font arrives late", async ({
  page,
}): Promise<void> => {
  const displayFont = '700 3rem "DM Sans Variable"';
  const fontGate = Promise.withResolvers<void>();
  const fontRequestStarted = Promise.withResolvers<void>();

  await page.route(
    "**/*dm-sans-latin-wght-normal*.woff2*",
    async (route): Promise<void> => {
      fontRequestStarted.resolve();
      await fontGate.promise;
      await route.continue();
    },
  );

  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/", { waitUntil: "commit" });

  const headline = page.locator("#headline");
  await headline.waitFor({ state: "attached" });
  await fontRequestStarted.promise;
  await page.waitForTimeout(250);
  const beforeFonts = await headline.evaluate(measureTextLines);

  fontGate.resolve();
  await page.evaluate(async (font: string): Promise<void> => {
    await document.fonts.load(font);
    await document.fonts.ready;
    await new Promise<void>((resolve: () => void): void => {
      requestAnimationFrame((): void => {
        requestAnimationFrame((): void => resolve());
      });
    });
  }, displayFont);
  const afterFonts = await headline.evaluate(measureTextLines);

  expect(afterFonts).toEqual(beforeFonts);
});

test("brings the complete pass into focus without spatial movement", async ({
  page,
}): Promise<void> => {
  await page.goto("/");
  await expect(page.locator("#pass-stage")).toHaveClass(/is-running/);

  // The property this guards is in the title: the pass may come into focus, but it may not move.
  // A reveal that shifts layout is the one that pushes the page around while somebody is reading it.
  //
  // What it deliberately no longer asserts is the animation's literal duration, filter strings, and
  // opacity stops. Those were the keyframe block transcribed into TypeScript, so any tweak to the
  // timing broke the test without anything being wrong, and matching them proved only that the CSS
  // said what the CSS said.
  const focus = await page.locator(".pass").evaluate(
    (
      element: HTMLElement,
    ): {
      readonly bounds: readonly {
        readonly height: number;
        readonly width: number;
        readonly x: number;
        readonly y: number;
      }[];
      readonly spatialProperties: readonly string[];
    } => {
      element.classList.remove("pass-reveal");
      void element.offsetWidth;
      element.classList.add("pass-reveal");

      const animation = element
        .getAnimations()
        .find(
          (candidate: Animation): boolean =>
            candidate instanceof CSSAnimation &&
            candidate.animationName === "focus-pass",
        );
      if (!(animation?.effect instanceof KeyframeEffect)) {
        throw new Error("The commute-pass focus animation is missing");
      }

      const timing = animation.effect.getComputedTiming();
      if (typeof timing.duration !== "number") {
        throw new TypeError("The commute-pass focus duration is not numeric");
      }

      animation.pause();
      const bounds = [0, timing.duration / 2, timing.duration].map(
        (
          currentTime: number,
        ): {
          readonly height: number;
          readonly width: number;
          readonly x: number;
          readonly y: number;
        } => {
          animation.currentTime = currentTime;
          const rect = element.getBoundingClientRect();
          return {
            height: rect.height,
            width: rect.width,
            x: rect.x,
            y: rect.y,
          };
        },
      );
      const spatialProperties = animation.effect
        .getKeyframes()
        .flatMap((keyframe: ComputedKeyframe): readonly string[] =>
          [
            typeof keyframe.transform === "string" ? "transform" : "",
            typeof keyframe.clipPath === "string" ? "clip-path" : "",
          ].filter((property: string): boolean => property !== ""),
        )
        .sort();
      animation.finish();

      return { bounds, spatialProperties };
    },
  );

  expect(focus.spatialProperties).toEqual([]);
  expect(focus.bounds[1]).toEqual(focus.bounds[0]);
  expect(focus.bounds[2]).toEqual(focus.bounds[0]);
  await expect(page.locator(".pass")).toHaveCSS("filter", "none");
});

test("keeps the page and prompt readable without JavaScript", async ({
  browser,
}): Promise<void> => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto("/");

  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "close your laptop. keep your agents running.",
    }),
  ).toBeVisible();
  await expect(page.locator("#install-prompt")).not.toBeEmpty();
  await expect(
    page.getByText("open the setup prompt below and copy it into your agent."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "copy setup prompt" }),
  ).toHaveCount(0);

  await context.close();
});

/// The README and the website hand out the same text, or one of them is lying to somebody.
///
/// They drifted once already: the README kept a truncated copy of a prompt two releases old,
/// missing the paragraphs that stop an agent running `pack` as a smoke test and that make the
/// departure instructions outlive the conversation they were pasted into.
test("the README hands out exactly the prompt the website does", async () => {
  const prompt = (await readFile(promptPath, "utf8")).replace(/\n+$/, "");
  const readme = await readFile(readmePath, "utf8");
  const block = readme.match(/```text\n([\s\S]*?)\n```/);

  expect(block, "README has no ```text prompt block").not.toBeNull();
  expect(block![1]).toBe(prompt);
});
