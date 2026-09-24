import assert from "node:assert/strict";
import {
  NATIVE_HOST_NAME,
  createNativeConnectionManager
} from "../src/background/native-connection.js";

class MockEvent {
  constructor() {
    /** @type {Set<(value: any) => void>} */
    this.listeners = new Set();
  }

  /** @param {(value: any) => void} listener */
  addListener(listener) {
    this.listeners.add(listener);
  }

  /** @param {(value: any) => void} listener */
  removeListener(listener) {
    this.listeners.delete(listener);
  }

  /** @param {any} value */
  emit(value) {
    for (const listener of [...this.listeners]) {
      listener(value);
    }
  }
}

class MockPort {
  /** @param {string} name */
  constructor(name) {
    this.name = name;
    /** @type {any[]} */
    this.messages = [];
    this.disconnectCalls = 0;
    this.throwOnPost = false;
    this.onMessage = new MockEvent();
    this.onDisconnect = new MockEvent();
  }

  /** @param {any} message */
  postMessage(message) {
    if (this.throwOnPost) {
      throw new Error("mock post failure");
    }
    this.messages.push(message);
  }

  disconnect() {
    this.disconnectCalls += 1;
  }

  /** @param {any} message */
  emitMessage(message) {
    this.onMessage.emit(message);
  }

  emitDisconnect() {
    this.onDisconnect.emit(undefined);
  }
}

/** @param {{failingConnects?: number}} [options] */
function makeHarness({ failingConnects = 0 } = {}) {
  /** @type {MockPort[]} */
  const ports = [];
  /** @type {string | null} */
  let lastError = null;
  let lastErrorReads = 0;
  let remainingConnectFailures = failingConnects;
  /** @type {string[]} */
  const hostNames = [];
  /** @type {{context: string, error: unknown}[]} */
  const reportedErrors = [];

  const manager = createNativeConnectionManager({
    /** @param {string} hostName */
    connectNative(hostName) {
      hostNames.push(hostName);
      if (remainingConnectFailures > 0) {
        remainingConnectFailures -= 1;
        throw new Error("mock connectNative failure");
      }
      const port = new MockPort(`port-${ports.length + 1}`);
      ports.push(port);
      return port;
    },
    getLastError() {
      lastErrorReads += 1;
      return lastError;
    },
    reportError(context, error) {
      reportedErrors.push({ context, error });
    }
  });

  return {
    manager,
    ports,
    hostNames,
    reportedErrors,
    get lastErrorReads() {
      return lastErrorReads;
    },
    /** @param {string | null} value */
    setLastError(value) {
      lastError = value;
    }
  };
}

/** @param {{error: unknown}[]} reports */
function reportedMessages(reports) {
  return reports.map(({ error }) => String(error));
}

/** @param {string} id @param {string} [method] */
function request(id, method = "provider.status") {
  return {
    version: 1,
    type: "request",
    request_id: id,
    method,
    payload: {}
  };
}

{
  const { manager, ports, hostNames } = makeHarness();

  assert.equal(manager.connected, false);
  assert.equal(ports.length, 0);

  manager.send(request("req_lazy"));

  assert.equal(ports.length, 1);
  assert.deepEqual(hostNames, [NATIVE_HOST_NAME]);
  assert.equal(manager.connected, true);
  assert.deepEqual(ports[0].messages, [request("req_lazy")]);

  manager.send(request("req_reuse"));
  assert.equal(ports.length, 1);
  assert.equal(ports[0].messages.length, 2);
}

{
  const { manager, ports } = makeHarness();
  /** @type {any[]} */
  const firstEvents = [];
  /** @type {any[]} */
  const secondEvents = [];

  manager.send(request("req_1"), {
    onEvent: (event) => firstEvents.push(event)
  });
  manager.send(request("req_2"), {
    onEvent: (event) => secondEvents.push(event)
  });

  ports[0].emitMessage({
    version: 1,
    type: "event",
    request_id: "req_2",
    event: "response.delta",
    payload: { text: "two" }
  });
  ports[0].emitMessage({
    version: 1,
    type: "event",
    request_id: "req_1",
    event: "response.delta",
    payload: { text: "one" }
  });

  assert.deepEqual(
    firstEvents.map((event) => event.payload.text),
    ["one"]
  );
  assert.deepEqual(
    secondEvents.map((event) => event.payload.text),
    ["two"]
  );
  assert.equal(manager.pendingRequestCount, 2);

  ports[0].emitMessage({
    version: 1,
    type: "event",
    request_id: "req_1",
    event: "response.completed",
    payload: {}
  });

  assert.equal(manager.pendingRequestCount, 1);

  assert.throws(
    () => manager.send(request("req_2")),
    /already in flight/
  );

  ports[0].emitMessage({
    version: 1,
    type: "event",
    request_id: "req_2",
    event: "response.failed",
    payload: {
      error: {
        code: "INVALID_REQUEST",
        reason: "TEST",
        message: "done",
        retryable: false
      }
    }
  });

  assert.equal(manager.pendingRequestCount, 0);
}

