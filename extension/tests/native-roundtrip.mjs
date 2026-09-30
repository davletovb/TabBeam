import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { serveConversationAskPort } from "../src/background/conversation-bridge.js";
import { createConversationStore } from "../src/background/conversation-store.js";
import { createNativeConnectionManager } from "../src/background/native-connection.js";
import { checkProviderStatus } from "../src/background/status-bridge.js";
import { bindAskForm } from "../src/popup/ask-form.js";
import { bindProviderState } from "../src/popup/provider-state.js";
import { ASK_PORT_NAME, QUESTION_TOO_LONG } from "../src/shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES } from "../src/shared/limits.js";
import {
  PERFORMANCE_BUDGETS_MS,
  PERFORMANCE_MARKS,
  withinPerformanceBudget
} from "../src/shared/performance.js";
import { MockEvent } from "./support/mock-port.mjs";

// This test uses the built tabbeam-host, not a mock of its JSON router or
// Native Messaging framing. Browser-only ports and DOM elements are in memory.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const hostPath = process.argv[2] ?? process.env.SEATLINE_NATIVE_TEST_BRIDGE ?? path.join(root, "native/target/debug/tabbeam-host");
const origin = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/";
const littleEndian = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
// An empty provider search path: whatever this machine has installed, the
// host finds no Codex.
const noProviders = fs.mkdtempSync(path.join(os.tmpdir(), "tabbeam-roundtrip-"));

/** @param {Uint8Array} bytes */
function frame(bytes) {
  const result = new Uint8Array(4 + bytes.length);
  new DataView(result.buffer).setUint32(0, bytes.length, littleEndian);
  result.set(bytes, 4);
  return result;
}

/** A Chrome-like Native Messaging port connected to the actual host process. */
class HostPort {
  constructor() {
    this.child = spawn(hostPath, [origin], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, TABBEAM_PROVIDER_PATH: noProviders }
    });
    this.onMessage = new MockEvent();
    this.onDisconnect = new MockEvent();
    this.messagesSent = 0;
    this.disconnected = false;
    /** @type {Uint8Array} */
    this.pending = new Uint8Array(0);
    /** @type {Error | null} */
    this.error = null;
    /** @type {string[]} */
    this.stderr = [];

    this.child.stdout.on("data", (/** @type {Uint8Array} */ chunk) => {
      const pending = new Uint8Array(this.pending.length + chunk.length);
      pending.set(this.pending);
      pending.set(chunk, this.pending.length);
      this.pending = pending;
      while (this.pending.length >= 4) {
        const length = new DataView(
          this.pending.buffer,
          this.pending.byteOffset,
          this.pending.byteLength
        ).getUint32(0, littleEndian);
        assert.ok(length <= MAX_NATIVE_MESSAGE_BYTES, "host output exceeds frame cap");
        if (this.pending.length < 4 + length) {
          break;
        }
        const message = JSON.parse(decoder.decode(this.pending.subarray(4, 4 + length)));
        this.pending = this.pending.slice(4 + length);
        this.onMessage.emit(message);
      }
    });
    this.child.stderr.on("data", (/** @type {Uint8Array} */ chunk) => {
      this.stderr.push(decoder.decode(chunk));
    });
    this.child.stdin.on("error", (/** @type {Error} */ error) => {
      // A host that rejects a frame may close stdin while a write is pending.
      // Keep EPIPE as a controlled test failure; the exit handler below can
      // replace it with the host's exit code and stderr diagnostics.
      this.error ??= new Error(`native host stdin write failed: ${error.message}`);
    });
    this.child.on("error", (/** @type {Error} */ error) => {
      this.error = error;
      this.close();
    });
    this.child.on("exit", (/** @type {number | null} */ code) => {
      if (!this.disconnected) {
        this.error = new Error(
          `native host exited ${code}: ${this.stderr.join("")}`
        );
        this.close();
      }
    });
  }

  /** @param {any} message */
  postMessage(message) {
    if (this.disconnected) {
      throw this.error ?? new Error("native host disconnected");
    }
    this.messagesSent += 1;
    this.child.stdin.write(frame(encoder.encode(JSON.stringify(message))));
  }

  close() {
    if (this.disconnected) {
      return;
    }
    this.disconnected = true;
    this.onDisconnect.emit(undefined);
  }

  disconnect() {
    this.close();
    this.child.stdin.end();
    this.child.kill();
  }
}

/** Two ends of a chrome.runtime.connect port, each with its own listeners. */
function pairedPorts() {
  /** @param {string} name */
  function side(name) {
    return {
      name,
      onMessage: new MockEvent(),
      onDisconnect: new MockEvent(),
      disconnected: false,
      /** @type {(message: any) => void} */
      postMessage: () => {},
      disconnect() {}
    };
  }
  const ui = side(ASK_PORT_NAME);
  const worker = side(ASK_PORT_NAME);
  for (const [local, remote] of [[ui, worker], [worker, ui]]) {
    local.postMessage = (message) => {
      if (local.disconnected || remote.disconnected) {
        throw new Error("port disconnected");
      }
      remote.onMessage.emit(message);
    };
    local.disconnect = () => {
      if (local.disconnected) {
        return;
      }
      local.disconnected = true;
      remote.disconnected = true;
      remote.onDisconnect.emit(undefined);
    };
  }
  return { ui, worker };
}

