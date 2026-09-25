import assert from "node:assert/strict";
import { createNativeConnectionManager } from "../src/background/native-connection.js";
import { bindAskForm } from "../src/popup/ask-form.js";
import {
  PERFORMANCE_BUDGETS_MS,
  PERFORMANCE_MARKS,
  recordDuration,
  withinPerformanceBudget
} from "../src/shared/performance.js";
import { MockPort } from "./support/mock-port.mjs";

class Element {
  constructor() {
    this.textContent = "";
    this.hidden = false;
    this.disabled = false;
    this.value = "";
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
  }
  /** @param {string} type @param {(event: any) => void} listener */
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  /** @param {string} type @param {any} event */
  dispatch(type, event) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name); }
  /** @param {...unknown} values */
  append(...values) { this.textContent += values.join(""); }
  focus() {}
}
class Form extends Element {
  requestSubmit() { this.dispatch("submit", { preventDefault() {} }); }
}

globalThis.performance.clearMeasures();

const popupStarted = globalThis.performance.now();
const ports = [];
const elements = {
  form: new Form(),
  input: new Element(),
  submit: new Element(),
  status: new Element(),
  answer: new Element(),
  cancel: new Element(),
  retry: new Element()
};
const view = bindAskForm(
  /** @type {any} */ (elements),
  {
    connect() {
      const port = new MockPort("pervue.ask");
      ports.push(port);
      return port;
    }
  }
);
assert.equal(view.getConversationId(), null);
const popupReady = recordDuration(
  "popup_input_ready",
  popupStarted,
  globalThis.performance.now()
);
assert.ok(withinPerformanceBudget("popup_input_ready", popupReady));

const manager = createNativeConnectionManager({
  connectNative() { return new MockPort(); }
});
const nativeStarted = globalThis.performance.now();
manager.ensurePort();
const nativeElapsed = globalThis.performance.now() - nativeStarted;
assert.ok(withinPerformanceBudget("native_connection", nativeElapsed));

elements.input.value = "Measure first chunk";
elements.form.requestSubmit();
const requestStarted = globalThis.performance.now();
ports[0].emitMessage({
  version: 1,
  type: "event",
  request_id: "req_perf",
  event: "response.delta",
  payload: { text: "first" }
});
const firstChunkElapsed = globalThis.performance.now() - requestStarted;
assert.ok(withinPerformanceBudget("first_response_chunk", firstChunkElapsed));

for (const [metric, mark] of Object.entries(PERFORMANCE_MARKS)) {
  const entries = globalThis.performance.getEntriesByName(mark);
  assert.ok(entries.length > 0, "missing performance measure: " + metric);
}

assert.deepEqual(PERFORMANCE_BUDGETS_MS, {
  popup_input_ready: 100,
  native_connection: 250,
  first_response_chunk: 1500
});

console.log(JSON.stringify({
  popup_input_ready_ms: Number(popupReady.toFixed(3)),
  native_connection_ms: Number(nativeElapsed.toFixed(3)),
  first_response_chunk_ms: Number(firstChunkElapsed.toFixed(3)),
  budgets_ms: PERFORMANCE_BUDGETS_MS
}));
