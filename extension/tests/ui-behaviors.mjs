import assert from "node:assert/strict";
import { bindDrawer } from "../src/fullpage/drawer.js";
import { bindAskForm } from "../src/popup/ask-form.js";
import { bindContextControls } from "../src/popup/context-controls.js";
import { bindSuggestions } from "../src/popup/suggestions.js";
import { bindThemeToggle } from "../src/shared/theme-toggle.js";
import { bindThreadView } from "../src/shared/thread-view.js";

/** Records observers so a test can deliver their mutations explicitly. */
/** @type {{callback: () => void, target: any}[]} */
const observers = [];
globalThis.MutationObserver = /** @type {any} */ (class {
  /** @param {() => void} callback */
  constructor(callback) { this.callback = callback; }
  /** @param {any} target */
  observe(target) { observers.push({ callback: this.callback, target }); }
});
/** @param {any} target */
function mutate(target) {
  for (const observer of observers) if (observer.target === target) observer.callback();
}

/** @type {{activeElement: any, createElement(): Element}} */
const doc = { activeElement: null, createElement: () => new Element() };

class Element {
  constructor() {
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {any[]} */
    this.children = [];
    this.ownerDocument = doc;
    this.textContent = "";
    this.className = "";
    this.value = "";
    this.hidden = false;
    this.disabled = false;
    this.inert = false;
    /** @type {Record<string, string>} */
    this.style = {};
  }
  /** @param {string} type @param {(event: any) => void} listener */
  addEventListener(type, listener) {
    this.listeners.set(type, [...this.listeners.get(type) ?? [], listener]);
  }
  /** @param {string} type @param {any} [event] */
  dispatch(type, event = {}) {
    const full = { preventDefault() { this.defaultPrevented = true; }, defaultPrevented: false, ...event };
    for (const listener of this.listeners.get(type) ?? []) listener(full);
    return full;
  }
  /** @param {any} event */
  dispatchEvent(event) { this.dispatch(event.type, event); return true; }
  click() { if (!this.disabled) this.dispatch("click"); }
  focus() { doc.activeElement = this; }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  /** @param {string} name */
  hasAttribute(name) { return this.attributes.has(name); }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {...any} nodes */
  append(...nodes) {
    for (const node of nodes) {
      if (typeof node === "string") this.textContent += node;
      else this.children.push(node);
    }
  }
  /** @param {...any} nodes */
  replaceChildren(...nodes) { this.children = nodes; this.textContent = ""; }
  /** @param {number} start @param {number} end */
  setSelectionRange(start, end) { this.selection = [start, end]; }
  requestSubmit() { this.dispatch("submit"); }
}

// ---------- Theme switch ----------
{
  const root = new Element();
  const select = new Element();
  select.value = "system";
  const buttons = ["system", "light", "dark"].map((option) => {
    const button = new Element();
    button.setAttribute("data-theme-option", option);
    return button;
  });
  const group = Object.assign(new Element(), { querySelectorAll: () => buttons });
  /** @type {string[]} */
  const changes = [];
  select.addEventListener("change", () => {
    changes.push(select.value);
    // As theme.js does: apply the preference to the root.
    if (select.value === "system") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", select.value);
    mutate(root);
  });

  const pressed = () => buttons.filter((button) => button.getAttribute("aria-pressed") === "true")
    .map((button) => button.getAttribute("data-theme-option"));

  root.setAttribute("data-theme", "dark");
  bindThemeToggle(/** @type {any} */ (group), /** @type {any} */ (select), /** @type {any} */ (root));
  assert.deepEqual(pressed(), ["dark"], "the switch shows the preference already applied");

  buttons[1].click();
  assert.deepEqual(changes, ["light"], "a click goes through the select, the source of truth");
  assert.deepEqual(pressed(), ["light"]);

  // Another surface changes the preference back to the system's.
  root.removeAttribute("data-theme");
  mutate(root);
  assert.deepEqual(pressed(), ["system"]);
}

