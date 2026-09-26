import { providerView } from "../popup/provider-state.js";
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
 *
 * @param {{options: HTMLElement, model: HTMLSelectElement, modelNote: HTMLElement}} elements
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>}} storage
 * @param {{addListener(callback: (changes: Record<string, any>, area: string) => void): void}} [storageChanges]
 */
export function bindProviderSettings(elements, runtime, storage, storageChanges) {
  const { options, model, modelNote } = elements;
  const doc = options.ownerDocument;
  let chosen = DEFAULT_PROVIDER_ID;
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
    chosen = id;
    for (const [other, { input }] of rows) input.checked = other === id;
    if (persist) void storage.set({ [PROVIDER_STORAGE_KEY]: id }).catch(() => {});
    showModel();
  }

  function showModel() {
    const label = providerLabel(chosen);
    const response = statuses.get(chosen);
    const support = response?.status?.capabilities?.model_selection;
    // v1 has no way to list or pass models yet, so even a provider that could
    // choose keeps its own default until the companion supports it.
    const option = doc.createElement("option");
    option.value = "";
    option.textContent = `${label} default`;
    model.replaceChildren(option);
    model.disabled = true;
    if (response === undefined) {
      modelNote.textContent = `Checking what ${label} supports…`;
    } else if (support === true) {
      modelNote.textContent = `${label} can switch models, but this version of the companion can't pass a choice yet, so it uses ${label}'s default.`;
    } else {
      modelNote.textContent = `${label} uses the model set in its own configuration. Choosing it here needs a companion that supports model selection.`;
    }
  }

  storageChanges?.addListener((changes, area) => {
    const next = changes[PROVIDER_STORAGE_KEY]?.newValue;
    if (area === "local" && typeof next === "string" && next !== chosen) choose(next, false);
  });

  const ready = (async () => {
    try {
      const saved = (await storage.get(PROVIDER_STORAGE_KEY))[PROVIDER_STORAGE_KEY];
      choose(typeof saved === "string" && rows.has(saved) ? saved : DEFAULT_PROVIDER_ID, false);
    } catch {
      choose(DEFAULT_PROVIDER_ID, false);
    }
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
