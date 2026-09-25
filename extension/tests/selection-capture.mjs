import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import {
  CONTEXT_CAPTURE_MESSAGE,
  getActiveTabMetadata,
  handleContextCapture
} from "../src/background/selection-capture.js";
import { bindContextControls } from "../src/popup/context-controls.js";
import { MAX_PAGE_BYTES, MAX_PAGE_TEXT_NODES, MAX_SELECTION_BYTES } from "../src/shared/limits.js";

const source = fs.readFileSync(
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../src/content/content-script.js"),
  "utf8"
);

/** Exercise the same classic script Chrome injects into the top frame. */
function contentScript() {
  /** @type {any} */
  let listener;
  let selection = "";
  let reads = 0;
  /** @type {{nodeValue: string, parentElement: {closest(selector: string): object | null}}[]} */
  let nodes = [];
  /** @type {any} */
  const document = {
    title: "Page",
    activeElement: null,
    body: {},
    querySelector() { return null; },
    createTreeWalker() {
      let index = 0;
      return { nextNode: () => nodes[index++] ?? null };
    }
  };
  const window = {
    location: { href: "https://example.com/" },
    getSelection() {
      reads += 1;
      return { toString: () => selection };
    }
  };
  runInNewContext(source, {
    chrome: { runtime: { onMessage: {
      addListener(/** @type {any} */ fn) { listener = fn; }
    } } },
    document,
    window,
    TextEncoder,
    NodeFilter: { SHOW_TEXT: 4 }
  });
  return {
    document,
    get reads() { return reads; },
    setSelection(/** @type {string} */ value) { selection = value; },
    setNodes(/** @type {{text: string, excluded?: boolean}[]} */ entries) {
      nodes = entries.map(({ text, excluded }) => ({
        nodeValue: text,
        parentElement: { closest: () => excluded ? {} : null }
      }));
    },
    /** @param {string} type */
    request(type) {
      /** @type {any} */
      let response;
      listener({ type }, {}, (/** @type {any} */ value) => { response = value; });
      return response;
    }
  };
}

{
  const page = contentScript();
  assert.equal(page.reads, 0);
  assert.equal(page.request("other"), undefined);
  assert.equal(page.reads, 0, "an unrelated message cannot read selection");
  assert.equal(page.request("pervue.selection.read").reason, "SELECTION_UNAVAILABLE");
  page.setSelection("Quote from page");
  assert.deepEqual(
    { ...page.request("pervue.selection.read") },
    { ok: true, text: "Quote from page", truncated: false }
  );
  page.document.activeElement = {
    tagName: "TEXTAREA", value: "before selected after",
    selectionStart: 7, selectionEnd: 15
  };
  assert.equal(page.request("pervue.selection.read").text, "selected");
  page.document.activeElement = null;
  page.setSelection("x".repeat(MAX_SELECTION_BYTES - 1) + "😀");
  const bounded = page.request("pervue.selection.read");
  assert.equal(bounded.truncated, true);
  assert.equal(new TextEncoder().encode(bounded.text).length, MAX_SELECTION_BYTES - 1);
  assert.equal(bounded.text.endsWith("\ud83d"), false);
}

{
  const page = contentScript();
  page.setNodes([
    { text: "navigation secret", excluded: true },
    { text: "  Main   article  " },
    { text: " Second paragraph " }
  ]);
  assert.equal(page.reads, 0, "page extraction is not a selection read");
  const extracted = page.request("pervue.page.read");
  assert.equal(extracted.text, "Main article\nSecond paragraph");
  assert.equal(extracted.truncated, false);
  assert.equal(extracted.inspected, 3);
  page.setNodes([{ text: "😀".repeat(MAX_PAGE_BYTES / 4) + "x" }]);
  const capped = page.request("pervue.page.read");
  assert.equal(capped.truncated, true);
  assert.equal(new TextEncoder().encode(capped.text).length, MAX_PAGE_BYTES);
  page.setNodes(Array.from({ length: MAX_PAGE_TEXT_NODES + 1 }, () => ({ text: "a" })));
  const nodeCap = page.request("pervue.page.read");
  assert.equal(nodeCap.inspected, MAX_PAGE_TEXT_NODES);
  assert.equal(nodeCap.truncated, true);
  page.setNodes([{ text: "hidden", excluded: true }]);
  assert.equal(page.request("pervue.page.read").reason, "PAGE_EXTRACTION_FAILED");
}

const popupUrl = "chrome-extension://test/src/popup/index.html";
/** @param {any} message @param {{url?: string}} sender @param {any} tabs @returns {Promise<any>} */
function workerRequest(message, sender, tabs) {
  return new Promise((resolve) => {
    const keepOpen = handleContextCapture(message, sender, resolve, tabs, popupUrl);
    if (!keepOpen) {
      resolve(undefined);
    }
  });
}

