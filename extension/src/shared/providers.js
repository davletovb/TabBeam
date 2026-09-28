/**
 * User-selectable providers. IDs only select native adapters; feature behavior
 * is driven by provider.status capabilities, never by ID checks in UI code.
 */
export const DEFAULT_PROVIDER_ID = "codex";

export const USER_PROVIDERS = Object.freeze([
  Object.freeze({ id: "codex", label: "Codex" }),
  Object.freeze({ id: "claude", label: "Claude" }),
  Object.freeze({ id: "gemini", label: "Gemini" })
]);

/** @type {Map<string, string>} */
const LABELS = new Map(
  USER_PROVIDERS.map(({ id, label }) => /** @type {[string, string]} */ ([id, label]))
);
LABELS.set("fake", "The test provider");

export const PROVIDER_ID_PATTERN = /^[a-z][a-z0-9_-]{0,31}$/;

/** @param {string} providerId */
export function providerLabel(providerId) {
  return LABELS.get(providerId) ?? providerId;
}

/** @param {unknown} providerId */
export function isProviderId(providerId) {
  return typeof providerId === "string" && PROVIDER_ID_PATTERN.test(providerId);
}
