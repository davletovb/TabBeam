import assert from "node:assert/strict";
import { createEntryActions, MENU_CONSUME_MESSAGE, MENU_PAGE_ID, MENU_SELECTION_ID } from "../src/background/entry-actions.js";
import { serveAskPort } from "../src/background/ask-bridge.js";
import { createNativeConnectionManager } from "../src/background/native-connection.js";
import { bindAskForm } from "../src/popup/ask-form.js";
import { bindContextControls } from "../src/popup/context-controls.js";
import { preloadMenuContext } from "../src/popup/menu-preload.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MAX_PAGE_BYTES, MAX_SELECTION_BYTES, utf8ByteLength } from "../src/shared/limits.js";
import { MockPort } from "./support/mock-port.mjs";

const popupUrl = "chrome-extension://test/src/popup/index.html";
const webTab = { id: 7, title: "<Article>", url: "https://user:pass@example.com/read?secret=1#part" };

class Element {
  constructor() {
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    this.textContent = "";
    this.hidden = false;
    this.disabled = false;
    this.value = "";
    this.focused = false;
  }
  /** @param {string} event @param {(value: any) => void} listener */
  addEventListener(event, listener) {
    this.listeners.set(event, [...this.listeners.get(event) ?? [], listener]);
  }
  /** @param {string} event */
  fire(event) {
    const value = { preventDefault() {}, key: event };
    for (const listener of this.listeners.get(event) ?? []) listener(value);
  }
  /** @param {string} key @param {string} value */
  setAttribute(key, value) { this.attributes.set(key, String(value)); }
  /** @param {string} key */
  getAttribute(key) { return this.attributes.get(key); }
  /** @param {...unknown} values */
  append(...values) {
    for (const value of values) {
      assert.equal(typeof value, "string", "untrusted content must be text, not markup");
      this.textContent += value;
    }
  }
  focus() { this.focused = true; }
  requestSubmit() { this.fire("submit"); }
}

/**
 * Drive menu click → one-time popup handoff → Ask bridge → native request.
 * @param {{tab?: any, pageReply?: any, popupFails?: boolean}} [options]
 */
function harness({ tab = webTab, pageReply = { ok: true, text: "Readable article", truncated: false }, popupFails = false } = {}) {
  /** @type {any} */
  let activeTab = tab;
  /** @type {any[]} */
  const contentRequests = [];
  /** @type {string[]} */
  const openedTabs = [];
  /** @type {MockPort[]} */
  const nativePorts = [];
  let popupOpens = 0;
  let requestCount = 0;
  let senderUrl = popupUrl;
  let currentTime = 0;
  let nextTimer = 0;
  /** @type {Map<number, {deadline: number, callback: () => void}>} */
  const timers = new Map();
  const tabs = {
    async query() { return [activeTab]; },
    async sendMessage(/** @type {number} */ tabId, /** @type {any} */ message, /** @type {any} */ options) {
      contentRequests.push({ tabId, message, options });
      return pageReply;
    },
    async create(/** @type {{url: string}} */ details) { openedTabs.push(details.url); return { id: 8 }; }
  };
  const entries = createEntryActions({
    tabs,
    action: { async openPopup() { popupOpens += 1; if (popupFails) throw new Error("unsupported"); } },
    popupUrl,
    now: () => currentTime,
    schedule(callback, delay) {
      const id = ++nextTimer;
      timers.set(id, { deadline: currentTime + delay, callback });
      return id;
    },
    cancel(id) { timers.delete(id); }
  });
  const manager = createNativeConnectionManager({
    connectNative() { const port = new MockPort("native"); nativePorts.push(port); return port; },
    reportError() { throw new Error("unexpected callback failure"); }
  });
  const runtime = {
    /** @param {any} message */
    sendMessage(message) { return entries.consume(message, { url: senderUrl }); },
    /** @param {{name: string}} info */
    connect(info) {
      assert.equal(info.name, ASK_PORT_NAME);
      const ui = new MockPort(info.name);
      const worker = new MockPort(info.name, { url: senderUrl });
      serveAskPort(worker, { manager, createRequestId: () => `req_journey_${++requestCount}` });
      const uiPost = ui.postMessage.bind(ui);
      const workerPost = worker.postMessage.bind(worker);
      ui.postMessage = (message) => { uiPost(message); worker.emitMessage(message); };
      worker.postMessage = (message) => { workerPost(message); ui.emitMessage(message); };
      return ui;
    }
  };
  const contextElements = {
    none: new Element(), selection: new Element(), page: new Element(),
    status: new Element(), preview: new Element()
  };
  const controls = bindContextControls(/** @type {any} */ (contextElements), runtime);
  const askElements = {
    form: new Element(), input: new Element(), submit: new Element(),
    status: new Element(), answer: new Element()
  };
  bindAskForm(/** @type {any} */ (askElements), runtime, controls);
  return {
    entries, runtime, controls, contextElements, askElements, contentRequests,
    openedTabs, nativePorts, get popupOpens() { return popupOpens; },
    get liveTimers() { return timers.size; },
    /** @param {number} milliseconds */
    advance(milliseconds) {
      currentTime += milliseconds;
      for (const [id, timer] of timers) {
        if (timer.deadline <= currentTime) {
          timers.delete(id);
          timer.callback();
        }
      }
    },
    /** @param {any} next */ setTab(next) { activeTab = next; },
    /** @param {string} url */ setSender(url) { senderUrl = url; },
    /** @param {string} search */ preload(search = "") { return preloadMenuContext(runtime, controls, search); },
    /** @param {string} question */ ask(question) {
      askElements.input.value = question;
      askElements.form.fire("submit");
    }
  };
}

