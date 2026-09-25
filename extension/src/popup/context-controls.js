import { CONTEXT_CAPTURE_MESSAGE } from "../background/selection-capture.js";

/**
 * Context lives only in this popup. No page is read on startup or Ask; the
 * user chooses a source and sees its preview before submitting a question.
 *
 * @param {{none: HTMLButtonElement, selection: HTMLButtonElement, page: HTMLButtonElement, status: HTMLElement, preview: HTMLElement}} elements
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 */
export function bindContextControls(elements, runtime) {
  const { none, selection, page, status, preview } = elements;
  /** @type {any | null} */
  let context = null;
  let pending = false;
  let generation = 0;

  /** @param {"none" | "selection" | "page"} mode */
  function setChoice(mode) {
    none.setAttribute("aria-pressed", String(mode === "none"));
    selection.setAttribute("aria-pressed", String(mode === "selection"));
    page.setAttribute("aria-pressed", String(mode === "page"));
  }

  function clear() {
    generation += 1;
    pending = false;
    context = null;
    selection.disabled = false;
    page.disabled = false;
    preview.hidden = true;
    preview.textContent = "";
    setChoice("none");
    status.textContent = "No context attached. Choose a source to grant access for this question.";
  }

  /** @param {"selection" | "page"} mode */
  async function capture(mode) {
    if (pending) {
      return;
    }
    const current = ++generation;
    context = null;
    setChoice("none");
    pending = true;
    selection.disabled = true;
    page.disabled = true;
    preview.hidden = true;
    preview.textContent = "";
    status.textContent = "Checking page access…";
    try {
      const result = await runtime.sendMessage({
        type: CONTEXT_CAPTURE_MESSAGE,
        mode,
        intent: "user_click"
      });
      if (generation !== current) {
        return;
      }
      if (result?.ok !== true || typeof result.context?.text !== "string") {
        const permission = result?.permission === "denied" ? "Page access denied. " : "";
        status.textContent = permission + (result?.error?.message ?? "Context is unavailable.");
        return;
      }
      context = result.context;
      setChoice(mode);
      const excerpt = [...context.text].slice(0, 240).join("");
      preview.textContent =
        `${context.page.title} • ${context.page.url}\n${excerpt}` +
        (context.text.length > excerpt.length ? "…" : "");
      preview.hidden = false;
      status.textContent =
        `Page access granted. ${mode === "page" ? "Current page" : "Selection"} attached` +
        (context.truncated ? " (truncated)." : ".");
    } catch {
      if (generation === current) {
        status.textContent = "Page access unavailable. Try again or choose No context.";
      }
    } finally {
      if (generation === current) {
        pending = false;
        selection.disabled = false;
        page.disabled = false;
      }
    }
  }

  none.addEventListener("click", clear);
  selection.addEventListener("click", () => capture("selection"));
  page.addEventListener("click", () => capture("page"));
  clear();
  return {
    getContext: () => context,
    isPending: () => pending
  };
}
