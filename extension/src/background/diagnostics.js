import {
  PROTOCOL_VERSION,
  sanitizedFailure,
  sanitizedHostReady
} from "../shared/diagnostics.js";

export const DIAGNOSTICS_STORAGE_KEY = "tabbeam.diagnostics";

/** @param {string} extensionVersion */
function emptySnapshot(extensionVersion) {
  /** @type {{
   * extension_version: string,
   * protocol_version: number,
   * host: {state: string, version: string | null, protocol_versions: number[]},
   * provider: {provider_id: string | null, availability: string, authentication: string},
   * recent_failure: {code: string, reason: string, retryable: boolean, at: number} | null
   * }} */
  const snapshot = {
    extension_version: extensionVersion,
    protocol_version: PROTOCOL_VERSION,
    host: { state: "unknown", version: null, protocol_versions: [] },
    provider: {
      provider_id: null,
      availability: "unknown",
      authentication: "unknown"
    },
    recent_failure: null
  };
  return snapshot;
}

/** @param {any} value @param {string} extensionVersion */
function validStoredSnapshot(value, extensionVersion) {
  if (
    !value ||
    value.extension_version !== extensionVersion ||
    value.protocol_version !== PROTOCOL_VERSION ||
    typeof value.host?.state !== "string" ||
    !Array.isArray(value.host?.protocol_versions) ||
    typeof value.provider?.availability !== "string" ||
    typeof value.provider?.authentication !== "string"
  ) {
    return null;
  }
  return value;
}

/**
 * The session-backed snapshot survives MV3 worker suspension but is cleared
 * when the browser session ends. Only sanitized identifiers and status values
 * are persisted.
 *
 * @param {string} extensionVersion
 * @param {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>} | undefined} [storage]
 * @param {() => number} [now]
 */
export function createDiagnosticsState(
  extensionVersion,
  storage,
  now = () => Date.now()
) {
  let snapshot = emptySnapshot(extensionVersion);
  let dirty = false;
  const ready = storage
    ? storage.get(DIAGNOSTICS_STORAGE_KEY).then((stored) => {
        if (dirty) return;
        const restored = validStoredSnapshot(stored[DIAGNOSTICS_STORAGE_KEY], extensionVersion);
        if (restored) snapshot = restored;
      }).catch(() => {})
    : Promise.resolve();

  function persist() {
    dirty = true;
    if (!storage) return;
    void storage.set({ [DIAGNOSTICS_STORAGE_KEY]: snapshot }).catch(() => {});
  }

  function resetProvider(providerId = snapshot.provider.provider_id) {
    snapshot.provider = {
      provider_id: providerId,
      availability: "unknown",
      authentication: "unknown"
    };
  }

  /** @param {any} error */
  function noteFailure(error) {
    const safe = sanitizedFailure(error);
    if (!safe) return;
    snapshot.recent_failure = { ...safe, at: now() };
    if (safe.code === "HOST_NOT_INSTALLED") {
      snapshot.host = { state: "not-installed", version: null, protocol_versions: [] };
      resetProvider();
    } else if (safe.code === "HOST_UNAVAILABLE") {
      snapshot.host = { ...snapshot.host, state: "unavailable" };
      resetProvider();
    }
    persist();
  }

  return {
    /** @param {any} event */
    noteLifecycle(event) {
      const host = sanitizedHostReady(event);
      if (!host) return;
      snapshot.host = { state: "available", ...host };
      persist();
    },

    noteDisconnect() {
      snapshot.host = { ...snapshot.host, state: "unavailable" };
      resetProvider();
      persist();
    },

    /** @param {any} response */
    noteProvider(response) {
      if (typeof response?.provider_id === "string") {
        snapshot.provider = { ...snapshot.provider, provider_id: response.provider_id };
      }
      if (response?.status) {
        snapshot.host = { ...snapshot.host, state: "available" };
        snapshot.provider = {
          provider_id: response.provider_id ?? snapshot.provider.provider_id,
          availability: response.status.availability,
          authentication: response.status.authentication
        };
        persist();
        return;
      }
      if (response?.error) {
        noteFailure(response.error);
        if (
          response.error.code !== "HOST_NOT_INSTALLED" &&
          response.error.code !== "HOST_UNAVAILABLE"
        ) {
          resetProvider(response.provider_id ?? snapshot.provider.provider_id);
          persist();
        }
      }
    },

    noteFailure,

    async summary() {
      await ready;
      return structuredClone(snapshot);
    }
  };
}
