import { ASK_PORT_NAME, QUESTION_TOO_LONG } from "../shared/ask-port.js";
import { MAX_NATIVE_MESSAGE_BYTES, utf8ByteLength } from "../shared/limits.js";
import { copyWithFeedback } from "../shared/clipboard.js";
import { describeFailure } from "../shared/outcomes.js";
import { recordDuration } from "../shared/performance.js";
import { createSourceSet, sourceFromEvent } from "../shared/sources.js";

/** @typedef {import("../shared/ask-port.js").AskPort} AskPort */

export const READY_STATUS = "Press Enter to ask. Shift+Enter adds a line.";

/** When the conversation on screen was deleted, here or in another view. */
export const DELETED_STATUS = "This conversation was deleted. Ask something to start a new one.";

/** When the extension's own service worker is gone before an answer ends. */
export const WORKER_LOST = "TabBeam stopped unexpectedly. Reopen it, then try again.";

/**
 * @typedef {{
 *   form: HTMLFormElement,
 *   input: HTMLTextAreaElement,
 *   submit: HTMLButtonElement,
 *   status: HTMLElement,
 *   answer: HTMLElement,
 *   history?: HTMLElement,
 *   sources?: HTMLElement,
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
 * cancellation is not a failure: its state is `cancelled`. Feedback on
 * something the person just did (an empty question, a capture still running,
 * a conversation deleted from under them) is `notice`; `idle` is only the
 * resting hint.
 *
 * A question can ask the provider to search the web (`getSearch`, EXT-16).
 * The sources a search turn finds are shown in `sources` as they arrive and
 * with the saved answer afterwards (`renderSources`), checked again here
 * because they're untrusted (SEC-05). A retry searches again only if the
 * question it repeats did.
 *
 * @param {AskElements} elements
 * @param {{connect(connectInfo: {name: string}): AskPort, sendMessage?(message: any): Promise<any>}} runtime
 * @param {{getContext(): any | null, isPending(): boolean, consume?(context: any): void}} [contextControls]
 * @param {{
 *   onOutcome?: (outcome: Outcome) => void,
 *   onConversationId?(id: string | null, providerId?: string): void,
 *   onConversationLoaded?(conversation: any | null): void,
 *   getProviderId?(): string,
 *   getModel?(providerId: string | undefined): string | undefined,
 *   getSearch?(): boolean,
 *   renderSources?(container: HTMLElement, sources: import("../shared/sources.js").Source[]): void,
 *   onSaved?(): void,
 *   onRequestStarted?(): void,
 *   onRequestEnded?(): void,
 *   renderMessage?(body: HTMLElement, text: string): void,
 *   renderAnswer?(answer: HTMLElement, text: string): Promise<void>,
 *   storageChanges?: {addListener(callback: (changes: any, area: string) => void): void}
 * }} [options]
 */
