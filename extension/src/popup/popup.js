import { bindAskForm } from "./ask-form.js";
import { bindContextControls } from "./context-controls.js";
import { preloadMenuContext } from "./menu-preload.js";
import { bindRecentConversations } from "../shared/recent-conversations.js";

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
const fullView = requireElement("#open-full-page", HTMLButtonElement);
const recent = requireElement("#recent-conversations", HTMLSelectElement);
let interacted = false;
const view = bindAskForm(
  {
    form: requireElement("#ask-form", HTMLFormElement),
    input: requireElement("#ask-input", HTMLTextAreaElement),
    submit: requireElement("#ask-submit", HTMLButtonElement),
    status: requireElement("#status", HTMLElement),
    answer: requireElement("#answer", HTMLElement),
    history: requireElement("#conversation-history", HTMLElement)
  },
  chrome.runtime,
  contextControls,
  {
    onConversationId(id) { fullView.disabled = !id; },
    onSaved() { void recentIndex.refresh(); },
    onRequestStarted() { interacted = true; },
    storageChanges: chrome.storage.onChanged
  }
);
const recentIndex = bindRecentConversations(recent, chrome.runtime, view);
requireElement("#new-conversation", HTMLButtonElement).addEventListener("click", () => {
  if (view.newConversation()) {
    interacted = true;
    recent.value = "";
  }
});
recent.addEventListener("change", () => { interacted = true; });
void preloadMenuContext(chrome.runtime, contextControls, window.location.search).then(async (menuOpened) => {
  const items = await recentIndex.refresh();
  if (!menuOpened && !interacted && !view.getConversationId() && items[0]) {
    await view.loadConversation(items[0].id);
    recent.value = view.getConversationId() ?? "";
  }
});

fullView.addEventListener(
  "click",
  async () => {
    const id = view.getConversationId();
    if (!id) return;
    const url = new URL(chrome.runtime.getURL("src/fullpage/index.html"));
    url.searchParams.set("conversation", id);
    await chrome.tabs.create({
      url: url.toString()
    });
  }
);

export {};
