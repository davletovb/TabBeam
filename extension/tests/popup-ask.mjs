import assert from "node:assert/strict";
import fs from "node:fs";
import { READY_STATUS, WORKER_LOST, bindAskForm } from "../src/popup/ask-form.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES } from "../src/shared/limits.js";
import { FAILURE_KINDS, KIND_MESSAGES } from "../src/shared/outcomes.js";
import { MockPort } from "./support/mock-port.mjs";

/** DOC-02's machine-readable fixtures. */
const DOC_02 = JSON.parse(
  fs.readFileSync(new URL("../../docs/protocol/fixtures/v1-errors-capabilities.json", import.meta.url), "utf8")
);

/** The subset of an HTML element that the ask form touches. */
class FakeElement {
  constructor() {
    /** @type {Map<string, ((event: any) => void)[]>} */
    this.listeners = new Map();
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    this.textContent = "";
    this.hidden = false;
    this.focused = false;
    this.disabled = false;
    /** @type {{activeElement?: any} | null} */
    this.ownerDocument = null;
  }

  /**
   * @param {string} type
   * @param {(event: any) => void} listener
   */
  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  /**
   * @param {string} type
   * @param {any} event
   */
  dispatch(type, event) {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
    return event;
  }

  /**
   * @param {string} name
   * @param {string} value
   */
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

  /**
   * Appends text nodes; anything else would be markup.
   * @param {...unknown} nodes
   */
  append(...nodes) {
    for (const node of nodes) {
      assert.equal(typeof node, "string");
      this.textContent += node;
    }
  }

  focus() {
    this.focused = true;
    if (this.ownerDocument) this.ownerDocument.activeElement = this;
  }
}

class FakeTextArea extends FakeElement {
  constructor() {
    super();
    this.value = "";
    this.disabled = false;
    this.readOnly = false;
  }
}

class FakeForm extends FakeElement {
  requestSubmit() {
    this.dispatch("submit", cancellableEvent({}));
  }
}

/**
 * @template {object} T
 * @param {T} fields
 */
function cancellableEvent(fields) {
  return {
    ...fields,
    defaultPrevented: false,
    preventDefault() {
      this.defaultPrevented = true;
    }
  };
}

/**
 * @param {{getContext(): any | null, isPending(): boolean, consume?(context: any): void}} [contextControls]
 * @param {{kind: string, message?: string}[]} [outcomes] receives each outcome
 */
function openPopup(contextControls, outcomes = []) {
  const elements = {
    form: new FakeForm(),
    input: new FakeTextArea(),
    submit: new FakeElement(),
    status: new FakeElement(),
    answer: new FakeElement(),
    cancel: new FakeElement(),
    retry: new FakeElement()
  };
  const ownerDocument = { activeElement: null };
  for (const element of Object.values(elements)) element.ownerDocument = ownerDocument;
  // As in index.html, the answer region starts hidden.
  elements.answer.hidden = true;

  /** @type {MockPort[]} */
  const ports = [];
  /** @type {{name: string}[]} */
  const connectInfos = [];
  /** @type {Error | null} */
  let connectError = null;
  let postThrows = false;

  const runtime = {
    /** @param {{name: string}} connectInfo */
    connect(connectInfo) {
      connectInfos.push(connectInfo);
      if (connectError) {
        throw connectError;
      }
      const port = new MockPort(connectInfo.name);
      port.throwOnPost = postThrows;
      ports.push(port);
      return port;
    }
  };

  bindAskForm(/** @type {any} */ (elements), runtime, contextControls, {
    onOutcome: (outcome) => outcomes.push(outcome)
  });

  return {
    ...elements,
    ports,
    connectInfos,
    /** @param {Error | null} error */
    failConnect(error) {
      connectError = error;
    },
    /** @param {boolean} value whether new ports throw on postMessage */
    failPost(value) {
      postThrows = value;
    },
    /**
     * Types into the input and presses Enter.
     * @param {string} text
     * @param {{shiftKey?: boolean, isComposing?: boolean, keyCode?: number}} [modifiers]
     */
    ask(text, modifiers = {}) {
      elements.input.value = text;
      return elements.input.dispatch(
        "keydown",
        cancellableEvent({
          key: "Enter",
          shiftKey: false,
          isComposing: false,
          ...modifiers
        })
      );
    },
    /** Clicking the submit button submits the form, even when aria-disabled. */
    clickAsk() {
      return elements.form.dispatch("submit", cancellableEvent({}));
    },
    clickCancel() {
      elements.cancel.focus();
      return elements.cancel.dispatch("click", cancellableEvent({}));
    },
    clickRetry() {
      elements.retry.focus();
      return elements.retry.dispatch("click", cancellableEvent({}));
    },
    get statusText() {
      return elements.status.textContent;
    },
    get statusState() {
      return elements.status.getAttribute("data-state");
    },
    get statusKind() {
      return elements.status.getAttribute("data-kind");
    },
    get busy() {
      return elements.submit.getAttribute("aria-disabled") === "true";
    },
    get activeElement() {
      return ownerDocument.activeElement;
    }
  };
}

