import { sourceHost, sourceUrl, storedSources } from "./sources.js";

/**
 * Renders an answer's sources (EXT-16, EXT-17). The popup shows them as
 * compact numbered chips; the full view shows cards with each source's
 * title, site, and excerpt. Both number sources in the order the answer
 * received them and mark each with `data-source-id`, so a source has the
 * same identity and number in either view.
 *
 * Sources are untrusted (SEC-05, ./sources.js): they're checked again here,
 * every field is set as text, and a link opens its checked http(s) URL in a
 * new tab without a referrer or an opener. Nothing is fetched to show a
 * source; there are no favicons or previews.
 *
 * @typedef {{
 *   variant?: "compact" | "rich",
 *   limit?: number,
 *   onMore?: () => void
 * }} SourceListOptions
 */

/**
 * Replaces `container`'s content with `sources`, and hides it when there are
 * none. The container is styled by its `sources` class and `data-variant`.
 *
 * @param {HTMLElement} container
 * @param {unknown} sources
 * @param {SourceListOptions} [options]
 * @returns {number} how many sources are shown
 */
export function renderSources(container, sources, options = {}) {
  const list = storedSources(sources);
  const variant = options.variant ?? "compact";
  container.replaceChildren();
  container.hidden = list.length === 0;
  if (list.length === 0) return 0;
  container.setAttribute("data-variant", variant);
  container.setAttribute("aria-label", list.length === 1 ? "1 source" : `${list.length} sources`);
  const doc = container.ownerDocument;

  const heading = doc.createElement("p");
  heading.className = "sources-title";
  heading.textContent = variant === "rich" ? `Sources · ${list.length}` : "Sources";
  const items = doc.createElement("ol");
  items.className = variant === "rich" ? "source-cards" : "source-chips";

  const limit = variant === "compact" ? Math.max(1, options.limit ?? Infinity) : Infinity;
  const shown = Math.min(list.length, limit);
  list.slice(0, shown).forEach((source, index) => {
    items.append(variant === "rich" ? card(doc, source, index + 1) : chip(doc, source, index + 1));
  });
  if (list.length > shown) {
    const item = doc.createElement("li");
    const more = doc.createElement("button");
    more.setAttribute("type", "button");
    more.className = "source-chip source-more";
    const hidden = list.length - shown;
    more.textContent = `+${hidden}`;
    more.setAttribute("aria-label", options.onMore
      ? `See all ${list.length} sources in the full view`
      : `Show ${hidden} more ${hidden === 1 ? "source" : "sources"}`);
    more.setAttribute("title", options.onMore ? "See all sources in the full view" : "Show all sources");
    more.addEventListener("click", () => {
      if (options.onMore) options.onMore();
      else renderSources(container, list, { ...options, limit: Infinity });
    });
    item.append(more);
    items.append(item);
  }
  container.append(heading, items);
  return list.length;
}

/**
 * A link to a source, opened in a new tab.
 *
 * @param {Document} doc
 * @param {import("./sources.js").Source} source
 * @param {string} className
 */
function link(doc, source, className) {
  const anchor = doc.createElement("a");
  anchor.className = className;
  // Checked again at the last moment: only an http(s) URL is ever an href.
  const href = sourceUrl(source.url);
  if (href) {
    anchor.setAttribute("href", href);
    anchor.setAttribute("target", "_blank");
    anchor.setAttribute("rel", "noopener noreferrer");
  }
  anchor.setAttribute("data-source-id", source.id);
  return anchor;
}

/** @param {Document} doc @param {number} number */
function badge(doc, number) {
  const span = doc.createElement("span");
  span.className = "source-number";
  span.textContent = String(number);
  return span;
}

/**
 * @param {Document} doc
 * @param {import("./sources.js").Source} source
 * @param {number} number
 */
function chip(doc, source, number) {
  const item = doc.createElement("li");
  const anchor = link(doc, source, "source-chip");
  const host = sourceHost(source.url);
  anchor.setAttribute("title", host && host !== source.title ? `${source.title} — ${host}` : source.title);
  anchor.setAttribute("aria-label", `Source ${number}: ${source.title}, ${host || source.url}`);
  const site = doc.createElement("span");
  site.className = "source-host";
  site.textContent = host || source.title;
  anchor.append(badge(doc, number), site);
  item.append(anchor);
  return item;
}

/**
 * @param {Document} doc
 * @param {import("./sources.js").Source} source
 * @param {number} number
 */
function card(doc, source, number) {
  const item = doc.createElement("li");
  item.className = "source-card";
  item.setAttribute("data-source-id", source.id);
  const anchor = link(doc, source, "source-link");
  const title = doc.createElement("span");
  title.className = "source-title";
  title.textContent = source.title;
  anchor.append(badge(doc, number), title);

  const meta = doc.createElement("p");
  meta.className = "source-meta";
  const host = doc.createElement("span");
  host.className = "source-host";
  host.textContent = sourceHost(source.url);
  meta.append(host);
  for (const detail of [source.source_name, source.age]) {
    if (!detail) continue;
    const part = doc.createElement("span");
    part.textContent = detail;
    meta.append(part);
  }
  item.append(anchor, meta);

  if (source.snippet) {
    const snippet = doc.createElement("p");
    snippet.className = "source-snippet";
    snippet.textContent = source.snippet;
    item.append(snippet);
  }
  return item;
}
