/**
 * Web sources behind an answer (protocol v1 `response.source`, SRCH-03).
 *
 * Everything in a source is untrusted: it comes from search results the AI
 * provider found (SEC-05). The service worker accepts a source only through
 * `sourceFromEvent`, stores only what that returns, and the popup and full
 * view render stored sources through `storedSources` again before showing
 * them. So a source is always:
 *
 * - identified by a short opaque ID the host assigned;
 * - linked only to an `http:` or `https:` URL with a host and no
 *   credentials, and only ever opened as a new tab (never navigated to in an
 *   extension page, never fetched);
 * - described by bounded single-line plain text, rendered as text nodes and
 *   never parsed as HTML, with control and bidirectional characters removed.
 *
 * An answer keeps at most MAX_SOURCES_PER_ANSWER sources, in the order the
 * host sent them, with duplicate IDs and URLs dropped; that order is each
 * source's number in the popup and in the full view.
 */

export const MAX_SOURCES_PER_ANSWER = 20;
export const MAX_SOURCE_URL_LENGTH = 4096;
export const MAX_SOURCE_TITLE_LENGTH = 300;
export const MAX_SOURCE_SNIPPET_LENGTH = 1000;
const MAX_META_LENGTH = 120;

const SOURCE_ID = /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/;
const BACKEND_ID = /^[a-z0-9][a-z0-9_-]{0,31}$/;
// Control characters separate words, as the host treats them.
const CONTROL = /[\u0000-\u001F\u007F-\u009F]/gu;
// Bidirectional marks, overrides and isolates, and zero-width spaces hide or
// reorder text without being visible, so they're removed, as the host does.
// Joiners stay, since emoji sequences need them.
const INVISIBLE = /[\u061C\u200B\u200E\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/gu;

/**
 * @typedef {{
 *   id: string,
 *   backend_id: string,
 *   title: string,
 *   url: string,
 *   snippet?: string,
 *   source_name?: string,
 *   age?: string
 * }} Source
 */

/**
 * A URL a source may link to, normalized, or null.
 *
 * @param {unknown} value
 * @returns {string | null}
 */
export function sourceUrl(value) {
  if (typeof value !== "string" || value.length > MAX_SOURCE_URL_LENGTH || /[\s\u0000-\u001F\u007F]/u.test(value)) {
    return null;
  }
  let url;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if ((url.protocol !== "http:" && url.protocol !== "https:") || !url.hostname || url.username || url.password) {
    return null;
  }
  return url.href.length <= MAX_SOURCE_URL_LENGTH ? url.href : null;
}

/**
 * The site a source is from, as people read it: `example.com`, not
 * `https://www.example.com/…`.
 *
 * @param {string} url a URL `sourceUrl` accepted
 */
export function sourceHost(url) {
  try {
    return new URL(url).hostname.replace(/^www\./u, "");
  } catch {
    return "";
  }
}

/**
 * Untrusted text as bounded single-line plain text.
 *
 * @param {unknown} value
 * @param {number} limit in characters (code points)
 */
export function sourceText(value, limit) {
  if (typeof value !== "string") return "";
  // Enough to fill `limit` after cleaning, without a dangling half of a
  // character the cut may have split.
  const scanned = value.slice(0, limit * 4).replace(/[\uD800-\uDBFF]$/u, "");
  const text = scanned.replace(INVISIBLE, "").replace(CONTROL, " ").replace(/\s+/gu, " ").trim();
  const characters = Array.from(text);
  return characters.length <= limit ? text : `${characters.slice(0, limit - 1).join("").trimEnd()}…`;
}

/**
 * A well-formed source from its data, or null.
 *
 * @param {any} data
 * @returns {Source | null}
 */
export function normalizeSource(data) {
  if (!data || typeof data !== "object") return null;
  const { id, backend_id: backend } = data;
  if (typeof id !== "string" || !SOURCE_ID.test(id)) return null;
  if (typeof backend !== "string" || !BACKEND_ID.test(backend)) return null;
  const url = sourceUrl(data.url);
  if (!url) return null;
  const title = sourceText(data.title, MAX_SOURCE_TITLE_LENGTH) || sourceHost(url);
  if (!title) return null;
  const snippet = sourceText(data.snippet, MAX_SOURCE_SNIPPET_LENGTH);
  const name = sourceText(data.source_name, MAX_META_LENGTH);
  const age = sourceText(data.age, MAX_META_LENGTH);
  return {
    id,
    backend_id: backend,
    title,
    url,
    ...(snippet ? { snippet } : {}),
    ...(name ? { source_name: name } : {}),
    ...(age ? { age } : {})
  };
}

/**
 * The source a `response.source` event carries, or null. Its `source_id`
 * must name its data (protocol v1).
 *
 * @param {any} payload
 * @returns {Source | null}
 */
export function sourceFromEvent(payload) {
  if (!payload || typeof payload.source_id !== "string" || payload.data?.id !== payload.source_id) return null;
  return normalizeSource(payload.data);
}

/**
 * Collects one answer's sources as their events arrive: a source is kept
 * only if it's well formed and new, up to the per-answer limit.
 */
export function createSourceSet() {
  /** @type {Source[]} */
  const sources = [];
  return {
    /**
     * @param {Source | null} source
     * @returns {boolean} whether it was kept
     */
    add(source) {
      if (!source || sources.length >= MAX_SOURCES_PER_ANSWER) return false;
      if (sources.some((kept) => kept.id === source.id || kept.url === source.url)) return false;
      sources.push(source);
      return true;
    },
    list: () => sources.slice(),
    get size() { return sources.length; },
    clear() { sources.length = 0; }
  };
}

/**
 * Saved sources, checked again before they're shown. Accepts the stored
 * shape and the `{id, data}` shape earlier versions saved.
 *
 * @param {unknown} value
 * @returns {Source[]}
 */
export function storedSources(value) {
  if (!Array.isArray(value)) return [];
  const set = createSourceSet();
  for (const item of value) {
    const data = item?.data && typeof item.data === "object" ? { ...item.data, id: item.id } : item;
    set.add(normalizeSource(data));
  }
  return set.list();
}