{
  // Ask waits for an explicit capture to settle, then includes its previewed
  // context. No choice continues to send only the question.
  let pending = true;
  const context = {
    mode: "page",
    text: "Visible article text",
    truncated: false,
    page: { title: "Example", url: "https://example.com/" }
  };
  const popup = openPopup({ isPending: () => pending, getContext: () => context });
  popup.ask("Summarize");
  assert.equal(popup.ports.length, 0);
  assert.equal(popup.statusText, "Wait for context capture to finish.");
  // Feedback on what was just done stays visible, unlike the idle hint.
  assert.equal(popup.statusState, "notice");
  pending = false;
  popup.ask("Summarize");
  assert.deepEqual(popup.ports[0].messages, [{ type: "ask", text: "Summarize", context }]);
}

/**
 * @param {string} event
 * @param {any} [payload]
 */
function hostEvent(event, payload = {}) {
  return { version: 1, type: "event", request_id: "req_popup", event, payload };
}

{
  // Opening the popup: the input is focused and usable at once, and nothing
  // connects until the user asks.
  const popup = openPopup();

  assert.equal(popup.input.focused, true);
  assert.equal(popup.input.disabled, false);
  assert.equal(popup.statusText, READY_STATUS);
  assert.equal(popup.statusState, "idle");
  assert.equal(popup.busy, false);
  assert.equal(popup.answer.hidden, true);
  assert.equal(popup.ports.length, 0);
}

{
  // Enter asks one question over one port.
  const popup = openPopup();
  const keydown = popup.ask("What is TabBeam?");

  assert.equal(keydown.defaultPrevented, true);
  assert.deepEqual(popup.connectInfos, [{ name: ASK_PORT_NAME }]);
  assert.deepEqual(popup.ports[0].messages, [
    { type: "ask", text: "What is TabBeam?" }
  ]);
  assert.equal(popup.statusText, "Sending…");
  assert.equal(popup.statusState, "pending");
  assert.equal(popup.busy, true);
  assert.equal(popup.answer.getAttribute("aria-busy"), "true");
  assert.equal(popup.answer.hidden, true);
}

{
  // Shift+Enter adds a line, and Enter that ends an IME composition is not a
  // submit, whether the composition shows in isComposing or in keyCode 229.
  const popup = openPopup();

  const shiftEnter = popup.ask("line one", { shiftKey: true });
  assert.equal(shiftEnter.defaultPrevented, false);

  const composing = popup.ask("日本", { isComposing: true });
  assert.equal(composing.defaultPrevented, false);

  const legacyComposing = popup.ask("日本", { keyCode: 229 });
  assert.equal(legacyComposing.defaultPrevented, false);

  assert.equal(popup.ports.length, 0);
}

{
  // An empty question is not sent.
  const popup = openPopup();
  popup.ask("   \n ");

  assert.equal(popup.ports.length, 0);
  assert.equal(popup.statusText, "Type a question first.");
  assert.equal(popup.statusState, "notice");
  assert.equal(popup.busy, false);
}

{
  // Duplicate-submit guard: nothing starts a second request while one is in
  // flight, whichever way the form is submitted.
  const popup = openPopup();
  popup.ask("Once");
  popup.ask("Twice");
  popup.clickAsk();
  popup.form.requestSubmit();

  const port = popup.ports[0];
  port.emitMessage(hostEvent("response.started", { provider_id: "fake" }));
  port.emitMessage(hostEvent("response.delta", { text: "partial" }));
  popup.clickAsk();

  assert.equal(popup.ports.length, 1);
  assert.deepEqual(port.messages, [{ type: "ask", text: "Once" }]);
}

