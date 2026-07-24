import "@fontsource-variable/dm-sans/wght.css";
import "@fontsource-variable/doto/wght.css";
import { inject } from "@vercel/analytics";
import { injectSpeedInsights } from "@vercel/speed-insights";
import "./styles.css";

type CopyElements = {
  readonly button: HTMLButtonElement;
  readonly buttonText: HTMLElement;
  readonly manualCopy: HTMLElement;
  readonly prompt: HTMLElement;
  readonly promptContainer: HTMLElement;
  readonly promptDetails: HTMLDetailsElement;
  readonly status: HTMLElement;
};

type PassElements = {
  readonly pass: HTMLElement;
  readonly stage: HTMLElement;
};

type GitHubElements = {
  readonly count: HTMLElement;
  readonly link: HTMLAnchorElement;
};

type GitHubRepositoryResponse = {
  readonly stargazers_count?: unknown;
};

const GITHUB_REPOSITORY_API =
  "https://api.github.com/repos/noahnawara/rucksack";
const GITHUB_REPOSITORY_URL = "https://github.com/noahnawara/rucksack";
const GITHUB_REQUEST_ATTEMPTS = 2;

const requireElement = <ElementType extends Element>(
  selector: string,
): ElementType => {
  const element = document.querySelector<ElementType>(selector);
  if (element === null) {
    throw new Error(`Required site element is missing: ${selector}`);
  }
  return element;
};

const getCopyElements = (): CopyElements => ({
  button: requireElement<HTMLButtonElement>("[data-copy-prompt]"),
  buttonText: requireElement<HTMLElement>(".copy-text"),
  manualCopy: requireElement<HTMLElement>("#manual-copy"),
  prompt: requireElement<HTMLElement>("#install-prompt"),
  promptContainer: requireElement<HTMLElement>("#install-prompt-container"),
  promptDetails: requireElement<HTMLDetailsElement>("#install-prompt-details"),
  status: requireElement<HTMLElement>("#copy-status"),
});

const getPassElements = (): PassElements => ({
  pass: requireElement<HTMLElement>(".pass"),
  stage: requireElement<HTMLElement>("#pass-stage"),
});

const getGitHubElements = (): GitHubElements => ({
  count: requireElement<HTMLElement>("[data-github-star-count]"),
  link: requireElement<HTMLAnchorElement>("[data-github-link]"),
});

const selectPrompt = (elements: CopyElements): void => {
  elements.manualCopy.dataset.visible = "";
  elements.promptDetails.open = true;

  const selection = window.getSelection();
  if (selection === null) {
    elements.promptContainer.focus();
    return;
  }

  const range = document.createRange();
  range.selectNodeContents(elements.prompt);
  selection.removeAllRanges();
  selection.addRange(range);
  elements.promptContainer.focus();
};

const showCopySuccess = (elements: CopyElements): void => {
  elements.buttonText.textContent = "agent prompt copied";
  elements.button.classList.add("is-copied");
  elements.status.dataset.state = "success";
  elements.status.textContent = "agent prompt copied.";
};

const showCopyFailure = (elements: CopyElements): void => {
  elements.buttonText.textContent = "select the agent prompt";
  elements.status.dataset.state = "error";
  elements.status.textContent =
    "rucksack stopped copying the agent prompt.\n\n" +
    "your browser blocked clipboard access.\n\n" +
    "you — select the prompt and copy it.";
  selectPrompt(elements);
};

const copyPrompt = async (elements: CopyElements): Promise<void> => {
  const prompt = elements.prompt.textContent;
  if (prompt === null) {
    throw new Error("The canonical install prompt has no text content");
  }

  if (navigator.clipboard === undefined) {
    showCopyFailure(elements);
    return;
  }

  try {
    await navigator.clipboard.writeText(prompt);
    showCopySuccess(elements);
  } catch (error: unknown) {
    console.warn("copy_agent_prompt_failed", {
      error,
      promptLength: prompt.length,
    });
    showCopyFailure(elements);
  }
};

const setScanDistance = (elements: PassElements): void => {
  elements.pass.style.setProperty(
    "--scan-distance",
    `${elements.pass.clientWidth + 4}px`,
  );
};

