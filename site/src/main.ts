import "@fontsource-variable/atkinson-hyperlegible-next/wght.css";
import "@fontsource/commit-mono/400.css";
import "@fontsource/commit-mono/700.css";
import "./styles.css";

type CopyElements = {
  readonly buttons: readonly HTMLButtonElement[];
  readonly prompt: HTMLElement;
  readonly promptContainer: HTMLElement;
  readonly promptDetails: HTMLDetailsElement;
  readonly status: HTMLElement;
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

const getCopyElements = (): CopyElements => {
  const buttons = Array.from(
    document.querySelectorAll<HTMLButtonElement>("[data-copy-prompt]"),
  );
  if (buttons.length === 0) {
    throw new Error("At least one install prompt copy button is required");
  }

  return {
    buttons,
    prompt: requireElement<HTMLElement>("#install-prompt"),
    promptContainer: requireElement<HTMLElement>("#install-prompt-container"),
    promptDetails: requireElement<HTMLDetailsElement>("#install-prompt-details"),
    status: requireElement<HTMLElement>("#copy-status"),
  };
};

const selectPrompt = (elements: CopyElements): void => {
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

const showCopySuccess = (
  button: HTMLButtonElement,
  status: HTMLElement,
): void => {
  button.textContent = "agent prompt copied";
  status.dataset.state = "success";
  status.textContent =
    "agent prompt copied.\n\npaste it into codex, claude code, or cursor on this mac.";
};

const showCopyFailure = (
  button: HTMLButtonElement,
  elements: CopyElements,
): void => {
  button.textContent = "select the prompt";
  elements.status.dataset.state = "error";
  elements.status.textContent =
    "rucksack stopped copying the agent prompt.\n\nclipboard access was blocked.\n\nselect the prompt and copy it.";
  selectPrompt(elements);
};

const copyPrompt = async (
  button: HTMLButtonElement,
  elements: CopyElements,
): Promise<void> => {
  const prompt = elements.prompt.textContent;
  if (prompt === null) {
    throw new Error("The canonical install prompt has no text content");
  }

  if (navigator.clipboard === undefined) {
    showCopyFailure(button, elements);
    return;
  }

  try {
    await navigator.clipboard.writeText(prompt);
    showCopySuccess(button, elements.status);
  } catch (error: unknown) {
    console.warn("Clipboard write failed", {
      error,
      promptLength: prompt.length,
    });
    showCopyFailure(button, elements);
  }
};

document.documentElement.classList.remove("no-js");
document.documentElement.classList.add("js");

const copyElements = getCopyElements();
for (const button of copyElements.buttons) {
  button.addEventListener("click", (): void => {
    void copyPrompt(button, copyElements);
  });
}
