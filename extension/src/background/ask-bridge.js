import { MAX_CONTEXT_TITLE_BYTES, MAX_CONTEXT_URL_BYTES, MAX_PAGE_BYTES, MAX_SELECTION_BYTES, utf8ByteLength } from "../shared/limits.js";

/** @typedef {import("../shared/provider-status.js").ErrorBody} ErrorBody */

export const EMPTY_QUESTION = Object.freeze({
  code: "INVALID_REQUEST",
  reason: "INVALID_PAYLOAD",
  message: "Type a question first.",
  retryable: false
});

export const INVALID_CONTEXT = Object.freeze({
  code: "INVALID_REQUEST",
  reason: "INVALID_PAYLOAD",
  message: "The selected page context is invalid. Choose a source again or use No context.",
  retryable: false
});

export const HOST_START_FAILED = Object.freeze({
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

/** @param {any} context Copy only validated context fields across the boundary. */
export function copyContext(context) {
  return {
    mode: context.mode, text: context.text, truncated: context.truncated,
    page: { title: context.page.title, url: context.page.url }
  };
}

/** @param {any} context */
export function isValidContext(context) {
  if (
    !context ||
    (context.mode !== "selection" && context.mode !== "page") ||
    typeof context.text !== "string" ||
    context.text.trim() === "" ||
    typeof context.truncated !== "boolean" ||
    typeof context.page?.title !== "string" ||
    typeof context.page?.url !== "string"
  ) {
    return false;
  }
  const limit = context.mode === "selection" ? MAX_SELECTION_BYTES : MAX_PAGE_BYTES;
  if (
    utf8ByteLength(context.text) > limit ||
    utf8ByteLength(context.page.title) > MAX_CONTEXT_TITLE_BYTES ||
    /[\uD800-\uDFFF]/u.test(context.text) ||
    /[\uD800-\uDFFF]/u.test(context.page.title)
  ) {
    return false;
  }
  try {
    const url = new URL(context.page.url);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      url.username === "" &&
      url.password === "" &&
      url.search === "" &&
      url.hash === "" &&
      utf8ByteLength(context.page.url) <= MAX_CONTEXT_URL_BYTES
    );
  } catch {
    return false;
  }
}

/**
 * A `response.failed` event for a failure the extension detects itself.
 *
 * @param {string | null} requestId
 * @param {ErrorBody} error
 */
export function failed(requestId, error) {
  return {
    version: 1,
    type: "event",
    request_id: requestId,
    event: "response.failed",
    payload: { error }
  };
}