{
  const { manager, ports } = makeHarness();
  /** @type {string[]} */
  const lifecycle = [];

  const unsubscribe = manager.onLifecycleEvent((event) => {
    lifecycle.push(event.event);
  });

  manager.send(request("req_lifecycle"));
  ports[0].emitMessage({
    version: 1,
    type: "event",
    request_id: null,
    event: "host.ready",
    payload: {
      host_version: "test",
      protocol_versions: [1]
    }
  });

  assert.deepEqual(lifecycle, ["host.ready"]);

  unsubscribe();
  ports[0].emitMessage({
    request_id: null,
    event: "host.ready",
    payload: {}
  });

  assert.deepEqual(lifecycle, ["host.ready"]);
}

{
  const { manager, ports, setLastError } = makeHarness();
  /** @type {{requestId: string, message: string | null}[]} */
  const requestDisconnects = [];
  /** @type {{message: string | null, requestIds: string[]}[]} */
  const managerDisconnects = [];

  manager.onDisconnect((details) => {
    managerDisconnects.push(details);
  });

  manager.send(request("req_dead_1"), {
    onDisconnect: (details) => requestDisconnects.push(details)
  });
  manager.send(request("req_dead_2"), {
    onDisconnect: (details) => requestDisconnects.push(details)
  });

  setLastError("native host exited");
  ports[0].emitDisconnect();

  assert.equal(manager.connected, false);
  assert.equal(manager.pendingRequestCount, 0);
  assert.deepEqual(
    requestDisconnects.map((item) => item.requestId),
    ["req_dead_1", "req_dead_2"]
  );
  assert.deepEqual(managerDisconnects, [
    {
      message: "native host exited",
      requestIds: ["req_dead_1", "req_dead_2"]
    }
  ]);

  manager.send(request("req_after_restart"));
  assert.equal(ports.length, 2);
  assert.equal(manager.connected, true);

  // Delayed callbacks from the dead port must not affect the replacement port.
  ports[0].emitMessage({
    request_id: "req_after_restart",
    event: "response.completed",
    payload: {}
  });
  ports[0].emitDisconnect();

  assert.equal(manager.connected, true);
  assert.equal(manager.pendingRequestCount, 1);

  ports[1].emitMessage({
    request_id: "req_after_restart",
    event: "response.completed",
    payload: {}
  });
  assert.equal(manager.pendingRequestCount, 0);
}

{
  const { manager, ports } = makeHarness();

  assert.throws(
    () => manager.send({ version: 1 }),
    /request\.request_id/
  );

  manager.send(request("req_manual"));
  manager.disconnect("manual");

  assert.equal(manager.connected, false);
  assert.equal(manager.pendingRequestCount, 0);
  assert.equal(ports[0].disconnectCalls, 1);

  manager.send(request("req_manual_reconnect"));
  assert.equal(ports.length, 2);
}

{
  const { manager, ports } = makeHarness();
  /** @type {{requestId: string, message: string | null}[]} */
  const requestDisconnects = [];

  manager.send(request("req_post_failure"), {
    onDisconnect: (details) => requestDisconnects.push(details)
  });
  ports[0].throwOnPost = true;

  assert.throws(
    () => manager.send(request("req_post_failure_2"), {
      onDisconnect: (details) => requestDisconnects.push(details)
    }),
    /mock post failure/
  );

  assert.equal(manager.connected, false);
  assert.equal(manager.pendingRequestCount, 0);
  assert.equal(ports[0].disconnectCalls, 1);
  assert.deepEqual(
    requestDisconnects.map((item) => item.requestId),
    ["req_post_failure", "req_post_failure_2"]
  );

  manager.send(request("req_post_failure_reconnect"));
  assert.equal(ports.length, 2);
}

{
  // request.cancelled is terminal for the cancellation control request (v1 §5.3).
  const { manager, ports } = makeHarness();
  /** @type {string[]} */
  const events = [];

  manager.send(request("req_cancel_1", "request.cancel"), {
    onEvent: (event) => events.push(event.event)
  });
  ports[0].emitMessage({
    version: 1,
    type: "event",
    request_id: "req_cancel_1",
    event: "request.cancelled",
    payload: { target_request_id: "req_answer_1" }
  });

  assert.deepEqual(events, ["request.cancelled"]);
  assert.equal(manager.pendingRequestCount, 0);
}

{
  // A connectNative failure propagates, leaves no route, and the next send retries.
  const { manager, ports, hostNames } = makeHarness({ failingConnects: 1 });

  assert.throws(
    () => manager.send(request("req_connect_retry")),
    /mock connectNative failure/
  );
  assert.equal(manager.connected, false);
  assert.equal(manager.pendingRequestCount, 0);

  manager.send(request("req_connect_retry"));
  assert.equal(hostNames.length, 2);
  assert.equal(ports.length, 1);
  assert.equal(manager.pendingRequestCount, 1);
}

