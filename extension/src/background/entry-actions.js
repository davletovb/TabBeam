import { captureContext } from "./selection-capture.js";

export const MENU_SELECTION_ID = "pervue-use-selection";
export const MENU_PAGE_ID = "pervue-use-page";
export const MENU_CONSUME_MESSAGE = "pervue.context.consume-menu";
const HANDOFF_MS = 30_000;

/**
 * Menu content stays only in this service worker until the next popup claims
 * it. No content is put into URLs or persistent storage; the popup must be
 * from the clicked tab (or carry the opaque fallback-tab token).
 * @param {{
 *   tabs: {query(query: object): Promise<{id?: number, url?: string, title?: string}[]>, create(properties: {url: string}): Promise<unknown>, sendMessage(tabId: number, message: any, options: object): Promise<any>},
 *   action: {openPopup?(): Promise<void>},
 *   popupUrl: string
 * }} options
 */
export function createEntryActions({ tabs, action, popupUrl }) {
  /** @type {{tabId: number | undefined, sourceUrl: string | null, token: string, expires: number, result: Promise<any>} | null} */
  let pending = null;

  /**
   * @param {{menuItemId: string | number, selectionText?: string}} info
   * @param {{id?: number, url?: string, title?: string} | undefined} tab
   */
  async function onMenuClick(info, tab) {
    const mode = info.menuItemId === MENU_SELECTION_ID
      ? "selection"
      : info.menuItemId === MENU_PAGE_ID ? "page" : null;
    if (!mode) return;

    // Start the capture and open the popup while the context-menu gesture is
    // still active. Awaiting extraction first could lose the user gesture.
    const entry = {
      tabId: tab?.id,
      // Keep the exact URL only for the short-lived navigation check. It is
      // never sent to the popup, provider, logs, or a URL parameter.
      sourceUrl: typeof tab?.url === "string" ? tab.url : null,
      token: crypto.randomUUID(),
      expires: Date.now() + HANDOFF_MS,
      result: captureContext(tabs, mode, tab ?? {}, mode === "selection" ? info.selectionText ?? "" : undefined)
    };
    pending = entry;
    try {
      if (!action.openPopup) throw new Error("popup API unavailable");
      await action.openPopup();
    } catch {
      // Older/unsupported popup openings get the same composer in a tab. The
      // URL carries only a one-time token, never page or selection content.
      if (pending === entry) {
        try {
          await tabs.create({ url: `${popupUrl}?menu=${entry.token}` });
        } catch {
          if (pending === entry) pending = null;
        }
      }
    }
  }

  /** @param {any} message @param {{url?: string}} sender */
  async function consume(message, sender) {
    const entry = pending;
    if (message?.type !== MENU_CONSUME_MESSAGE || !entry || Date.now() > entry.expires) {
      if (entry && Date.now() > entry.expires) pending = null;
      return { available: false };
    }
    if (typeof message.token === "string") {
      if (message.token !== entry.token || sender?.url !== `${popupUrl}?menu=${entry.token}`) {
        return { available: false };
      }
    } else {
      if (sender?.url !== popupUrl || message.token !== null) return { available: false };
      try {
        const [active] = await tabs.query({ active: true, currentWindow: true });
        if (active?.id !== entry.tabId ||
            (entry.sourceUrl !== null && active?.url !== entry.sourceUrl)) {
          // A different tab or navigation invalidates the one-time capture;
          // reopening the popup later cannot silently attach stale content.
          if (pending === entry) pending = null;
          return { available: false };
        }
        if (pending !== entry) return { available: false };
      } catch {
        return { available: false };
      }
    }
    if (pending !== entry) return { available: false };
    pending = null;
    return { available: true, result: await entry.result };
  }

  return { onMenuClick, consume };
}
