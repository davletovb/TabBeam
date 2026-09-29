import { providerView } from "../popup/provider-state.js";
import { MODEL_PREFERENCES_KEY, isModelId, readModelPreferences, suggestedModels, validPreferences } from "../shared/models.js";
import { PROVIDER_STATUS_MESSAGE } from "../shared/provider-status.js";
import { PROVIDER_STORAGE_KEY } from "../shared/provider-selector.js";
import { DEFAULT_PROVIDER_ID, USER_PROVIDERS, providerLabel } from "../shared/providers.js";

/**
 * The provider and model choice on the companion setup page. The popup and
 * full view follow what's saved here (PROVIDER_STORAGE_KEY); a conversation
 * keeps the provider it started with.
 *
 * Each provider shows its own status, so the page also says what's left to
 * set up. A provider can be chosen before it's ready: the popup falls back to
 * one that is until then.
 *
 * Model choice follows the chosen provider's `model_selection` capability
 * (DOC-02 §5): the page never offers a choice the companion can't honour.
 * Where a provider can switch, it lists the provider's suggestions and any
 * other valid model ID ("Other model…"), saved per provider under
 * MODEL_PREFERENCES_KEY; "default" removes the entry.
 *
 * @param {{
 *   options: HTMLElement,
 *   model: HTMLSelectElement,
 *   modelNote: HTMLElement,
 *   customField?: HTMLElement,
 *   customInput?: HTMLInputElement,
 *   customSave?: HTMLElement
 * }} elements
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>}} storage
 * @param {{addListener(callback: (changes: Record<string, any>, area: string) => void): void}} [storageChanges]
 */
