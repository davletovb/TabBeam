import { HOST_START_FAILED, createRequestId, hostDisconnectError } from "./ask-bridge.js";
import { DEFAULT_PROVIDER_ID } from "../shared/providers.js";

/** @typedef {import("../shared/provider-status.js").ErrorBody} ErrorBody */
/** @typedef {import("../shared/provider-status.js").ProviderStatus} ProviderStatus */
/** @typedef {import("../shared/provider-status.js").ProviderStatusResponse} ProviderStatusResponse */

const AVAILABILITY = new Set(["available", "unavailable", "not_found", "unknown"]);
const AUTHENTICATION = new Set(["authenticated", "unauthenticated", "unknown"]);

/** The v1 capability keys (DOC-02 §5). A status without one of them is invalid. */
export const CAPABILITY_KEYS = Object.freeze([
  "streaming",
  "continuation",
  "web_search",
  "page_context",
  "attachments",
  "model_selection",
  "cancellation"
]);

/**
 * The failure for a host whose answer doesn't follow the protocol this
 * extension speaks: an older or newer companion app.
 */
export const HOST_OUT_OF_DATE = Object.freeze({
  code: "HOST_UNAVAILABLE",
  reason: "HOST_PROTOCOL_MISMATCH",
  message: "Pervue's companion app needs an update. Update it, then try again.",
  retryable: false
});

/**
 * Asks the native host for one provider's status (EXT-04), and answers in
 * the shape ../shared/provider-status.js describes. Only the normalized
 * fields reach the page: anything else the host sent is dropped, and a status
 * or error that doesn't follow DOC-02 counts as an out-of-date host.
 *
 * @param {{
 *   manager: {send(request: any, owner?: import("./native-connection.js").RequestOwner): void},
 *   providerId?: string,
 *   createRequestId?: () => string
 * }} options
 * @returns {Promise<ProviderStatusResponse>}
 */
export function checkProviderStatus(options) {
  const { manager, providerId = DEFAULT_PROVIDER_ID } = options;
  const requestId = (options.createRequestId ?? createRequestId)();
  return new Promise((resolve) => {
    /** @type {ProviderStatus | null} */
    let status = null;
    /** @param {{status: ProviderStatus} | {error: ErrorBody}} result */
    const settle = (result) => resolve({ provider_id: providerId, ...result });
    try {
      manager.send(
        {
          version: 1,
          type: "request",
          request_id: requestId,
          method: "provider.status",
          payload: { provider_id: providerId }
        },
        {
          onEvent(event) {
            switch (event?.event) {
              case "provider.status":
                if (status === null && event.payload?.provider_id === providerId) {
                  status = normalizedStatus(event.payload.status);
                }
                break;
              case "response.completed":
                settle(status === null ? { error: HOST_OUT_OF_DATE } : { status });
                break;
              case "response.failed":
                settle({ error: normalizedError(event.payload?.error) ?? HOST_OUT_OF_DATE });
                break;
              default:
                break;
            }
          },
          onDisconnect: ({ message }) => settle({ error: hostDisconnectError(message) })
        }
      );
    } catch {
      settle({ error: HOST_START_FAILED });
    }
  });
}

/**
 * The DOC-02 fields of `status`, or null if it doesn't follow DOC-02 §4–5.
 *
 * @param {any} status
 * @returns {ProviderStatus | null}
 */
export function normalizedStatus(status) {
  if (
    !AVAILABILITY.has(status?.availability) ||
    !AUTHENTICATION.has(status?.authentication) ||
    typeof status.capabilities !== "object" ||
    status.capabilities === null
  ) {
    return null;
  }
  /** @type {Record<string, boolean | "unknown">} */
  const capabilities = {};
  for (const key of CAPABILITY_KEYS) {
    const value = status.capabilities[key];
    if (value !== true && value !== false && value !== "unknown") {
      return null;
    }
    capabilities[key] = value;
  }
  return {
    availability: status.availability,
    authentication: status.authentication,
    capabilities
  };
}

/**
 * The DOC-02 fields of `error`, or null if it isn't a normalized error.
 *
 * @param {any} error
 * @returns {ErrorBody | null}
 */
function normalizedError(error) {
  if (
    typeof error?.code !== "string" ||
    typeof error.reason !== "string" ||
    typeof error.message !== "string" ||
    typeof error.retryable !== "boolean"
  ) {
    return null;
  }
  return {
    code: error.code,
    reason: error.reason,
    message: error.message,
    retryable: error.retryable
  };
}
