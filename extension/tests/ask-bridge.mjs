import assert from "node:assert/strict";
import {
  DEFAULT_PROVIDER_ID,
  createRequestId,
  isExtensionPage,
  serveAskPort
} from "../src/background/ask-bridge.js";
import {
  REQUEST_ID_PATTERN,
  createNativeConnectionManager
} from "../src/background/native-connection.js";
import { ASK_PORT_NAME, QUESTION_TOO_LONG } from "../src/shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES, MAX_PAGE_BYTES, MAX_SELECTION_BYTES } from "../src/shared/limits.js";
import { MockPort } from "./support/mock-port.mjs";

const POPUP_URL = "chrome-extension://pervue-test/src/popup/index.html";

/** @param {{failingConnects?: number}} [options] */
function makeWorker({ failingConnects = 0 } = {}) {
  /** @type {MockPort[]} */
  const nativePorts = [];
  /** @type {{context: string, error: unknown}[]} */
  const reportedErrors = [];
  let remainingConnectFailures = failingConnects;
  let nextId = 0;
  /** @type {string | null} */
  let lastError = null;

  const manager = createNativeConnectionManager({
    connectNative() {
      if (remainingConnectFailures > 0) {
        remainingConnectFailures -= 1;
        throw new Error("mock connectNative failure");
      }
      const port = new MockPort("native");
      nativePorts.push(port);
      return port;
    },
    getLastError: () => lastError,
    reportError(context, error) {
      reportedErrors.push({ context, error });
    }
  });

  /** Opens a page's ask port and has the bridge serve it. */
  function openPage() {
    const page = new MockPort(ASK_PORT_NAME, { url: POPUP_URL });
    serveAskPort(page, {
      manager,
      createRequestId: () => `req_test_${(nextId += 1)}`
    });
    return page;
  }

  return {
    manager,
    nativePorts,
    reportedErrors,
    openPage,
    /** @param {string | null} value */
    setLastError(value) {
      lastError = value;
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

/** @param {any} message */
function errorOf(message) {
  assert.equal(message.event, "response.failed");
  return message.payload.error;
}

{
  // One question sends exactly one conversation.send, and only when asked.
  const { manager, nativePorts, openPage } = makeWorker();
  const page = openPage();
  assert.equal(nativePorts.length, 0);

  page.emitMessage({ type: "ask", text: "What is Pervue?" });

  assert.equal(nativePorts.length, 1);
  assert.deepEqual(nativePorts[0].messages, [
    {
      version: 1,
      type: "request",
      request_id: "req_test_1",
      method: "conversation.send",
      payload: {
        provider_id: DEFAULT_PROVIDER_ID,
        input: { text: "What is Pervue?" }
      }
    }
  ]);
  assert.equal(DEFAULT_PROVIDER_ID, "fake");
  assert.equal(manager.pendingRequestCount, 1);

  // A second message on the same port is not a second question.
  page.emitMessage({ type: "ask", text: "And again?" });
  assert.equal(nativePorts[0].messages.length, 1);
  assert.equal(manager.pendingRequestCount, 1);
}

{
  // Explicitly selected context reaches the host as structured request data.
  // The bridge copies only the fields it validates.
  const { nativePorts, openPage } = makeWorker();
  const context = {
    mode: "selection",
    text: "A quoted passage",
    truncated: false,
    page: { title: "Example", url: "https://example.com/story" },
    unexpected: "discard me"
  };
  openPage().emitMessage({ type: "ask", text: "Explain this", context });
  assert.deepEqual(nativePorts[0].messages[0].payload.context, {
    mode: "selection",
    text: context.text,
    truncated: false,
    page: context.page
  });
}

{
  // Malformed, over-limit, or sensitive URL context never opens a native port.
  const good = {
    mode: "page",
    text: "Readable page",
    truncated: false,
    page: { title: "Example", url: "https://example.com/story" }
  };
  for (const context of [
    { ...good, mode: "other" },
    { ...good, text: "x".repeat(MAX_PAGE_BYTES + 1) },
    { ...good, mode: "selection", text: "x".repeat(MAX_SELECTION_BYTES + 1) },
    { ...good, text: "bad\ud800text" },
    { ...good, page: { ...good.page, title: "bad\udc00title" } },
    { ...good, page: { ...good.page, url: "https://example.com/story?token=secret" } },
    { ...good, page: { ...good.page, url: "chrome://settings/" } }
  ]) {
    const { nativePorts, openPage } = makeWorker();
    const page = openPage();
    page.emitMessage({ type: "ask", text: "Explain", context });
    assert.equal(nativePorts.length, 0);
    assert.deepEqual(errorOf(page.messages[0]), {
      code: "INVALID_REQUEST",
      reason: "INVALID_PAYLOAD",
      message: "The selected page context is invalid. Choose a source again or use No context.",
      retryable: false
    });
  }
}

{
  // Events reach the page in order as they arrive; the terminal event closes
  // the page's port but keeps the native port for the next question.
  const { manager, nativePorts, reportedErrors, openPage } = makeWorker();
  const page = openPage();
  page.emitMessage({ type: "ask", text: "Stream, please." });
  const native = nativePorts[0];

  const events = [
    hostEvent("req_test_1", "conversation.created", { conversation_id: "conv_1" }),
    hostEvent("req_test_1", "response.started", { provider_id: "fake" }),
    hostEvent("req_test_1", "response.delta", { text: "alpha" }),
    hostEvent("req_test_1", "response.delta", { text: " beta" }),
    hostEvent("req_test_1", "response.completed")
  ];

  for (const [index, event] of events.entries()) {
    native.emitMessage(event);
    assert.deepEqual(page.messages, events.slice(0, index + 1));
    assert.equal(page.disconnectCalls, index === events.length - 1 ? 1 : 0);
  }

  assert.equal(manager.pendingRequestCount, 0);
  assert.equal(native.disconnectCalls, 0);
  assert.deepEqual(reportedErrors, []);
}

{
  // A host failure is forwarded as the page's terminal event.
  const { openPage, nativePorts } = makeWorker();
  const page = openPage();
  page.emitMessage({ type: "ask", text: "Which provider?" });

  const failure = hostEvent("req_test_1", "response.failed", {
    error: {
      code: "PROVIDER_NOT_FOUND",
      reason: "PROVIDER_NOT_INSTALLED",
      message: "The selected provider runtime is not installed.",
      retryable: false
    }
  });
  nativePorts[0].emitMessage(failure);

  assert.deepEqual(page.messages, [failure]);
  assert.equal(page.disconnectCalls, 1);
}

{
  // Pages asking at the same time share one native port, and each receives
  // only its own request's events.
  const { nativePorts, openPage } = makeWorker();
  const popup = openPage();
  const fullPage = openPage();
  popup.emitMessage({ type: "ask", text: "First" });
  fullPage.emitMessage({ type: "ask", text: "Second" });

  assert.equal(nativePorts.length, 1);
  const native = nativePorts[0];
  native.emitMessage(hostEvent("req_test_2", "response.delta", { text: "two" }));
  native.emitMessage(hostEvent("req_test_1", "response.delta", { text: "one" }));

  assert.deepEqual(
    popup.messages.map((message) => message.payload.text),
    ["one"]
  );
  assert.deepEqual(
    fullPage.messages.map((message) => message.payload.text),
    ["two"]
  );
}

{
  // A native disconnect before the terminal event becomes the page's only
  // terminal event, and the next question reconnects.
  const { manager, nativePorts, openPage } = makeWorker();
  const page = openPage();
  page.emitMessage({ type: "ask", text: "Will the host crash?" });
  nativePorts[0].emitMessage(
    hostEvent("req_test_1", "response.delta", { text: "partial" })
  );
  nativePorts[0].emitDisconnect();

  assert.equal(page.messages.length, 2);
  assert.equal(page.messages[1].request_id, "req_test_1");
  assert.deepEqual(errorOf(page.messages[1]), {
    code: "HOST_UNAVAILABLE",
    reason: "HOST_DISCONNECTED",
    message: "Pervue lost its connection to the companion app. Try again.",
    retryable: true
  });
  assert.equal(page.disconnectCalls, 1);
  assert.equal(manager.pendingRequestCount, 0);

  const next = openPage();
  next.emitMessage({ type: "ask", text: "Try again" });
  assert.equal(nativePorts.length, 2);
  assert.equal(nativePorts[1].messages[0].request_id, "req_test_2");
}

{
  // Chrome explains a native port that closes early only through
  // runtime.lastError. A missing host, or one registered only for other
  // extensions, is DOC-02's HOST_NOT_INSTALLED, which retrying cannot fix.
  for (const [lastError, code, reason, retryable] of [
    [
      "Specified native messaging host not found.",
      "HOST_NOT_INSTALLED",
      "NATIVE_HOST_NOT_FOUND",
      false
    ],
    [
      "Access to the specified native messaging host is forbidden.",
      "HOST_NOT_INSTALLED",
      "NATIVE_HOST_NOT_REGISTERED",
      false
    ],
    [
      "Failed to start native messaging host.",
      "HOST_UNAVAILABLE",
      "HOST_START_FAILED",
      true
    ],
    ["Native host has exited.", "HOST_UNAVAILABLE", "HOST_DISCONNECTED", true],
    [
      "Error when communicating with the native messaging host.",
      "HOST_UNAVAILABLE",
      "HOST_DISCONNECTED",
      true
    ],
    [null, "HOST_UNAVAILABLE", "HOST_DISCONNECTED", true]
  ]) {
    const worker = makeWorker();
    const page = worker.openPage();
    page.emitMessage({ type: "ask", text: "Is the host installed?" });
    worker.setLastError(/** @type {string | null} */ (lastError));
    worker.nativePorts[0].emitDisconnect();

    const error = errorOf(page.messages[0]);
    assert.equal(error.code, code, String(lastError));
    assert.equal(error.reason, reason, String(lastError));
    assert.equal(error.retryable, retryable, String(lastError));
    assert.ok(typeof error.message === "string" && error.message !== "");
    assert.equal(page.disconnectCalls, 1);
  }
}

{
  // connectNative throwing fails the question without leaving a route, and a
  // later question connects.
  const { manager, nativePorts, openPage } = makeWorker({ failingConnects: 1 });
  const page = openPage();
  page.emitMessage({ type: "ask", text: "Is the host there?" });

  assert.equal(page.messages.length, 1);
  assert.equal(errorOf(page.messages[0]).reason, "HOST_START_FAILED");
  assert.equal(errorOf(page.messages[0]).code, "HOST_UNAVAILABLE");
  assert.equal(page.disconnectCalls, 1);
  assert.equal(manager.pendingRequestCount, 0);

  openPage().emitMessage({ type: "ask", text: "Now?" });
  assert.equal(nativePorts.length, 1);
}

{
  // A post failure on the native port produces exactly one terminal event.
  const { manager, nativePorts, openPage } = makeWorker();
  openPage().emitMessage({ type: "ask", text: "Open the port" });
  nativePorts[0].throwOnPost = true;

  const page = openPage();
  page.emitMessage({ type: "ask", text: "Post fails" });

  assert.equal(page.messages.length, 1);
  assert.equal(errorOf(page.messages[0]).code, "HOST_UNAVAILABLE");
  assert.equal(page.disconnectCalls, 1);
  assert.equal(manager.pendingRequestCount, 0);
}

{
  // A page that closes mid-answer gets nothing more; its request still runs
  // to its terminal event and releases its route.
  const { manager, nativePorts, reportedErrors, openPage } = makeWorker();
  const page = openPage();
  page.emitMessage({ type: "ask", text: "Closing soon" });
  const native = nativePorts[0];
  native.emitMessage(hostEvent("req_test_1", "response.started", { provider_id: "fake" }));

  page.emitDisconnect();
  native.emitMessage(hostEvent("req_test_1", "response.delta", { text: "unseen" }));
  native.emitMessage(hostEvent("req_test_1", "response.completed"));

  assert.equal(page.messages.length, 1);
  assert.equal(page.disconnectCalls, 0);
  assert.equal(manager.pendingRequestCount, 0);
  assert.equal(native.disconnectCalls, 0);
  assert.deepEqual(reportedErrors, []);
}

{
  // If the page is gone before its disconnect event arrives, posting throws;
  // the bridge stops forwarding without reporting an error.
  const { manager, nativePorts, reportedErrors, openPage } = makeWorker();
  const page = openPage();
  page.emitMessage({ type: "ask", text: "Race" });
  page.throwOnPost = true;

  const native = nativePorts[0];
  native.emitMessage(hostEvent("req_test_1", "response.delta", { text: "lost" }));
  page.throwOnPost = false;
  native.emitMessage(hostEvent("req_test_1", "response.completed"));

  assert.deepEqual(page.messages, []);
  assert.equal(manager.pendingRequestCount, 0);
  assert.deepEqual(reportedErrors, []);
}

{
  // A question without text fails before anything reaches the native host.
  for (const message of [
    { type: "ask", text: "" },
    { type: "ask", text: " \n\t" },
    { type: "ask", text: 42 },
    { type: "ask" },
    { type: "other", text: "Hello" },
    null
  ]) {
    const { nativePorts, openPage } = makeWorker();
    const page = openPage();
    page.emitMessage(message);

    assert.equal(nativePorts.length, 0, JSON.stringify(message));
    assert.equal(page.messages.length, 1);
    assert.equal(page.messages[0].request_id, null);
    assert.deepEqual(errorOf(page.messages[0]), {
      code: "INVALID_REQUEST",
      reason: "INVALID_PAYLOAD",
      message: "Type a question first.",
      retryable: false
    });
    assert.equal(page.disconnectCalls, 1);
  }
}

{
  // A question too large for the native host fails before a native port
  // opens, as INVALID_REQUEST / REQUEST_TOO_LARGE.
  const { manager, nativePorts, openPage } = makeWorker();
  const page = openPage();
  page.emitMessage({ type: "ask", text: "x".repeat(MAX_NATIVE_MESSAGE_BYTES) });

  assert.equal(nativePorts.length, 0);
  assert.equal(manager.pendingRequestCount, 0);
  assert.equal(page.messages.length, 1);
  assert.equal(page.messages[0].request_id, "req_test_1");
  assert.deepEqual(errorOf(page.messages[0]), {
    code: "INVALID_REQUEST",
    reason: "REQUEST_TOO_LARGE",
    message: QUESTION_TOO_LONG.message,
    retryable: false
  });
  assert.equal(page.disconnectCalls, 1);
}

{
  // Only the extension's own pages may open an ask port.
  const base = "chrome-extension://pervue-test/";
  assert.equal(isExtensionPage({ url: POPUP_URL }, base), true);
  assert.equal(
    isExtensionPage({ url: `${base}src/fullpage/index.html?entry=popup` }, base),
    true
  );
  assert.equal(isExtensionPage({ url: "https://example.com/" }, base), false);
  assert.equal(
    isExtensionPage({ url: "chrome-extension://pervue-test-other/x.html" }, base),
    false
  );
  assert.equal(isExtensionPage({}, base), false);
  assert.equal(isExtensionPage(undefined, base), false);
}

{
  // Generated request IDs follow the protocol v1 grammar and do not repeat.
  const ids = new Set();
  for (let index = 0; index < 100; index += 1) {
    const id = createRequestId();
    assert.ok(REQUEST_ID_PATTERN.test(id), id);
    ids.add(id);
  }
  assert.equal(ids.size, 100);
}

console.log("EXT-03 service-worker ask bridge tests passed");
