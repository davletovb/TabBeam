export const THEME_STORAGE_KEY = "pervue.theme";
/** @typedef {"system" | "light" | "dark"} ThemePreference */
/** @type {readonly ThemePreference[]} */
export const THEME_PREFERENCES = Object.freeze(["system", "light", "dark"]);

/** @param {unknown} value @returns {ThemePreference} */
export function normalizeThemePreference(value) {
  return value === "light" || value === "dark" || value === "system" ? value : "system";
}

/**
 * Applies a preference without reading the OS theme ourselves: `system`
 * leaves the root on `color-scheme: light dark`, while explicit choices
 * narrow the root to one scheme.
 *
 * @param {HTMLElement} root
 * @param {unknown} preference
 */
export function applyTheme(root, preference) {
  const normalized = normalizeThemePreference(preference);
  if (normalized === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", normalized);
  return normalized;
}

/**
 * @param {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>}} storage
 * @param {HTMLElement} root
 */
export async function loadTheme(storage, root) {
  const stored = await storage.get(THEME_STORAGE_KEY);
  return applyTheme(root, stored[THEME_STORAGE_KEY]);
}

/**
 * @param {HTMLSelectElement} select
 * @param {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>}} storage
 * @param {HTMLElement} root
 * @param {{addListener(callback: (changes: Record<string, any>, area: string) => void): void}} [storageChanges]
 */
export async function bindThemeSelect(select, storage, root, storageChanges) {
  const initial = await loadTheme(storage, root);
  select.value = initial;
  select.addEventListener("change", () => {
    const preference = applyTheme(root, select.value);
    select.value = preference;
    void storage.set({ [THEME_STORAGE_KEY]: preference });
  });
  storageChanges?.addListener((changes, area) => {
    if (area !== "local" || !changes[THEME_STORAGE_KEY]) return;
    const preference = applyTheme(root, changes[THEME_STORAGE_KEY].newValue);
    select.value = preference;
  });
  return initial;
}
