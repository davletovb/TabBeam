/**
 * The Search the web switch in the composer (EXT-16). When it's on, the next
 * questions ask the chosen AI provider to search the web and cite what it
 * found (protocol v1 `conversation.send` `search`). It stays on until it's
 * turned off, and it follows the provider's `web_search` capability: it's
 * off and unavailable for a provider whose status says it can't search, and
 * available while that's still being checked (the host refuses a search the
 * provider can't do, with a clear message).
 *
 * @param {HTMLButtonElement} button
 * @param {{onChange?(on: boolean): void}} [options]
 */
export function bindSearchToggle(button, options = {}) {
  let on = false;
  let supported = true;
  let label = "This provider";

  function sync() {
    button.setAttribute("aria-pressed", String(on));
    button.disabled = !supported;
    button.title = !supported
      ? `${label} can't search the web.`
      : on
        ? "Web search is on: answers cite the pages they used"
        : "Search the web for your questions";
  }

  /** @param {boolean} next */
  function set(next) {
    const value = next && supported;
    if (value === on) return;
    on = value;
    sync();
    options.onChange?.(on);
  }

  button.addEventListener("click", () => {
    if (supported) set(!on);
  });
  sync();

  return {
    /** Whether the next question searches the web. */
    isOn: () => on,
    /** @param {boolean} next */
    set,
    /**
     * @param {boolean | "unknown" | undefined} capability the provider's
     *   `web_search`; undefined while it hasn't been checked
     * @param {string} [providerLabel]
     */
    setSupported(capability, providerLabel = label) {
      label = providerLabel;
      supported = capability !== false;
      if (!supported && on) {
        on = false;
        options.onChange?.(false);
      }
      sync();
    }
  };
}
