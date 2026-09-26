/**
 * Model choice (protocol v1 `conversation.send` `model`). The setup page saves
 * one model per provider under MODEL_PREFERENCES_KEY; no entry means the
 * provider's own default. A model is only ever sent to a provider whose
 * status reports `model_selection: true`.
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
  return models
    .filter((model) => isModelId(model?.id) && typeof model?.label === "string" &&
      model.label.length > 0 && model.label.length <= MAX_LABEL_LENGTH)
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
  void readModelPreferences(storage).then((saved) => { preferences = saved; });
  storageChanges?.addListener((changes, area) => {
    if (area === "local" && changes[MODEL_PREFERENCES_KEY]) {
      preferences = validPreferences(changes[MODEL_PREFERENCES_KEY].newValue);
    }
  });
  return {
    /**
     * @param {string | undefined} providerId
     * @param {boolean} supported whether that provider reports model_selection
     * @returns {string | undefined}
     */
    modelFor(providerId, supported) {
      return supported && providerId ? preferences[providerId] : undefined;
    }
  };
}
