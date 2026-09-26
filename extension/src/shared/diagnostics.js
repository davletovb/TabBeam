import { isErrorCode } from "./outcomes.js";

export const DIAGNOSTICS_MESSAGE = "pervue.diagnostics";
export const PROTOCOL_VERSION = 1;

/** Keep diagnostics to the frozen normalized vocabulary; never copy raw provider text. */
/** @param {any} error */
export function sanitizedFailure(error) {
  if (
    !isErrorCode(error?.code) ||
    typeof error?.reason !== "string" ||
    typeof error?.retryable !== "boolean"
  ) {
    return null;
  }
  return {
    code: error.code,
    reason: /^[A-Z][A-Z0-9_]{0,63}$/.test(error.reason) ? error.reason : "UNKNOWN",
    retryable: error.retryable
  };
}

/** Only host.ready fields defined by protocol v1 may enter user-visible diagnostics. */
/** @param {any} event */
export function sanitizedHostReady(event) {
  if (
    event?.event !== "host.ready" ||
    typeof event.payload?.host_version !== "string" ||
    event.payload.host_version.length > 64 ||
    !Array.isArray(event.payload.protocol_versions) ||
    event.payload.protocol_versions.length > 16 ||
    !event.payload.protocol_versions.every((/** @type {any} */ version) => Number.isInteger(version))
  ) {
    return null;
  }
  return {
    version: /^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$/.test(event.payload.host_version)
      ? event.payload.host_version
      : "unknown",
    protocol_versions: [...event.payload.protocol_versions]
  };
}
