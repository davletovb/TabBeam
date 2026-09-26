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
 * @param {{onChange?(selection: {providerId: string, label: string, status: any | null}): void}} [options]
 */
export function bindProviderSelector(select, runtime, storage, options = {}) {
  /** @type {Map<string, any>} */
  const statuses = new Map();
  let current = DEFAULT_PROVIDER_ID;
  let locked = false;

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

  function notify() {
    options.onChange?.({
      providerId: current,
      label: providerLabel(current),
      status: statuses.get(current)?.status ?? null
    });
  }

  /** @param {string} id @param {boolean} [persist] */
  function choose(id, persist = true) {
    const option = optionFor(id);
    if (!option || (option.disabled && !locked)) return false;
    current = id;
    select.value = id;
    if (persist && !locked) void storage.set({ [PROVIDER_STORAGE_KEY]: id }).catch(() => {});
    notify();
    return true;
  }

  select.addEventListener("change", () => {
    if (!locked) choose(select.value);
  });

  const ready = (async () => {
    let preferred = DEFAULT_PROVIDER_ID;
    try {
      const saved = await storage.get(PROVIDER_STORAGE_KEY);
      if (typeof saved?.[PROVIDER_STORAGE_KEY] === "string") {
        preferred = saved[PROVIDER_STORAGE_KEY];
      }
    } catch {
      // A storage failure should not make the selector unusable.
    }

    await Promise.all(USER_PROVIDERS.map(async ({ id }) => {
      try {
        const response = await runtime.sendMessage({
          type: PROVIDER_STATUS_MESSAGE,
          provider_id: id
        });
        if (response?.provider_id === id) statuses.set(id, response);
      } catch {
        // Host/worker failures stay visible in the provider state line.
      }
    }));

    for (const option of Array.from(select.options)) {
      const response = statuses.get(option.value);
      const availability = response?.status?.availability;
      option.disabled = availability === "not_found" || availability === "unavailable";
    }

    const fallback = [preferred, DEFAULT_PROVIDER_ID, ...USER_PROVIDERS.map(({ id }) => id)]
      .find((id) => {
        const option = optionFor(id);
        return option && !option.disabled;
      }) ?? DEFAULT_PROVIDER_ID;
    choose(fallback, false);
    return current;
  })();

  return {
    ready,
    getProviderId: () => current,
    /** @param {string} id */
    getStatus: (id = current) => statuses.get(id)?.status ?? null,
    /** @param {string} providerId */
    lock(providerId) {
      locked = true;
      const option = optionFor(providerId);
      if (option) {
        current = providerId;
        select.value = providerId;
      }
      select.disabled = true;
      notify();
    },
    unlock() {
      locked = false;
      select.disabled = false;
      notify();
    }
  };
}
