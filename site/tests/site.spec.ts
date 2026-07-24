import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { fileURLToPath, URL } from "node:url";

const GITHUB_REPOSITORY_API =
  "https://api.github.com/repos/noahnawara/rucksack";
const promptPath = fileURLToPath(
  new URL("../src/content/install-agent-prompt.txt", import.meta.url),
);

const readCanonicalPrompt = async (): Promise<string> =>
  readFile(promptPath, "utf8");

test.beforeEach(async ({ page }): Promise<void> => {
  await page.route(
    GITHUB_REPOSITORY_API,
    async (route): Promise<void> => {
      await route.fulfill({
        json: {
          stargazers_count: 1_234,
        },
      });
    },
  );
});

test("shows one promise, one distilled commute pass, one action, and the live star count", async ({
  page,
}): Promise<void> => {
  await page.goto("/");

  await expect(page).toHaveTitle(
    "rucksack — switch to your hotspot. keep your agent running.",
  );
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute(
    "href",
    "https://rucksack.wtf",
  );
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "switch to your hotspot. keep your agent running.",
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
    page.getByText("seamless commute", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "close the lid. keep steering from your phone.",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", {
      level: 2,
      name: "don’t move the project. move yourself.",
    }),
  ).toBeVisible();
  await expect(page.getByText("pack → connect hotspot → go")).toBeVisible();
  await expect(
    page.getByRole("heading", {
      level: 2,
      name: "packed means you can leave.",
    }),
  ).toBeVisible();
  await expect(page.getByText("phone hotspot has internet")).toBeVisible();
  await expect(page.getByText("current task observed")).toBeVisible();
  await expect(page.getByText("access confirmed by you")).toBeVisible();
  await expect(
    page.getByText("closed-lid lease active and bounded"),
  ).toBeVisible();
  await expect(
    page.getByText(
      "permissions stay unchanged. rucksack never relays your code.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: "project" }),
  ).toContainText("security");
  await expect(
    page.getByRole("link", {
      name: "rucksack on GitHub; 1,234 stars",
    }),
  ).toHaveAttribute("href", "https://github.com/noahnawara/rucksack");
  await expect(page.locator("[data-github-star-count]")).toHaveText("1.2K");
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

  const clipboardText = await page.evaluate(
    async (): Promise<string> => navigator.clipboard.readText(),
  );
  expect(clipboardText).toBe(canonicalPrompt);
  await expect(page.locator(".copy-text")).toHaveText("setup prompt copied");
  await expect(page.locator("#copy-status")).toHaveText(
    "setup prompt copied.",
  );
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
            new DOMException(
              "Clipboard access was blocked",
              "NotAllowedError",
            ),
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
  await expect(status).toContainText(
    "your browser blocked clipboard access.",
  );
  await expect(status).toContainText(
    "you — select the prompt and copy it.",
  );
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
  if (
    buttonBounds === null ||
    passBounds === null ||
    headlineBounds === null
  ) {
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
  expect(phraseLineCounts.every((lineCount: number): boolean => lineCount === 1)).toBe(
    true,
  );
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
        setupHeadlineFontSize: Number.parseFloat(
          setupHeadlineStyle.fontSize,
        ),
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
  expect(
    hierarchy.headlineLineHeight / hierarchy.headlineFontSize,
  ).toBeCloseTo(1.2, 2);
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
  expect(Math.abs(hierarchy.passRight - hierarchy.setupPathsRight)).toBeLessThan(
    1,
  );
});

test("shows the final pass state without motion when reduced motion is requested", async ({
  page,
}): Promise<void> => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");

  await expect(page.locator("#pass-stage")).not.toHaveClass(/is-running/);
  await expect(page.locator(".pass-state")).toHaveText("packed");
  await expect(
    page.getByText("seamless commute", { exact: true }),
  ).toBeVisible();

  const runningAnimations = await page.evaluate(
    (): number =>
      document
        .getAnimations()
        .filter((animation: Animation): boolean => animation.playState === "running")
        .length,
  );
  expect(runningAnimations).toBe(0);
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
      name: "switch to your hotspot. keep your agent running.",
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
