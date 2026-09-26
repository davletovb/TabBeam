/**
 * Copies `text`, failing when the Clipboard API is missing rather than
 * seeming to succeed.
 *
 * @param {string} text
 */
export async function copyText(text) {
  const clipboard = globalThis.navigator?.clipboard;
  if (!clipboard) throw new Error("clipboard unavailable");
  await clipboard.writeText(text);
}

/**
 * Copies `text` from a button labelled "Copy", saying on the button whether
 * it worked; a success reads "Copied" with `data-copied` for a moment.
 *
 * @param {{textContent: string | null, setAttribute(name: string, value: string): void, removeAttribute(name: string): void}} button
 * @param {string} text
 */
export async function copyWithFeedback(button, text) {
  try {
    await copyText(text);
    button.textContent = "Copied";
    button.setAttribute("data-copied", "true");
    setTimeout(() => {
      button.textContent = "Copy";
      button.removeAttribute("data-copied");
    }, 1500);
  } catch {
    button.textContent = "Couldn't copy";
    button.removeAttribute("data-copied");
  }
}