{
  const app = harness();
  assert.equal(app.askElements.input.focused, true, "keyboard entry focuses the composer");
  await app.entries.onMenuClick({ menuItemId: MENU_SELECTION_ID, selectionText: "<selected>" }, webTab);
  assert.equal(app.popupOpens, 1);
  assert.equal(app.nativePorts.length, 0, "a menu click never creates a conversation");
  assert.deepEqual(app.contentRequests, []); // Chrome provides frame selectionText.
  assert.deepEqual(await app.entries.consume({ type: MENU_CONSUME_MESSAGE, token: null }, { url: "https://other/" }), { available: false });
  await app.preload();
  assert.equal(app.controls.getContext().text, "<selected>");
  assert.equal(app.contextElements.preview.textContent.includes("<selected>"), true);
  assert.equal(app.contextElements.preview.textContent.includes("secret=1"), false);
  assert.equal(app.contextElements.selection.getAttribute("aria-pressed"), "true");
  assert.deepEqual(await app.entries.consume({ type: MENU_CONSUME_MESSAGE, token: null }, { url: popupUrl }), { available: false });
  app.ask("Explain it");
  app.ask("Accidental duplicate");
  assert.equal(app.nativePorts.length, 1);
  assert.equal(app.nativePorts[0].messages.length, 1);
  assert.deepEqual(app.nativePorts[0].messages[0].payload.context, {
    mode: "selection", text: "<selected>", truncated: false,
    page: { title: "<Article>", url: "https://example.com/read" }
  });
  app.nativePorts[0].emitMessage({
    version: 1, type: "event", request_id: "req_journey_1",
    event: "response.delta", payload: { text: "<img src=x onerror=bad()>" }
  });
  assert.equal(app.askElements.answer.textContent, "<img src=x onerror=bad()>");
}

{
  // Production callback logging must never copy an exception's page content.
  const original = console.error;
  /** @type {string[]} */
  const logs = [];
  console.error = (...values) => { logs.push(values.map(String).join(" ")); };
  try {
    const port = new MockPort("native");
    const manager = createNativeConnectionManager({ connectNative: () => port });
    manager.send({ request_id: "req_private", type: "request" }, {
      onEvent() { throw new Error("private page text"); }
    });
    port.emitMessage({ version: 1, type: "event", request_id: "req_private", event: "response.delta", payload: { text: "private page text" } });
    assert.deepEqual(logs, ["Pervue native connection callback failed."]);
  } finally {
    console.error = original;
  }
}

{
  const app = harness();
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  await app.preload();
  assert.deepEqual(app.contentRequests, [
    { tabId: 7, message: { type: "pervue.page.read" }, options: { frameId: 0 } }
  ]);
  assert.equal(app.controls.getContext().text, "Readable article");
  app.ask("Summarize");
  assert.equal(app.nativePorts[0].messages[0].payload.context.mode, "page");
  app.contextElements.none.fire("click");
  assert.equal(app.controls.getContext(), null);
  assert.equal(app.contextElements.preview.hidden, true);
}

{
  /** @type {(value: any) => void} */
  let resolvePage = () => {};
  const pageReply = new Promise((resolve) => { resolvePage = resolve; });
  const app = harness({ pageReply });
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  assert.equal(app.popupOpens, 1, "the popup opens before page extraction completes");
  const loading = app.preload();
  assert.equal(app.controls.isPending(), true);
  app.ask("Too early");
  assert.equal(app.nativePorts.length, 0);
  app.contextElements.none.fire("click");
  resolvePage({ ok: true, text: "late private content", truncated: false });
  await loading;
  assert.equal(app.controls.getContext(), null, "a cancelled handoff cannot attach late text");
  app.ask("No context now");
  assert.equal(app.nativePorts[0].messages[0].payload.context, undefined);
}

