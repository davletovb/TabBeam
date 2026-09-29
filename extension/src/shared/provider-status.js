/**
 * Contract between a UI page and the service worker for the provider's status
 * (EXT-04).
 *
 * The page sends `{type: PROVIDER_STATUS_MESSAGE, provider_id}` with
 * `chrome.runtime.sendMessage`. The worker asks the native host for the
 * requested provider's `provider.status` and answers with one of:
 *
 * - `{provider_id, status}`: the provider's status in the DOC-02 vocabulary,
 *   `{availability, authentication, capabilities}`;
 * - `{provider_id, error}`: why there is no status, as a DOC-02 error, such as
 *   `HOST_NOT_INSTALLED` when the companion app is missing.
 *
 * The worker answers within {@link PROVIDER_STATUS_TIMEOUT_MS}.
 */
export const PROVIDER_STATUS_MESSAGE = "tabbeam.provider-status";

/**
 * How long the worker waits for the host's answer. After that it answers with
 * `REQUEST_TIMEOUT` and stops listening for the host's. The host itself gives
 * up on a stuck sign-in check after 10 seconds, so only a host that has
 * stopped answering takes this long.
 */
export const PROVIDER_STATUS_TIMEOUT_MS = 15_000;

/**
 * @typedef {{
 *   availability: "available" | "unavailable" | "not_found" | "unknown",
 *   authentication: "authenticated" | "unauthenticated" | "unknown",
 *   capabilities: Record<string, boolean | "unknown">,
 *   models?: {id: string, label: string}[]
 * }} ProviderStatus
 */

/**
 * @typedef {{code: string, reason: string, message: string, retryable: boolean}} ErrorBody
 */

/**
 * @typedef {{provider_id: string, status: ProviderStatus} |
 *   {provider_id: string, error: ErrorBody}} ProviderStatusResponse
 */
