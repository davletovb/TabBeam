import { MENU_CONSUME_MESSAGE } from "../background/entry-actions.js";

/**
 * One-time menu handoff. The URL contains only an opaque fallback token; the
 * background worker returns page text directly to this popup, never via URL.
 * @param {{sendMessage(message: any): Promise<any>}} runtime
 * @param {{beginMenuHandoff(): (handoff: any) => void}} controls
 * @param {string} search
 */
export async function preloadMenuContext(runtime, controls, search) {
  const finish = controls.beginMenuHandoff();
  try {
    const token = new URLSearchParams(search).get("menu");
    const handoff = await runtime.sendMessage({ type: MENU_CONSUME_MESSAGE, token });
    finish(handoff);
    return handoff?.available === true;
  } catch {
    finish(null);
    return false;
  }
}
