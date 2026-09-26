import { copyText } from "../shared/clipboard.js";
import { bindProviderSettings } from "./provider-settings.js";
import { bindThemeSelect } from "../shared/theme.js";
import { bindThemeToggle } from "../shared/theme-toggle.js";

const select = document.querySelector("#theme-select");
const toggle = document.querySelector("#theme-toggle");
if (select instanceof HTMLSelectElement && toggle instanceof HTMLElement) {
  void bindThemeSelect(select, chrome.storage.local, document.documentElement, chrome.storage.onChanged);
  bindThemeToggle(toggle, select, document.documentElement);
}

const providerOptions = document.querySelector("#provider-options");
const modelSelect = document.querySelector("#model-select");
const modelNote = document.querySelector("#model-note");
const customField = document.querySelector("#model-custom-field");
const customInput = document.querySelector("#model-custom");
const customSave = document.querySelector("#model-custom-save");
if (providerOptions instanceof HTMLElement && modelSelect instanceof HTMLSelectElement &&
    modelNote instanceof HTMLElement && customField instanceof HTMLElement &&
    customInput instanceof HTMLInputElement && customSave instanceof HTMLButtonElement) {
  bindProviderSettings(
    { options: providerOptions, model: modelSelect, modelNote, customField, customInput, customSave },
    chrome.runtime,
    chrome.storage.local,
    chrome.storage.onChanged
  );
}

for (const button of Array.from(document.querySelectorAll(".copy-command"))) {
  button.addEventListener("click", async () => {
    const command = button.parentElement?.querySelector("code")?.textContent ?? "";
    try {
      await copyText(command);
      button.setAttribute("data-copied", "true");
      button.setAttribute("aria-label", "Copied");
      setTimeout(() => {
        button.removeAttribute("data-copied");
        button.setAttribute("aria-label", "Copy command");
      }, 1500);
    } catch {
      // Copying is a convenience; the command stays selectable.
    }
  });
}
