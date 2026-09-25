import { bindAskForm } from "./ask-form.js";
import { bindContextControls } from "./context-controls.js";
import { preloadMenuContext } from "./menu-preload.js";
import { bindProviderState } from "./provider-state.js";
import { bindRecentConversations } from "../shared/recent-conversations.js";
import { bindThemeSelect } from "../shared/theme.js";
import { bindDiagnostics } from "./diagnostics.js";
import { recordDuration } from "../shared/performance.js";

const popupStartedAt = globalThis.performance?.now?.() ?? 0;

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
const providerState = bindProviderState(
  requireElement("#provider-state", HTMLElement),
  chrome.runtime,
  { setupLink: requireElement("#companion-setup", HTMLElement) }
);
const view = bindAskForm(
  {
    form: requireElement("#ask-form", HTMLFormElement),
    input: requireElement("#ask-input", HTMLTextAreaElement),
    submit: requireElement("#ask-submit", HTMLButtonElement),
    status: requireElement("#status", HTMLElement),
    answer: requireElement("#answer", HTMLElement),
    history: requireElement("#conversation-history", HTMLElement),
    cancel: requireElement("#ask-cancel", HTMLButtonElement),
    retry: requireElement("#ask-retry", HTMLButtonElement)
  },
  chrome.runtime,
  contextControls,
  {
    onOutcome: (outcome) => providerState.update(outcome),
    onConversationId(id) { fullView.disabled = !id; },
    onSaved() { void recentIndex.refresh(); },
    onRequestStarted() { interacted = true; },
    storageChanges: chrome.storage.onChanged
  }
);
const recentIndex = bindRecentConversations(recent, chrome.runtime, view);

void bindThemeSelect(
  requireElement("#theme-select", HTMLSelectElement),
  chrome.storage.local,
  document.documentElement
);

bindDiagnostics(
  {
    details: requireElement("#diagnostics", HTMLDetailsElement),
    refresh: requireElement("#diag-refresh", HTMLButtonElement),
    host: requireElement("#diag-host", HTMLElement),
    protocol: requireElement("#diag-protocol", HTMLElement),
    provider: requireElement("#diag-provider", HTMLElement),
    failure: requireElement("#diag-failure", HTMLElement)
  },
  chrome.runtime
);

recordDuration(
  "popup_input_ready",
  popupStartedAt,
  globalThis.performance?.now?.() ?? popupStartedAt
);
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