class Element {
  constructor() {
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    this.textContent = "";
    this.hidden = false;
  }

  /** @param {string} type @param {(event: any) => void} listener */
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  /** @param {string} type @param {any} event */
  dispatch(type, event) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }

  /** @param {string} name @param {string} value */
  setAttribute(name, value) {
    this.attributes.set(name, value);
  }

  /** @param {string} name */
  getAttribute(name) {
    return this.attributes.get(name);
  }

  /** @param {string} name */
  removeAttribute(name) {
    this.attributes.delete(name);
  }

  /** @param {...string} nodes */
  append(...nodes) {
    this.textContent += nodes.join("");
  }

  focus() {}
}

class Form extends Element {
  requestSubmit() {
    this.dispatch("submit", { preventDefault() {} });
  }
}

/** @param {string} text */
function question(text) {
  return {
    version: 1,
    type: "request",
    request_id: "req_roundtrip_3",
    method: "conversation.send",
    payload: { provider_id: "fake", input: { text } }
  };
}

/** @type {HostPort | undefined} */
let native;
let nextId = 0;
let providerId = "fake";
let searchNext = false;
/** @type {Record<string, any>} */
const savedConversations = {};
const store = createConversationStore({
  async get(/** @type {string} */ key) { return { [key]: savedConversations[key] }; },
  async set(/** @type {Record<string, any>} */ values) { Object.assign(savedConversations, values); },
  async remove(/** @type {string} */ key) { delete savedConversations[key]; }
});
const inFlight = new Set();
const manager = createNativeConnectionManager({
  connectNative(name) {
    assert.equal(name, "com.seatline.host");
    assert.equal(native, undefined, "the host should be reused");
    native = new HostPort();
    return native;
  },
  getLastError: () => null
});
/** @type {any[]} */
const lifecycle = [];
manager.onLifecycleEvent((event) => lifecycle.push(event));

/** @type {any[]} */
const events = [];
/** @type {{event: string, status: string, answer: string, busy: string | undefined}[]} */
const snapshots = [];
const elements = {
  form: new Form(),
  input: Object.assign(new Element(), { value: "" }),
  submit: new Element(),
  status: new Element(),
  answer: new Element()
};
elements.answer.hidden = true;
bindAskForm(/** @type {any} */ (elements), {
  connect({ name }) {
    assert.equal(name, ASK_PORT_NAME);
    const { ui, worker } = pairedPorts();
    serveConversationAskPort(/** @type {any} */ (worker), {
      manager,
      store,
      inFlight,
      providerId,
      createRequestId: () => `req_roundtrip_${++nextId}`
    });
    // The popup registers its renderer after connect() returns. Place the
    // observer immediately after it, including for synchronous local errors.
    const addListener = ui.onMessage.addListener.bind(ui.onMessage);
    ui.onMessage.addListener = (listener) => {
      addListener(listener);
      addListener((event) => {
        events.push(event);
        snapshots.push({
          event: event.event,
          status: elements.status.textContent,
          answer: elements.answer.textContent,
          busy: elements.answer.getAttribute("aria-busy")
        });
      });
    };
    return /** @type {any} */ (ui);
  }
}, undefined, { getSearch: () => searchNext });

