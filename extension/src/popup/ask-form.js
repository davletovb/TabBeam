import { ASK_PORT_NAME, QUESTION_TOO_LONG } from "../shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES, utf8ByteLength } from "../shared/limits.js";
import { describeFailure } from "../shared/outcomes.js";
import { recordDuration } from "../shared/performance.js";

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
 *   answer: HTMLElement,
 *   history?: HTMLElement,
 *   cancel?: HTMLButtonElement,
 *   retry?: HTMLButtonElement
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
 * @param {{connect(connectInfo: {name: string}): AskPort, sendMessage?(message: any): Promise<any>}} runtime
 * @param {{getContext(): any | null, isPending(): boolean, consume?(context: any): void}} [contextControls]
 * @param {{
 *   onOutcome?: (outcome: Outcome) => void,
 *   onConversationId?(id: string | null): void,
 *   onSaved?(): void,
 *   onRequestStarted?(): void,
 *   storageChanges?: {addListener(callback: (changes: any, area: string) => void): void}
 * }} [options]
 */
export function bindAskForm(elements, runtime, contextControls, options = {}) {
  const { form, input, submit, status, answer } = elements;
  const { onOutcome } = options;
  const history = elements.history;
  const cancel = elements.cancel;
  const retry = elements.retry;
  /** @type {string | null} */
  let conversationId = null;
  let viewGeneration = 0;
  let loadPending = false;
  let submittedText = "";
  /** @type {any} */
  let submittedContext = null;
  /** @type {{text: string, conversationId: string | null} | null} */
  let lastAttempt = null;
  let requestStartedAt = 0;
  let firstChunkRecorded = false;

  // The port of the question in flight, or null when idle.
  /** @type {AskPort | null} */
  let active = null;

  /** @param {HTMLButtonElement | undefined} control */
  function hideControl(control) {
    if (!control) return;
    if (control.ownerDocument?.activeElement === control) input.focus();
    control.hidden = true;
    control.removeAttribute("aria-disabled");
  }

  function clearRetry() {
    lastAttempt = null;
    hideControl(retry);
  }

  /** @param {string} id @param {boolean} [preserveStatus] @param {boolean} [preserveRetry] */
  async function loadConversation(id, preserveStatus = false, preserveRetry = false) {
    if (!history || typeof runtime.sendMessage !== "function" || active) return false;
    const generation = ++viewGeneration;
    loadPending = true;
    try {
      const result = await runtime.sendMessage?.({ type: "pervue.conversations.get", conversation_id: id });
      if (generation !== viewGeneration || active) return false;
      if (result?.ok !== true || !result.value) throw new Error("unavailable");
      const nextConversationId = result.value.id;
      if (!preserveRetry || conversationId !== nextConversationId) clearRetry();
      conversationId = nextConversationId;
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
    if (role === "assistant" && state === "complete" && text) item.append(copyAction(owner, text));
    return item;
  }

  /** @param {any} owner the document @param {string} text */
  function copyAction(owner, text) {
    const actions = owner.createElement("div");
    actions.className = "message-actions";
    const copy = owner.createElement("button");
    copy.className = "message-copy";
    copy.setAttribute("type", "button");
    copy.textContent = "Copy";
    copy.addEventListener("click", async () => {
      try {
        await globalThis.navigator?.clipboard?.writeText(text);
        copy.textContent = "Copied";
        copy.setAttribute("data-copied", "true");
        setTimeout(() => {
          copy.textContent = "Copy";
          copy.removeAttribute("data-copied");
        }, 1500);
      } catch {
        copy.textContent = "Couldn't copy";
      }
    });
    actions.append(copy);
    return actions;
  }

  function newConversation() {
    if (active) return false;
    ++viewGeneration;
    loadPending = false;
    conversationId = null;
    clearRetry();
    history?.replaceChildren();
    answer.textContent = "";
    answer.hidden = true;
    options.onConversationId?.(null);
    setStatus(READY_STATUS, "idle");
    hideControl(cancel);
    hideControl(retry);
    input.focus();
    return true;
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    ask();
  });

  cancel?.addEventListener("click", () => {
    if (!active || cancel.getAttribute("aria-disabled") === "true") return;
    try {
      active.postMessage({ type: "cancel" });
      cancel.setAttribute("aria-disabled", "true");
      setStatus("Stopping…", "cancelled", "cancelled");
    } catch {
      finish(active, WORKER_LOST, "failed", "internal-error", true, true);
    }
  });

  retry?.addEventListener("click", () => {
    if (!lastAttempt || active || loadPending) return;
    if (lastAttempt.conversationId !== conversationId) {
      clearRetry();
      return;
    }
    ask(lastAttempt);
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
  hideControl(cancel);
  hideControl(retry);
  input.focus();

  /** @param {{text: string, conversationId: string | null} | null} [attempt] */
  function ask(attempt = null) {
    // Duplicate-submit guard: a question in flight blocks every other submit
    // path (Enter, the button, requestSubmit), not just the button.
    if (active !== null || loadPending) {
      return;
    }
    if (contextControls?.isPending()) {
      setStatus("Wait for context capture to finish.", "idle");
      return;
    }

    const text = attempt?.text ?? input.value;
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
    ++viewGeneration;
    submittedText = text;
    submittedContext = contextControls?.getContext();
    lastAttempt = { text, conversationId };
    requestStartedAt = globalThis.performance?.now?.() ?? 0;
    firstChunkRecorded = false;
    options.onRequestStarted?.();
    if (history && !attempt) history.append(bubble("user", text, "pending"));
    answer.textContent = "";
    answer.hidden = true;
    setBusy(true);
    if (cancel) {
      cancel.hidden = false;
      cancel.removeAttribute("aria-disabled");
    }
    hideControl(retry);
    setStatus("Sending…", "pending");

    port.onMessage.addListener((/** @type {any} */ event) => {
      if (port === active) {
        render(port, event);
      }
    });
    port.onDisconnect.addListener(() => {
      if (port === active) {
        finish(port, WORKER_LOST, "failed", "internal-error", true, true);
      }
    });
    try {
      const context = submittedContext;
      port.postMessage({
        type: "ask", text,
        ...(history && conversationId ? { conversation_id: conversationId } : {}),
        ...(attempt && conversationId ? { retry: true } : {}),
        ...(context ? { context } : {})
      });
    } catch {
      // No events will follow a question the port couldn't carry, such as
      // one over Chrome's 64 MiB message limit, so fail it now.
      finish(port, WORKER_LOST, "failed", "internal-error", true, true);
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
          if (lastAttempt) lastAttempt.conversationId = conversationId;
          options.onConversationId?.(conversationId);
        }
        break;
      case "response.started":
        setStatus("Answering…", "pending");
        break;
      case "response.delta":
        if (typeof event.payload?.text === "string") {
          if (!firstChunkRecorded) {
            firstChunkRecorded = true;
            const endedAt = globalThis.performance?.now?.() ?? requestStartedAt;
            recordDuration("first_response_chunk", requestStartedAt, endedAt);
          }
          answer.hidden = false;
          // Text nodes only: provider output is never parsed as HTML.
          answer.append(event.payload.text);
        }
        break;
      case "response.completed":
        finish(port, "Answer complete.", "done", null, true, false);
        if (history && input.value === submittedText) input.value = "";
        contextControls?.consume?.(submittedContext);
        onOutcome?.({ kind: "completed" });
        break;
      case "cancel.failed":
        if (cancel) cancel.removeAttribute("aria-disabled");
        setStatus("Couldn't stop. The answer is still running.", "pending");
        break;
      case "response.failed": {
        const { kind, message, reference } = describeFailure(event.payload?.error);
        // A request ID lets the host's diagnostics be matched to this failure.
        const shown =
          reference && typeof event.request_id === "string"
            ? `${message} Reference: ${event.request_id}`
            : message;
        // A conversation the store no longer has can't be reloaded: keep the
        // message that says to start a new one.
        const reload = event.payload?.error?.reason !== "UNKNOWN_CONVERSATION";
        finish(
          port,
          shown,
          kind === "cancelled" ? "cancelled" : "failed",
          kind,
          reload,
          event.payload?.error?.retryable === true
        );
        onOutcome?.({ kind, message });
        break;
      }
      default:
        // Sources are persisted by the background worker for later display.
        break;
    }
  }

  /**
   * @param {AskPort} port
   * @param {string} message
   * @param {string} state
   * @param {string | null} [kind] the failure kind, for a failure
   * @param {boolean} [reload] whether to show the saved conversation again
   */
  function finish(port, message, state, kind = null, reload = true, retryable = false) {
    active = null;
    port.disconnect();
    setBusy(false);
    hideControl(cancel);
    if (retry) {
      if (retryable) {
        retry.hidden = false;
        retry.removeAttribute("aria-disabled");
      } else {
        hideControl(retry);
        lastAttempt = null;
      }
    }
    setStatus(message, state, kind);
    // A question that ended without an answer, failed or stopped, before its
    // conversation existed leaves nothing saved: drop its provisional bubble.
    if (history && !conversationId && state !== "done") history.replaceChildren();
    if (history && conversationId && reload) {
      void loadConversation(conversationId, state !== "done", retryable).then((loaded) => {
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

  return { getConversationId: () => conversationId, loadConversation, newConversation };
}
