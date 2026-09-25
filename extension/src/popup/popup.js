import { bindAskForm } from "./ask-form.js";
import { bindContextControls } from "./context-controls.js";
import { preloadMenuContext } from "./menu-preload.js";
import { bindProviderState } from "./provider-state.js";

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

const contextControls = bindContextControls(
  {
    none: requireElement("#context-none", HTMLButtonElement),
    selection: requireElement("#context-selection", HTMLButtonElement),
    page: requireElement("#context-page", HTMLButtonElement),
    status: requireElement("#context-status", HTMLElement),
    preview: requireElement("#context-preview", HTMLElement)
  },
  chrome.runtime
);
void preloadMenuContext(chrome.runtime, contextControls, window.location.search);

const providerState = bindProviderState(
  requireElement("#provider-state", HTMLElement),
  chrome.runtime
);

bindAskForm(
  {
    form: requireElement("#ask-form", HTMLFormElement),
    input: requireElement("#ask-input", HTMLTextAreaElement),
    submit: requireElement("#ask-submit", HTMLButtonElement),
    status: requireElement("#status", HTMLElement),
    answer: requireElement("#answer", HTMLElement)
  },
  chrome.runtime,
  contextControls,
  { onOutcome: (outcome) => providerState.update(outcome) }
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
