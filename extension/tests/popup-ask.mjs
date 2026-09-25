import assert from "node:assert/strict";
import { READY_STATUS, bindAskForm } from "../src/popup/ask-form.js";
import { ASK_PORT_NAME } from "../src/shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES } from "../src/shared/limits.js";
import { MockPort } from "./support/mock-port.mjs";

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

/** @param {{getContext(): any | null, isPending(): boolean}} [contextControls] */
function openPopup(contextControls) {
  const elements = {
    form: new FakeForm(),
    input: new FakeTextArea(),
    submit: new FakeElement(),
    status: new FakeElement(),
    answer: new FakeElement()
  };
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

  bindAskForm(/** @type {any} */ (elements), runtime, contextControls);

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
    get statusText() {
      return elements.status.textContent;
    },
    get statusState() {
      return elements.status.getAttribute("data-state");
    },
    get busy() {
      return elements.submit.getAttribute("aria-disabled") === "true";
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
  const keydown = popup.ask("What is Pervue?");

  assert.equal(keydown.defaultPrevented, true);
  assert.deepEqual(popup.connectInfos, [{ name: ASK_PORT_NAME }]);
  assert.deepEqual(popup.ports[0].messages, [
    { type: "ask", text: "What is Pervue?" }
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
  assert.equal(popup.answer.textContent, "Half an");
  assert.equal(popup.busy, false);
  assert.equal(port.disconnectCalls, 1);

  popup.ask("Retry");
  assert.equal(popup.ports.length, 2);
}

{
  // A failure without a usable message gets a generic one.
  for (const error of [undefined, {}, { message: "" }, { message: 7 }]) {
    const popup = openPopup();
    popup.ask("Unknown failure");
    popup.ports[0].emitMessage(hostEvent("response.failed", { error }));

    assert.equal(popup.statusText, "Something went wrong. Try again.");
    assert.equal(popup.statusState, "failed");
  }
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

  assert.equal(
    popup.statusText,
    "Pervue's background service stopped. Reopen the popup and try again."
  );
  assert.equal(popup.statusState, "failed");
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
  assert.equal(
    popup.statusText,
    "Pervue's background service stopped. Reopen the popup and try again."
  );
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

console.log("EXT-03 popup ask/stream tests passed");
