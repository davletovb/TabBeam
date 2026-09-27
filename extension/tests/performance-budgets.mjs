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

// Budget boundary logic is deterministic; real native/first-chunk budgets are
// enforced by native-roundtrip.mjs against the built host.
for (const [metric, budget] of Object.entries(PERFORMANCE_BUDGETS_MS)) {
  assert.equal(withinPerformanceBudget(/** @type {any} */ (metric), budget), true);
  assert.equal(withinPerformanceBudget(/** @type {any} */ (metric), budget + 0.001), false);
}

// Popup readiness uses the navigation origin (0), not module-evaluation time.
const popupNow = globalThis.performance.now();
recordDuration("popup_input_ready", 0, popupNow);
assert.equal(
  globalThis.performance.getEntriesByName(PERFORMANCE_MARKS.popup_input_ready).at(-1)?.duration,
  popupNow
);

// Native connection timing must not fire at connectNative(); it fires only on
// the first real host message, which is host.ready in production.
const nativePort = new MockPort();
const manager = createNativeConnectionManager({
  requireHandshake: false, // The synthetic port has no host.ready event.
  connectNative() { return nativePort; }
});
manager.ensurePort();
assert.equal(
  globalThis.performance.getEntriesByName(PERFORMANCE_MARKS.native_connection).length,
  0
);
nativePort.emitMessage({
  version: 1,
  type: "event",
  request_id: null,
  event: "host.ready",
  payload: { host_version: "test", protocol_versions: [1] }
});
assert.equal(
  globalThis.performance.getEntriesByName(PERFORMANCE_MARKS.native_connection).length,
  1
);

// First-chunk instrumentation is emitted by the production ask renderer.
/** @type {MockPort[]} */
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
bindAskForm(
  /** @type {any} */ (elements),
  {
    connect() {
      const port = new MockPort("pervue.ask");
      ports.push(port);
      return port;
    }
  }
);
elements.input.value = "Measure first chunk";
elements.form.requestSubmit();
ports[0].emitMessage({
  version: 1,
  type: "event",
  request_id: "req_perf",
  event: "response.delta",
  payload: { text: "first" }
});
assert.equal(
  globalThis.performance.getEntriesByName(PERFORMANCE_MARKS.first_response_chunk).length,
  1
);

console.log("TST-09 performance instrumentation contract passed");