export function bindAskForm(elements, runtime, contextControls, options = {}) {
  const { form, input, submit, status, answer } = elements;
  const { onOutcome } = options;
  const history = elements.history;
  const cancel = elements.cancel;
  const retry = elements.retry;
  const liveSources = elements.sources;
  // The sources of the answer in flight.
  const sources = createSourceSet();
  let submittedSearch = false;
  /** @type {string | null} */
  let conversationId = null;
  let viewGeneration = 0;
  let loadPending = false;
  let submittedText = "";
  /** The provider the question in flight was sent to. @type {string | undefined} */
  let submittedProvider;
  /** @type {any} */
  let submittedContext = null;
  /**
   * A question as it was asked. Retry repeats its search mode. A search
   * question is retried without page context, as it was asked (the two never
   * combine); any other question re-reads the current context choice, so
   * context removed since is never sent again.
   * @typedef {{text: string, conversationId: string | null, search: boolean}} Attempt
   */
  /** @type {Attempt | null} */
  let lastAttempt = null;
  let requestStartedAt = 0;
  let firstChunkRecorded = false;

  // The answer so far, and its reveal, when a renderer types it out.
  let answerText = "";
  /** @type {Promise<void> | null} */
  let reveal = null;
  let revealing = false;

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

  function clearAnswer() {
    answer.textContent = "";
    answer.hidden = true;
    answerText = "";
    reveal = options.renderAnswer ? options.renderAnswer(answer, "") : null;
    sources.clear();
    if (liveSources) {
      liveSources.replaceChildren();
      liveSources.hidden = true;
    }
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
      const result = await runtime.sendMessage?.({ type: "tabbeam.conversations.get", conversation_id: id });
      if (generation !== viewGeneration || active) return false;
      if (result?.ok !== true || !result.value) throw new Error("unavailable");
      const nextConversationId = result.value.id;
      if (!preserveRetry || conversationId !== nextConversationId) clearRetry();
      conversationId = nextConversationId;
      options.onConversationLoaded?.(result.value);
      renderHistory(result.value.messages);
      clearAnswer();
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
    const change = area === "local" && conversationId ? changes[`tabbeam.conversation.${conversationId}`] : undefined;
    if (!change || active) return;
    if (change.newValue === undefined) {
      // Deleted, here or in another view: there's nothing left to show or to
      // follow up on.
      reset(DELETED_STATUS, "notice");
    } else if (!loadPending && !revealing) {
      void loadConversation(conversationId ?? "", true);
    }
  });

  /** @param {any[]} messages */
  function renderHistory(messages) {
    if (!history) return;
    history.replaceChildren(...messages.map((message) => {
      const item = bubble(message.role, message.text, message.status, message.sources);
      if (message.role === "user" && message.search === true) item.setAttribute("data-search", "true");
      return item;
    }));
  }

  /**
   * @param {string} role @param {string} text @param {string} state
   * @param {unknown} [saved] a complete answer's sources
   */
  function bubble(role, text, state, saved) {
    const owner = history?.ownerDocument ?? document;
    const item = owner.createElement("article");
    item.className = `message message-${role}`;
    item.setAttribute("data-state", state);
    const label = owner.createElement("strong");
    label.textContent = role === "user" ? "You" : "Beam";
    const body = owner.createElement("div");
    body.className = "message-body";
    if (role === "assistant" && text && options.renderMessage) {
      body.className = "message-body markdown";
      options.renderMessage(body, text);
    } else {
      body.textContent = text || (state === "pending" ? "Answering…" : "No answer.");
    }
    item.append(label, body);
    if (role === "assistant" && state === "complete" && Array.isArray(saved) && saved.length && options.renderSources) {
      const list = owner.createElement("section");
      list.className = "sources message-sources";
      options.renderSources(list, /** @type {any[]} */ (saved));
      if (!list.hidden) item.append(list);
    }
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
    copy.addEventListener("click", () => { void copyWithFeedback(copy, text); });
    actions.append(copy);
    return actions;
  }

  function newConversation() {
    if (active) return false;
    reset(READY_STATUS, "idle");
    input.focus();
    return true;
  }

  /** Leaves the conversation for a new one. @param {string} message @param {string} state */
  function reset(message, state) {
    ++viewGeneration;
    loadPending = false;
    revealing = false;
    conversationId = null;
    clearRetry();
    history?.replaceChildren();
    clearAnswer();
    options.onConversationId?.(null);
    options.onConversationLoaded?.(null);
    setStatus(message, state);
    hideControl(cancel);
    hideControl(retry);
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

  /** @param {Attempt | null} [attempt] */
  function ask(attempt = null) {
    // Duplicate-submit guard: a question in flight blocks every other submit
    // path (Enter, the button, requestSubmit), not just the button.
    if (active !== null || loadPending) {
      return;
    }
    // A search retry doesn't use what's being captured now.
    if (!attempt?.search && contextControls?.isPending()) {
      setStatus("Wait for context capture to finish.", "notice");
      return;
    }

    const text = attempt?.text ?? input.value;
    if (text.trim() === "") {
      setStatus("Type a question first.", "notice");
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
    submittedProvider = options.getProviderId?.();
    const submittedModel = options.getModel?.(submittedProvider);
    // A retry repeats the question's search mode (see Attempt).
    submittedSearch = attempt ? attempt.search : options.getSearch?.() === true;
    submittedContext = attempt?.search ? null : contextControls?.getContext();
    lastAttempt = { text, conversationId, search: submittedSearch };
    requestStartedAt = globalThis.performance?.now?.() ?? 0;
    firstChunkRecorded = false;
    options.onRequestStarted?.();
    // An answer still typing out when the next question is asked would
    // vanish with the live answer: keep it in the thread as a finished turn.
    if (history && revealing && answerText) {
      history.append(bubble("assistant", answerText, "complete", sources.list()));
    }
    if (history && !attempt) {
      const item = bubble("user", text, "pending");
      if (submittedSearch) item.setAttribute("data-search", "true");
      history.append(item);
    }
    clearAnswer();
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
        ...(submittedProvider ? { provider_id: submittedProvider } : {}),
        ...(submittedModel ? { model: submittedModel } : {}),
        ...(history && conversationId ? { conversation_id: conversationId } : {}),
        ...(attempt && conversationId ? { retry: true } : {}),
        ...(context ? { context } : {}),
        ...(submittedSearch ? { search: true } : {})
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
          options.onConversationId?.(conversationId, submittedProvider);
        }
        break;
      case "response.started":
        setStatus(submittedSearch ? "Searching the web…" : "Answering…", "pending");
        break;
      case "response.source": {
        const source = sourceFromEvent(event.payload);
        if (sources.add(source) && liveSources && options.renderSources) {
          options.renderSources(liveSources, sources.list());
        }
        break;
      }
      case "response.delta":
        if (typeof event.payload?.text === "string") {
          if (!firstChunkRecorded) {
            firstChunkRecorded = true;
            const endedAt = globalThis.performance?.now?.() ?? requestStartedAt;
            recordDuration("first_response_chunk", requestStartedAt, endedAt);
            if (submittedSearch) setStatus("Answering…", "pending");
          }
          answer.hidden = false;
          if (options.renderAnswer) {
            answerText += event.payload.text;
            reveal = options.renderAnswer(answer, answerText);
          } else {
            // Text nodes only: provider output is never parsed as HTML.
            answer.append(event.payload.text);
          }
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
    options.onRequestEnded?.();
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
      const showSaved = () => {
        void loadConversation(conversationId ?? "", state !== "done", retryable).then((loaded) => {
          if (loaded) options.onSaved?.();
        });
      };
      if (state === "done" && reveal) {
        // Let the answer finish typing out before the saved copy replaces
        // it, unless the person has moved on by then.
        const generation = viewGeneration;
        revealing = true;
        answer.setAttribute("aria-busy", "true");
        void reveal.then(() => {
          revealing = false;
          if (generation !== viewGeneration) return;
          answer.setAttribute("aria-busy", "false");
          showSaved();
        });
      } else {
        showSaved();
      }
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
