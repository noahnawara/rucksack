import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { fileURLToPath, URL } from "node:url";

const promptPath = fileURLToPath(
  new URL("../src/content/install-agent-prompt.txt", import.meta.url),
);

const readCanonicalPrompt = async (): Promise<string> =>
  readFile(promptPath, "utf8");

test("shows the product, main action, source, and honest release state", async ({
  page,
}): Promise<void> => {
  await page.goto("/");

  await expect(
    page.getByRole("heading", {
      level: 1,
      name: "keep your agent running on your commute",
    }),
  ).toBeVisible();
  await expect(
    page.locator("#install").getByText("compiler-verified alpha", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "copy the agent prompt" }).first(),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "read source" }).first(),
  ).toHaveAttribute("href", "https://github.com/noahnawara/rucksack");
});

test("renders and copies the canonical prompt byte for byte", async ({
  context,
  page,
}): Promise<void> => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  const canonicalPrompt = await readCanonicalPrompt();

  await expect(page.locator("#install-prompt")).toHaveText(canonicalPrompt);
  await page
    .getByRole("button", { name: "copy the agent prompt" })
    .first()
    .click();

  const clipboardText = await page.evaluate(
    async (): Promise<string> => navigator.clipboard.readText(),
  );
  expect(clipboardText).toBe(canonicalPrompt);
  await expect(page.locator("#copy-status")).toContainText("agent prompt copied.");
  await expect(page.locator("#copy-status")).toContainText(
    "paste it into codex, claude code, or cursor on this mac.",
  );
});

test("gives a direct manual-copy handoff when clipboard access fails", async ({
  page,
}): Promise<void> => {
  await page.addInitScript((): void => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: (): Promise<never> =>
          Promise.reject(new DOMException("Clipboard access was blocked", "NotAllowedError")),
      },
    });
  });
  await page.goto("/");

  await page
    .getByRole("button", { name: "copy the agent prompt" })
    .first()
    .click();

  const status = page.locator("#copy-status");
  await expect(status).toContainText("rucksack stopped copying the agent prompt.");
  await expect(status).toContainText("clipboard access was blocked.");
  await expect(status).toContainText("select the prompt and copy it.");
  await expect(page.locator("#install-prompt-details")).toHaveAttribute("open", "");
  await expect(page.locator("#install-prompt")).not.toBeEmpty();
  const selectedText = await page.evaluate(
    (): string => window.getSelection()?.toString() ?? "",
  );
  expect(selectedText).toBe((await readCanonicalPrompt()).trimEnd());
});

test("has no automated accessibility violations", async ({ page }): Promise<void> => {
  await page.goto("/");

  const accessibilityScanResults = await new AxeBuilder({ page }).analyze();
  expect(accessibilityScanResults.violations).toEqual([]);
});

test("does not create page-level horizontal overflow at 320 pixels", async ({
  page,
}): Promise<void> => {
  await page.setViewportSize({ width: 320, height: 900 });
  await page.goto("/");

  const widths = await page.evaluate(
    (): { readonly clientWidth: number; readonly scrollWidth: number } => ({
      clientWidth: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
    }),
  );

  expect(widths.scrollWidth).toBeLessThanOrEqual(widths.clientWidth);
});

test("keeps the primary mobile action large and inside the first viewport", async ({
  page,
}): Promise<void> => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");

  const button = page.locator(".action-group--hero [data-copy-prompt]");
  const bounds = await button.boundingBox();
  expect(bounds).not.toBeNull();
  if (bounds === null) {
    throw new Error("The primary mobile copy action has no rendered bounds");
  }

  expect(bounds.height).toBeGreaterThanOrEqual(44);
  expect(bounds.width).toBeGreaterThanOrEqual(300);
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(844);
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
      name: "keep your agent running on your commute",
    }),
  ).toBeVisible();
  await expect(page.locator("#install-prompt")).not.toBeEmpty();
  await expect(page.getByText("open the prompt below")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "copy the agent prompt" }),
  ).toHaveCount(0);

  await context.close();
});
