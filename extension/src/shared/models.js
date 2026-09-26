/**
 * Model choice (protocol v1 `conversation.send` `model`). The setup page saves
 * one model per provider under MODEL_PREFERENCES_KEY; no entry means the
 * provider's own default. A saved model is sent unless the provider's status
 * says it can't switch (`model_selection` other than true). While that status
 * is still being checked the model is sent anyway: the host refuses it
 * (MODEL_SELECTION_UNSUPPORTED) for a provider that can't switch, so a saved
 * choice is never silently dropped.
 */

export const MODEL_PREFERENCES_KEY = "pervue.models";

/** The host's rule for a model ID: it reaches a provider's command line. */
export const MODEL_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:/@-]{0,127}$/;

const MAX_MODELS = 32;
const MAX_LABEL_LENGTH = 64;

/** @param {unknown} model @returns {model is string} */
export function isModelId(model) {
  return typeof model === "string" && MODEL_ID_PATTERN.test(model);
}

/**
 * The suggested models in a provider status, kept only if well formed.
 *
 * @param {unknown} models
 * @returns {{id: string, label: string}[]}
 */
export function suggestedModels(models) {
  if (!Array.isArray(models)) return [];
  const seen = new Set();
  return models
    .filter((model) => isModelId(model?.id) && typeof model?.label === "string" &&
      model.label.length > 0 && model.label.length <= MAX_LABEL_LENGTH &&
      !seen.has(model.id) && Boolean(seen.add(model.id)))
    .slice(0, MAX_MODELS)
    .map(({ id, label }) => ({ id, label }));
}

/**
 * Every provider's saved model.
 *
 * @param {{get(key: string): Promise<Record<string, any>>}} storage
 * @returns {Promise<Record<string, string>>}
 */
export async function readModelPreferences(storage) {
  try {
    return validPreferences((await storage.get(MODEL_PREFERENCES_KEY))[MODEL_PREFERENCES_KEY]);
  } catch {
    return {};
  }
}

/** @param {unknown} value @returns {Record<string, string>} */
export function validPreferences(value) {
  if (!value || typeof value !== "object") return {};
  return Object.fromEntries(Object.entries(value).filter(([, model]) => isModelId(model)));
}

/**
 * Tracks the saved models in a view, including changes made on the setup
 * page while it's open, and says which model a question to `providerId`
 * should ask for.
 *
 * @param {{get(key: string): Promise<Record<string, any>>}} storage
 * @param {{addListener(callback: (changes: Record<string, any>, area: string) => void): void}} [storageChanges]
 */
export function followModelPreferences(storage, storageChanges) {
  /** @type {Record<string, string>} */
  let preferences = {};
  // A change that lands first is newer than the initial read.
  let changed = false;
  void readModelPreferences(storage).then((saved) => {
    if (!changed) preferences = saved;
  });
  storageChanges?.addListener((changes, area) => {
    if (area === "local" && changes[MODEL_PREFERENCES_KEY]) {
      changed = true;
      preferences = validPreferences(changes[MODEL_PREFERENCES_KEY].newValue);
    }
  });
  /** @type {Map<string, boolean>} whether each checked provider can switch */
  const switchable = new Map();
  return {
    /**
     * Records what a provider's status says (a provider selector's
     * `onChange` selection); a selection without a status changes nothing.
     *
     * @param {{providerId: string, status: any | null}} selection
     */
    observe(selection) {
      if (selection.status) {
        switchable.set(selection.providerId, selection.status.capabilities?.model_selection === true);
      }
    },
    /**
     * @param {string | undefined} providerId
     * @returns {string | undefined}
     */
    modelFor(providerId) {
      return providerId && switchable.get(providerId) !== false ? preferences[providerId] : undefined;
    }
  };
}
