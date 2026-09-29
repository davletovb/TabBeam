import assert from "node:assert/strict";
import fs from "node:fs";
import { HOST_START_FAILED } from "../src/background/ask-bridge.js";
import { createNativeConnectionManager } from "../src/background/native-connection.js";
import {
  CAPABILITY_KEYS,
  HOST_OUT_OF_DATE,
  STATUS_TIMED_OUT,
  checkProviderStatus
} from "../src/background/status-bridge.js";
import { STATUS_WAIT_MS, bindProviderState, providerView } from "../src/popup/provider-state.js";
import {
  PROVIDER_STATUS_MESSAGE,
  PROVIDER_STATUS_TIMEOUT_MS
} from "../src/shared/provider-status.js";
import { DEFAULT_PROVIDER_ID } from "../src/shared/providers.js";
import { MockPort } from "./support/mock-port.mjs";

/** DOC-02's machine-readable fixtures. */
const DOC_02 = JSON.parse(
  fs.readFileSync(new URL("../../docs/protocol/fixtures/v1-errors-capabilities.json", import.meta.url), "utf8")
);

const READY = DOC_02.provider_statuses[0].status;

/** A worker's native connection, with the native port it opens. */
function worker({ connectFails = false } = {}) {
  /** @type {MockPort[]} */
  const ports = [];
  /** @type {string | null} */
  let lastError = null;
  const manager = createNativeConnectionManager({
    requireHandshake: false, // Synthetic host responses are injected directly.
    connectNative() {
      if (connectFails) {
        throw new Error("mock connectNative failure");
      }
      const port = new MockPort("native");
      ports.push(port);
      return port;
    },
    getLastError: () => lastError,
    reportError() {}
  });
  let next = 0;
  return {
    manager,
    ports,
    /**
     * @param {{
     *   providerId?: string,
     *   timers?: {schedule?: (callback: () => void, ms: number) => any, cancel?: (timer: any) => void}
     * }} [options]
     */
    check(options = {}) {
      return checkProviderStatus({
        manager,
        createRequestId: () => `req_status_${(next += 1)}`,
        ...options
      });
    },
    /** @param {string | null} message */
    disconnect(message) {
      lastError = message;
      ports[0].emitDisconnect();
    }
  };
}

/**
 * @param {string} requestId
 * @param {string} event
 * @param {any} [payload]
 */
function hostEvent(requestId, event, payload = {}) {
  return { version: 1, type: "event", request_id: requestId, event, payload };
}

// The status bridge (service worker).

{
  // The worker asks the host about the default provider, and passes on its
  // normalized status alone.
  const { ports, check } = worker();
  const answer = check();
  assert.deepEqual(ports[0].messages, [
    {
      request_id: "req_status_1",
      version: 1,
      type: "request",
      method: "provider.status",
      payload: { provider_id: DEFAULT_PROVIDER_ID }
    }
  ]);
  ports[0].emitMessage(
    hostEvent("req_status_1", "provider.status", {
      provider_id: "codex",
      status: { ...READY, extra: "dropped", capabilities: { ...READY.capabilities, future: true } }
    })
  );
  ports[0].emitMessage(hostEvent("req_status_1", "response.completed"));
  assert.deepEqual(await answer, { provider_id: "codex", status: READY });
}

{
  // Every DOC-02 status fixture passes through unchanged.
  for (const fixture of DOC_02.provider_statuses) {
    const { ports, check } = worker();
    const answer = check({ providerId: fixture.provider_id });
    ports[0].emitMessage(hostEvent("req_status_1", "provider.status", fixture));
    ports[0].emitMessage(hostEvent("req_status_1", "response.completed"));
    assert.deepEqual(await answer, fixture);
  }
}

{
  // A status that doesn't follow DOC-02, or none at all, means the host
  // speaks another version of the protocol.
  const broken = [
    { ...READY, availability: "maybe" },
    { ...READY, authentication: "yes" },
    { ...READY, capabilities: null },
    { ...READY, capabilities: { ...READY.capabilities, streaming: "sometimes" } },
    {
      ...READY,
      capabilities: Object.fromEntries(CAPABILITY_KEYS.slice(1).map((key) => [key, true]))
    },
    undefined
  ];
  for (const status of broken) {
    const { ports, check } = worker();
    const answer = check();
    ports[0].emitMessage(hostEvent("req_status_1", "provider.status", { provider_id: "codex", status }));
    ports[0].emitMessage(hostEvent("req_status_1", "response.completed"));
    assert.deepEqual(await answer, { provider_id: "codex", error: HOST_OUT_OF_DATE }, `${JSON.stringify(status)}`);
  }

  // Another provider's status doesn't count.
  const { ports, check } = worker();
  const answer = check();
  ports[0].emitMessage(hostEvent("req_status_1", "provider.status", { provider_id: "other", status: READY }));
  ports[0].emitMessage(hostEvent("req_status_1", "response.completed"));
  assert.deepEqual(await answer, { provider_id: "codex", error: HOST_OUT_OF_DATE });
}

