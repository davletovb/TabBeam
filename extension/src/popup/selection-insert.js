import { SELECTION_CAPTURE_MESSAGE } from "../background/selection-capture.js";

/**
 * A separate, explicit action. The selected text is placed in the composer
 * for review; sending it remains the user's choice via the Ask button.
 *
 * @param {HTMLButtonElement} button
 * @param {HTMLTextAreaElement} input
 * @param {HTMLElement} status
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 */
export function bindSelectionInsert(button, input, status, runtime) {
  let pending = false;
  button.addEventListener("click", async () => {
    if (pending) {
      return;
    }
    pending = true;
    button.disabled = true;
    status.textContent = "Reading selection…";

    try {
      const result = await runtime.sendMessage({ type: SELECTION_CAPTURE_MESSAGE });
      if (result?.ok !== true || typeof result.text !== "string" || !result.text.trim()) {
        status.textContent = selectionFailure(result?.error?.reason);
        return;
      }
      const quote = result.text
        .split("\n")
        .map((line) => `> ${line}`)
        .join("\n");
      const start = input.selectionStart ?? input.value.length;
      const end = input.selectionEnd ?? start;
      const prefix = start > 0 && !input.value.slice(0, start).endsWith("\n") ? "\n\n" : "";
      input.setRangeText(prefix + quote, start, end, "end");
      input.focus();
      status.textContent =
        result.truncated === true
          ? "Inserted the first 16 KB of selected text. Review it before asking."
          : "Selected text inserted. Review it before asking.";
    } catch {
      status.textContent = selectionFailure("PAGE_NOT_SCRIPTABLE");
    } finally {
      pending = false;
      button.disabled = false;
    }
  });
}

/** @param {unknown} reason */
function selectionFailure(reason) {
  switch (reason) {
    case "SELECTION_UNAVAILABLE":
      return "Select some text on the page first.";
    case "PAGE_ACCESS_DENIED":
      return "Selection can only be inserted from the Pervue popup.";
    default:
      return "This page does not support selected-text capture.";
  }
}
