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
  let handoffPending = false;
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
    handoffPending = false;
    context = null;
    selection.disabled = false;
    page.disabled = false;
    preview.hidden = true;
    preview.textContent = "";
    setChoice("none");
    status.textContent = "No context attached. Choose a source to grant access for this question.";
  }

  /** @param {any} result @param {"selection" | "page"} mode */
  function showCapture(result, mode) {
    if (
      result?.ok !== true || result.context?.mode !== mode ||
      typeof result.context?.text !== "string" ||
      typeof result.context?.page?.title !== "string" ||
      typeof result.context?.page?.url !== "string"
    ) {
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
  }

  // A menu opens the popup while extraction may still be running. Keep Ask
  // blocked until its one-time handoff completes; an explicit choice replaces it.
  function beginMenuHandoff() {
    const current = ++generation;
    pending = true;
    handoffPending = true;
    status.textContent = "Preparing browser context…";
    /** @param {{available?: boolean, result?: any} | null} handoff */
    return (handoff) => {
      if (generation !== current) return;
      pending = false;
      handoffPending = false;
      selection.disabled = false;
      page.disabled = false;
      if (handoff?.available) {
        const mode = handoff.result?.context?.mode;
        showCapture(handoff.result, mode === "selection" ? "selection" : "page");
      } else {
        clear();
      }
    };
  }

  /** @param {"selection" | "page"} mode */
  async function capture(mode) {
    if (pending && !handoffPending) {
      return;
    }
    const current = ++generation;
    handoffPending = false;
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
      showCapture(result, mode);
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
    isPending: () => pending,
    beginMenuHandoff
  };
}
