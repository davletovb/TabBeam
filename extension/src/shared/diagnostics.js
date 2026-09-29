import { isErrorCode } from "./outcomes.js";

export const DIAGNOSTICS_MESSAGE = "tabbeam.diagnostics";
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


export const DIAGNOSTICS_EXPORT_FORMAT = "tabbeam-support-diagnostics";
export const DIAGNOSTICS_EXPORT_VERSION = 1;

const HOST_STATES = new Set(["unknown", "available", "unavailable", "not-installed"]);
const AVAILABILITY = new Set(["unknown", "available", "unavailable", "not_found"]);
const AUTHENTICATION = new Set(["unknown", "authenticated", "unauthenticated"]);

/** @param {any} value */
function safeVersion(value) {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$/.test(value)
    ? value
    : "unknown";
}

/** @param {any} versions */
function safeProtocolVersions(versions) {
  return Array.isArray(versions) && versions.length <= 16
    ? versions.filter((version) => Number.isInteger(version)).slice(0, 16)
    : [];
}

/**
 * Builds the support bundle from an explicit allowlist. Unknown members of
 * the in-memory diagnostics snapshot are never copied into the export.
 * @param {any} summary
 * @param {number} generatedAt
 */
export function buildDiagnosticsExport(summary, generatedAt = Date.now()) {
  const host = summary?.host ?? {};
  const provider = summary?.provider ?? {};
  const failure = sanitizedFailure(summary?.recent_failure);
  const at = Number.isFinite(summary?.recent_failure?.at)
    ? Math.max(0, Math.trunc(summary.recent_failure.at))
    : 0;
  const providerId = typeof provider.provider_id === "string" &&
      /^[a-z][a-z0-9_-]{0,31}$/.test(provider.provider_id)
    ? provider.provider_id
    : "unknown";

  return {
    format: DIAGNOSTICS_EXPORT_FORMAT,
    version: DIAGNOSTICS_EXPORT_VERSION,
    generated_at: Number.isFinite(generatedAt) ? Math.max(0, Math.trunc(generatedAt)) : 0,
    extension_version: safeVersion(summary?.extension_version),
    protocol_version: Number.isInteger(summary?.protocol_version) ? summary.protocol_version : 0,
    host: {
      state: HOST_STATES.has(host.state) ? host.state : "unknown",
      version: safeVersion(host.version),
      protocol_versions: safeProtocolVersions(host.protocol_versions)
    },
    provider: {
      provider_id: providerId,
      availability: AVAILABILITY.has(provider.availability) ? provider.availability : "unknown",
      authentication: AUTHENTICATION.has(provider.authentication) ? provider.authentication : "unknown"
    },
    recent_failure: failure
      ? { ...failure, at }
      : null
  };
}
