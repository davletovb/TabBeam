import { bindAskForm } from "./ask-form.js";
import { bindContextControls } from "./context-controls.js";
import { preloadMenuContext } from "./menu-preload.js";
import { bindProviderState } from "./provider-state.js";
import { bindSuggestions } from "./suggestions.js";
import { conversationToResume, rememberConversation } from "./session.js";
import { bindConversationList } from "../shared/conversation-list.js";
import { renderMarkdown } from "../shared/markdown.js";
import { createStreamReveal } from "../shared/stream-reveal.js";
import { bindThemeSelect } from "../shared/theme.js";
import { bindThemeToggle } from "../shared/theme-toggle.js";
import { bindThreadView } from "../shared/thread-view.js";
import { bindDiagnostics } from "./diagnostics.js";
import { recordDuration } from "../shared/performance.js";

const popupStartedAt = 0;

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
let interacted = false;

// The tab the popup was opened over, for resuming its conversation.
/** @type {Promise<{id?: number, url?: string} | null>} */
const tab = chrome.tabs.query({ active: true, currentWindow: true })
  .then((/** @type {{id?: number, url?: string}[]} */ tabs) => tabs[0] ?? null, () => null);
/** @param {string | null} id */
function remember(id) {
  void tab.then((current) => rememberConversation(chrome.storage.session, current, id, Date.now()));
}
const providerState = bindProviderState(
  requireElement("#provider-state", HTMLElement),
  chrome.runtime,
  {},
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
    onConversationId(id) {
      fullView.disabled = !id;
      remember(id);
      historyList.render();
    },
    onSaved() {
      remember(view.getConversationId());
      void refreshLists();
    },
    onRequestStarted() { interacted = true; },
    renderMessage: renderMarkdown,
    renderAnswer: createStreamReveal({
      render: (element, text) => renderMarkdown(element, text, { interactive: false }),
      animate: () => !window.matchMedia("(prefers-reduced-motion: reduce)").matches
    }),
    storageChanges: chrome.storage.onChanged
  }
);
const input = requireElement("#ask-input", HTMLTextAreaElement);
const thread = bindThreadView({
  scroller: requireElement("#thread-scroll", HTMLElement),
  thread: requireElement("#thread", HTMLElement),
  input,
  maxInputHeight: 160,
  follows: () => {
    const conversation = requireElement("#conversation-history", HTMLElement);
    return conversation.childElementCount > 0 || !requireElement("#answer", HTMLElement).hidden;
  }
});

const themeSelect = requireElement("#theme-select", HTMLSelectElement);
void bindThemeSelect(
  themeSelect,
  chrome.storage.local,
  document.documentElement,
  chrome.storage.onChanged
);
bindThemeToggle(requireElement("#theme-toggle", HTMLElement), themeSelect, document.documentElement);

// The settings menu closes like a menu: Escape, or a click outside it.
const settings = requireElement("#settings", HTMLDetailsElement);
document.addEventListener("click", (event) => {
  if (settings.open && event.target instanceof Node && !settings.contains(event.target)) settings.open = false;
});
settings.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && settings.open) {
    // Escape would otherwise close the whole popup.
    event.preventDefault();
    event.stopPropagation();
    settings.open = false;
    settings.querySelector("summary")?.focus();
  }
});

// ---------- History ----------

const historyView = requireElement("#history-view", HTMLElement);
const historyButton = requireElement("#open-history", HTMLButtonElement);
const historySearch = requireElement("#history-search", HTMLInputElement);
const historyStatus = requireElement("#history-status", HTMLElement);
const threadScroll = requireElement("#thread-scroll", HTMLElement);
const dock = requireElement(".dock", HTMLElement);

/** Shows History in place of the conversation, or goes back to it. @param {boolean} open */
function showHistory(open) {
  historyView.hidden = !open;
  threadScroll.hidden = open;
  dock.hidden = open;
  historyButton.setAttribute("aria-expanded", String(open));
  historyStatus.textContent = "";
  if (open) {
    historySearch.value = "";
    historyList.setQuery("");
    void historyList.refresh();
    historySearch.focus();
  } else {
    input.focus();
  }
}

/** After choosing a conversation, from History or the recent list. */
function opened() {
  interacted = true;
  if (!historyView.hidden) showHistory(false);
  thread.reveal();
  input.focus();
}

const historyList = bindConversationList(
  {
    list: requireElement("#history-list", HTMLElement),
    empty: requireElement("#history-empty", HTMLElement)
  },
  chrome.runtime,
  view,
  {
    deletable: true,
    onOpen: opened,
    onBlocked() {
      historyStatus.textContent = "Stop or finish the current answer to switch conversations.";
    },
    onDeleted(_id, wasActive) {
      historyStatus.textContent = "";
      if (wasActive) view.newConversation();
      void recent.refresh();
    },
    onError(message) {
      historyStatus.textContent = message;
    }
  }
);

const recentSection = requireElement("#recent", HTMLElement);
const recent = bindConversationList(
  { list: requireElement("#recent-list", HTMLElement) },
  chrome.runtime,
  view,
  {
    limit: 3,
    grouped: false,
    onOpen: opened,
    onChange(items) { recentSection.hidden = items.length === 0; }
  }
);

function refreshLists() {
  return Promise.all([recent.refresh(), historyView.hidden ? Promise.resolve([]) : historyList.refresh()])
    .then(([items]) => items);
}

historyButton.addEventListener("click", () => showHistory(historyView.hidden));
requireElement("#close-history", HTMLButtonElement).addEventListener("click", () => showHistory(false));
requireElement("#recent-all", HTMLButtonElement).addEventListener("click", () => showHistory(true));
historySearch.addEventListener("input", () => historyList.setQuery(historySearch.value));
historyView.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    // Escape would otherwise close the whole popup.
    event.preventDefault();
    event.stopPropagation();
    showHistory(false);
  }
});

bindSuggestions(
  Array.from(document.querySelectorAll(".suggestion")).filter((node) => node instanceof HTMLElement),
  {
    input,
    chips: {
      page: requireElement("#context-page", HTMLButtonElement),
      selection: requireElement("#context-selection", HTMLButtonElement)
    },
    contextControls,
    onFill: () => thread.fitInput()
  }
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
    if (!historyView.hidden) showHistory(false);
  }
});

// A new popup starts a new conversation, unless it reopens over the tab and
// page it was just used on (./session.js). A context-menu handoff always
// starts fresh: it brings its own question.
void preloadMenuContext(chrome.runtime, contextControls, window.location.search).then(async (menuOpened) => {
  const items = await refreshLists();
  if (menuOpened || interacted || view.getConversationId()) return;
  const resume = await conversationToResume(chrome.storage.session, await tab, items, Date.now());
  if (resume && !interacted && await view.loadConversation(resume)) thread.reveal();
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
