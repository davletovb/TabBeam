/**
 * Which conversation the popup opens with.
 *
 * Opening the popup usually means a new question, often about a different
 * page, so it starts fresh. The exception is picking up where you just were:
 * Chrome closes the popup whenever you click the page (to select text for a
 * follow-up, say), so reopening it on the same tab and page within
 * RESUME_WINDOW_MS continues that conversation. Anything older is one click
 * away in History.
 *
 * The tab → conversation map lives in `chrome.storage.session`: memory only,
 * gone when the browser closes. Pages are keyed by origin and path, without
 * the query or fragment, as context capture does; only http(s) pages can be
 * resumed.
 */

export const RESUME_WINDOW_MS = 30 * 60_000;
export const TAB_SESSIONS_KEY = "tabbeam.popup.tab-sessions";
const MAX_TAB_SESSIONS = 50;

/**
 * @typedef {{get(key: string): Promise<Record<string, any>>, set(values: Record<string, any>): Promise<void>}} SessionStorage
 * @typedef {{id?: number, url?: string}} Tab
 * @typedef {{conversation_id: string, page: string | null, at: number}} TabSession
 */

/** The page a tab shows, as origin and path. @param {string | undefined} url */
export function pageKey(url) {
  try {
    const parsed = new URL(url ?? "");
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed.origin + parsed.pathname : null;
  } catch {
    return null;
  }
}

/** @param {SessionStorage} storage @returns {Promise<Record<string, TabSession>>} */
async function readSessions(storage) {
  try {
    const stored = (await storage.get(TAB_SESSIONS_KEY))[TAB_SESSIONS_KEY];
    return stored && typeof stored === "object" ? stored : {};
  } catch {
    return {};
  }
}

/**
 * The conversation to resume in `tab`, or null to start fresh.
 *
 * @param {SessionStorage} storage
 * @param {Tab | null} tab
 * @param {{id: string}[]} saved the conversations that still exist
 * @param {number} now
 */
export async function conversationToResume(storage, tab, saved, now) {
  if (typeof tab?.id !== "number") return null;
  // Without a web page to match (a chrome:// page, or a URL Chrome doesn't
  // share), there's no telling whether it's the same page: start fresh.
  const page = pageKey(tab.url);
  if (page === null) return null;
  const session = (await readSessions(storage))[String(tab.id)];
  if (!session || now - session.at > RESUME_WINDOW_MS || session.page !== page) return null;
  return saved.some((item) => item.id === session.conversation_id) ? session.conversation_id : null;
}

/**
 * Remembers that `tab` is on `conversationId`, or forgets it for null (a new
 * conversation). Old entries are dropped as it goes.
 *
 * @param {SessionStorage} storage
 * @param {Tab | null} tab
 * @param {string | null} conversationId
 * @param {number} now
 */
export async function rememberConversation(storage, tab, conversationId, now) {
  if (typeof tab?.id !== "number") return;
  const sessions = await readSessions(storage);
  if (conversationId) {
    sessions[String(tab.id)] = { conversation_id: conversationId, page: pageKey(tab.url), at: now };
  } else {
    delete sessions[String(tab.id)];
  }
  const kept = Object.entries(sessions)
    .filter(([, session]) => now - session.at <= RESUME_WINDOW_MS)
    .sort(([, a], [, b]) => b.at - a.at)
    .slice(0, MAX_TAB_SESSIONS);
  try {
    await storage.set({ [TAB_SESSIONS_KEY]: Object.fromEntries(kept) });
  } catch {
    // Resuming is a convenience; the popup works without it.
  }
}
