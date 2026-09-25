import { DEFAULT_PROVIDER_ID, createRequestId, hostDisconnectError, isValidContext } from "./ask-bridge.js";
import { RequestTooLargeError } from "./native-connection.js";
import { dialogueHistory, CONVERSATION_ID_PATTERN } from "../shared/conversation-model.js";
import { ASK_TERMINAL_EVENTS, QUESTION_TOO_LONG } from "../shared/ask-port.js";

const FAILURE = Object.freeze({
  code: "INVALID_REQUEST", reason: "INVALID_PAYLOAD", message: "Type a question first.", retryable: false
});
const INVALID_CONTEXT = Object.freeze({
  code: "INVALID_REQUEST", reason: "INVALID_PAYLOAD",
  message: "The selected page context is invalid. Choose a source again or use No context.", retryable: false
});
const NOT_FOUND = Object.freeze({
  code: "INVALID_REQUEST", reason: "UNKNOWN_CONVERSATION",
  message: "Conversation not found. Start a new one.", retryable: false
});
const BUSY = Object.freeze({
  code: "INVALID_REQUEST", reason: "CONVERSATION_BUSY",
  message: "This conversation is answering another question. Try again when it finishes.", retryable: true
});
const STORAGE_FAILED = Object.freeze({
  code: "INTERNAL_ERROR", reason: "CONVERSATION_STORE_FAILED",
  message: "Conversation history couldn't be saved. Check browser storage and try again.", retryable: true
});
const HOST_START_FAILED = Object.freeze({
  code: "HOST_UNAVAILABLE", reason: "HOST_START_FAILED",
  message: "Pervue's companion app couldn't start. Try again.", retryable: true
});

/** @param {string | null} requestId @param {any} error */
function failed(requestId, error) {
  return { version: 1, type: "event", request_id: requestId, event: "response.failed", payload: { error } };
}

/**
 * Owns one question, including the extension's stable conversation ID and
 * the native host's opaque continuation ID. No page receives the latter.
 * @param {import("../shared/ask-port.js").AskPort} port
 * @param {{manager: {send(request: any, owner?: any): void}, store: any, inFlight: Set<string>, providerId?: string, createRequestId?: () => string}} options
 */
export function serveConversationAskPort(port, options) {
  const { manager, store, inFlight, providerId = DEFAULT_PROVIDER_ID } = options;
  const nextRequestId = options.createRequestId ?? createRequestId;
  let open = true;
  let asked = false;
  let finished = false;
  /** @type {string | null} */
  let conversationId = null;
  /** @type {string | null} */
  let assistantId = null;
  let answer = "";
  /** @type {any[]} */
  const sources = [];
  let work = Promise.resolve();

  port.onDisconnect.addListener(() => { open = false; });

  /** @param {any} event */
  function forward(event) {
    if (finished || !open) return;
    try {
      port.postMessage(event);
    } catch {
      open = false;
    }
    if (ASK_TERMINAL_EVENTS.has(event.event)) {
      finished = true;
      if (open) port.disconnect();
    }
  }

  /** @param {string | null} requestId @param {any} error */
  async function stop(requestId, error) {
    if (finished) return;
    if (conversationId && assistantId) {
      try { await store.finish(conversationId, assistantId, answer, sources, error); }
      catch { error = STORAGE_FAILED; }
    }
    if (conversationId) inFlight.delete(conversationId);
    forward(failed(requestId, error));
  }

  /** @param {any} event */
  async function onNativeEvent(event) {
    if (finished) return;
    if (event?.event === "conversation.created") {
      const providerSessionId = event.payload?.conversation_id;
      if (typeof providerSessionId !== "string" || !providerSessionId) throw new Error("invalid native session");
      if (conversationId) {
        await store.setSession(conversationId, providerSessionId);
      } else {
        const created = await store.create({ providerId, providerSessionId, text: question, context });
        conversationId = created.id;
        assistantId = created.assistantId;
        inFlight.add(created.id);
      }
      forward({ ...event, payload: { conversation_id: conversationId } });
      return;
    }
    if (event?.event === "response.started") {
      const payload = { ...event.payload };
      delete payload.conversation_id;
      forward({ ...event, payload: { ...payload, ...(conversationId ? { conversation_id: conversationId } : {}) } });
      return;
    }
    if (event?.event === "response.delta" && typeof event.payload?.text === "string") {
      answer += event.payload.text;
    }
    if (event?.event === "response.source" && typeof event.payload?.source_id === "string") {
      sources.push({ id: event.payload.source_id, data: event.payload.data });
    }
    if (ASK_TERMINAL_EVENTS.has(event?.event)) {
      if (conversationId && assistantId) {
        await store.finish(conversationId, assistantId, answer, sources,
          event.event === "response.failed" ? event.payload?.error ?? HOST_START_FAILED : undefined);
        inFlight.delete(conversationId);
      }
      forward(event);
      return;
    }
    forward(event);
  }

  let question = "";
  /** @type {any} */
  let context;

  port.onMessage.addListener((/** @type {any} */ message) => {
    if (asked) return;
    asked = true;
    const text = message?.type === "ask" ? message.text : undefined;
    if (typeof text !== "string" || !text.trim()) {
      forward(failed(null, FAILURE));
      return;
    }
    question = text;
    context = message.context;
    if (context !== undefined && !isValidContext(context)) {
      forward(failed(null, INVALID_CONTEXT));
      return;
    }
    const requestedId = message.conversation_id;
    if (requestedId !== undefined &&
        (typeof requestedId !== "string" || !CONVERSATION_ID_PATTERN.test(requestedId))) {
      forward(failed(null, NOT_FOUND));
      return;
    }
    if (requestedId && inFlight.has(requestedId)) {
      forward(failed(null, BUSY));
      return;
    }

    void (async () => {
      const requestId = nextRequestId();
      let sending = false;
      try {
        let sessionId;
        /** @type {{role: string, text: string}[]} */
        let history = [];
        let selectedProvider = providerId;
        if (requestedId) {
          const stored = await store.getPrivate(requestedId);
          if (inFlight.has(requestedId)) {
            forward(failed(requestId, BUSY));
            return;
          }
          conversationId = requestedId;
          selectedProvider = stored.provider_id;
          sessionId = stored.provider_session_id;
          history = dialogueHistory(stored);
          inFlight.add(requestedId);
          ({ assistantId } = await store.begin(requestedId, question, context));
        }
        const request = {
          version: 1, type: "request", request_id: requestId, method: "conversation.send",
          payload: {
            provider_id: selectedProvider,
            ...(sessionId ? { conversation_id: sessionId } : {}),
            input: { text: question, ...(history.length ? { history } : {}) },
            ...(context === undefined ? {} : { context: {
              mode: context.mode, text: context.text, truncated: context.truncated,
              page: { title: context.page.title, url: context.page.url }
            } })
          }
        };
        sending = true;
        manager.send(request, {
          onEvent(/** @type {any} */ event) {
            work = work.then(() => onNativeEvent(event)).catch(() => stop(requestId, STORAGE_FAILED));
          },
          onDisconnect(/** @type {{message: string | null}} */ { message: reason }) {
            work = work.then(() => stop(requestId, hostDisconnectError(reason)));
          }
        });
      } catch (error) {
        const reason = error instanceof RequestTooLargeError ? QUESTION_TOO_LONG
          : error instanceof Error && error.message === "Conversation not found." ? NOT_FOUND
            : sending ? HOST_START_FAILED : STORAGE_FAILED;
        await stop(requestId, reason);
      }
    })();
  });
}
