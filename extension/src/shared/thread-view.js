/**
 * Presentation helpers for a conversation surface: the composer grows with
 * its text, and the thread follows a streaming answer while the reader is at
 * the bottom, without pulling them back when they have scrolled up to read.
 *
 * @param {{scroller: HTMLElement, thread: HTMLElement, input: HTMLTextAreaElement, maxInputHeight?: number}} elements
 */
export function bindThreadView({ scroller, thread, input, maxInputHeight = 200 }) {
  let pinned = true;

  function fitInput() {
    input.style.height = "auto";
    const height = Math.min(input.scrollHeight, maxInputHeight);
    input.style.height = `${height}px`;
    input.style.overflowY = input.scrollHeight > maxInputHeight ? "auto" : "hidden";
  }

  function toBottom() {
    scroller.scrollTop = scroller.scrollHeight;
  }

  scroller.addEventListener("scroll", () => {
    pinned = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 48;
  }, { passive: true });

  // The answer, the history, and the input value all change from ask-form.js;
  // follow them from here rather than coupling that module to layout.
  new MutationObserver(() => {
    if (pinned) toBottom();
    fitInput();
  }).observe(thread, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["hidden"] });

  input.addEventListener("input", fitInput);
  fitInput();

  return {
    fitInput,
    /** Jump to the newest message, e.g. after a conversation loads. */
    reveal() {
      pinned = true;
      toBottom();
    }
  };
}