{
  // An unclaimed capture expires and releases its timer; a late popup sees
  // no context even when the service worker stays alive.
  const app = harness();
  await app.entries.onMenuClick({ menuItemId: MENU_SELECTION_ID, selectionText: "private text" }, webTab);
  assert.equal(app.liveTimers, 1);
  app.advance(30_000);
  assert.equal(app.liveTimers, 0);
  await app.preload();
  assert.equal(app.controls.getContext(), null);
}

{
  // A popup claiming just before expiry cannot attach a capture that finishes
  // after the deadline; a second claim cannot consume the same entry.
  /** @type {(value: any) => void} */
  let resolvePage = () => {};
  const app = harness({ pageReply: new Promise((resolve) => { resolvePage = resolve; }) });
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  app.advance(29_999);
  const loading = app.preload();
  await Promise.resolve(); // Let the first claim finish its active-tab check.
  assert.deepEqual(
    await app.entries.consume({ type: MENU_CONSUME_MESSAGE, token: null }, { url: popupUrl }),
    { available: false }
  );
  app.advance(2);
  resolvePage({ ok: true, text: "too late", truncated: false });
  await loading;
  assert.equal(app.controls.getContext(), null);
  assert.equal(app.liveTimers, 0);
}

{
  const app = harness();
  await app.preload();
  app.ask("Normal Ask");
  assert.equal(app.nativePorts[0].messages[0].payload.context, undefined);
  assert.deepEqual(app.contentRequests, []); // No context never reads the page.
}

{
  const app = harness({ tab: { id: 9, title: "Settings", url: "chrome://settings" } });
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, { id: 9, title: "Settings", url: "chrome://settings" });
  await app.preload();
  assert.equal(app.controls.getContext(), null);
  assert.ok(app.contextElements.status.textContent.includes("does not support"));
  assert.deepEqual(app.contentRequests, []);
  app.ask("Can I still ask?");
  assert.equal(app.nativePorts[0].messages[0].payload.context, undefined);
}

{
  const app = harness();
  await app.entries.onMenuClick({ menuItemId: MENU_SELECTION_ID, selectionText: "x".repeat(MAX_SELECTION_BYTES - 1) + "😀" }, webTab);
  await app.preload();
  const selection = app.controls.getContext();
  assert.equal(selection.truncated, true);
  assert.equal(utf8ByteLength(selection.text), MAX_SELECTION_BYTES - 1);
  app.ask("Explain the excerpt");
  assert.equal(app.nativePorts[0].messages[0].payload.context.truncated, true);
  const oversized = harness({ pageReply: { ok: true, text: "x".repeat(MAX_PAGE_BYTES + 1) } });
  await oversized.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  await oversized.preload();
  assert.equal(oversized.controls.getContext(), null);
  assert.ok(oversized.contextElements.status.textContent.includes("more context"));
}

{
  const app = harness({ popupFails: true });
  await app.entries.onMenuClick({ menuItemId: MENU_SELECTION_ID, selectionText: "Fallback text" }, webTab);
  assert.equal(app.openedTabs.length, 1);
  assert.equal(app.openedTabs[0].includes("Fallback text"), false);
  const url = new URL(app.openedTabs[0]);
  app.setSender(url.href);
  await app.preload(url.search);
  assert.equal(app.controls.getContext().text, "Fallback text");
}

{
  const app = harness();
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  app.setTab({ id: 10, title: "Other", url: "https://other.example" });
  await app.preload();
  assert.equal(app.controls.getContext(), null, "another tab cannot claim a menu capture");
  app.setTab(webTab);
  await app.preload();
  assert.equal(app.controls.getContext(), null, "switching back cannot reclaim stale content");
}

{
  const app = harness();
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  app.setTab({ id: 7, title: "New page", url: "https://example.com/different" });
  await app.preload();
  assert.equal(app.controls.getContext(), null, "same-tab navigation invalidates the capture");
}

{
  const app = harness();
  await app.entries.onMenuClick({ menuItemId: MENU_PAGE_ID }, webTab);
  app.setTab({ ...webTab, url: "https://user:pass@example.com/read?secret=2#part" });
  await app.preload();
  assert.equal(app.controls.getContext(), null, "query-only navigation invalidates stale content too");
}

console.log("EXT-09/10, SEC-03, TST-07 browser-context journey tests passed");