// ---------- Thread view ----------
{
  const scroller = Object.assign(new Element(), { scrollTop: 0, scrollHeight: 1000, clientHeight: 400 });
  const thread = new Element();
  const input = Object.assign(new Element(), { scrollHeight: 60 });
  const view = bindThreadView(/** @type {any} */ ({ scroller, thread, input, maxInputHeight: 160 }));

  assert.equal(input.style.height, "60px");
  assert.equal(input.style.overflowY, "hidden");
  input.scrollHeight = 400;
  input.dispatch("input");
  assert.equal(input.style.height, "160px", "the composer stops growing at its cap");
  assert.equal(input.style.overflowY, "auto");

  // Streaming while the reader is at the bottom: follow the answer.
  scroller.scrollHeight = 1200;
  mutate(thread);
  assert.equal(scroller.scrollTop, 1200);

  // The reader scrolls up to read: new text doesn't pull them down.
  scroller.scrollTop = 200;
  scroller.dispatch("scroll");
  scroller.scrollHeight = 1400;
  mutate(thread);
  assert.equal(scroller.scrollTop, 200);

  view.reveal();
  assert.equal(scroller.scrollTop, 1400, "reveal jumps to the newest message and follows again");
  scroller.scrollHeight = 1500;
  mutate(thread);
  assert.equal(scroller.scrollTop, 1500);
}

// ---------- Full-view drawer ----------
{
  const app = new Element();
  const main = new Element();
  const scrim = new Element();
  scrim.hidden = true;
  const openButton = new Element();
  const closeButton = new Element();
  const composer = new Element();
  const keys = new Element();
  /** @type {(() => void)[]} */
  const mediaListeners = [];
  const narrow = {
    matches: true,
    /** @param {string} _type @param {() => void} listener */
    addEventListener(_type, listener) { mediaListeners.push(listener); }
  };
  /** @param {boolean} matches */
  const resize = (matches) => {
    narrow.matches = matches;
    for (const listener of mediaListeners) listener();
  };
  const drawer = bindDrawer(/** @type {any} */ ({ app, main, scrim, openButton, closeButton, narrow }), keys);

  openButton.focus();
  openButton.click();
  assert.equal(app.getAttribute("data-sidebar"), "open");
  assert.equal(scrim.hidden, false);
  assert.equal(openButton.getAttribute("aria-expanded"), "true");
  assert.equal(main.inert, true, "the page behind the drawer is inert");
  assert.equal(doc.activeElement, closeButton, "focus moves into the drawer");

  const escape = keys.dispatch("keydown", { key: "Escape" });
  assert.equal(escape.defaultPrevented, true);
  assert.equal(app.getAttribute("data-sidebar"), null);
  assert.equal(scrim.hidden, true);
  assert.equal(main.inert, false);
  assert.equal(openButton.getAttribute("aria-expanded"), "false");
  assert.equal(doc.activeElement, openButton, "closing returns focus to the opener");

  // Escape with the drawer closed is left for the page.
  assert.equal(keys.dispatch("keydown", { key: "Escape" }).defaultPrevented, false);

  // Choosing a conversation sends focus to the composer instead.
  openButton.click();
  drawer.close(/** @type {any} */ (composer));
  assert.equal(doc.activeElement, composer);

  // The scrim closes it too.
  openButton.click();
  scrim.click();
  assert.equal(drawer.isOpen(), false);

  // Widening past the breakpoint while open leaves nothing behind.
  openButton.click();
  closeButton.focus();
  resize(false);
  assert.equal(drawer.isOpen(), false);
  assert.equal(scrim.hidden, true, "no invisible scrim over the desktop layout");
  assert.equal(main.inert, false);
  assert.equal(doc.activeElement, closeButton, "focus isn't sent to the hidden open button");

  // On desktop the sidebar is always shown; the drawer can't open.
  drawer.open();
  assert.equal(drawer.isOpen(), false);
  resize(true);
  drawer.open();
  assert.equal(drawer.isOpen(), true);
}

