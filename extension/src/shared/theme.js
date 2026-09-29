export const THEME_STORAGE_KEY = "tabbeam.theme";
/** @typedef {"system" | "light" | "dark"} ThemePreference */
/** @type {readonly ThemePreference[]} */
export const THEME_PREFERENCES = Object.freeze(["system", "light", "dark"]);

/** @param {unknown} value @returns {ThemePreference} */
export function normalizeThemePreference(value) {
  return value === "light" || value === "dark" || value === "system" ? value : "system";
}

/**
 * @param {HTMLElement} root
 * @param {unknown} preference
 */
export function applyTheme(root, preference) {
  const normalized = normalizeThemePreference(preference);
  if (normalized === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", normalized);
  return normalized;
}

/** @param {Storage | undefined} cache */
function cachedPreference(cache = globalThis.localStorage) {
  try {
    return normalizeThemePreference(cache?.getItem(THEME_STORAGE_KEY));
  } catch {
    return "system";
  }
}

/** @param {ThemePreference} preference @param {Storage | undefined} cache */
function cachePreference(preference, cache = globalThis.localStorage) {
  try {
    cache?.setItem(THEME_STORAGE_KEY, preference);
  } catch {
    // The extension storage value remains authoritative.
  }
}

/**
 * @param {{get(key: string): Promise<Record<string, any>>}} storage
 * @param {HTMLElement} root
 * @param {Storage | undefined} [cache]
 */
export async function loadTheme(storage, root, cache = globalThis.localStorage) {
  try {
    const stored = await storage.get(THEME_STORAGE_KEY);
    const preference = normalizeThemePreference(stored[THEME_STORAGE_KEY]);
    cachePreference(preference, cache);
    return applyTheme(root, preference);
  } catch {
    return applyTheme(root, cachedPreference(cache));
  }
}

/**
 * Listeners are installed synchronously before storage is read so user input
 * cannot be lost behind a slow or failed chrome.storage lookup.
 *
 * @param {HTMLSelectElement} select
 * @param {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>}} storage
 * @param {HTMLElement} root
 * @param {{addListener(callback: (changes: Record<string, any>, area: string) => void): void}} [storageChanges]
 * @param {Storage | undefined} [cache]
 */
export async function bindThemeSelect(
  select,
  storage,
  root,
  storageChanges,
  cache = globalThis.localStorage
) {
  let userChanged = false;
  const initial = applyTheme(root, cachedPreference(cache));
  select.value = initial;

  select.addEventListener("change", () => {
    userChanged = true;
    const preference = applyTheme(root, select.value);
    select.value = preference;
    cachePreference(preference, cache);
    void storage.set({ [THEME_STORAGE_KEY]: preference }).catch(() => {});
  });
  storageChanges?.addListener((changes, area) => {
    if (area !== "local" || !changes[THEME_STORAGE_KEY]) return;
    const preference = normalizeThemePreference(changes[THEME_STORAGE_KEY].newValue);
    cachePreference(preference, cache);
    applyTheme(root, preference);
    select.value = preference;
  });

  let storedPreference = initial;
  try {
    const stored = await storage.get(THEME_STORAGE_KEY);
    storedPreference = normalizeThemePreference(stored[THEME_STORAGE_KEY]);
    cachePreference(storedPreference, cache);
  } catch {
    return initial;
  }
  if (!userChanged) {
    applyTheme(root, storedPreference);
    select.value = storedPreference;
  }
  return userChanged ? normalizeThemePreference(select.value) : storedPreference;
}
