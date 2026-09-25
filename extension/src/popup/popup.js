import { bindAskForm } from "./ask-form.js";
import { bindSelectionInsert } from "./selection-insert.js";

/**
 * @template {HTMLElement} T
 * @param {string} selector
 * @param {{new (): T}} type
 * @returns {T}
 */
function requireElement(selector, type) {
  const node = document.querySelector(selector);
  if (!(node instanceof type)) {
    throw new Error(`popup element is missing: ${selector}`);
  }
  return node;
}

bindAskForm(
  {
    form: requireElement("#ask-form", HTMLFormElement),
    input: requireElement("#ask-input", HTMLTextAreaElement),
    submit: requireElement("#ask-submit", HTMLButtonElement),
    status: requireElement("#status", HTMLElement),
    answer: requireElement("#answer", HTMLElement)
  },
  chrome.runtime
);

bindSelectionInsert(
  requireElement("#insert-selection", HTMLButtonElement),
  requireElement("#ask-input", HTMLTextAreaElement),
  requireElement("#selection-status", HTMLElement),
  chrome.runtime
);

requireElement("#open-full-page", HTMLButtonElement).addEventListener(
  "click",
  async () => {
    await chrome.tabs.create({
      url: chrome.runtime.getURL("src/fullpage/index.html?entry=popup")
    });
  }
);

export {};