{
  // Deltas render as they arrive, before the response completes.
  const popup = openPopup();
  popup.ask("Stream it");
  const port = popup.ports[0];

  port.emitMessage(hostEvent("conversation.created", { conversation_id: "c1" }));
  port.emitMessage(hostEvent("response.started", { provider_id: "fake" }));
  assert.equal(popup.statusText, "Answering…");
  assert.equal(popup.answer.textContent, "");
  assert.equal(popup.answer.hidden, true);

  port.emitMessage(hostEvent("response.delta", { text: "alpha" }));
  assert.equal(popup.answer.textContent, "alpha");
  assert.equal(popup.answer.hidden, false);
  assert.equal(popup.busy, true);

  port.emitMessage(hostEvent("response.delta", { text: " beta" }));
  assert.equal(popup.answer.textContent, "alpha beta");
  assert.equal(popup.busy, true);

  port.emitMessage(hostEvent("response.completed"));
  assert.equal(popup.answer.textContent, "alpha beta");
  assert.equal(popup.statusText, "Answer complete.");
  assert.equal(popup.statusState, "done");
  assert.equal(popup.busy, false);
  assert.equal(popup.answer.getAttribute("aria-busy"), "false");
  assert.equal(port.disconnectCalls, 1);
}

{
  // EXT-12: Stop sends a cancellation request over the same UI port. The
  // target's normalized cancellation makes Retry available, and Retry starts
  // a fresh request with the original question rather than duplicating an
  // assistant message locally.
  const popup = openPopup();
  popup.ask("Long answer");
  const first = popup.ports[0];
  assert.equal(popup.cancel.hidden, false);
  assert.equal(popup.retry.hidden, true);

  popup.clickCancel();
  assert.deepEqual(first.messages, [
    { type: "ask", text: "Long answer" },
    { type: "cancel" }
  ]);
  assert.equal(popup.statusText, "Stopping…");
  assert.equal(popup.statusState, "cancelled");
  assert.equal(popup.cancel.getAttribute("aria-disabled"), "true");

  first.emitMessage(hostEvent("response.failed", {
    error: {
      code: "REQUEST_CANCELLED",
      reason: "USER_CANCELLED",
      message: "Stopped. You can ask again.",
      retryable: true
    }
  }));
  assert.equal(popup.busy, false);
  assert.equal(popup.cancel.hidden, true);
  assert.equal(popup.retry.hidden, false);
  assert.equal(popup.activeElement, popup.input);

  popup.input.value = "edited after stop";
  popup.clickRetry();
  assert.equal(popup.ports.length, 2);
  assert.deepEqual(popup.ports[1].messages, [{ type: "ask", text: "Long answer" }]);
  assert.equal(popup.retry.hidden, true);
  assert.equal(popup.cancel.hidden, false);
  assert.equal(popup.activeElement, popup.input);
}

{
  // Retry re-reads the user's current context choice and consumes that exact
  // one-shot grant after success; revoked context is not resurrected.
  /** @type {any} */
  let currentContext = {
    mode: "page", text: "old page", truncated: false,
    page: { title: "Old", url: "https://example.com/" }
  };
  /** @type {any[]} */
  const consumed = [];
  const popup = openPopup({
    isPending: () => false,
    getContext: () => currentContext,
    consume: (/** @type {any} */ context) => consumed.push(context)
  });
  popup.ask("Use context");
  popup.ports[0].emitMessage(hostEvent("response.failed", {
    error: {
      code: "PROVIDER_FAILED", reason: "PROCESS_EXITED",
      message: "Try again.", retryable: true
    }
  }));
  currentContext = null;
  popup.clickRetry();
  assert.deepEqual(popup.ports[1].messages, [{ type: "ask", text: "Use context" }]);
  popup.ports[1].emitMessage(hostEvent("response.completed"));
  assert.deepEqual(consumed, [null]);

  currentContext = {
    mode: "selection", text: "new selection", truncated: false,
    page: { title: "New", url: "https://example.com/" }
  };
  popup.ask("Again");
  popup.ports[2].emitMessage(hostEvent("response.failed", {
    error: {
      code: "PROVIDER_FAILED", reason: "PROCESS_EXITED",
      message: "Try again.", retryable: true
    }
  }));
  popup.clickRetry();
  assert.equal(popup.ports[3].messages[0].context, currentContext);
  popup.ports[3].emitMessage(hostEvent("response.completed"));
  assert.equal(consumed.at(-1), currentContext);
}

