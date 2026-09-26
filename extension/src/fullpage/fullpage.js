import { bindAskForm } from "../popup/ask-form.js";
import { CONVERSATIONS_KEY } from "../background/conversation-store.js";
import { bindConversationList } from "../shared/conversation-list.js";
import { bindDrawer } from "./drawer.js";
import { bindProviderState } from "../popup/provider-state.js";
import { renderMarkdown } from "../shared/markdown.js";
import { createStreamReveal } from "../shared/stream-reveal.js";
import { bindThemeSelect } from "../shared/theme.js";
import { bindThemeToggle } from "../shared/theme-toggle.js";
import { bindThreadView } from "../shared/thread-view.js";

/** @template {HTMLElement} T @param {string} selector @param {{new (): T}} type @returns {T} */
function requireElement(selector, type) {
  const node = document.querySelector(selector);
  if (!(node instanceof type)) throw new Error(`full-page element is missing: ${selector}`);
  return node;
}

const app = requireElement(".app", HTMLElement);
const title = requireElement("#conversation-title", HTMLElement);
const input = requireElement("#ask-input", HTMLTextAreaElement);
const historyStatus = requireElement("#history-status", HTMLElement);
let interacted = false;
const providerState = bindProviderState(
  requireElement("#provider-state", HTMLElement),
  chrome.runtime,
  {},
  { setupLink: requireElement("#companion-setup", HTMLElement) }
);
const view = bindAskForm({
  form: requireElement("#ask-form", HTMLFormElement),
  input,
  submit: requireElement("#ask-submit", HTMLButtonElement),
  status: requireElement("#status", HTMLElement),
  answer: requireElement("#answer", HTMLElement),
  history: requireElement("#conversation-history", HTMLElement),
  cancel: requireElement("#ask-cancel", HTMLButtonElement),
  retry: requireElement("#ask-retry", HTMLButtonElement)
}, chrome.runtime, undefined, {
  onOutcome: (outcome) => providerState.update(outcome),
  onConversationId(id) {
    const url = new URL(window.location.href);
    if (id) url.searchParams.set("conversation", id);
    else url.searchParams.delete("conversation");
    window.history.replaceState(null, "", url);
    conversations.render();
    showTitle();
  },
  onSaved() { void conversations.refresh(); },
  onRequestStarted() { interacted = true; },
  renderMessage: renderMarkdown,
  renderAnswer: createStreamReveal({
    render: (element, text) => renderMarkdown(element, text, { interactive: false }),
    animate: () => !window.matchMedia("(prefers-reduced-motion: reduce)").matches
  }),
  storageChanges: chrome.storage.onChanged
});
const thread = bindThreadView({
  scroller: requireElement("#thread-scroll", HTMLElement),
  thread: requireElement("#thread", HTMLElement),
  input,
  maxInputHeight: 240
});

/** @param {string} message */
function note(message) {
  historyStatus.textContent = message;
}

const conversations = bindConversationList(
  {
    list: requireElement("#conversation-list", HTMLElement),
    empty: requireElement("#conversation-list-empty", HTMLElement)
  },
  chrome.runtime,
  view,
  {
    deletable: true,
    onOpen() {
      interacted = true;
      note("");
      thread.reveal();
      drawer.close(input);
    },
    onBlocked: () => note("Stop or finish the current answer to switch conversations."),
    onDeleted(_id, wasActive) {
      note("");
      if (wasActive) view.newConversation();
    },
    onError: note,
    onChange: showTitle
  }
);

function showTitle() {
  const active = view.getConversationId();
  const current = conversations.items().find((item) => item.id === active);
  title.textContent = current?.title ?? "New conversation";
  document.title = current ? `${current.title} — Pervue` : "Pervue — Full view";
}

// Another view saved or deleted a conversation: keep the list current.
chrome.storage.onChanged.addListener((/** @type {Record<string, any>} */ changes, /** @type {string} */ area) => {
  if (area === "local" && changes[CONVERSATIONS_KEY]) void conversations.refresh();
});

requireElement("#history-search", HTMLInputElement).addEventListener("input", (event) => {
  if (event.target instanceof HTMLInputElement) conversations.setQuery(event.target.value);
});

const themeSelect = requireElement("#theme-select", HTMLSelectElement);
void bindThemeSelect(
  themeSelect,
  chrome.storage.local,
  document.documentElement,
  chrome.storage.onChanged
);
bindThemeToggle(requireElement("#theme-toggle", HTMLElement), themeSelect, document.documentElement);

const drawer = bindDrawer({
  app,
  main: requireElement(".main", HTMLElement),
  scrim: requireElement("#scrim", HTMLElement),
  openButton: requireElement("#sidebar-open", HTMLButtonElement),
  closeButton: requireElement("#sidebar-close", HTMLButtonElement),
  narrow: window.matchMedia("(max-width: 860px)")
}, document);

document.addEventListener("keydown", (event) => {
  if (event.defaultPrevented) return;
  // "/" jumps to the composer from anywhere that isn't already a text field.
  const target = event.target;
  const typing = target instanceof HTMLElement &&
    (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
  if (event.key === "/" && !typing && !event.metaKey && !event.ctrlKey && !event.altKey) {
    event.preventDefault();
    input.focus();
  }
});

requireElement("#new-conversation", HTMLButtonElement).addEventListener("click", () => {
  if (view.newConversation()) {
    interacted = true;
    drawer.close(input);
  }
});

// The full view opens the conversation it was asked for (from the popup, or
// its own URL on reload), and otherwise starts a new one.
void (async () => {
  const requested = new URLSearchParams(window.location.search).get("conversation");
  await conversations.refresh();
  if (requested && !interacted) {
    await view.loadConversation(requested);
    thread.reveal();
  }
})();
