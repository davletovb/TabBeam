import { isErrorCode } from "./outcomes.js";

export const DIAGNOSTICS_MESSAGE = "pervue.diagnostics";
export const PROTOCOL_VERSION = 1;

/** Keep diagnostics to the frozen normalized vocabulary; never copy raw provider text. */
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
    reason: error.reason.slice(0, 96),
    retryable: error.retryable
  };
}

/** Only host.ready fields defined by protocol v1 may enter user-visible diagnostics. */
export function sanitizedHostReady(event) {
  if (
    event?.event !== "host.ready" ||
    typeof event.payload?.host_version !== "string" ||
    event.payload.host_version.length > 64 ||
    !Array.isArray(event.payload.protocol_versions) ||
    event.payload.protocol_versions.length > 16 ||
    !event.payload.protocol_versions.every((version) => Number.isInteger(version))
  ) {
    return null;
  }
  return {
    version: event.payload.host_version,
    protocol_versions: [...event.payload.protocol_versions]
  };
}