{
  // A failed stop is non-terminal: Stop becomes actionable again and the
  // answer can continue without leaving the UI stuck in Stopping.
  const popup = openPopup();
  popup.ask("Keep running");
  popup.clickCancel();
  popup.ports[0].emitMessage(hostEvent("cancel.failed", {
    error: {
      code: "INVALID_REQUEST", reason: "UNKNOWN_TARGET_REQUEST",
      message: "Target is not running.", retryable: true
    }
  }));
  assert.equal(popup.cancel.getAttribute("aria-disabled"), null);
  assert.equal(popup.statusState, "pending");
  assert.equal(popup.statusText, "Couldn't stop. The answer is still running.");
  popup.clickCancel();
  assert.deepEqual(popup.ports[0].messages.map((item) => item.type), ["ask", "cancel", "cancel"]);
}

{
  // Provider text is rendered as text, never as markup.
  const popup = openPopup();
  popup.ask("Inject");
  const markup = '<img src=x onerror="alert(1)"><b>bold</b>';
  popup.ports[0].emitMessage(hostEvent("response.delta", { text: markup }));

  assert.equal(popup.answer.textContent, markup);
  assert.equal("innerHTML" in popup.answer, false);
}

{
  // The input stays editable while an answer streams, and the next question
  // starts a fresh answer on a new port.
  const popup = openPopup();
  popup.ask("First question");
  const first = popup.ports[0];
  first.emitMessage(hostEvent("response.delta", { text: "first answer" }));

  assert.equal(popup.input.disabled, false);
  assert.equal(popup.input.readOnly, false);

  first.emitMessage(hostEvent("response.completed"));
  popup.ask("Second question");

  assert.equal(popup.ports.length, 2);
  assert.deepEqual(popup.ports[1].messages, [
    { type: "ask", text: "Second question" }
  ]);
  assert.equal(popup.answer.textContent, "");
  assert.equal(popup.answer.hidden, true);

  // A late event on the first port cannot touch the second answer.
  first.emitMessage(hostEvent("response.delta", { text: "stale" }));
  popup.ports[1].emitMessage(hostEvent("response.delta", { text: "second" }));
  assert.equal(popup.answer.textContent, "second");
}

{
  // A failure renders its message, keeps any partial answer, and frees the
  // form for another question.
  const popup = openPopup();
  popup.ask("Fail after starting");
  const port = popup.ports[0];
  port.emitMessage(hostEvent("response.started", { provider_id: "fake" }));
  port.emitMessage(hostEvent("response.delta", { text: "Half an" }));
  port.emitMessage(
    hostEvent("response.failed", {
      error: {
        code: "PROVIDER_FAILED",
        reason: "PROCESS_EXITED",
        message: "The provider process exited unexpectedly.",
        retryable: true
      }
    })
  );

  assert.equal(popup.statusText, "The provider process exited unexpectedly.");
  assert.equal(popup.statusState, "failed");
  assert.equal(popup.statusKind, "provider-failed");
  assert.equal(popup.answer.textContent, "Half an");
  assert.equal(popup.busy, false);
  assert.equal(port.disconnectCalls, 1);

  popup.ask("Retry");
  assert.equal(popup.ports.length, 2);
}

{
  // A failure without a usable code or message gets a generic one, with the
  // request's ID for the host's diagnostics.
  for (const error of [undefined, {}, { message: "" }, { message: 7 }, { code: "NEW_CODE" }]) {
    const popup = openPopup();
    popup.ask("Unknown failure");
    popup.ports[0].emitMessage(hostEvent("response.failed", { error }));

    assert.equal(popup.statusText, "Something went wrong. Try again. Reference: req_popup");
    assert.equal(popup.statusState, "failed");
    assert.equal(popup.statusKind, "internal-error");
  }
}

