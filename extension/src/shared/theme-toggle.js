import { normalizeThemePreference } from "./theme.js";

/**
 * A segmented System / Light / Dark switch in front of the theme <select>
 * that ./theme.js binds. The select stays the source of truth: a click sets
 * it and dispatches `change`, and the pressed button follows the preference
 * theme.js writes to <html data-theme>, so a change made on another surface
 * shows here too.
 *
 * @param {HTMLElement} group holds one button per preference, each with `data-theme-option`
 * @param {HTMLSelectElement} select
 * @param {HTMLElement} root
 */
export function bindThemeToggle(group, select, root) {
  const buttons = Array.from(group.querySelectorAll("button[data-theme-option]"));

  function sync() {
    const current = normalizeThemePreference(root.getAttribute("data-theme") ?? "system");
    for (const button of buttons) {
      button.setAttribute("aria-pressed", String(button.getAttribute("data-theme-option") === current));
    }
  }

  for (const button of buttons) {
    button.addEventListener("click", () => {
      select.value = normalizeThemePreference(button.getAttribute("data-theme-option"));
      select.dispatchEvent(new Event("change"));
    });
  }

  new MutationObserver(sync).observe(root, { attributes: true, attributeFilter: ["data-theme"] });
  sync();
}
