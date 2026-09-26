/**
 * Types an answer out as it arrives instead of dropping it in all at once.
 *
 * Codex's exec mode hands over each message whole when it completes, and a
 * streaming provider sends uneven bursts; either way the reader sees text
 * flow in at a steady pace. The pace adapts to the backlog: whatever is
 * waiting shows within about a second, and a short tail never crawls. With
 * reduced motion, or in a hidden page where animation frames don't run, text
 * shows at once.
 *
 * @param {{
 *   render(element: HTMLElement, text: string): void,
 *   animate?: () => boolean,
 *   schedule?: (callback: (now: number) => void) => any,
 *   cancel?: (frame: any) => void
 * }} options
 * @returns {(element: HTMLElement, text: string) => Promise<void>} shows
 *   `text` in `element`, settling once it is all visible; "" clears it and
 *   settles anything still being revealed
 */
export function createStreamReveal({
  render,
  animate = () => true,
  schedule = (callback) => globalThis.requestAnimationFrame(callback),
  cancel = (frame) => globalThis.cancelAnimationFrame(frame)
}) {
  /** Characters per millisecond, at the least. */
  const MIN_RATE = 0.12;
  /** How long a backlog takes to drain, roughly. */
  const DRAIN_MS = 700;

  let target = "";
  let shown = 0;
  /** @type {HTMLElement | null} */
  let element = null;
  /** @type {any} */
  let frame = null;
  let last = 0;
  /** @type {(() => void)[]} */
  let waiting = [];

  /** @param {HTMLElement} target @param {string} text @returns {boolean} whether it rendered */
  function draw(target, text) {
    try {
      render(target, text);
      return true;
    } catch {
      return false;
    }
  }

  function settle() {
    const done = waiting;
    waiting = [];
    for (const resolve of done) resolve();
  }

  function stop() {
    if (frame !== null) cancel(frame);
    frame = null;
  }

  /** @param {number} now */
  function tick(now) {
    frame = null;
    if (!element) return;
    const elapsed = last ? Math.min(now - last, 64) : 16;
    last = now;
    const backlog = target.length - shown;
    let next = shown + Math.max(1, Math.round(elapsed * Math.max(MIN_RATE, backlog / DRAIN_MS)));
    // Finish the word in progress, so text doesn't flicker mid-word.
    const space = target.slice(next, next + 12).search(/\s/);
    if (space > 0) next += space;
    // Never split a surrogate pair.
    if (/[\uD800-\uDBFF]/.test(target[next - 1] ?? "")) next += 1;
    shown = Math.min(next, target.length);
    // A render that throws ends the reveal rather than leaving it pending.
    if (!draw(element, target.slice(0, shown))) shown = target.length;
    if (shown < target.length) frame = schedule(tick);
    else settle();
  }

  return (nextElement, text) => {
    element = nextElement;
    if (text === "" || !text.startsWith(target.slice(0, shown))) {
      // A new answer, or one that changed rather than grew: start over.
      shown = 0;
    }
    target = text;
    const promise = new Promise((resolve) => waiting.push(() => resolve(undefined)));
    const hidden = globalThis.document?.visibilityState === "hidden";
    if (text === "" || !animate() || hidden) {
      stop();
      shown = text.length;
      draw(nextElement, text);
      settle();
    } else if (shown >= text.length) {
      settle();
    } else if (frame === null) {
      last = 0;
      frame = schedule(tick);
    }
    return promise;
  };
}
