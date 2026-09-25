import { bindAskForm } from "../popup/ask-form.js";
import { bindRecentConversations } from "../shared/recent-conversations.js";
import { bindThemeSelect } from "../shared/theme.js";

/** @template {HTMLElement} T @param {string} selector @param {{new (): T}} type @returns {T} */
function requireElement(selector, type) {
  const node = document.querySelector(selector);
  if (!(node instanceof type)) throw new Error(`full-page element is missing: ${selector}`);
  return node;
}

const recent = requireElement("#recent-conversations", HTMLSelectElement);
let interacted = false;
const view = bindAskForm({
  form: requireElement("#ask-form", HTMLFormElement),
  input: requireElement("#ask-input", HTMLTextAreaElement),
  submit: requireElement("#ask-submit", HTMLButtonElement),
  status: requireElement("#status", HTMLElement),
  answer: requireElement("#answer", HTMLElement),
  history: requireElement("#conversation-history", HTMLElement),
  cancel: requireElement("#ask-cancel", HTMLButtonElement),
  retry: requireElement("#ask-retry", HTMLButtonElement)
}, chrome.runtime, undefined, {
  onConversationId(id) {
    const url = new URL(window.location.href);
    if (id) url.searchParams.set("conversation", id);
    else url.searchParams.delete("conversation");
    window.history.replaceState(null, "", url);
  },
  onSaved() { void recentIndex.refresh(); },
  onRequestStarted() { interacted = true; },
  storageChanges: chrome.storage.onChanged
});
const recentIndex = bindRecentConversations(recent, chrome.runtime, view);

void bindThemeSelect(
  requireElement("#theme-select", HTMLSelectElement),
  chrome.storage.local,
  document.documentElement
);

requireElement("#new-conversation", HTMLButtonElement).addEventListener("click", () => {
  if (view.newConversation()) {
    interacted = true;
    recent.value = "";
  }
});
recent.addEventListener("change", () => { interacted = true; });

void (async () => {
  const requested = new URLSearchParams(window.location.search).get("conversation");
  const items = await recentIndex.refresh();
  const id = requested ?? items[0]?.id;
  if (id && !interacted) {
    await view.loadConversation(id);
    recent.value = view.getConversationId() ?? "";
  }
})();
