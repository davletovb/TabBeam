import { ASK_PORT_NAME } from "../shared/ask-port.js";

/** @typedef {import("../shared/ask-port.js").AskPort} AskPort */

export const READY_STATUS = "Press Enter to ask. Shift+Enter adds a line.";

const GENERIC_FAILURE = "Something went wrong. Try again.";
const WORKER_LOST =
  "Pervue's background service stopped. Reopen the popup and try again.";

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
 * Wires the ask form to the service worker. Each question opens its own
 * runtime port (../shared/ask-port.js) and renders that request's events as
 * they arrive. Only one question runs at a time; the input stays editable
 * while an answer streams.
 *
 * @param {AskElements} elements
 * @param {{connect(connectInfo: {name: string}): AskPort}} runtime
 */
export function bindAskForm(elements, runtime) {
  const { form, input, submit, status, answer } = elements;

  // The port of the question in flight, or null when idle.
  /** @type {AskPort | null} */
  let active = null;

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    ask();
  });

  input.addEventListener("keydown", (event) => {
    // Enter asks and Shift+Enter adds a line. An Enter that confirms an IME
    // composition does neither.
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
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

    const text = input.value;
    if (text.trim() === "") {
      setStatus("Type a question first.", "idle");
      return;
    }

    /** @type {AskPort} */
    let port;
    try {
      port = runtime.connect({ name: ASK_PORT_NAME });
    } catch {
      setStatus(WORKER_LOST, "failed");
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
        finish(port, WORKER_LOST, "failed");
      }
    });
    port.postMessage({ type: "ask", text });
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
        break;
      case "response.failed":
        finish(port, failureMessage(event.payload?.error), "failed");
        break;
      default:
        // conversation.created and response.source belong to later items.
        break;
    }
  }

  /**
   * @param {AskPort} port
   * @param {string} message
   * @param {string} state
   */
  function finish(port, message, state) {
    active = null;
    port.disconnect();
    setBusy(false);
    setStatus(message, state);
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
   */
  function setStatus(message, state) {
    status.textContent = message;
    status.setAttribute("data-state", state);
  }
}

/** @param {any} error */
function failureMessage(error) {
  const message = error?.message;
  return typeof message === "string" && message.trim() !== ""
    ? message
    : GENERIC_FAILURE;
}
