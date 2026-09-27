/**
 * How the popup tells failures apart (EXT-04). A failure's kind comes from its
 * DOC-02 `code` alone, never from its message (DOC-02 §3). The message shown is
 * the error's own when it has one, because the host and the service worker
 * write their messages for people, saying what to do next; otherwise it is
 * the kind's.
 */

/** @typedef {import("./provider-status.js").ErrorBody} ErrorBody */

/**
 * The kind of each DOC-02 error code.
 * @type {Readonly<Record<string, string>>}
 */
export const FAILURE_KINDS = Object.freeze({
  HOST_NOT_INSTALLED: "host-missing",
  HOST_UNAVAILABLE: "host-unavailable",
  PROVIDER_NOT_FOUND: "provider-missing",
  PROVIDER_NOT_AUTHENTICATED: "provider-signed-out",
  PROVIDER_FAILED: "provider-failed",
  SEARCH_FAILED: "search-failed",
  REQUEST_CANCELLED: "cancelled",
  REQUEST_TIMEOUT: "timeout",
  CONTEXT_UNAVAILABLE: "context-unavailable",
  INVALID_REQUEST: "invalid-request",
  INTERNAL_ERROR: "internal-error"
});

/**
 * Whether `code` is a DOC-02 error code: one of the keys of
 * {@link FAILURE_KINDS}, a vocabulary DOC-02 freezes.
 *
 * @param {unknown} code
 * @returns {code is string}
 */
export function isErrorCode(code) {
  return typeof code === "string" && Object.hasOwn(FAILURE_KINDS, code);
}

/**
 * What each kind says when its error has no message of its own.
 * @type {Readonly<Record<string, string>>}
 */
export const KIND_MESSAGES = Object.freeze({
  "host-missing": "Pervue's companion app isn't installed. Install it, then try again.",
  "host-unavailable": "Pervue's companion app isn't responding. Try again.",
  "provider-missing": "The AI provider isn't installed. Install it, then try again.",
  "provider-signed-out": "The AI provider isn't signed in. Sign in, then try again.",
  "provider-failed": "The AI provider couldn't answer. Try again.",
  "search-failed": "Web search couldn't retrieve sources. Try again.",
  timeout: "The answer took too long. Try again.",
  cancelled: "Stopped. You can ask again.",
  "context-unavailable": "Pervue couldn't use this page. Choose No context, then ask again.",
  "invalid-request": "Pervue couldn't send that question. Try again.",
  "internal-error": "Something went wrong. Try again."
});

/**
 * Reasons that mean the extension and the host disagree about the protocol,
 * which only an update fixes: the popup adds the request ID, so the host's
 * diagnostics can be matched to what the user saw (DOC-02 §7).
 */
const PROTOCOL_REASONS = new Set([
  "MALFORMED_MESSAGE",
  "INVALID_ENVELOPE",
  "UNKNOWN_METHOD",
  "UNSUPPORTED_PROTOCOL_VERSION",
  "DUPLICATE_REQUEST_ID",
  "UNKNOWN_TARGET_REQUEST"
]);

/**
 * @param {any} error a DOC-02 error, or anything else
 * @returns {{kind: string, message: string, reference: boolean}} its kind, the
 *   message to show, and whether to show a reference with it
 */
export function describeFailure(error) {
  const kind = isErrorCode(error?.code) ? FAILURE_KINDS[error.code] : "internal-error";
  const message =
    typeof error?.message === "string" && error.message.trim() !== ""
      ? error.message
      : KIND_MESSAGES[kind];
  const reference =
    kind === "internal-error" ||
    (kind === "invalid-request" && PROTOCOL_REASONS.has(error?.reason));
  return { kind, message, reference };
}