{
  // The host's own failure is passed on, normalized fields only; a malformed
  // one means an out-of-date host.
  const failure = {
    code: "PROVIDER_NOT_FOUND",
    reason: "PROVIDER_NOT_INSTALLED",
    message: "TabBeam's companion app doesn't support this AI provider yet.",
    retryable: false
  };
  let { ports, check } = worker();
  let answer = check();
  ports[0].emitMessage(hostEvent("req_status_1", "response.failed", { error: { ...failure, metadata: {} } }));
  assert.deepEqual(await answer, { provider_id: "codex", error: failure });

  ({ ports, check } = worker());
  answer = check();
  ports[0].emitMessage(hostEvent("req_status_1", "response.failed", { error: { code: 7 } }));
  assert.deepEqual(await answer, { provider_id: "codex", error: HOST_OUT_OF_DATE });

  // A code outside DOC-02's frozen vocabulary, however well formed the rest,
  // comes from a host speaking another version of the protocol.
  ({ ports, check } = worker());
  answer = check();
  ports[0].emitMessage(
    hostEvent("req_status_1", "response.failed", {
      error: { code: "SOME_FUTURE_ERROR", reason: "NEW_REASON", message: "...", retryable: false }
    })
  );
  assert.deepEqual(await answer, { provider_id: "codex", error: HOST_OUT_OF_DATE });

  // Every DOC-02 error passes through, without its metadata.
  for (const error of DOC_02.valid_errors) {
    ({ ports, check } = worker());
    answer = check();
    ports[0].emitMessage(hostEvent("req_status_1", "response.failed", { error }));
    const { code, reason, message, retryable } = error;
    assert.deepEqual(
      await answer,
      { provider_id: "codex", error: { code, reason, message, retryable } },
      code
    );
  }
}

{
  // A host that never ends the request: the worker answers when its time is
  // up, stops routing the request, and keeps the connection for the next one.
  /** @type {{callback: () => void, ms: number, cancelled: boolean}[]} */
  const timers = [];
  const fakeTimers = {
    /** @param {() => void} callback @param {number} ms */
    schedule(callback, ms) {
      const timer = { callback, ms, cancelled: false };
      timers.push(timer);
      return timer;
    },
    /** @param {{cancelled: boolean}} timer */
    cancel(timer) {
      timer.cancelled = true;
    }
  };
  const { manager, ports, check } = worker();
  const answer = check({ timers: fakeTimers });
  ports[0].emitMessage(hostEvent("req_status_1", "provider.status", { provider_id: "codex", status: READY }));
  assert.equal(timers.length, 1);
  assert.equal(timers[0].ms, PROVIDER_STATUS_TIMEOUT_MS);
  assert.equal(manager.pendingRequestCount, 1);
  timers[0].callback();
  assert.deepEqual(await answer, { provider_id: "codex", error: STATUS_TIMED_OUT });
  assert.equal(manager.pendingRequestCount, 0);
  // Its late end changes nothing.
  ports[0].emitMessage(hostEvent("req_status_1", "response.completed"));
  assert.equal(manager.pendingRequestCount, 0);

  // The next check uses the same connection, and its answer stops its timer.
  const next = check({ timers: fakeTimers });
  assert.equal(ports.length, 1);
  ports[0].emitMessage(hostEvent("req_status_2", "provider.status", { provider_id: "codex", status: READY }));
  ports[0].emitMessage(hostEvent("req_status_2", "response.completed"));
  assert.deepEqual(await next, { provider_id: "codex", status: READY });
  assert.equal(timers.length, 2);
  assert.equal(timers[1].cancelled, true);

  // A connection that can't open answers at once, with no timer.
  const failed = await checkProviderStatus({
    manager: createNativeConnectionManager({
      requireHandshake: false,
      connectNative() {
        throw new Error("mock connectNative failure");
      },
      reportError() {}
    }),
    timers: fakeTimers
  });
  assert.deepEqual(failed, { provider_id: "codex", error: HOST_START_FAILED });
  assert.equal(timers.length, 2);

  // The popup waits longer than the worker, so the worker's answer is shown.
  assert.ok(STATUS_WAIT_MS > PROVIDER_STATUS_TIMEOUT_MS);
}

