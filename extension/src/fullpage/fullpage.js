import { bindAskForm } from "../popup/ask-form.js";
import { bindProviderState } from "../popup/provider-state.js";
import { bindRecentConversations } from "../shared/recent-conversations.js";
import { bindThemeSelect } from "../shared/theme.js";
import { bindThemeToggle } from "../shared/theme-toggle.js";
import { bindThreadView } from "../shared/thread-view.js";

/** @template {HTMLElement} T @param {string} selector @param {{new (): T}} type @returns {T} */
function requireElement(selector, type) {
  const node = document.querySelector(selector);
  if (!(node instanceof type)) throw new Error(`full-page element is missing: ${selector}`);
  return node;
}

const SVG_NS = "http://www.w3.org/2000/svg";
const app = requireElement(".app", HTMLElement);
const recent = requireElement("#recent-conversations", HTMLSelectElement);
const list = requireElement("#conversation-list", HTMLElement);
const listEmpty = requireElement("#conversation-list-empty", HTMLElement);
const title = requireElement("#conversation-title", HTMLElement);
const input = requireElement("#ask-input", HTMLTextAreaElement);
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
    renderList();
  },
  onSaved() { void recentIndex.refresh(); },
  onRequestStarted() { interacted = true; },
  storageChanges: chrome.storage.onChanged
});
const recentIndex = bindRecentConversations(recent, chrome.runtime, view);
const thread = bindThreadView({
  scroller: requireElement("#thread-scroll", HTMLElement),
  thread: requireElement("#thread", HTMLElement),
  input,
  maxInputHeight: 240
});

const themeSelect = requireElement("#theme-select", HTMLSelectElement);
void bindThemeSelect(
  themeSelect,
  chrome.storage.local,
  document.documentElement,
  chrome.storage.onChanged
);
bindThemeToggle(requireElement("#theme-toggle", HTMLElement), themeSelect, document.documentElement);

/**
 * The sidebar list is a view of the recent-conversations select, which
 * ../shared/recent-conversations.js keeps: choosing an item chooses its
 * option, so both share one loading path.
 */
function renderList() {
  const active = view.getConversationId();
  const items = Array.from(recent.options).filter((option) => option.value !== "");
  list.replaceChildren(...items.map((option) => {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.title = option.text;
    if (option.value === active) button.setAttribute("aria-current", "true");
    const label = document.createElement("span");
    // Titles come from what the person asked: text only, never markup.
    label.textContent = option.text;
    button.append(icon("chat"), label);
    button.addEventListener("click", () => {
      if (option.value !== view.getConversationId()) {
        recent.value = option.value;
        recent.dispatchEvent(new Event("change"));
      }
      setSidebar(false);
    });
    item.append(button);
    return item;
  }));
  listEmpty.hidden = items.length > 0;
  const current = items.find((option) => option.value === active);
  title.textContent = current?.text ?? "New conversation";
  document.title = current ? `${current.text} — Pervue` : "Pervue — Full view";
}

/** @param {string} name */
function icon(name) {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", "icon icon-sm");
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(SVG_NS, "use");
  use.setAttribute("href", `../shared/icons.svg#${name}`);
  svg.append(use);
  return svg;
}

new MutationObserver(renderList).observe(recent, { childList: true });

/** @param {boolean} open */
function setSidebar(open) {
  if (open) app.setAttribute("data-sidebar", "open");
  else app.removeAttribute("data-sidebar");
  requireElement("#scrim", HTMLElement).hidden = !open;
  requireElement("#sidebar-open", HTMLButtonElement).setAttribute("aria-expanded", String(open));
}

requireElement("#sidebar-open", HTMLButtonElement).addEventListener("click", () => setSidebar(true));
requireElement("#sidebar-close", HTMLButtonElement).addEventListener("click", () => setSidebar(false));
requireElement("#scrim", HTMLElement).addEventListener("click", () => setSidebar(false));

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && app.hasAttribute("data-sidebar")) {
    setSidebar(false);
    return;
  }
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
    recent.value = "";
    renderList();
    setSidebar(false);
  }
});
recent.addEventListener("change", () => {
  interacted = true;
  thread.reveal();
});

void (async () => {
  const requested = new URLSearchParams(window.location.search).get("conversation");
  const items = await recentIndex.refresh();
  const id = requested ?? items[0]?.id;
  if (id && !interacted) {
    await view.loadConversation(id);
    recent.value = view.getConversationId() ?? "";
    renderList();
    thread.reveal();
  }
})();