const playPass = (
  elements: PassElements,
  reduceMotion: MediaQueryList,
): void => {
  setScanDistance(elements);
  elements.stage.classList.remove("is-running");

  if (reduceMotion.matches) {
    return;
  }

  requestAnimationFrame((): void => {
    elements.stage.classList.add("is-running");
  });
};

const wait = async (milliseconds: number): Promise<void> =>
  new Promise((resolve: () => void): void => {
    window.setTimeout(resolve, milliseconds);
  });

const readGitHubStarCount = (value: unknown): number => {
  if (typeof value !== "object" || value === null) {
    throw new TypeError("GitHub repository response was not an object");
  }

  const response = value as GitHubRepositoryResponse;
  const count = response.stargazers_count;
  if (
    typeof count !== "number" ||
    !Number.isSafeInteger(count) ||
    count < 0
  ) {
    throw new TypeError(
      `GitHub repository response has an invalid stargazers_count: ${String(count)}`,
    );
  }

  return count;
};

const requestGitHubStarCount = async (): Promise<number> => {
  const response = await fetch(GITHUB_REPOSITORY_API, {
    headers: {
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });

  if (!response.ok) {
    const responseBody = await response.text();
    throw new Error(
      `GitHub star request failed for ${GITHUB_REPOSITORY_API} with ` +
        `${response.status} ${response.statusText}: ${responseBody.slice(0, 500)}`,
    );
  }

  return readGitHubStarCount(await response.json());
};

const fetchGitHubStarCount = async (): Promise<number> => {
  let lastError: unknown = new Error(
    `GitHub star request did not run for ${GITHUB_REPOSITORY_API}`,
  );

  for (
    let attempt = 1;
    attempt <= GITHUB_REQUEST_ATTEMPTS;
    attempt += 1
  ) {
    try {
      return await requestGitHubStarCount();
    } catch (error: unknown) {
      lastError = error;
      console.warn("github_star_request_failed", {
        attempt,
        error,
        maxAttempts: GITHUB_REQUEST_ATTEMPTS,
        repository: GITHUB_REPOSITORY_URL,
      });

      if (attempt < GITHUB_REQUEST_ATTEMPTS) {
        await wait(250 * attempt);
      }
    }
  }

  if (lastError instanceof Error) {
    throw lastError;
  }
  throw new Error("GitHub star request failed with an unknown error");
};

const formatCompactCount = (count: number): string =>
  new Intl.NumberFormat("en", {
    maximumFractionDigits: 1,
    notation: "compact",
  }).format(count);

const showGitHubStarCount = (
  elements: GitHubElements,
  count: number,
): void => {
  const exactCount = new Intl.NumberFormat("en").format(count);
  elements.count.textContent = formatCompactCount(count);
  elements.link.setAttribute(
    "aria-label",
    `rucksack on GitHub; ${exactCount} stars`,
  );
};

const loadGitHubStarCount = async (
  elements: GitHubElements,
): Promise<void> => {
  try {
    showGitHubStarCount(elements, await fetchGitHubStarCount());
  } catch (error: unknown) {
    console.warn("github_star_count_unavailable", {
      error,
      repository: GITHUB_REPOSITORY_URL,
    });
  }
};

const isLocalHostname = (hostname: string): boolean =>
  hostname === "127.0.0.1" || hostname === "localhost";

const initializeVercelTelemetry = (): void => {
  if (isLocalHostname(window.location.hostname)) {
    return;
  }

  inject({ framework: "vite" });
  injectSpeedInsights({ framework: "vite" });
};

document.documentElement.classList.remove("no-js");
document.documentElement.classList.add("js");

const copyElements = getCopyElements();
const passElements = getPassElements();
const githubElements = getGitHubElements();
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

copyElements.button.addEventListener("click", (): void => {
  void copyPrompt(copyElements);
});
reduceMotion.addEventListener("change", (): void => {
  playPass(passElements, reduceMotion);
});
window.addEventListener("resize", (): void => {
  setScanDistance(passElements);
});
window.addEventListener(
  "load",
  (): void => {
    playPass(passElements, reduceMotion);
  },
  { once: true },
);

initializeVercelTelemetry();
void loadGitHubStarCount(githubElements);