{
  // A native port that closes says why, in the DOC-02 vocabulary.
  let { check, disconnect } = worker();
  let answer = check();
  disconnect("Specified native messaging host not found.");
  let result = /** @type {any} */ (await answer);
  assert.equal(result.error.code, "HOST_NOT_INSTALLED");
  assert.equal(result.error.reason, "NATIVE_HOST_NOT_FOUND");

  ({ check, disconnect } = worker());
  answer = check();
  disconnect(null);
  result = await answer;
  assert.equal(result.error.code, "HOST_UNAVAILABLE");
  assert.equal(result.error.reason, "HOST_DISCONNECTED");

  // A native port that can't open at all.
  result = await worker({ connectFails: true }).check();
  assert.deepEqual(result, { provider_id: "codex", error: HOST_START_FAILED });
}

// The provider line (popup).

/** The subset of an HTML element the provider line touches. */
class FakeElement {
  constructor() {
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    this.textContent = "";
    this.hidden = true;
  }

  /** @param {string} name @param {string} value */
  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  /** @param {string} name */
  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }

  /** @param {string} name */
  removeAttribute(name) {
    this.attributes.delete(name);
  }
}

/**
 * Opens the provider line with a worker that answers `reply`.
 * @param {() => Promise<any>} reply
 */
function openLine(reply) {
  const element = new FakeElement();
  /** @type {any[]} */
  const sent = [];
  /** @type {{callback: () => void, ms: number, cancelled: boolean}[]} */
  const timers = [];
  const line = bindProviderState(
    /** @type {any} */ (element),
    {
      sendMessage(message) {
        sent.push(message);
        return reply();
      }
    },
    {
      schedule(callback, ms) {
        const timer = { callback, ms, cancelled: false };
        timers.push(timer);
        return timer;
      },
      cancel(timer) {
        timer.cancelled = true;
      }
    }
  );
  return {
    line,
    element,
    sent,
    timers,
    get view() {
      return {
        state: element.getAttribute("data-state"),
        kind: element.getAttribute("data-kind"),
        message: element.textContent
      };
    }
  };
}

/** Lets pending promise callbacks run. */
function settle() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

{
  // The line shows at once that it's checking, asks the worker, and then
  // shows the answer; the wait is bounded.
  /** @type {(value: any) => void} */
  let answer = () => {};
  const popup = openLine(() => new Promise((resolve) => (answer = resolve)));
  assert.deepEqual(popup.sent, [{ type: PROVIDER_STATUS_MESSAGE }]);
  assert.equal(popup.element.hidden, false);
  assert.deepEqual(popup.view, { state: "checking", kind: null, message: "Checking Codex…" });
  assert.equal(popup.timers[0].ms, STATUS_WAIT_MS);

  answer({ provider_id: "codex", status: READY });
  await settle();
  assert.deepEqual(popup.view, { state: "ready", kind: null, message: "Codex is ready." });
  assert.equal(popup.timers[0].cancelled, true);
}

{
  // Each status says what to do next, in the user's terms.
  /** @type {[any, {state: string, kind: string | null, message: string}][]} */
  const cases = [
    [
      { availability: "not_found", authentication: "unknown" },
      { state: "attention", kind: "provider-missing", message: "Codex isn't installed. Install it, then try again." }
    ],
    [
      { availability: "available", authentication: "unauthenticated" },
      {
        state: "attention",
        kind: "provider-signed-out",
        message: "Codex isn't signed in. Sign in to Codex, then try again."
      }
    ],
    [
      { availability: "unavailable", authentication: "unknown" },
      {
        state: "attention",
        kind: "provider-failed",
        message: "Codex is installed but can't start. Reinstall it, then try again."
      }
    ],
    [
      { availability: "available", authentication: "unknown" },
      { state: "unknown", kind: null, message: "Codex is installed." }
    ],
    [
      { availability: "unknown", authentication: "unknown" },
      { state: "unknown", kind: null, message: "TabBeam couldn't check Codex." }
    ]
  ];
  for (const [status, expected] of cases) {
    const popup = openLine(async () => ({
      provider_id: "codex",
      status: { ...status, capabilities: READY.capabilities }
    }));
    await settle();
    assert.deepEqual(popup.view, expected, JSON.stringify(status));
  }

  // DOC-02's fixtures, by the provider they name.
  assert.equal(providerView(DOC_02.provider_statuses[0], "Codex").message, "Codex is ready.");
  assert.equal(
    providerView(DOC_02.provider_statuses[1], "Codex").message,
    "Claude isn't installed. Install it, then try again."
  );
}

