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

function makeHarness() {
  /** @type {MockPort[]} */
  const ports = [];
  /** @type {string | null} */
  let lastError = null;
  /** @type {string[]} */
  const hostNames = [];

  const manager = createNativeConnectionManager({
    /** @param {string} hostName */
    connectNative(hostName) {
      hostNames.push(hostName);
      const port = new MockPort(`port-${ports.length + 1}`);
      ports.push(port);
      return port;
    },
    getLastError() {
      return lastError;
    }
  });

  return {
    manager,
    ports,
    hostNames,
    /** @param {string | null} value */
    setLastError(value) {
      lastError = value;
    }
  };
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

console.log("EXT-02 Native Messaging connection manager tests passed");
