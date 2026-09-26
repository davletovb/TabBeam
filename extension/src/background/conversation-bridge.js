import {
  EMPTY_QUESTION, INVALID_CONTEXT, HOST_START_FAILED,
  copyContext, createRequestId, failed, hostDisconnectError, isValidContext
} from "./ask-bridge.js";
import { DEFAULT_PROVIDER_ID } from "../shared/providers.js";
import { RequestTooLargeError } from "./native-connection.js";
import { dialogueHistory, CONVERSATION_ID_PATTERN } from "../shared/conversation-model.js";
import { ASK_TERMINAL_EVENTS, QUESTION_TOO_LONG } from "../shared/ask-port.js";

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
const USER_CANCELLED = Object.freeze({
  code: "REQUEST_CANCELLED", reason: "USER_CANCELLED",
  message: "Stopped. You can ask again.", retryable: true
});

/**
 * Owns one question, including the extension's stable conversation ID and
 * the native host's opaque continuation ID. No page receives the latter.
 * @param {import("../shared/ask-port.js").AskPort} port
 * @param {{manager: {send(request: any, owner?: any): void}, store: any, inFlight: Set<string>, providerId?: string, createRequestId?: () => string, onFailure?: (error: any) => void}} options
 */
export function serveConversationAskPort(port, options) {
  const { manager, store, inFlight, providerId = DEFAULT_PROVIDER_ID } = options;
  const nextRequestId = options.createRequestId ?? createRequestId;
  let open = true;
  let asked = false;
  let finished = false;
  let stopped = false;
  let nativePending = false;
  let cancelRequested = false;
  let cancelSent = false;
  /** @type {string | null} */
  let activeRequestId = null;
  /** @type {string | null} */
  let conversationId = null;
  /** @type {string | null} */
  let assistantId = null;
  let answer = "";
  /** @type {any[]} */
  const sources = [];
  let work = Promise.resolve();

  port.onDisconnect.addListener(() => { open = false; });

  /** @param {any} error */
  function cancelFailed(error) {
    cancelSent = false;
    if (!open || finished) return;
    try {
      port.postMessage({
        version: 1,
        type: "event",
        request_id: activeRequestId,
        event: "cancel.failed",
        payload: { error }
      });
    } catch {
      open = false;
    }
  }

  /** @param {string} targetRequestId */
  function sendCancel(targetRequestId) {
    if (cancelSent || stopped || finished) return;
    cancelSent = true;
    try {
      manager.send({
        version: 1,
        type: "request",
        request_id: nextRequestId(),
        method: "request.cancel",
        payload: { target_request_id: targetRequestId }
      }, {
        onEvent(event) {
          if (event?.event === "response.failed") {
            cancelFailed(event.payload?.error ?? {
              code: "INVALID_REQUEST",
              reason: "UNKNOWN_TARGET_REQUEST",
              message: "The request could not be stopped.",
              retryable: true
            });
          }
        },
        onDisconnect() {
          cancelFailed({
            code: "HOST_UNAVAILABLE",
            reason: "HOST_DISCONNECTED",
            message: "Pervue lost its connection to the companion app.",
            retryable: true
          });
        }
      });
    } catch {
      cancelFailed({
        code: "HOST_UNAVAILABLE",
        reason: "HOST_START_FAILED",
        message: "Pervue couldn't send the stop request.",
        retryable: true
      });
    }
  }

  function requestNativeCancel() {
    if (!cancelRequested || !activeRequestId || !nativePending || stopped || finished) return;
    sendCancel(activeRequestId);
  }

  /** @param {any} event */
  function forward(event) {
    if (finished) return;
    if (open) {
      try { port.postMessage(event); }
      catch { open = false; }
    }
    if (ASK_TERMINAL_EVENTS.has(event.event)) {
      finished = true;
      if (open) port.disconnect();
    }
  }

  /** @param {string | null} requestId @param {any} error */
  async function stop(requestId, error, recordFailure = true) {
    if (stopped || finished) return;
    stopped = true;
    if (recordFailure) options.onFailure?.(error);
    if (nativePending && requestId) sendCancel(requestId);
    if (conversationId && assistantId) {
      try { await store.finish(conversationId, assistantId, answer, sources, error); }
      catch { error = STORAGE_FAILED; }
    }
    if (conversationId && !nativePending) inFlight.delete(conversationId);
    forward(failed(requestId, error));
  }

  /** @param {any} event */
  async function onNativeEvent(event) {
    if (ASK_TERMINAL_EVENTS.has(event?.event)) {
      nativePending = false;
      activeRequestId = null;
    }
    if (stopped) {
      if (conversationId && !nativePending) inFlight.delete(conversationId);
      return;
    }
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
      if (
        event.event === "response.failed" &&
        !(cancelRequested &&
          event.payload?.error?.code === "REQUEST_CANCELLED" &&
          event.payload?.error?.reason === "USER_CANCELLED")
      ) {
        options.onFailure?.(event.payload?.error);
      }
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
    if (asked) {
      if (message?.type === "cancel") {
        cancelRequested = true;
        requestNativeCancel();
      }
      return;
    }
    if (message?.type === "cancel") {
      cancelRequested = true;
      return;
    }
    asked = true;
    if (message?.type !== "ask") {
      forward(failed(null, EMPTY_QUESTION));
      return;
    }
    const text = message.text;
    if (typeof text !== "string" || !text.trim()) {
      forward(failed(null, EMPTY_QUESTION));
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
      activeRequestId = requestId;
      let sending = false;
      const retry = message.retry === true;
      try {
        if (cancelRequested) {
          await stop(requestId, USER_CANCELLED, false);
          return;
        }
        let sessionId;
        /** @type {{role: string, text: string}[]} */
        let history = [];
        let selectedProvider = providerId;
        if (requestedId) {
          const stored = await store.getPrivate(requestedId);
          if (cancelRequested) {
            await stop(requestId, USER_CANCELLED, false);
            return;
          }
          if (inFlight.has(requestedId)) {
            forward(failed(requestId, BUSY));
            return;
          }
          conversationId = requestedId;
          selectedProvider = stored.provider_id;
          sessionId = stored.provider_session_id;
          history = dialogueHistory(stored);
          inFlight.add(requestedId);
          ({ assistantId } = retry
            ? await store.retry(requestedId, question, context)
            : await store.begin(requestedId, question, context));
          if (cancelRequested) {
            await store.discardPending(requestedId, assistantId, retry);
            inFlight.delete(requestedId);
            assistantId = null;
            await stop(requestId, USER_CANCELLED, false);
            return;
          }
        }
        const request = {
          version: 1, type: "request", request_id: requestId, method: "conversation.send",
          payload: {
            provider_id: selectedProvider,
            ...(sessionId ? { conversation_id: sessionId } : {}),
            input: { text: question, ...(history.length ? { history } : {}) },
            ...(context === undefined ? {} : { context: copyContext(context) })
          }
        };
        if (cancelRequested) {
          await stop(requestId, USER_CANCELLED, false);
          return;
        }
        sending = true;
        nativePending = true;
        manager.send(request, {
          onEvent(/** @type {any} */ event) {
            work = work.then(() => onNativeEvent(event)).catch(() => stop(requestId, STORAGE_FAILED));
          },
          onDisconnect(/** @type {{message: string | null}} */ { message: reason }) {
            nativePending = false;
            work = work.then(() => {
              if (stopped) {
                if (conversationId) inFlight.delete(conversationId);
                return;
              }
              return stop(requestId, hostDisconnectError(reason));
            });
          }
        });
        requestNativeCancel();
      } catch (error) {
        if (sending) nativePending = false;
        const reason = error instanceof RequestTooLargeError ? QUESTION_TOO_LONG
          : error instanceof Error && error.message === "Conversation not found." ? NOT_FOUND
            : sending ? HOST_START_FAILED : STORAGE_FAILED;
        await stop(requestId, reason);
      }
    })();
  });
}
