import { PROVIDER_STATUS_MESSAGE } from "./provider-status.js";
import { DEFAULT_PROVIDER_ID, USER_PROVIDERS, providerLabel } from "./providers.js";

export const PROVIDER_STORAGE_KEY = "pervue.provider";

/**
 * Capability-aware provider picker (EXT-15). Availability only controls which
 * provider can start a new conversation; an existing conversation is locked
 * to the provider that created it.
 *
 * @param {HTMLSelectElement} select
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{get(key: string): Promise<any>, set(values: object): Promise<void>}} storage
 * @param {{onChange?(selection: {
 *   providerId: string,
 *   label: string,
 *   status: any | null,
 *   providerChanged: boolean,
 *   statusUpdated: boolean
 * }): void}} [options]
 */
export function bindProviderSelector(select, runtime, storage, options = {}) {
  /** @type {Map<string, any>} */
  const statuses = new Map();
  let current = DEFAULT_PROVIDER_ID;
  let preferred = DEFAULT_PROVIDER_ID;
  let locked = false;
  let touched = false;

  select.replaceChildren(...USER_PROVIDERS.map(({ id, label }) => {
    const option = select.ownerDocument.createElement("option");
    option.value = id;
    option.textContent = label;
    return option;
  }));

  /** @param {string} id */
  function optionFor(id) {
    return Array.from(select.options).find((option) => option.value === id);
  }

  /** @param {boolean} providerChanged @param {boolean} statusUpdated */
  function notify(providerChanged, statusUpdated) {
    options.onChange?.({
      providerId: current,
      label: providerLabel(current),
      status: statuses.get(current)?.status ?? null,
      providerChanged,
      statusUpdated
    });
  }

  /** @param {string} id @param {boolean} persist @param {boolean} markTouched */
  function choose(id, persist, markTouched) {
    const option = optionFor(id);
    if (!option || (option.disabled && !locked)) return false;
    const changed = current !== id;
    current = id;
    select.value = id;
    if (markTouched) touched = true;
    if (persist && !locked) {
      preferred = id;
      void storage.set({ [PROVIDER_STORAGE_KEY]: id }).catch(() => {});
    }
    if (changed) notify(true, false);
    return true;
  }

  /** The first usable provider following the saved preference. */
  function fallbackProvider() {
    return [preferred, DEFAULT_PROVIDER_ID, ...USER_PROVIDERS.map(({ id }) => id)]
      .find((id) => {
        const option = optionFor(id);
        return option && !option.disabled;
      }) ?? DEFAULT_PROVIDER_ID;
  }

  select.addEventListener("change", () => {
    if (!locked) choose(select.value, true, true);
  });

  const ready = (async () => {
    try {
      const saved = await storage.get(PROVIDER_STORAGE_KEY);
      if (typeof saved?.[PROVIDER_STORAGE_KEY] === "string" && optionFor(saved[PROVIDER_STORAGE_KEY])) {
        preferred = saved[PROVIDER_STORAGE_KEY];
      }
    } catch {
      // A storage failure should not make the selector unusable.
    }

    await Promise.all(USER_PROVIDERS.map(async ({ id }) => {
      try {
        const response = await runtime.sendMessage({
          type: PROVIDER_STATUS_MESSAGE,
          provider_id: id,
          background: true
        });
        if (response?.provider_id === id) statuses.set(id, response);
      } catch {
        // Host/worker failures stay visible in the active provider state line.
      }
    }));

    for (const option of Array.from(select.options)) {
      const response = statuses.get(option.value);
      const availability = response?.status?.availability;
      option.disabled = availability === "not_found" || availability === "unavailable";
    }

    if (!touched) {
      choose(fallbackProvider(), false, false);
    } else if (!locked && optionFor(current)?.disabled) {
      // A choice made before discovery completed cannot start a new
      // conversation once discovery proves that provider unavailable.
      choose(fallbackProvider(), true, false);
    }

    // A lock or explicit choice made while probes were pending wins, but it
    // still needs the capabilities/status that just arrived.
    notify(false, true);
    return current;
  })();

  return {
    ready,
    getProviderId: () => current,
    /** @param {string} id */
    getStatus: (id = current) => statuses.get(id)?.status ?? null,
    /** @param {string} providerId */
    lock(providerId) {
      touched = true;
      locked = true;
      const option = optionFor(providerId);
      const changed = Boolean(option) && current !== providerId;
      if (option) {
        current = providerId;
        select.value = providerId;
      }
      select.disabled = true;
      if (changed) notify(true, false);
    },
    unlock() {
      touched = true;
      locked = false;
      select.disabled = false;
      choose(fallbackProvider(), false, false);
    }
  };
}
