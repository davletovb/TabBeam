import { describeFailure } from "../shared/outcomes.js";
import { PROVIDER_STATUS_MESSAGE } from "../shared/provider-status.js";
import { DEFAULT_PROVIDER_ID, providerLabel } from "../shared/providers.js";

/** @typedef {import("../shared/provider-status.js").ProviderStatusResponse} ProviderStatusResponse */

/**
 * How long the popup waits for the provider's status before it stops saying
 * it is checking. The host gives up on a stuck sign-in check sooner.
 */
export const STATUS_WAIT_MS = 20_000;

/** Failure kinds that describe the companion app or the provider itself. */
const STATE_KINDS = new Set([
  "host-missing",
  "host-unavailable",
  "provider-missing",
  "provider-signed-out"
]);

/**
 * What the provider line shows.
 *
 * @typedef {{
 *   state: "checking" | "ready" | "attention" | "unknown",
 *   kind: string | null,
 *   message: string
 * }} ProviderView
 */

/**
 * Shows the state of the companion app and the provider (EXT-04): the host's
 * `provider.status` when the popup opens, then what later questions find out.
 * Asking stays possible whatever the line says: the state may have changed
 * since, and the question reports its own failure.
 *
 * @param {HTMLElement} element
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{
 *   schedule?: (callback: () => void, ms: number) => any,
 *   cancel?: (timer: any) => void
 * }} [timers]
 */
export function bindProviderState(element, runtime, timers = {}) {
  const schedule = timers.schedule ?? setTimeout;
  const cancel = timers.cancel ?? clearTimeout;
  const label = providerLabel(DEFAULT_PROVIDER_ID);
  let checking = true;

  show({ state: "checking", kind: null, message: `Checking ${label}…` });
  const timer = schedule(() => settle(unknownView(label)), STATUS_WAIT_MS);

  /** @param {ProviderView} view */
  function settle(view) {
    if (checking) {
      checking = false;
      cancel(timer);
      show(view);
    }
  }

  /** @type {Promise<any>} */
  let sent;
  try {
    sent = Promise.resolve(runtime.sendMessage({ type: PROVIDER_STATUS_MESSAGE }));
  } catch (error) {
    sent = Promise.reject(error);
  }
  sent.then(
    (response) => settle(providerView(response, label)),
    // The service worker is gone; the next question reports that itself.
    () => settle(unknownView(label))
  );

  /** @param {ProviderView} view */
  function show(view) {
    element.textContent = view.message;
    element.setAttribute("data-state", view.state);
    if (view.kind === null) {
      element.removeAttribute("data-kind");
    } else {
      element.setAttribute("data-kind", view.kind);
    }
    element.hidden = false;
  }

  return {
    /**
     * Keeps the line in step with a question's outcome: an answer means the
     * provider is ready, and a missing app or provider, or a sign-in, shows
     * what the question found. Other failures leave the line as it is.
     *
     * @param {{kind: string, message?: string}} outcome
     */
    update(outcome) {
      /** @type {ProviderView | null} */
      let view = null;
      if (outcome.kind === "completed") {
        view = { state: "ready", kind: null, message: `${label} is ready.` };
      } else if (STATE_KINDS.has(outcome.kind) && typeof outcome.message === "string") {
        view = { state: "attention", kind: outcome.kind, message: outcome.message };
      }
      if (view !== null) {
        checking = false;
        cancel(timer);
        show(view);
      }
    }
  };
}

/**
 * The line for a status response from the service worker.
 *
 * @param {any} response a {@link ProviderStatusResponse}, or anything else
 * @param {string} fallbackLabel
 * @returns {ProviderView}
 */
export function providerView(response, fallbackLabel) {
  if (response === null || typeof response !== "object") {
    return unknownView(fallbackLabel);
  }
  const label =
    typeof response.provider_id === "string"
      ? providerLabel(response.provider_id)
      : fallbackLabel;
  if (response.error !== undefined) {
    const { kind, message } = describeFailure(response.error);
    return { state: "attention", kind, message };
  }
  const status = response.status;
  switch (status?.availability) {
    case "not_found":
      return {
        state: "attention",
        kind: "provider-missing",
        message: `${label} isn't installed. Install it, then try again.`
      };
    case "unavailable":
      return {
        state: "attention",
        kind: "provider-failed",
        message: `${label} is installed but can't start. Reinstall it, then try again.`
      };
    case "available":
      switch (status.authentication) {
        case "authenticated":
          return { state: "ready", kind: null, message: `${label} is ready.` };
        case "unauthenticated":
          return {
            state: "attention",
            kind: "provider-signed-out",
            message: `${label} isn't signed in. Sign in to ${label}, then try again.`
          };
        default:
          // The sign-in couldn't be checked; asking will tell.
          return { state: "unknown", kind: null, message: `${label} is installed.` };
      }
    default:
      return unknownView(label);
  }
}

/**
 * @param {string} label
 * @returns {ProviderView}
 */
function unknownView(label) {
  return { state: "unknown", kind: null, message: `Pervue couldn't check ${label}.` };
}