export function bindProviderSettings(elements, runtime, storage, storageChanges) {
  const { options, model, modelNote, customField, customInput, customSave } = elements;
  const doc = options.ownerDocument;
  const OTHER = "other";
  let chosen = DEFAULT_PROVIDER_ID;
  // A choice made (here or elsewhere) before the saved one is read is newer.
  let touched = false;
  /** @type {Record<string, string>} each provider's saved model */
  let preferences = {};
  let modelsChanged = false;
  /** @type {Map<string, any>} status responses; null for a check that failed */
  const statuses = new Map();
  /** @type {Map<string, {input: HTMLInputElement, status: HTMLElement}>} */
  const rows = new Map();

  for (const { id, label } of USER_PROVIDERS) {
    const row = doc.createElement("label");
    row.className = "provider-option";
    const input = /** @type {HTMLInputElement} */ (doc.createElement("input"));
    input.type = "radio";
    input.name = "provider";
    input.value = id;
    input.addEventListener("change", () => {
      if (input.checked) choose(id, true);
    });
    const text = doc.createElement("span");
    text.className = "provider-option-text";
    const name = doc.createElement("strong");
    name.textContent = label;
    const status = doc.createElement("span");
    status.className = "provider-state";
    status.setAttribute("data-state", "checking");
    status.textContent = `Checking ${label}…`;
    text.append(name, status);
    row.append(input, text);
    options.append(row);
    rows.set(id, { input, status });
  }

  /** @param {string} id @param {boolean} persist */
  function choose(id, persist) {
    if (!rows.has(id)) return;
    touched = true;
    chosen = id;
    for (const [other, { input }] of rows) input.checked = other === id;
    if (persist) void storage.set({ [PROVIDER_STORAGE_KEY]: id }).catch(() => {});
    showModel();
  }

  /** @param {string} value @param {string} text */
  function option(value, text) {
    const node = /** @type {HTMLOptionElement} */ (doc.createElement("option"));
    node.value = value;
    node.textContent = text;
    return node;
  }

  /** @param {string} message @param {"error" | null} [state] */
  function note(message, state = null) {
    modelNote.textContent = message;
    if (state) modelNote.setAttribute("data-state", state);
    else modelNote.removeAttribute("data-state");
  }

  function showModel() {
    const label = providerLabel(chosen);
    const response = statuses.get(chosen);
    const supported = response?.status?.capabilities?.model_selection === true;
    const saved = preferences[chosen];
    const suggestions = supported ? suggestedModels(response?.status?.models) : [];
    const nodes = [option("", `${label} default`), ...suggestions.map(({ id, label: name }) => option(id, name))];
    if (supported) nodes.push(option(OTHER, "Other model…"));
    model.replaceChildren(...nodes);
    model.disabled = !supported;
    const listed = !saved || suggestions.some(({ id }) => id === saved);
    model.value = supported ? (listed ? saved ?? "" : OTHER) : "";
    showCustom(supported && model.value === OTHER, listed ? "" : saved ?? "");

    if (response === undefined) {
      note(`Checking what ${label} supports…`);
    } else if (response === null) {
      note(`TabBeam couldn't check what ${label} supports, so its model can't be chosen yet. Reopen this page once the companion is connected.`);
    } else if (!supported) {
      note(`${label} uses the model set in its own configuration. This companion can't switch its model.`);
    } else if (!saved) {
      note(`New questions to ${label} use its default model.`);
    } else {
      note(`New questions to ${label} use ${saved}. If ${label} doesn't recognize a model, the question fails and says so.`);
    }
  }

  /** @param {boolean} visible @param {string} value */
  function showCustom(visible, value) {
    if (!customField || !customInput) return;
    customField.hidden = !visible;
    customInput.value = value;
    customInput.removeAttribute("aria-invalid");
  }

  /** @param {string | null} id null for the provider's default */
  function saveModel(id) {
    modelsChanged = true;
    const next = { ...preferences };
    if (id) next[chosen] = id;
    else delete next[chosen];
    preferences = next;
    void storage.set({ [MODEL_PREFERENCES_KEY]: next }).catch(() => {});
    showModel();
  }

  model.addEventListener("change", () => {
    if (model.value === OTHER) {
      showCustom(true, "");
      customInput?.focus();
      note(`Enter a model ID ${providerLabel(chosen)} accepts, then choose Use model.`);
      return;
    }
    saveModel(model.value || null);
  });

  function saveCustom() {
    if (!customInput) return;
    const value = customInput.value.trim();
    if (!isModelId(value)) {
      customInput.setAttribute("aria-invalid", "true");
      note("A model ID is up to 128 letters, digits, and . _ - : / @, starting with a letter or digit.", "error");
      return;
    }
    saveModel(value);
  }
  customSave?.addEventListener("click", saveCustom);
  customInput?.addEventListener("keydown", (/** @type {any} */ event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      saveCustom();
    }
  });

  storageChanges?.addListener((changes, area) => {
    if (area !== "local") return;
    const next = changes[PROVIDER_STORAGE_KEY]?.newValue;
    if (typeof next === "string" && next !== chosen) choose(next, false);
    if (changes[MODEL_PREFERENCES_KEY]) {
      modelsChanged = true;
      preferences = validPreferences(changes[MODEL_PREFERENCES_KEY].newValue);
      showModel();
    }
  });

  const ready = (async () => {
    const savedModels = await readModelPreferences(storage);
    if (!modelsChanged) preferences = savedModels;
    /** @type {unknown} */
    let saved;
    try {
      saved = (await storage.get(PROVIDER_STORAGE_KEY))[PROVIDER_STORAGE_KEY];
    } catch {
      // Falls back to the default below.
    }
    if (!touched) choose(typeof saved === "string" && rows.has(saved) ? saved : DEFAULT_PROVIDER_ID, false);
    else showModel();
    await Promise.all(USER_PROVIDERS.map(async ({ id, label }) => {
      let response = null;
      try {
        const reply = await runtime.sendMessage({ type: PROVIDER_STATUS_MESSAGE, provider_id: id, record_diagnostics: false });
        response = reply?.provider_id === id ? reply : null;
      } catch {
        // Shown as a check that couldn't be made.
      }
      statuses.set(id, response);
      const view = providerView(response, label);
      const row = /** @type {{input: HTMLInputElement, status: HTMLElement}} */ (rows.get(id));
      row.status.textContent = view.message;
      row.status.setAttribute("data-state", view.state);
      if (view.kind) row.status.setAttribute("data-kind", view.kind);
      else row.status.removeAttribute("data-kind");
      if (id === chosen) showModel();
    }));
  })();

  return { ready, getProviderId: () => chosen };
}
