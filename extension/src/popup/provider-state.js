import { describeFailure } from "../shared/outcomes.js";
import { PROVIDER_STATUS_MESSAGE } from "../shared/provider-status.js";
import { DEFAULT_PROVIDER_ID, providerLabel } from "../shared/providers.js";

/** @typedef {import("../shared/provider-status.js").ProviderStatusResponse} ProviderStatusResponse */
/** @typedef {{kind: string, message?: string}} Outcome */

/**
 * How long the popup waits for the provider's status before it stops saying
 * it is checking. The service worker answers sooner, within
 * PROVIDER_STATUS_TIMEOUT_MS, so the popup gives up on its own only if the
 * worker never answers.
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
 * @param {{setupLink?: HTMLElement, checkOnOpen?: boolean}} [options]
 *   `checkOnOpen: false` leaves the checks to a provider selector, whose
 *   results arrive through `follow`.
 */
export function bindProviderState(element, runtime, timers = {}, options = {}) {
  const schedule = timers.schedule ?? setTimeout;
  const cancel = timers.cancel ?? clearTimeout;
  const setupLink = options.setupLink;
  let providerId = DEFAULT_PROVIDER_ID;
  let checking = false;
  /** @type {any} */
  let timer = null;
  let generation = 0;
  // A question's outcome was shown after the status check began: the check's
  // result, older news, doesn't replace it.
  let outcomeShown = false;

  /** @param {ProviderView} view */
  function show(view) {
    element.textContent = view.message;
    element.setAttribute("data-state", view.state);
    if (view.kind === null) element.removeAttribute("data-kind");
    else element.setAttribute("data-kind", view.kind);
    element.hidden = false;
    if (setupLink) {
      setupLink.hidden = view.kind !== "host-missing" && view.kind !== "host-unavailable";
    }
  }

  /**
   * Shows that `nextProvider` is being checked, giving up after
   * STATUS_WAIT_MS. Returns the generation the check's answer must match.
   * @param {string} nextProvider
   */
  function wait(nextProvider) {
    providerId = nextProvider;
    const label = providerLabel(providerId);
    generation += 1;
    const current = generation;
    outcomeShown = false;
    if (timer !== null) cancel(timer);
    checking = true;
    show({ state: "checking", kind: null, message: `Checking ${label}…` });
    timer = schedule(() => {
      if (current !== generation) return;
      checking = false;
      timer = null;
      show(unknownView(label));
    }, STATUS_WAIT_MS);
    return current;
  }

  /** @param {string} nextProvider @param {any} [knownResponse] @param {boolean} [explicit] */
  function setProvider(nextProvider, knownResponse, explicit = true) {
    if (knownResponse !== undefined) {
      providerId = nextProvider;
      generation += 1;
      outcomeShown = false;
      if (timer !== null) cancel(timer);
      checking = false;
      timer = null;
      show(providerView(knownResponse, providerLabel(providerId)));
      return;
    }
    const current = wait(nextProvider);
    const label = providerLabel(providerId);

    let sent;
    try {
      sent = Promise.resolve(runtime.sendMessage({
        type: PROVIDER_STATUS_MESSAGE,
        ...(explicit ? { provider_id: providerId } : {})
      }));
    } catch (error) {
      sent = Promise.reject(error);
    }
    sent.then(
      (response) => {
        if (current !== generation || !checking) return;
        checking = false;
        if (timer !== null) cancel(timer);
        timer = null;
        show(providerView(response, label));
      },
      () => {
        if (current !== generation || !checking) return;
        checking = false;
        if (timer !== null) cancel(timer);
        timer = null;
        show(unknownView(label));
      }
    );
  }

  // Preserve the original EXT-04 wire shape for the default-provider check.
  if (options.checkOnOpen === false) wait(DEFAULT_PROVIDER_ID);
  else setProvider(DEFAULT_PROVIDER_ID, undefined, false);

  return {
    setProvider,
    /**
     * Follows a provider selector: a new provider shows its status, or that
     * it is being checked; a check that finishes for the same provider shows
     * its result unless a question's outcome has been shown since.
     * @param {{providerId: string, response: any, providerChanged: boolean, statusUpdated: boolean}} selection
     */
    follow({ providerId: next, response, providerChanged, statusUpdated }) {
      if (providerChanged) {
        if (response === undefined) wait(next);
        else setProvider(next, response);
      } else if (statusUpdated && response !== undefined && next === providerId && !outcomeShown) {
        setProvider(next, response);
      }
    },
    /** @param {{kind: string, message?: string}} outcome */
    update(outcome) {
      const label = providerLabel(providerId);
      /** @type {ProviderView | null} */
      let view = null;
      if (outcome.kind === "completed") {
        view = { state: "ready", kind: null, message: `${label} is ready.` };
      } else if (STATE_KINDS.has(outcome.kind) && typeof outcome.message === "string") {
        view = { state: "attention", kind: outcome.kind, message: outcome.message };
      }
      if (view !== null) {
        checking = false;
        outcomeShown = true;
        generation += 1;
        if (timer !== null) cancel(timer);
        timer = null;
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
  return { state: "unknown", kind: null, message: `TabBeam couldn't check ${label}.` };
}