/** @param {() => boolean} condition */
async function until(condition) {
  const deadline = Date.now() + 10000;
  while (!condition()) {
    if (native?.error) {
      throw native.error;
    }
    assert.ok(Date.now() < deadline, "timed out waiting for the host");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

/** @param {string} text */
function ask(text) {
  elements.input.value = text;
  elements.form.requestSubmit();
}

try {
  globalThis.performance.clearMeasures();
  ask("What is TabBeam?");
  await until(() => events.at(-1)?.event === "response.completed");
  assert.deepEqual(
    lifecycle.map((event) => event.event),
    ["host.ready"]
  );
  assert.deepEqual(
    events.map((event) => event.event),
    ["conversation.created", "response.started", "response.delta", "response.completed"]
  );
  assert.ok(events.every((event) => event.request_id === "req_roundtrip_1"));
  assert.equal(snapshots[1].status, "Answering…");
  assert.equal(snapshots[1].answer, "");
  assert.equal(snapshots[2].answer, "Fake provider response.");
  assert.equal(snapshots[2].busy, "true");
  assert.equal(elements.status.getAttribute("data-state"), "done");
  assert.equal(elements.answer.textContent, "Fake provider response.");
  assert.equal(manager.pendingRequestCount, 0);

  const nativeReady = globalThis.performance
    .getEntriesByName(PERFORMANCE_MARKS.native_connection).at(-1)?.duration;
  const firstChunk = globalThis.performance
    .getEntriesByName(PERFORMANCE_MARKS.first_response_chunk).at(-1)?.duration;
  if (typeof nativeReady !== "number" || typeof firstChunk !== "number") {
    throw new Error("production performance measures were not recorded");
  }
  assert.ok(
    withinPerformanceBudget("native_connection", nativeReady),
    "built host native readiness " + nativeReady.toFixed(1) + " ms exceeded " +
      PERFORMANCE_BUDGETS_MS.native_connection + " ms"
  );
  assert.ok(
    withinPerformanceBudget("first_response_chunk", firstChunk),
    "built host first chunk " + firstChunk.toFixed(1) + " ms exceeded " +
      PERFORMANCE_BUDGETS_MS.first_response_chunk + " ms"
  );
  console.log(JSON.stringify({
    native_connection_ms: Number(nativeReady.toFixed(3)),
    first_response_chunk_ms: Number(firstChunk.toFixed(3)),
    budgets_ms: {
      native_connection: PERFORMANCE_BUDGETS_MS.native_connection,
      first_response_chunk: PERFORMANCE_BUDGETS_MS.first_response_chunk
    }
  }));

  events.length = 0;
  // Selecting a missing provider produces a real host failure, which the
  // bridge forwards to the same popup renderer.
  providerId = "missing";
  ask("What failed?");
  await until(() => events.at(-1)?.event === "response.failed");
  assert.equal(events[0].event, "response.failed");
  assert.equal(events[0].request_id, "req_roundtrip_2");
  assert.equal(events[0].payload.error.code, "PROVIDER_NOT_FOUND");
  assert.equal(elements.status.getAttribute("data-state"), "failed");
  assert.equal(manager.pendingRequestCount, 0);
  providerId = "fake";

  // SEC-01: a serialized request at exactly 1 MiB passes. One byte above
  // fails in the extension before postMessage touches the native port.
  const base = encoder.encode(JSON.stringify(question(""))).length;
  const exact = "x".repeat(MAX_NATIVE_MESSAGE_BYTES - base);
  assert.equal(encoder.encode(JSON.stringify(question(exact))).length, MAX_NATIVE_MESSAGE_BYTES);
  events.length = 0;
  ask(exact);
  await until(() => events.at(-1)?.event === "response.completed");
  assert.equal(events.at(-1).request_id, "req_roundtrip_3");
  if (!native) {
    throw new Error("native host did not start");
  }
  const count = native.messagesSent;

  events.length = 0;
  ask(exact + "x");
  await until(() => events.at(-1)?.event === "response.failed");
  assert.equal(events[0].request_id, "req_roundtrip_4");
  assert.deepEqual(events[0].payload.error, QUESTION_TOO_LONG);
  assert.equal(elements.status.getAttribute("data-state"), "failed");
  assert.equal(native.messagesSent, count);

  events.length = 0;
  ask("Host still alive?");
  await until(() => events.at(-1)?.event === "response.completed");
  assert.equal(events.at(-1).request_id, "req_roundtrip_5");
  assert.equal(native.messagesSent, count + 1);
  assert.equal(elements.status.getAttribute("data-state"), "done");

  // TST-14: a web search the provider can't do fails with the host's own
  // explanation, and the host keeps answering afterwards.
  events.length = 0;
  searchNext = true;
  ask("Search the web for this");
  await until(() => events.at(-1)?.event === "response.failed");
  searchNext = false;
  assert.equal(events.at(-1).request_id, "req_roundtrip_6");
  assert.equal(events.at(-1).payload.error.code, "SEARCH_FAILED");
  assert.equal(events.at(-1).payload.error.reason, "NATIVE_SEARCH_UNSUPPORTED");
  assert.equal(elements.status.getAttribute("data-kind"), "search-failed");
  assert.equal(elements.status.textContent, "The selected AI provider does not support native web search.");
  assert.ok(events.every((event) => event.event !== "response.source" && event.event !== "response.delta"));
  events.length = 0;
  ask("Still answering?");
  await until(() => events.at(-1)?.event === "response.completed");
  assert.equal(events.at(-1).request_id, "req_roundtrip_7");

  // EXT-04: provider state through the same host. The fake scaffold is
  // ready, Codex isn't installed here, and an unknown provider fails.
  const ready = /** @type {any} */ (await checkProviderStatus({ manager, providerId: "fake" }));
  assert.equal(ready.status.availability, "available");
  assert.equal(ready.status.authentication, "authenticated");
  const unknown = /** @type {any} */ (await checkProviderStatus({ manager, providerId: "missing" }));
  assert.equal(unknown.error.code, "PROVIDER_NOT_FOUND");

  const line = Object.assign(new Element(), { hidden: true });
  bindProviderState(/** @type {any} */ (line), {
    sendMessage: () => checkProviderStatus({ manager })
  });
  assert.equal(line.textContent, "Checking Codex…");
  await until(() => line.getAttribute("data-state") !== "checking");
  assert.equal(line.getAttribute("data-state"), "attention");
  assert.equal(line.getAttribute("data-kind"), "provider-missing");
  assert.equal(line.textContent, "Codex isn't installed. Install it, then try again.");
  assert.equal(manager.pendingRequestCount, 0);
  console.log("Extension → native host → popup round trip passed");
} finally {
  native?.disconnect();
  fs.rmSync(noProviders, { recursive: true, force: true });
}