{
  // Without a status, the line shows the failure: a missing companion app
  // differs from a companion app that isn't responding.
  const missing = {
    code: "HOST_NOT_INSTALLED",
    reason: "NATIVE_HOST_NOT_FOUND",
    message: "TabBeam's companion app isn't installed. Install it, then try again.",
    retryable: false
  };
  let popup = openLine(async () => ({ provider_id: "codex", error: missing }));
  await settle();
  assert.deepEqual(popup.view, { state: "attention", kind: "host-missing", message: missing.message });

  popup = openLine(async () => ({ provider_id: "codex", error: HOST_OUT_OF_DATE }));
  await settle();
  assert.deepEqual(popup.view, {
    state: "attention",
    kind: "host-unavailable",
    message: HOST_OUT_OF_DATE.message
  });
}

{
  // A worker that never answers, or is gone, leaves the line unknown rather
  // than checking forever; a late answer changes nothing.
  /** @type {(value: any) => void} */
  let answer = () => {};
  let popup = openLine(() => new Promise((resolve) => (answer = resolve)));
  popup.timers[0].callback();
  assert.deepEqual(popup.view, { state: "unknown", kind: null, message: "TabBeam couldn't check Codex." });
  answer({ provider_id: "codex", status: READY });
  await settle();
  assert.equal(popup.view.state, "unknown");

  popup = openLine(() => Promise.reject(new Error("Could not establish connection.")));
  await settle();
  assert.equal(popup.view.state, "unknown");
  assert.equal(popup.timers[0].cancelled, true);

  popup = openLine(() => {
    throw new Error("Extension context invalidated.");
  });
  await settle();
  assert.equal(popup.view.state, "unknown");

  for (const response of [undefined, null, "ready", {}]) {
    popup = openLine(async () => response);
    await settle();
    assert.equal(popup.view.state, "unknown", JSON.stringify(response));
  }
}

{
  // A question's outcome keeps the line current: an answer means ready, and
  // a missing app or provider, or a sign-in, shows what the question found.
  // Other failures say nothing about the provider's state.
  const popup = openLine(async () => ({ provider_id: "codex", status: READY }));
  await settle();
  popup.line.update({ kind: "provider-signed-out", message: "Codex isn't signed in. Run codex login." });
  assert.deepEqual(popup.view, {
    state: "attention",
    kind: "provider-signed-out",
    message: "Codex isn't signed in. Run codex login."
  });
  for (const kind of ["timeout", "cancelled", "provider-failed", "invalid-request", "internal-error"]) {
    popup.line.update({ kind, message: "Something else." });
    assert.equal(popup.view.kind, "provider-signed-out", kind);
  }
  popup.line.update({ kind: "completed" });
  assert.deepEqual(popup.view, { state: "ready", kind: null, message: "Codex is ready." });
  popup.line.update({ kind: "host-missing", message: "TabBeam's companion app isn't installed." });
  assert.equal(popup.view.kind, "host-missing");
}

{
  // An outcome before the status arrives wins over the late status.
  /** @type {(value: any) => void} */
  let answer = () => {};
  const popup = openLine(() => new Promise((resolve) => (answer = resolve)));
  popup.line.update({ kind: "completed" });
  assert.equal(popup.timers[0].cancelled, true);
  answer({ provider_id: "codex", status: { ...READY, authentication: "unauthenticated" } });
  await settle();
  assert.equal(popup.view.state, "ready");
}

{
  // Following a provider selector: no check of its own, "checking" while the
  // selector's check runs, and a finished check never replaces a question's
  // outcome shown since.
  const element = new FakeElement();
  /** @type {any[]} */
  const sent = [];
  const line = bindProviderState(
    /** @type {any} */ (element),
    { async sendMessage(message) { sent.push(message); } },
    { schedule: () => ({}), cancel() {} },
    { checkOnOpen: false }
  );
  const view = () => ({
    state: element.getAttribute("data-state"),
    message: element.textContent
  });
  assert.deepEqual(sent, [], "the selector does the checking");
  assert.deepEqual(view(), { state: "checking", message: "Checking Codex…" });

  line.follow({ providerId: "claude", response: undefined, providerChanged: true, statusUpdated: false });
  assert.deepEqual(view(), { state: "checking", message: "Checking Claude…" });

  line.update({
    kind: "provider-signed-out",
    message: "Claude isn't signed in. Run \"claude auth login\" in a terminal, then try again."
  });
  line.follow({
    providerId: "claude",
    response: { provider_id: "claude", status: READY },
    providerChanged: false,
    statusUpdated: true
  });
  assert.equal(element.getAttribute("data-kind"), "provider-signed-out");

  line.follow({ providerId: "codex", response: null, providerChanged: true, statusUpdated: false });
  assert.deepEqual(view(), { state: "unknown", message: "TabBeam couldn't check Codex." });
  assert.deepEqual(sent, []);
}

console.log("EXT-04 provider status tests passed");
