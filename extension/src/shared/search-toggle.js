/**
 * The Web search switch in the composer (EXT-16). When it's on, questions ask
 * the chosen AI provider to search the web and cite what it found (protocol
 * v1 `conversation.send` `search`). It's on by default and stays as the
 * person leaves it.
 *
 * It follows the provider's `web_search` capability: for a provider whose
 * status says it can't search, it's off and unavailable; the person's choice
 * comes back with a provider that can. While a status is still being checked
 * it's available (the host refuses a search the provider can't do, with a
 * clear message).
 *
 * @param {HTMLButtonElement} button
 * @param {{onChange?(on: boolean): void, initial?: boolean}} [options]
 */
export function bindSearchToggle(button, options = {}) {
  // What the person chose, and whether the provider can search.
  let wanted = options.initial ?? true;
  let supported = true;
  let label = "This provider";
  let on = wanted && supported;

  function sync() {
    button.setAttribute("aria-pressed", String(on));
    button.disabled = !supported;
    button.title = !supported
      ? `${label} can't search the web.`
      : on
        ? "Web search is on: answers cite the pages they used"
        : "Search the web for your questions";
  }

  function update() {
    const next = wanted && supported;
    const changed = next !== on;
    on = next;
    sync();
    if (changed) options.onChange?.(on);
  }

  button.addEventListener("click", () => {
    if (!supported) return;
    wanted = !wanted;
    update();
  });
  sync();

  return {
    /** Whether the next question searches the web. */
    isOn: () => on,
    /** @param {boolean} next the person's choice */
    set(next) {
      wanted = next;
      update();
    },
    /**
     * @param {boolean | "unknown" | undefined} capability the provider's
     *   `web_search`; undefined while it hasn't been checked
     * @param {string} [providerLabel]
     */
    setSupported(capability, providerLabel = label) {
      label = providerLabel;
      supported = capability !== false;
      update();
    }
  };
}