// ---------- Starter suggestions ----------
{
  /** @type {((value: any) => void)[]} */
  const pendingCaptures = [];
  /** @type {any[]} */
  const requests = [];
  const runtime = {
    /** @param {any} message */
    sendMessage(message) {
      requests.push(message);
      return new Promise((resolve) => pendingCaptures.push(resolve));
    }
  };
  const chips = { none: new Element(), selection: new Element(), page: new Element(), status: new Element(), preview: new Element() };
  const controls = bindContextControls(/** @type {any} */ (chips), runtime);
  const input = new Element();
  const summarize = new Element();
  summarize.setAttribute("data-context", "page");
  summarize.setAttribute("data-prompt", "Summarize this page in a few key points.");
  const explain = new Element();
  explain.setAttribute("data-context", "selection");
  explain.setAttribute("data-prompt", "Explain this in simple terms.");
  let fills = 0;
  bindSuggestions(/** @type {any} */ ([summarize, explain]), {
    input: /** @type {any} */ (input),
    chips: /** @type {any} */ (chips),
    contextControls: controls,
    onFill: () => { fills += 1; }
  });

  summarize.click();
  assert.equal(input.value, "Summarize this page in a few key points.");
  assert.equal(doc.activeElement, input);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].mode, "page");

  // A second suggestion while the page is still being captured would pair
  // its question with the page: it is ignored.
  explain.click();
  assert.equal(requests.length, 1);
  assert.equal(input.value, "Summarize this page in a few key points.");
  assert.equal(fills, 1);

  pendingCaptures[0]({
    ok: true,
    context: { mode: "page", text: "Readable text", truncated: false, page: { title: "Article", url: "https://example.com/" } }
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(controls.getContext()?.mode, "page");
  assert.equal(input.value, "Summarize this page in a few key points.", "question and context still match");

  // Once the capture settles, the next suggestion goes through as a pair.
  explain.click();
  assert.equal(requests.length, 2);
  assert.equal(requests[1].mode, "selection");
  assert.equal(input.value, "Explain this in simple terms.");
}

// ---------- Copy on a finished answer ----------
{
  const conversation = {
    id: "conv_00000000-0000-4000-8000-000000000001",
    messages: [
      { role: "user", text: "Question", status: "complete" },
      { role: "assistant", text: "The answer", status: "complete" }
    ]
  };
  const elements = {
    form: new Element(),
    input: new Element(),
    submit: new Element(),
    status: new Element(),
    answer: new Element(),
    history: new Element()
  };
  const view = bindAskForm(/** @type {any} */ (elements), /** @type {any} */ ({
    connect() { throw new Error("not used"); },
    async sendMessage() { return { ok: true, value: conversation }; }
  }));
  assert.equal(await view.loadConversation(conversation.id), true);
  const [question, reply] = elements.history.children;
  assert.equal(question.children.length, 2, "questions have no copy action");
  const copy = reply.children[2].children[0];
  assert.equal(copy.textContent, "Copy");

  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  /** @param {any} value */
  const setNavigator = (value) => Object.defineProperty(globalThis, "navigator", { value, configurable: true });
  try {
    // No Clipboard API: say so rather than claim a copy.
    setNavigator({});
    copy.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(copy.textContent, "Couldn't copy");
    assert.equal(copy.getAttribute("data-copied"), null);

    /** @type {string[]} */
    const written = [];
    setNavigator({
      clipboard: {
        /** @param {string} text */
        async writeText(text) { written.push(text); }
      }
    });
    copy.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(written, ["The answer"]);
    assert.equal(copy.textContent, "Copied");
    assert.equal(copy.getAttribute("data-copied"), "true");

    setNavigator({ clipboard: { async writeText() { throw new Error("denied"); } } });
    copy.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(copy.textContent, "Couldn't copy");
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "navigator", descriptor);
  }
}

console.log("UI theme switch, thread view, drawer, suggestion, and copy behavior tests passed");