{
  // One throwing owner or listener cannot stop the others hearing about a disconnect.
  const { manager, ports, reportedErrors } = makeHarness();
  /** @type {string[]} */
  const notified = [];
  /** @type {string[][]} */
  const managerNotified = [];

  manager.onDisconnect((details) => managerNotified.push(details.requestIds));
  manager.onDisconnect(() => {
    throw new Error("listener boom");
  });
  manager.onDisconnect((details) => managerNotified.push(details.requestIds));

  manager.send(request("req_owner_1"), {
    onDisconnect: ({ requestId }) => notified.push(requestId)
  });
  manager.send(request("req_owner_2"), {
    onDisconnect: () => {
      throw new Error("owner boom");
    }
  });
  manager.send(request("req_owner_3"), {
    onDisconnect: ({ requestId }) => notified.push(requestId)
  });

  ports[0].emitDisconnect();

  const allIds = ["req_owner_1", "req_owner_2", "req_owner_3"];
  assert.deepEqual(notified, ["req_owner_1", "req_owner_3"]);
  assert.deepEqual(managerNotified, [allIds, allIds]);
  assert.deepEqual(reportedMessages(reportedErrors), [
    "Error: owner boom",
    "Error: listener boom"
  ]);
  assert.equal(manager.connected, false);
  assert.equal(manager.pendingRequestCount, 0);
}

{
  // A throwing lifecycle listener cannot hide host.ready from the others.
  const { manager, ports, reportedErrors } = makeHarness();
  /** @type {string[]} */
  const seen = [];

  manager.onLifecycleEvent(() => {
    throw new Error("lifecycle boom");
  });
  manager.onLifecycleEvent((event) => seen.push(event.event));

  manager.send(request("req_ready"));
  ports[0].emitMessage({ request_id: null, event: "host.ready", payload: {} });

  assert.deepEqual(seen, ["host.ready"]);
  assert.deepEqual(reportedMessages(reportedErrors), ["Error: lifecycle boom"]);
}

{
  // An explicit disconnect closes the native port even when an owner throws.
  const { manager, ports, reportedErrors } = makeHarness();

  manager.send(request("req_throwing_owner"), {
    onDisconnect: () => {
      throw new Error("owner boom");
    }
  });
  manager.disconnect("manual");

  assert.equal(ports[0].disconnectCalls, 1);
  assert.equal(manager.connected, false);
  assert.deepEqual(reportedMessages(reportedErrors), ["Error: owner boom"]);

  manager.send(request("req_after_throwing_owner"));
  assert.equal(ports.length, 2);
}

{
  // A throwing event handler is reported and its finished request is still retired.
  const { manager, ports, reportedErrors } = makeHarness();

  manager.send(request("req_throwing_event"), {
    onEvent: () => {
      throw new Error("event boom");
    }
  });
  ports[0].emitMessage({
    request_id: "req_throwing_event",
    event: "response.completed",
    payload: {}
  });

  assert.equal(manager.pendingRequestCount, 0);
  assert.deepEqual(reportedMessages(reportedErrors), ["Error: event boom"]);
}

{
  // A finished request is retired before its owner runs, so the owner can
  // disconnect or reuse the ID without being told its request was cut off.
  const { manager, ports } = makeHarness();
  /** @type {string[]} */
  const disconnects = [];
  /** @type {string[]} */
  const replacementEvents = [];

  manager.send(request("req_done"), {
    onEvent: () => {
      manager.disconnect("owner closed the connection");
      manager.send(request("req_done"), {
        onEvent: (event) => replacementEvents.push(event.event)
      });
    },
    onDisconnect: ({ requestId }) => disconnects.push(requestId)
  });

  ports[0].emitMessage({
    request_id: "req_done",
    event: "response.completed",
    payload: {}
  });

  assert.deepEqual(disconnects, []);
  assert.equal(ports.length, 2);
  assert.equal(manager.pendingRequestCount, 1);

  ports[1].emitMessage({
    request_id: "req_done",
    event: "response.delta",
    payload: { text: "again" }
  });
  assert.deepEqual(replacementEvents, ["response.delta"]);
}

{
  // Every onDisconnect reads lastError, even from a replaced port, so Chrome
  // does not log an unchecked runtime.lastError.
  const harness = makeHarness();
  const { manager, ports } = harness;

  manager.send(request("req_first_port"));
  manager.disconnect("manual");
  manager.send(request("req_second_port"));

  const readsBefore = harness.lastErrorReads;
  ports[0].emitDisconnect();

  assert.equal(harness.lastErrorReads, readsBefore + 1);
  assert.equal(manager.connected, true);
  assert.equal(manager.pendingRequestCount, 1);
}

console.log("EXT-02 Native Messaging connection manager tests passed");