{
  const tabs = { async query() { return [{ id: 42, title: "Article", url: "https://user:pass@example.com/read?q=secret#section" }]; } };
  const metadata = await getActiveTabMetadata(tabs);
  assert.deepEqual(metadata, {
    ok: true, permission: "granted", tabId: 42,
    page: { title: "Article", url: "https://example.com/read" }
  });
  const internal = await getActiveTabMetadata({ async query() { return [{ id: 1, url: "chrome://settings" }]; } });
  const inaccessible = await getActiveTabMetadata({ async query() { return [{ id: 1 }]; } });
  assert.equal(internal.ok, false);
  assert.equal(inaccessible.ok, false);
  if (internal.ok || inaccessible.ok) throw new Error("metadata should be unavailable");
  assert.equal(internal.error.reason, "PAGE_NOT_SCRIPTABLE");
  assert.equal(inaccessible.error.reason, "PAGE_ACCESS_DENIED");
}

{
  let queries = 0;
  /** @type {any[]} */
  const sent = [];
  const tabs = {
    async query() { queries += 1; return [{ id: 42, title: "Page", url: "https://example.com/path?token=secret" }]; },
    async sendMessage(/** @type {any[]} */ ...args) {
      sent.push(args);
      return { ok: true, text: "Selected", truncated: false };
    }
  };
  const request = { type: CONTEXT_CAPTURE_MESSAGE, mode: "selection", intent: "user_click" };
  assert.equal(await workerRequest({ type: "other" }, { url: popupUrl }, tabs), undefined);
  const denied = await workerRequest(request, { url: "https://example.com/" }, tabs);
  assert.equal(denied.error.reason, "PAGE_ACCESS_DENIED");
  assert.equal(denied.permission, "denied");
  assert.equal((await workerRequest({ ...request, intent: undefined }, { url: popupUrl }, tabs)).error.reason, "PAGE_ACCESS_DENIED");
  assert.equal(queries, 0);
  const captured = await workerRequest(request, { url: popupUrl }, tabs);
  assert.equal(captured.context.text, "Selected");
  assert.equal(captured.context.page.url, "https://example.com/path");
  assert.deepEqual(sent, [[42, { type: "pervue.selection.read" }, { frameId: 0 }]]);
  await workerRequest({ ...request, mode: "page" }, { url: popupUrl }, tabs);
  assert.deepEqual(sent[1], [42, { type: "pervue.page.read" }, { frameId: 0 }]);
}

{
  const request = { type: CONTEXT_CAPTURE_MESSAGE, mode: "page", intent: "user_click" };
  const tab = async () => [{ id: 1, title: "Page", url: "https://example.com" }];
  const unavailable = await workerRequest(request, { url: popupUrl }, {
    query: tab, async sendMessage() { throw new Error("raw browser error"); }
  });
  assert.equal(unavailable.error.reason, "PAGE_ACCESS_DENIED");
  assert.equal(JSON.stringify(unavailable).includes("raw browser error"), false);
  const oversized = await workerRequest(request, { url: popupUrl }, {
    query: tab, async sendMessage() { return { ok: true, text: "😀".repeat(MAX_PAGE_BYTES) }; }
  });
  assert.equal(oversized.error.reason, "CONTEXT_TOO_LARGE");
  const empty = await workerRequest(request, { url: popupUrl }, {
    query: tab, async sendMessage() { return { ok: false, reason: "PAGE_EXTRACTION_FAILED" }; }
  });
  assert.equal(empty.error.reason, "PAGE_EXTRACTION_FAILED");
}

{
  /** @type {Record<string, () => void | Promise<void>>} */
  const clicks = {};
  function button(/** @type {string} */ name) {
    return {
      disabled: false,
      addEventListener(/** @type {string} */ type, /** @type {() => void | Promise<void>} */ handler) {
        assert.equal(type, "click");
        clicks[name] = handler;
      }
    };
  }
  const controls = {
    none: button("none"), selection: button("selection"), page: button("page"),
    status: { textContent: "" }, preview: { hidden: true, textContent: "" }
  };
  let calls = 0;
  /** @type {(value: any) => void} */
  let resolveCapture = (/** @type {any} */ _value) => {};
  const state = bindContextControls(/** @type {any} */ (controls), {
    sendMessage(/** @type {any} */ message) {
      calls += 1;
      assert.equal(message.type, CONTEXT_CAPTURE_MESSAGE);
      assert.equal(message.intent, "user_click");
      return new Promise((resolve) => { resolveCapture = resolve; });
    }
  });
  assert.equal(calls, 0, "opening the popup must never capture context");
  assert.equal(state.getContext(), null);
  const pending = clicks.selection();
  assert.equal(state.isPending(), true);
  await clicks.selection();
  assert.equal(calls, 1, "a second click cannot request more content");
  resolveCapture({
    ok: true, permission: "granted",
    context: {
      mode: "selection", text: "<untrusted>", truncated: false,
      page: { title: "Page", url: "https://example.com" }
    }
  });
  await pending;
  assert.equal(state.getContext().text, "<untrusted>");
  assert.equal(controls.preview.textContent.includes("<untrusted>"), true);
  assert.equal(controls.preview.hidden, false);
  assert.equal(controls.status.textContent.startsWith("Page access granted."), true);
  clicks.none();
  assert.equal(state.getContext(), null);
  assert.equal(controls.preview.hidden, true);
  const stale = clicks.page();
  clicks.none();
  resolveCapture({
    ok: true, context: { mode: "page", text: "stale", page: { title: "X", url: "https://example.com" } }
  });
  await stale;
  assert.equal(state.getContext(), null, "clearing during a read must discard its late reply");
}

console.log("CTX-01/02/03/04 browser-context capture tests passed");
