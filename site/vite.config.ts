import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";
import { defineConfig, type Plugin } from "vite";

const PROMPT_TOKEN = "%%rucksack_install_agent_prompt%%";
const promptPath = fileURLToPath(
  new URL("./src/content/install-agent-prompt.txt", import.meta.url),
);

const escapeHtml = (value: string): string =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");

const injectInstallPrompt = (): Plugin => ({
  name: "inject-install-agent-prompt",
  transformIndexHtml(html: string): string {
    if (!html.includes(PROMPT_TOKEN)) {
      throw new Error(`Missing install prompt token ${PROMPT_TOKEN} in site/index.html`);
    }

    const prompt = readFileSync(promptPath, "utf8");
    return html.replace(PROMPT_TOKEN, escapeHtml(prompt));
  },
});

export default defineConfig({
  plugins: [injectInstallPrompt()],
});
