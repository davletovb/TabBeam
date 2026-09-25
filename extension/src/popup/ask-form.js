import { ASK_PORT_NAME, QUESTION_TOO_LONG } from "../shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES, utf8ByteLength } from "../shared/limits.js";

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
 *   answer: HTMLElement,
 *   history?: HTMLElement
 * }} AskElements
 */

/**
 * Wires the ask form to the service worker. Each question opens its own
 * runtime port (../shared/ask-port.js) and renders that request's events as
 * they arrive. Only one question runs at a time; the input stays editable
 * while an answer streams.
 *
 * @param {AskElements} elements
 * @param {{connect(connectInfo: {name: string}): AskPort, sendMessage?(message: any): Promise<any>}} runtime
 * @param {{getContext(): any | null, isPending(): boolean, consume?(context: any): void}} [contextControls]
 * @param {{onConversationId?(id: string | null): void, onSaved?(): void, onRequestStarted?(): void, storageChanges?: {addListener(callback: (changes: any, area: string) => void): void}}} [options]
 */
export function bindAskForm(elements, runtime, contextControls, options = {}) {
  const { form, input, submit, status, answer } = elements;
  const history = elements.history;
  /** @type {string | null} */
  let conversationId = null;
  let viewGeneration = 0;
  let loadPending = false;
  let submittedText = "";
  /** @type {any} */
  let submittedContext = null;

  // The port of the question in flight, or null when idle.
  /** @type {AskPort | null} */
  let active = null;

  /** @param {string} id */
  async function loadConversation(id, preserveStatus = false) {
    if (!history || typeof runtime.sendMessage !== "function" || active) return false;
    const generation = ++viewGeneration;
    loadPending = true;
    try {
      const result = await runtime.sendMessage?.({ type: "pervue.conversations.get", conversation_id: id });
      if (generation !== viewGeneration || active) return false;
      if (result?.ok !== true || !result.value) throw new Error("unavailable");
      conversationId = result.value.id;
      renderHistory(result.value.messages);
      answer.textContent = "";
      answer.hidden = true;
      options.onConversationId?.(conversationId);
      return true;
    } catch {
      if (generation === viewGeneration && !preserveStatus) setStatus("Conversation history unavailable.", "failed");
      return false;
    } finally {
      if (generation === viewGeneration) loadPending = false;
    }
  }

  options.storageChanges?.addListener((changes, area) => {
    if (area === "local" && conversationId && !active && !loadPending &&
        changes[`pervue.conversation.${conversationId}`]) {
      void loadConversation(conversationId, true);
    }
  });

  /** @param {any[]} messages */
  function renderHistory(messages) {
    if (!history) return;
    history.replaceChildren(...messages.map((message) => bubble(message.role, message.text, message.status)));
  }

  /** @param {string} role @param {string} text @param {string} state */
  function bubble(role, text, state) {
    const owner = history?.ownerDocument ?? document;
    const item = owner.createElement("article");
    item.className = `message message-${role}`;
    item.setAttribute("data-state", state);
    const label = owner.createElement("strong");
    label.textContent = role === "user" ? "You" : "Pervue";
    const body = owner.createElement("p");
    body.textContent = text || (state === "pending" ? "Answering…" : "No answer.");
    item.append(label, body);
    return item;
  }

  function newConversation() {
    if (active) return false;
    ++viewGeneration;
    loadPending = false;
    conversationId = null;
    history?.replaceChildren();
    answer.textContent = "";
    answer.hidden = true;
    options.onConversationId?.(null);
    setStatus(READY_STATUS, "idle");
    input.focus();
    return true;
  }

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
    if (active !== null || loadPending) {
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
      setStatus(QUESTION_TOO_LONG.message, "failed");
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
    ++viewGeneration;
    submittedText = text;
    submittedContext = contextControls?.getContext();
    options.onRequestStarted?.();
    if (history) history.append(bubble("user", text, "pending"));
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
    try {
      const context = submittedContext;
      port.postMessage({
        type: "ask", text,
        ...(history && conversationId ? { conversation_id: conversationId } : {}),
        ...(context ? { context } : {})
      });
    } catch {
      // No events will follow a question the port couldn't carry, such as
      // one over Chrome's 64 MiB message limit, so fail it now.
      finish(port, WORKER_LOST, "failed");
    }
  }

  /**
   * @param {AskPort} port
   * @param {any} event
   */
  function render(port, event) {
    switch (event?.event) {
      case "conversation.created":
        if (history && typeof event.payload?.conversation_id === "string") {
          conversationId = event.payload.conversation_id;
          options.onConversationId?.(conversationId);
        }
        break;
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
        if (history && input.value === submittedText) input.value = "";
        contextControls?.consume?.(submittedContext);
        break;
      case "response.failed":
        finish(port, failureMessage(event.payload?.error), "failed",
          event.payload?.error?.reason === "UNKNOWN_CONVERSATION");
        break;
      default:
        // Sources are persisted by the background worker for later display.
        break;
    }
  }

  /**
   * @param {AskPort} port
   * @param {string} message
   * @param {string} state
   */
  function finish(port, message, state, skipReload = false) {
    active = null;
    port.disconnect();
    setBusy(false);
    setStatus(message, state);
    if (history && !conversationId && state === "failed") history.replaceChildren();
    if (history && conversationId && !skipReload) {
      void loadConversation(conversationId, state === "failed").then((loaded) => {
        if (loaded) options.onSaved?.();
      });
    }
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

  return { getConversationId: () => conversationId, loadConversation, newConversation };
}

/** @param {any} error */
function failureMessage(error) {
  const message = error?.message;
  return typeof message === "string" && message.trim() !== ""
    ? message
    : GENERIC_FAILURE;
}
