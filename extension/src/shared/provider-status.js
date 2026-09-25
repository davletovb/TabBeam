/**
 * Contract between a UI page and the service worker for the provider's status
 * (EXT-04).
 *
 * The page sends `{type: PROVIDER_STATUS_MESSAGE}` with
 * `chrome.runtime.sendMessage`. The worker asks the native host for the
 * default provider's `provider.status` and answers with one of:
 *
 * - `{provider_id, status}`: the provider's status in the DOC-02 vocabulary,
 *   `{availability, authentication, capabilities}`;
 * - `{provider_id, error}`: why there is no status, as a DOC-02 error, such as
 *   `HOST_NOT_INSTALLED` when the companion app is missing.
 */
export const PROVIDER_STATUS_MESSAGE = "pervue.provider-status";

/**
 * @typedef {{
 *   availability: "available" | "unavailable" | "not_found" | "unknown",
 *   authentication: "authenticated" | "unauthenticated" | "unknown",
 *   capabilities: Record<string, boolean | "unknown">
 * }} ProviderStatus
 */

/**
 * @typedef {{code: string, reason: string, message: string, retryable: boolean}} ErrorBody
 */

/**
 * @typedef {{provider_id: string, status: ProviderStatus} |
 *   {provider_id: string, error: ErrorBody}} ProviderStatusResponse
 */
