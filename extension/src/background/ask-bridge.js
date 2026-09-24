import { ASK_TERMINAL_EVENTS } from "../shared/ask-port.js";

/** @typedef {import("../shared/ask-port.js").AskPort} AskPort */
/** @typedef {import("./native-connection.js").RequestOwner} RequestOwner */

// Milestone A answers with the native host's deterministic fake provider.
// Choosing a real provider arrives with Milestone B (PRO-02, EXT-04).
export const DEFAULT_PROVIDER_ID = "fake";

const COMPANION_UNAVAILABLE =
  "Pervue couldn't reach its companion app. Make sure it's installed, then try again.";

const EMPTY_QUESTION = Object.freeze({
  code: "INVALID_REQUEST",
  reason: "INVALID_PAYLOAD",
  message: "Type a question first.",
  retryable: false
});

/** @returns {string} a fresh protocol v1 request ID */
export function createRequestId() {
  return `req_${crypto.randomUUID()}`;
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
        onDisconnect: () =>
          forward(failed(requestId, hostUnavailable("HOST_DISCONNECTED")))
      });
    } catch {
      // A post failure has already reported HOST_DISCONNECTED; forward()
      // ignores anything after the first terminal event.
      forward(failed(requestId, hostUnavailable("HOST_START_FAILED")));
    }
  });
}

/** @param {string} reason */
function hostUnavailable(reason) {
  return {
    code: "HOST_UNAVAILABLE",
    reason,
    message: COMPANION_UNAVAILABLE,
    retryable: true
  };
}

/**
 * A `response.failed` event for a failure the extension detects itself.
 *
 * @param {string | null} requestId
 * @param {{code: string, reason: string, message: string, retryable: boolean}} error
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
