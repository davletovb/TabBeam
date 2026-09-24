import { ASK_TERMINAL_EVENTS, QUESTION_TOO_LONG } from "../shared/ask-port.js";
import { RequestTooLargeError } from "./native-connection.js";

/** @typedef {import("../shared/ask-port.js").AskPort} AskPort */
/** @typedef {import("./native-connection.js").RequestOwner} RequestOwner */
/** @typedef {{code: string, reason: string, message: string, retryable: boolean}} ErrorBody */

// Milestone A answers with the native host's deterministic fake provider.
// Choosing a real provider arrives with Milestone B (PRO-02, EXT-04).
export const DEFAULT_PROVIDER_ID = "fake";

const EMPTY_QUESTION = Object.freeze({
  code: "INVALID_REQUEST",
  reason: "INVALID_PAYLOAD",
  message: "Type a question first.",
  retryable: false
});

const HOST_START_FAILED = Object.freeze({
  code: "HOST_UNAVAILABLE",
  reason: "HOST_START_FAILED",
  message: "Pervue's companion app couldn't start. Try again.",
  retryable: true
});

const HOST_DISCONNECTED = Object.freeze({
  code: "HOST_UNAVAILABLE",
  reason: "HOST_DISCONNECTED",
  message: "Pervue lost its connection to the companion app. Try again.",
  retryable: true
});

// Chrome explains why a native port closed only through the text of
// chrome.runtime.lastError. These are Chromium's messages for a host that is
// missing, registered for other extensions only, or unable to launch.
const HOST_ERRORS_BY_LAST_ERROR = new Map(
  /** @type {[string, ErrorBody][]} */ ([
    [
      "Specified native messaging host not found.",
      Object.freeze({
        code: "HOST_NOT_INSTALLED",
        reason: "NATIVE_HOST_NOT_FOUND",
        message: "Pervue's companion app isn't installed. Install it, then try again.",
        retryable: false
      })
    ],
    [
      "Access to the specified native messaging host is forbidden.",
      Object.freeze({
        code: "HOST_NOT_INSTALLED",
        reason: "NATIVE_HOST_NOT_REGISTERED",
        message:
          "Pervue's companion app isn't set up for this browser. Reinstall it, then try again.",
        retryable: false
      })
    ],
    ["Failed to start native messaging host.", HOST_START_FAILED]
  ])
);

/** @returns {string} a fresh protocol v1 request ID */
export function createRequestId() {
  return `req_${crypto.randomUUID()}`;
}

/**
 * The DOC-02 error for a native port that closed before its request finished.
 *
 * @param {string | null} lastError the port's `chrome.runtime.lastError` message
 */
export function hostDisconnectError(lastError) {
  return HOST_ERRORS_BY_LAST_ERROR.get(lastError ?? "") ?? HOST_DISCONNECTED;
}

/**
 * Whether a runtime port was opened by one of this extension's own pages,
 * such as the popup, rather than by a content script inside a web page.
 *
 * @param {{url?: string} | undefined} sender
 * @param {string} extensionBaseUrl `chrome.runtime.getURL("")`
 */
export function isExtensionPage(sender, extensionBaseUrl) {
  return typeof sender?.url === "string" && sender.url.startsWith(extensionBaseUrl);
}

/**
 * Serves one question from a UI page (see ../shared/ask-port.js): sends one
 * `conversation.send` and forwards its events to the page in order, ending
 * with exactly one terminal event.
 *
 * If the page closes first, its remaining events are dropped and the request
 * runs to its own terminal event; cancelling it is EXT-12's job.
 *
 * @param {AskPort} port
 * @param {{
 *   manager: {send(request: any, owner?: RequestOwner): void},
 *   providerId?: string,
 *   createRequestId?: () => string
 * }} options
 */
export function serveAskPort(port, options) {
  const { manager, providerId = DEFAULT_PROVIDER_ID } = options;
  const nextRequestId = options.createRequestId ?? createRequestId;
  let asked = false;
  let open = true;

  port.onDisconnect.addListener(() => {
    open = false;
  });

  /** @param {any} event */
  function forward(event) {
    if (!open) {
      return;
    }
    try {
      port.postMessage(event);
    } catch {
      // The page closed before its disconnect event reached the worker.
      open = false;
      return;
    }
    if (ASK_TERMINAL_EVENTS.has(event.event)) {
      open = false;
      port.disconnect();
    }
  }

  port.onMessage.addListener((/** @type {any} */ message) => {
    if (asked) {
      return;
    }
    asked = true;

    const text = message?.type === "ask" ? message.text : undefined;
    if (typeof text !== "string" || text.trim() === "") {
      forward(failed(null, EMPTY_QUESTION));
      return;
    }

    const requestId = nextRequestId();
    const request = {
      version: 1,
      type: "request",
      request_id: requestId,
      method: "conversation.send",
      payload: { provider_id: providerId, input: { text } }
    };

    try {
      manager.send(request, {
        onEvent: forward,
        onDisconnect: ({ message }) =>
          forward(failed(requestId, hostDisconnectError(message)))
      });
    } catch (error) {
      // A post failure has already reported a disconnect; forward() ignores
      // anything after the first terminal event.
      forward(
        failed(
          requestId,
          error instanceof RequestTooLargeError ? QUESTION_TOO_LONG : HOST_START_FAILED
        )
      );
    }
  });
}

/**
 * A `response.failed` event for a failure the extension detects itself.
 *
 * @param {string | null} requestId
 * @param {ErrorBody} error
 */
function failed(requestId, error) {
  return {
    version: 1,
    type: "event",
    request_id: requestId,
    event: "response.failed",
    payload: { error }
  };
}