{
  // EXT-04: each DOC-02 code shows as its own kind, decided by the code
  // alone. The six states the popup must tell apart all differ.
  assert.deepEqual(Object.keys(FAILURE_KINDS).sort(), [...DOC_02.error_codes].sort());
  /** @type {Map<string, string>} */
  const shown = new Map();
  for (const code of DOC_02.error_codes) {
    const popup = openPopup();
    popup.ask("Which failure?");
    popup.ports[0].emitMessage(
      hostEvent("response.failed", {
        error: { code, reason: "SOME_REASON", message: `Message for ${code}.`, retryable: false }
      })
    );
    shown.set(code, popup.statusKind ?? "");
    assert.equal(popup.statusState, code === "REQUEST_CANCELLED" ? "cancelled" : "failed", code);
    assert.ok(popup.statusText.startsWith(`Message for ${code}.`), code);
    assert.equal(popup.busy, false);
  }
  const states = [
    "HOST_UNAVAILABLE",
    "PROVIDER_NOT_FOUND",
    "PROVIDER_NOT_AUTHENTICATED",
    "PROVIDER_FAILED",
    "REQUEST_TIMEOUT",
    "REQUEST_CANCELLED"
  ].map((code) => shown.get(code));
  assert.equal(new Set(states).size, states.length, `${states}`);
  assert.equal(shown.get("HOST_NOT_INSTALLED"), "host-missing");
  assert.equal(shown.get("HOST_UNAVAILABLE"), "host-unavailable");
  assert.equal(shown.get("PROVIDER_NOT_FOUND"), "provider-missing");
  assert.equal(shown.get("PROVIDER_NOT_AUTHENTICATED"), "provider-signed-out");
  assert.equal(shown.get("PROVIDER_FAILED"), "provider-failed");
  assert.equal(shown.get("SEARCH_FAILED"), "search-failed");
  assert.equal(shown.get("REQUEST_TIMEOUT"), "timeout");
  assert.equal(shown.get("REQUEST_CANCELLED"), "cancelled");
}

{
  // DOC-02's example errors render with their own messages.
  for (const error of DOC_02.valid_errors) {
    const popup = openPopup();
    popup.ask("Fixture");
    popup.ports[0].emitMessage(hostEvent("response.failed", { error }));
    assert.equal(popup.statusKind, FAILURE_KINDS[error.code], error.code);
    assert.ok(popup.statusText.startsWith(error.message), error.code);
  }
}

{
  // Without a message, each kind says what to do in its own words, and none
  // of them talks about ports, hosts, or processes.
  for (const [code, kind] of Object.entries(FAILURE_KINDS)) {
    const popup = openPopup();
    popup.ask("No message");
    popup.ports[0].emitMessage(
      hostEvent("response.failed", { error: { code, reason: "SOME_REASON", retryable: true } })
    );
    assert.ok(popup.statusText.startsWith(KIND_MESSAGES[kind]), code);
    assert.ok(!/native|port|process|host|stdin|protocol/i.test(KIND_MESSAGES[kind]), kind);
  }
}

{
  // Only failures a person can't act on carry a reference: protocol
  // mismatches and internal errors, not a question that is too long.
  /** @type {[any, boolean][]} */
  const cases = [
    [{ code: "INVALID_REQUEST", reason: "UNKNOWN_METHOD", message: "Unsupported method.", retryable: false }, true],
    [{ code: "INVALID_REQUEST", reason: "REQUEST_TOO_LARGE", message: "Your question is too long.", retryable: false }, false],
    [{ code: "INVALID_REQUEST", reason: "PAGE_CONTEXT_UNSUPPORTED", message: "Choose No context.", retryable: false }, false],
    [{ code: "INTERNAL_ERROR", reason: "INTERNAL_STATE_ERROR", message: "The answer stopped.", retryable: false }, true],
    [{ code: "PROVIDER_FAILED", reason: "PROCESS_EXITED", message: "Codex stopped.", retryable: true }, false]
  ];
  for (const [error, referenced] of cases) {
    const popup = openPopup();
    popup.ask("Reference?");
    popup.ports[0].emitMessage(hostEvent("response.failed", { error }));
    assert.equal(popup.statusText, referenced ? `${error.message} Reference: req_popup` : error.message);
  }
}

