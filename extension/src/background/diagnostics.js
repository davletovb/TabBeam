import {
  PROTOCOL_VERSION,
  sanitizedFailure,
  sanitizedHostReady
} from "../shared/diagnostics.js";

/**
 * A deliberately tiny in-memory diagnostics snapshot. It contains versions,
 * provider state, and normalized error identifiers only: no prompt, page,
 * selection, provider stderr, or raw Native Messaging text.
 */
/**
 * @param {string} extensionVersion
 * @param {() => number} [now]
 */
export function createDiagnosticsState(extensionVersion, now = () => Date.now()) {
  /** @type {{state: string, version: string | null, protocol_versions: number[]}} */
  let host = { state: "unknown", version: null, protocol_versions: [] };
  /** @type {{provider_id: string | null, availability: string, authentication: string}} */
  let provider = {
    provider_id: null,
    availability: "unknown",
    authentication: "unknown"
  };
  /** @type {{code: string, reason: string, retryable: boolean, at: number} | null} */
  let recentFailure = null;

  /** @param {any} error */
  function noteFailure(error) {
    const safe = sanitizedFailure(error);
    if (safe) recentFailure = { ...safe, at: now() };
  }

  return {
    /** @param {any} event */
    noteLifecycle(event) {
      const ready = sanitizedHostReady(event);
      if (ready) host = { state: "available", ...ready };
    },

    /** @param {any} response */
    noteProvider(response) {
      if (typeof response?.provider_id === "string") {
        provider = { ...provider, provider_id: response.provider_id };
      }
      if (response?.status) {
        host = { ...host, state: "available" };
        provider = {
          provider_id: response.provider_id ?? provider.provider_id,
          availability: response.status.availability,
          authentication: response.status.authentication
        };
        return;
      }
      if (response?.error) {
        noteFailure(response.error);
        if (response.error.code === "HOST_NOT_INSTALLED") {
          host = { state: "not-installed", version: null, protocol_versions: [] };
        } else if (response.error.code === "HOST_UNAVAILABLE") {
          host = { ...host, state: "unavailable" };
        }
      }
    },

    noteFailure,

    summary() {
      return {
        extension_version: extensionVersion,
        protocol_version: PROTOCOL_VERSION,
        host: { ...host, protocol_versions: [...host.protocol_versions] },
        provider: { ...provider },
        recent_failure: recentFailure ? { ...recentFailure } : null
      };
    }
  };
}
