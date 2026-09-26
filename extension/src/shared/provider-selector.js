import { PROVIDER_STATUS_MESSAGE } from "./provider-status.js";
import { DEFAULT_PROVIDER_ID, USER_PROVIDERS, providerLabel } from "./providers.js";

export const PROVIDER_STORAGE_KEY = "pervue.provider";

/**
 * @typedef {{
 *   providerId: string,
 *   label: string,
 *   status: any | null,
 *   response: any | null | undefined,
 *   providerChanged: boolean,
 *   statusUpdated: boolean
 * }} ProviderSelection
 */

/**
 * Capability-aware provider picker (EXT-15). Availability only controls which
 * provider can start a new conversation; an existing conversation is locked
 * to the provider that created it.
 *
 * The saved preference applies as soon as storage answers; each provider's
 * status is checked once, and a response (or `null` for a check that failed)
 * is passed on as `response`, `undefined` while the check is still running.
 * Only the check of the provider current when checks start is recorded in
 * diagnostics, so a background check can't overwrite it.
 *
 * @param {HTMLSelectElement} select
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{get(key: string): Promise<any>, set(values: object): Promise<void>}} storage
 * @param {{onChange?(selection: ProviderSelection): void}} [options]
 */
export function bindProviderSelector(select, runtime, storage, options = {}) {
  /** @type {Map<string, any>} */
  const statuses = new Map();
  let current = DEFAULT_PROVIDER_ID;
  let preferred = DEFAULT_PROVIDER_ID;
  let locked = false;
  // A first question in flight: its conversation will be locked to the
  // provider it was sent to, so the choice can't change underneath it.
  let held = false;
  let touched = false;
  let preferenceTouched = false;

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

  /** @param {boolean} [providerChanged] @param {boolean} [statusUpdated] */
  function notify(providerChanged = false, statusUpdated = false) {
    options.onChange?.({
      providerId: current,
      label: providerLabel(current),
      status: statuses.get(current)?.status ?? null,
      response: statuses.has(current) ? statuses.get(current) : undefined,
      providerChanged,
      statusUpdated
    });
  }

  function syncDisabled() {
    select.disabled = locked || held;
  }

  /** @param {string} id */
  function firstEnabled(id) {
    const candidates = [id, DEFAULT_PROVIDER_ID, ...USER_PROVIDERS.map(({ id: candidate }) => candidate)];
    return candidates.find((candidate) => {
      const option = optionFor(candidate);
      return option && !option.disabled;
    }) ?? DEFAULT_PROVIDER_ID;
  }

  /** @param {string} id @param {boolean} [persist] @param {boolean} [emit] */
  function choose(id, persist = true, emit = true) {
    const option = optionFor(id);
    if (!option || (option.disabled && !locked)) return false;
    const changed = current !== id;
    current = id;
    select.value = id;
    if (persist && !locked) {
      preferenceTouched = true;
      preferred = id;
      void storage.set({ [PROVIDER_STORAGE_KEY]: id }).catch(() => {});
    }
    if (emit && changed) notify(true);
    return true;
  }

  select.addEventListener("change", () => {
    if (locked || held) {
      select.value = current;
      return;
    }
    touched = true;
    choose(select.value);
  });

  const ready = (async () => {
    try {
      const saved = await storage.get(PROVIDER_STORAGE_KEY);
      if (!preferenceTouched && typeof saved?.[PROVIDER_STORAGE_KEY] === "string") {
        preferred = saved[PROVIDER_STORAGE_KEY];
      }
    } catch {
      // A storage failure should not make the selector unusable.
    }
    // Show the saved choice at once: a question asked before the checks
    // finish goes to it, not to the default.
    if (!touched && !held) choose(preferred, false);

    const recorded = current;
    await Promise.all(USER_PROVIDERS.map(async ({ id }) => {
      try {
        const response = await runtime.sendMessage({
          type: PROVIDER_STATUS_MESSAGE,
          provider_id: id,
          record_diagnostics: id === recorded
        });
        statuses.set(id, response?.provider_id === id ? response : null);
      } catch {
        // The provider line says the check failed.
        statuses.set(id, null);
      }
    }));

    for (const option of Array.from(select.options)) {
      const response = statuses.get(option.value);
      const availability = response?.status?.availability;
      option.disabled = availability === "not_found" || availability === "unavailable";
    }

    // A question in flight keeps the provider it was sent to.
    let providerChanged = false;
    if (!held && !touched) {
      const next = firstEnabled(preferred);
      providerChanged = current !== next;
      choose(next, false, false);
    } else if (!held && !locked && optionFor(current)?.disabled) {
      const next = firstEnabled(preferred);
      providerChanged = current !== next;
      choose(next, false, false);
    }
    // Even if a newer choice/lock won while probes were pending, publish the
    // now-known capability status for that current provider without changing it.
    notify(providerChanged, true);
    return current;
  })();

  return {
    ready,
    getProviderId: () => current,
    /** @param {string} providerId */
    lock(providerId) {
      touched = true;
      const changed = current !== providerId;
      locked = true;
      const option = optionFor(providerId);
      if (option) {
        current = providerId;
        select.value = providerId;
      }
      syncDisabled();
      // Re-loading the same conversation after a request must not erase a
      // fresh failure/status message. A real provider transition still emits.
      if (changed) notify(true);
    },
    /** @param {boolean} value whether a first question is in flight */
    hold(value) {
      held = value;
      syncDisabled();
    },
    unlock() {
      locked = false;
      syncDisabled();
      const next = firstEnabled(preferred);
      const changed = current !== next;
      choose(next, false, false);
      if (changed) notify(true);
    }
  };
}
