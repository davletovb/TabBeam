/**
 * The empty popup's starter suggestions. Each attaches its context the same
 * way the chips do and leaves its question in the composer to send or edit.
 *
 * A suggestion and its context go together: while a capture is pending the
 * chips ignore clicks, so a second suggestion would change the question but
 * not the context. Suggestions are ignored until the capture settles.
 *
 * @param {HTMLElement[]} suggestions each with `data-prompt`, and `data-context` of "page" or "selection"
 * @param {{
 *   input: HTMLTextAreaElement,
 *   chips: {page: HTMLElement, selection: HTMLElement},
 *   contextControls: {isPending(): boolean},
 *   onFill?(): void
 * }} options
 */
export function bindSuggestions(suggestions, { input, chips, contextControls, onFill }) {
  for (const suggestion of suggestions) {
    suggestion.addEventListener("click", () => {
      if (contextControls.isPending()) return;
      const source = suggestion.getAttribute("data-context");
      if (source === "page" || source === "selection") chips[source].click();
      input.value = suggestion.getAttribute("data-prompt") ?? "";
      onFill?.();
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    });
  }
}