{
  // A new question clears the last failure's kind; outcomes go to the
  // provider line.
  /** @type {{kind: string, message?: string}[]} */
  const outcomes = [];
  const popup = openPopup(undefined, outcomes);
  popup.ask("Signed out?");
  popup.ports[0].emitMessage(
    hostEvent("response.failed", {
      error: {
        code: "PROVIDER_NOT_AUTHENTICATED",
        reason: "LOGIN_REQUIRED",
        message: "Codex isn't signed in.",
        retryable: false
      }
    })
  );
  popup.ask("Again");
  assert.equal(popup.statusKind, null);
  assert.equal(popup.statusState, "pending");
  popup.ports[1].emitMessage(hostEvent("response.completed"));
  assert.deepEqual(outcomes, [
    { kind: "provider-signed-out", message: "Codex isn't signed in." },
    { kind: "completed" }
  ]);
}

{
  // Unknown events and malformed deltas are ignored.
  const popup = openPopup();
  popup.ask("Odd events");
  const port = popup.ports[0];
  port.emitMessage(hostEvent("response.source", { source_id: "s1", data: {} }));
  port.emitMessage(hostEvent("response.delta", { text: 42 }));
  port.emitMessage(hostEvent("response.delta"));
  port.emitMessage(null);

  assert.equal(popup.answer.textContent, "");
  assert.equal(popup.answer.hidden, true);
  assert.equal(popup.statusText, "Sending…");
  assert.equal(popup.busy, true);
}

{
  // Losing the service worker before a terminal event fails the question, and
  // the next question opens a new port.
  const popup = openPopup();
  popup.ask("Worker restarts");
  popup.ports[0].emitDisconnect();

  assert.equal(popup.statusText, WORKER_LOST);
  assert.equal(popup.statusState, "failed");
  assert.equal(popup.statusKind, "internal-error");
  assert.equal(popup.busy, false);

  popup.ask("Again");
  assert.equal(popup.ports.length, 2);
}

{
  // A worker that closes the port after the terminal event is not a failure.
  const popup = openPopup();
  popup.ask("Clean close");
  const port = popup.ports[0];
  port.emitMessage(hostEvent("response.completed"));
  port.emitDisconnect();

  assert.equal(popup.statusText, "Answer complete.");
  assert.equal(popup.statusState, "done");
}

{
  // If the extension context is gone, connecting throws; the popup says so and
  // stays usable.
  const popup = openPopup();
  popup.failConnect(new Error("Extension context invalidated."));
  popup.ask("No worker");

  assert.equal(popup.statusState, "failed");
  assert.equal(popup.busy, false);

  popup.failConnect(null);
  popup.ask("Worker back");
  assert.equal(popup.ports.length, 1);
  assert.equal(popup.busy, true);
}

{
  // If the first postMessage throws (in Chrome, for example, a question over
  // the 64 MiB message limit), the question fails instead of leaving the
  // popup busy, and the next question gets a new port.
  const popup = openPopup();
  popup.failPost(true);
  popup.ask("Too large to send");

  assert.equal(popup.ports.length, 1);
  assert.equal(popup.statusText, WORKER_LOST);
  assert.equal(popup.statusState, "failed");
  assert.equal(popup.busy, false);
  assert.equal(popup.answer.getAttribute("aria-busy"), "false");
  assert.equal(popup.ports[0].disconnectCalls, 1);

  popup.failPost(false);
  popup.ask("Small enough");
  assert.equal(popup.ports.length, 2);
  assert.deepEqual(popup.ports[1].messages, [{ type: "ask", text: "Small enough" }]);
  assert.equal(popup.busy, true);
}

{
  // A question the native host could never accept is not sent. The limit is
  // in UTF-8 bytes, so two-byte characters reach it at half the length.
  const popup = openPopup();
  const twoByte = String.fromCharCode(0xe9);
  for (const text of [
    "x".repeat(MAX_NATIVE_MESSAGE_BYTES + 1),
    twoByte.repeat(MAX_NATIVE_MESSAGE_BYTES / 2 + 1)
  ]) {
    popup.ask(text);
    assert.equal(popup.ports.length, 0);
    assert.equal(popup.statusText, "Your question is too long. Shorten it and try again.");
    assert.equal(popup.statusState, "failed");
    assert.equal(popup.busy, false);
  }

  // At the limit, the question goes to the service worker, which checks the
  // whole request.
  popup.ask("x".repeat(MAX_NATIVE_MESSAGE_BYTES));
  assert.equal(popup.ports.length, 1);
}

console.log("EXT-03/EXT-04 popup ask/stream and failure tests passed");
