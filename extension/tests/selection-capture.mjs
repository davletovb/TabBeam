import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import {
  SELECTION_CAPTURE_MESSAGE,
  handleSelectionCapture
} from "../src/background/selection-capture.js";
import { bindSelectionInsert } from "../src/popup/selection-insert.js";
import { MAX_SELECTION_BYTES } from "../src/shared/limits.js";

const source = fs.readFileSync(
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../src/content/content-script.js"),
  "utf8"
);

/** Exercise the actual classic content script as Chrome loads it. */
function contentScript() {
  /** @type {any} */
  let listener;
  let reads = 0;
  let selection = "";
  /** @type {any} */
  const document = { title: "Page", activeElement: null };
  const window = {
    location: { href: "https://example.com/" },
    getSelection() {
      reads += 1;
      return { toString: () => selection };
    }
  };
  runInNewContext(source, {
    chrome: { runtime: { onMessage: { addListener(/** @type {any} */ fn) { listener = fn; } } } },
    document,
    window
  });
  return {
    document,
    get reads() { return reads; },
    /** @param {string} value */
    setSelection(value) { selection = value; },
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
  assert.equal(page.reads, 0, "unknown requests cannot read the page selection");
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
  page.document.activeElement = {
    tagName: "INPUT", type: "password", value: "secret",
    selectionStart: 0, selectionEnd: 6
  };
  page.setSelection("");
  assert.equal(page.request("pervue.selection.read").reason, "SELECTION_UNAVAILABLE");
}

{
  const page = contentScript();
  page.setSelection("é".repeat(MAX_SELECTION_BYTES / 2));
  assert.equal(page.request("pervue.selection.read").truncated, false);
  page.setSelection("x".repeat(MAX_SELECTION_BYTES - 1) + "😀");
  const result = page.request("pervue.selection.read");
  assert.equal(result.truncated, true);
  assert.equal(new TextEncoder().encode(result.text).length, MAX_SELECTION_BYTES - 1);
  assert.equal(result.text.endsWith("\ud83d"), false, "never split a Unicode character");
}

const popupUrl = "chrome-extension://test/src/popup/index.html";
/** @param {any} message @param {{url?: string}} sender @param {any} tabs @returns {Promise<any>} */
async function workerRequest(message, sender, tabs) {
  return await new Promise((resolve) => {
    const keepOpen = handleSelectionCapture(message, sender, resolve, tabs, popupUrl);
    if (!keepOpen) {
      // Unauthorized requests respond synchronously; other message types do not.
      resolve(undefined);
    }
  });
}

{
  let queries = 0;
  /** @type {any[]} */
  const sent = [];
  const tabs = {
    async query() { queries += 1; return [{ id: 42 }]; },
    async sendMessage(/** @type {any[]} */ ...args) {
      sent.push(args);
      return { ok: true, text: "Selected", truncated: false };
    }
  };
  const request = { type: SELECTION_CAPTURE_MESSAGE };
  assert.equal(await workerRequest({ type: "other" }, { url: popupUrl }, tabs), undefined);
  const denied = await workerRequest(request, { url: "https://example.com/" }, tabs);
  assert.equal(denied.error.reason, "PAGE_ACCESS_DENIED");
  assert.equal(queries, 0);
  assert.deepEqual(
    await workerRequest(request, { url: popupUrl }, tabs),
    { ok: true, text: "Selected", truncated: false }
  );
  assert.deepEqual(sent, [[42, { type: "pervue.selection.read" }, { frameId: 0 }]]);
}

{
  const request = { type: SELECTION_CAPTURE_MESSAGE };
  const unavailable = await workerRequest(request, { url: popupUrl }, {
    async query() { return []; },
    async sendMessage() { throw new Error("should not send"); }
  });
  assert.equal(unavailable.error.reason, "PAGE_NOT_SCRIPTABLE");
  const restricted = await workerRequest(request, { url: popupUrl }, {
    async query() { return [{ id: 1 }]; },
    async sendMessage() { throw new Error("raw browser error"); }
  });
  assert.equal(restricted.error.reason, "PAGE_NOT_SCRIPTABLE");
  assert.equal(JSON.stringify(restricted).includes("raw browser error"), false);
  const oversized = await workerRequest(request, { url: popupUrl }, {
    async query() { return [{ id: 1 }]; },
    async sendMessage() { return { ok: true, text: "😀".repeat(MAX_SELECTION_BYTES) }; }
  });
  assert.equal(oversized.error.reason, "CONTEXT_TOO_LARGE");
  const empty = await workerRequest(request, { url: popupUrl }, {
    async query() { return [{ id: 1 }]; },
    async sendMessage() { return { ok: false, reason: "SELECTION_UNAVAILABLE" }; }
  });
  assert.equal(empty.error.reason, "SELECTION_UNAVAILABLE");
}

{
  /** @type {any} */
  let click;
  const button = {
    disabled: false,
    addEventListener(/** @type {string} */ type, /** @type {any} */ listener) {
      assert.equal(type, "click");
      click = listener;
    }
  };
  const input = {
    value: "Explain this",
    selectionStart: 12,
    selectionEnd: 12,
    setRangeText(/** @type {string} */ text, /** @type {number} */ start, /** @type {number} */ end) {
      this.value = this.value.slice(0, start) + text + this.value.slice(end);
    },
    focus() {}
  };
  const status = { textContent: "" };
  let calls = 0;
  /** @type {(value: any) => void} */
  let resolveCapture = (/** @type {any} */ _value) => {};
  const runtime = {
    sendMessage(/** @type {any} */ message) {
      calls += 1;
      assert.deepEqual(message, { type: SELECTION_CAPTURE_MESSAGE });
      return new Promise((resolve) => { resolveCapture = resolve; });
    }
  };
  bindSelectionInsert(/** @type {any} */ (button), /** @type {any} */ (input), /** @type {any} */ (status), runtime);
  assert.equal(calls, 0, "opening the popup must not capture a selection");
  const pending = click();
  await click();
  assert.equal(calls, 1, "double click cannot read twice");
  assert.equal(button.disabled, true);
  resolveCapture({ ok: true, text: "first\nsecond", truncated: false });
  await pending;
  assert.equal(input.value, "Explain this\n\n> first\n> second");
  assert.equal(button.disabled, false);
  assert.equal(status.textContent, "Selected text inserted. Review it before asking.");
  const failed = click();
  resolveCapture({ ok: false, error: { reason: "SELECTION_UNAVAILABLE" } });
  await failed;
  assert.equal(input.value, "Explain this\n\n> first\n> second");
  assert.equal(status.textContent, "Select some text on the page first.");
}

console.log("CTX-01 explicit selected-text capture tests passed");
