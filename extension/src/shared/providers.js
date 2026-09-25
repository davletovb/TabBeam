/**
 * Providers as the extension names them. What the extension does never
 * depends on a provider's ID (DOC-02 §5): the ID only chooses the provider
 * the host runs, and the label is how the popup refers to it.
 */

// Milestone B's first provider (PRO-03). Choosing among providers is EXT-15.
export const DEFAULT_PROVIDER_ID = "codex";

const LABELS = new Map([
  ["codex", "Codex"],
  ["fake", "The test provider"]
]);

/**
 * The name the popup uses for a provider: its label, or else its ID.
 *
 * @param {string} providerId
 */
export function providerLabel(providerId) {
  return LABELS.get(providerId) ?? providerId;
}
