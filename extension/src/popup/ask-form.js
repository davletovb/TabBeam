import { ASK_PORT_NAME, QUESTION_TOO_LONG } from "../shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES, utf8ByteLength } from "../shared/limits.js";
import { describeFailure } from "../shared/outcomes.js";

/** @typedef {import("../shared/ask-port.js").AskPort} AskPort */

export const READY_STATUS = "Press Enter to ask. Shift+Enter adds a line.";

/** When the extension's own service worker is gone before an answer ends. */
export const WORKER_LOST = "Pervue stopped unexpectedly. Reopen it, then try again.";

/**
 * @typedef {{
 *   form: HTMLFormElement,
 *   input: HTMLTextAreaElement,
 *   submit: HTMLButtonElement,
 *   status: HTMLElement,
 *   answer: HTMLElement
 * }} AskElements
 */

/**
 * How a question ended, for the provider line: `completed`, or a failure
 * kind from ../shared/outcomes.js with the message shown.
 *
 * @typedef {{kind: string, message?: string}} Outcome
 */

/**
 * Wires the ask form to the service worker. Each question opens its own
 * runtime port (../shared/ask-port.js) and renders that request's events as
 * they arrive. Only one question runs at a time; the input stays editable
 * while an answer streams.
 *
 * A failure shows its message, and the status line names its kind in
 * `data-kind` (EXT-04), so a missing companion app or provider, a sign-in, a
 * provider failure, a timeout, and a cancellation each look different. A
 * cancellation is not a failure: its state is `cancelled`.
 *
 * @param {AskElements} elements
 * @param {{connect(connectInfo: {name: string}): AskPort}} runtime
 * @param {{getContext(): any | null, isPending(): boolean}} [contextControls]
 * @param {{onOutcome?: (outcome: Outcome) => void}} [options]
 */
export function bindAskForm(elements, runtime, contextControls, options = {}) {
  const { form, input, submit, status, answer } = elements;
  const { onOutcome } = options;

  // The port of the question in flight, or null when idle.
  /** @type {AskPort | null} */
  let active = null;

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    ask();
  });

  input.addEventListener("keydown", (event) => {
    // Enter asks and Shift+Enter adds a line. An Enter that confirms an IME
    // composition does neither; some IMEs report that keypress only through
    // the legacy keyCode 229.
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.isComposing &&
      event.keyCode !== 229
    ) {
      event.preventDefault();
      form.requestSubmit();
    }
  });

  setStatus(READY_STATUS, "idle");
  input.focus();

  function ask() {
    // Duplicate-submit guard: a question in flight blocks every other submit
    // path (Enter, the button, requestSubmit), not just the button.
    if (active !== null) {
      return;
    }
    if (contextControls?.isPending()) {
      setStatus("Wait for context capture to finish.", "idle");
      return;
    }

    const text = input.value;
    if (text.trim() === "") {
      setStatus("Type a question first.", "idle");
      return;
    }
    // The native host could never accept this question, and Chrome refuses a
    // runtime message over 64 MiB outright, so don't send it.
    if (utf8ByteLength(text) > MAX_NATIVE_MESSAGE_BYTES) {
      setStatus(QUESTION_TOO_LONG.message, "failed", "invalid-request");
      return;
    }

    /** @type {AskPort} */
    let port;
    try {
      port = runtime.connect({ name: ASK_PORT_NAME });
    } catch {
      setStatus(WORKER_LOST, "failed", "internal-error");
      return;
    }

    active = port;
    answer.textContent = "";
    answer.hidden = true;
    setBusy(true);
    setStatus("Sending…", "pending");

    port.onMessage.addListener((/** @type {any} */ event) => {
      if (port === active) {
        render(port, event);
      }
    });
    port.onDisconnect.addListener(() => {
      if (port === active) {
        finish(port, WORKER_LOST, "failed", "internal-error");
      }
    });
    try {
      const context = contextControls?.getContext();
      port.postMessage(context ? { type: "ask", text, context } : { type: "ask", text });
    } catch {
      // No events will follow a question the port couldn't carry, such as
      // one over Chrome's 64 MiB message limit, so fail it now.
      finish(port, WORKER_LOST, "failed", "internal-error");
    }
  }

  /**
   * @param {AskPort} port
   * @param {any} event
   */
  function render(port, event) {
    switch (event?.event) {
      case "response.started":
        setStatus("Answering…", "pending");
        break;
      case "response.delta":
        if (typeof event.payload?.text === "string") {
          answer.hidden = false;
          // Text nodes only: provider output is never parsed as HTML.
          answer.append(event.payload.text);
        }
        break;
      case "response.completed":
        finish(port, "Answer complete.", "done");
        onOutcome?.({ kind: "completed" });
        break;
      case "response.failed": {
        const { kind, message, reference } = describeFailure(event.payload?.error);
        // A request ID lets the host's diagnostics be matched to this failure.
        const shown =
          reference && typeof event.request_id === "string"
            ? `${message} Reference: ${event.request_id}`
            : message;
        finish(port, shown, kind === "cancelled" ? "cancelled" : "failed", kind);
        onOutcome?.({ kind, message });
        break;
      }
      default:
        // conversation.created and response.source belong to later items.
        break;
    }
  }

  /**
   * @param {AskPort} port
   * @param {string} message
   * @param {string} state
   * @param {string | null} [kind] the failure kind, for a failure
   */
  function finish(port, message, state, kind = null) {
    active = null;
    port.disconnect();
    setBusy(false);
    setStatus(message, state, kind);
  }

  /** @param {boolean} busy */
  function setBusy(busy) {
    // aria-disabled rather than disabled, so a focused button keeps focus.
    submit.setAttribute("aria-disabled", String(busy));
    answer.setAttribute("aria-busy", String(busy));
  }

  /**
   * @param {string} message
   * @param {string} state
   * @param {string | null} [kind] the failure kind, for a failure
   */
  function setStatus(message, state, kind = null) {
    status.textContent = message;
    status.setAttribute("data-state", state);
    if (kind === null) {
      status.removeAttribute("data-kind");
    } else {
      status.setAttribute("data-kind", kind);
    }
  }
}
