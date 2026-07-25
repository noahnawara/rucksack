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
  readonly stage: HTMLElement;
};

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
  stage: requireElement<HTMLElement>("#pass-stage"),
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
  elements.buttonText.textContent = "setup prompt copied";
  elements.button.classList.add("is-copied");
  elements.status.dataset.state = "success";
  elements.status.textContent = "setup prompt copied.";
};

const showCopyFailure = (elements: CopyElements): void => {
  elements.buttonText.textContent = "select the setup prompt";
  elements.status.dataset.state = "error";
  elements.status.textContent =
    "rucksack stopped copying the setup prompt.\n\n" +
    "your browser blocked clipboard access.\n\n" +
    "you — select the prompt and copy it.";
  selectPrompt(elements);
};

const copyPrompt = async (elements: CopyElements): Promise<void> => {
  const prompt = elements.prompt.textContent;
  if (prompt === null) {
    throw new Error("The canonical setup prompt has no text content");
  }

  if (navigator.clipboard === undefined) {
    showCopyFailure(elements);
    return;
  }

  try {
    await navigator.clipboard.writeText(prompt);
    showCopySuccess(elements);
  } catch (error: unknown) {
    console.warn("copy_setup_prompt_failed", {
      error,
      promptLength: prompt.length,
    });
    showCopyFailure(elements);
  }
};

const playPass = (
  elements: PassElements,
  reduceMotion: MediaQueryList,
): void => {
  elements.stage.classList.remove("is-running");

  if (reduceMotion.matches) {
    return;
  }

  requestAnimationFrame((): void => {
    elements.stage.classList.add("is-running");
  });
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
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

copyElements.button.addEventListener("click", (): void => {
  void copyPrompt(copyElements);
});
reduceMotion.addEventListener("change", (): void => {
  playPass(passElements, reduceMotion);
});
window.addEventListener(
  "load",
  (): void => {
    playPass(passElements, reduceMotion);
  },
  { once: true },
);

initializeVercelTelemetry();
