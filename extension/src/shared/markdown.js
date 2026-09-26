/**
 * Markdown for provider answers, rendered as DOM nodes.
 *
 * Provider output is untrusted (SEC-03), so this never produces or parses
 * HTML: text reaches the page only through text nodes, raw HTML in the
 * source stays visible as text, and a link keeps its href only when it is an
 * absolute http, https, or mailto URL. Images render as links to their
 * source rather than loading it, so an answer can't make the browser fetch
 * anything.
 *
 * The dialect is what chat models write: paragraphs (a single newline is a
 * line break), ATX headings, fenced code, block quotes, nested ordered,
 * bullet, and task lists, GFM tables, thematic breaks, and inline code,
 * emphasis, strikethrough, links, and autolinks. Unclosed constructs render
 * as text, so a partial answer renders sensibly while it streams.
 *
 * Parsing is bounded, since an answer can be crafted (a page's text can
 * steer it): lookups use indexes built in one pass, nesting deeper than
 * MAX_DEPTH blocks or MAX_INLINE_DEPTH spans stays text, and an answer that
 * still costs more than its work budget renders as plain text.
 */

import { copyWithFeedback } from "./clipboard.js";

/**
 * @typedef {{type: "text", text: string}
 *   | {type: "code", text: string}
 *   | {type: "break"}
 *   | {type: "strong" | "em" | "del", children: Inline[]}
 *   | {type: "link", href: string, children: Inline[]}} Inline
 *
 * @typedef {{type: "paragraph", children: Inline[]}
 *   | {type: "heading", level: number, children: Inline[]}
 *   | {type: "code", lang: string, text: string}
 *   | {type: "quote", children: Block[]}
 *   | {type: "list", ordered: boolean, start: number, items: ListItem[]}
 *   | {type: "rule"}
 *   | {type: "table", align: Align[], header: Inline[][], rows: Inline[][][]}} Block
 *
 * @typedef {{checked: boolean | null, children: Block[]}} ListItem
 * @typedef {"left" | "center" | "right" | null} Align
 */

const FENCE_OPEN = /^( {0,3})(`{3,}|~{3,})(.*)$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const RULE = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])(?:[ \t]+(.*))?$/;
const TABLE_DIVIDER = /^ {0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;
const TASK = /^\[([ xX])\][ \t]+/;
const ESCAPABLE = /[!-/:-@[-`{-~]/;
const WORD = /[\p{L}\p{N}]/u;
const SPACE = /\s/;

/** Deepest nesting of block quotes and lists; deeper content stays text. */
const MAX_DEPTH = 12;
/** Deepest nesting of emphasis and links; deeper content stays text. */
const MAX_INLINE_DEPTH = 12;
/** Longest link destination or title considered. */
const MAX_DESTINATION = 2048;

/** @typedef {{steps: number, limit: number}} Budget */

/** Parsing ran over its budget. */
class TooComplex extends Error {}

/** Work allowed for `source`: generous for real answers, linear in size. @param {string} source @returns {Budget} */
function budgetFor(source) {
  return { steps: 0, limit: 100_000 + source.length * 40 };
}

/** @param {Budget} budget @param {number} [steps] */
function spend(budget, steps = 1) {
  budget.steps += steps;
  if (budget.steps > budget.limit) throw new TooComplex("Markdown too complex to render");
}

/**
 * @param {string} source
 * @returns {Block[]}
 * @throws {TooComplex} when the source costs more than its budget to parse
 */
export function parseMarkdown(source) {
  const lines = source.replace(/\r\n?/g, "\n").split("\n").map(expandIndent);
  return parseBlocks(lines, 0, budgetFor(source));
}

/** Leading tabs count as four columns. @param {string} line */
function expandIndent(line) {
  const indent = /^[ \t]*/.exec(line)?.[0] ?? "";
  return indent.includes("\t") ? indent.replace(/\t/g, "    ") + line.slice(indent.length) : line;
}

/** @param {string} line */
const isBlank = (line) => line.trim() === "";

/** @param {string} line */
const indentOf = (line) => /^ */.exec(line)?.[0].length ?? 0;

/** @param {string[]} lines @param {number} i */
function isTableStart(lines, i) {
  const next = lines[i + 1];
  return next !== undefined && lines[i].includes("|") && TABLE_DIVIDER.test(next) &&
    (next.includes("|") || lines[i].trim().startsWith("|")) &&
    splitRow(next).length === splitRow(lines[i]).length;
}

/** Whether a line ends a paragraph by starting a block of its own. @param {string[]} lines @param {number} i */
function interrupts(lines, i) {
  const line = lines[i];
  const item = LIST_ITEM.exec(line);
  return FENCE_OPEN.test(line) || HEADING.test(line) || RULE.test(line) || QUOTE.test(line) ||
    (item !== null && item[1].length <= 3 && item[3] !== undefined) || isTableStart(lines, i);
}

/** Lines kept as one paragraph of text. @param {string[]} lines @param {Budget} budget @returns {Block} */
function asText(lines, budget) {
  spend(budget);
  return { type: "paragraph", children: [{ type: "text", text: lines.join("\n") }] };
}

/** @param {string[]} lines @param {number} depth @param {Budget} budget @returns {Block[]} */
function parseBlocks(lines, depth, budget) {
  /** @type {Block[]} */
  const blocks = [];
  let i = 0;
  while (i < lines.length) {
    spend(budget);
    const line = lines[i];
    if (isBlank(line)) {
      i += 1;
      continue;
    }

    const fence = FENCE_OPEN.exec(line);
    if (fence && !(fence[2][0] === "`" && fence[3].includes("`"))) {
      const [, indent, marker, info] = fence;
      const close = new RegExp(`^ {0,3}${marker[0] === "`" ? "`" : "~"}{${marker.length},}[ \\t]*$`);
      const body = [];
      i += 1;
      // An unclosed fence runs to the end: that's a code block still streaming.
      while (i < lines.length && !close.test(lines[i])) {
        body.push(lines[i].slice(Math.min(indent.length, indentOf(lines[i]))));
        i += 1;
      }
      i += 1;
      blocks.push({ type: "code", lang: info.trim().split(/\s+/)[0] ?? "", text: body.join("\n") });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      blocks.push({ type: "heading", level: heading[1].length, children: inline(heading[2] ?? "", false, 0, budget) });
      i += 1;
      continue;
    }

    if (RULE.test(line)) {
      blocks.push({ type: "rule" });
      i += 1;
      continue;
    }

    if (QUOTE.test(line)) {
      const inner = [];
      while (i < lines.length && !isBlank(lines[i])) {
        const quoted = QUOTE.exec(lines[i]);
        // A line without ">" continues the quote's paragraph.
        if (!quoted && interrupts(lines, i)) break;
        inner.push(quoted ? quoted[1] : lines[i]);
        i += 1;
      }
      blocks.push({ type: "quote", children: depth + 1 >= MAX_DEPTH ? [asText(inner, budget)] : parseBlocks(inner, depth + 1, budget) });
      continue;
    }

    const item = LIST_ITEM.exec(line);
    if (item && item[1].length <= 3) {
      i = parseList(lines, i, blocks, depth, budget);
      continue;
    }

    if (isTableStart(lines, i)) {
      i = parseTable(lines, i, blocks, budget);
      continue;
    }

    const paragraph = [line.trim()];
    i += 1;
    while (i < lines.length && !isBlank(lines[i]) && !interrupts(lines, i)) {
      paragraph.push(lines[i].trim());
      i += 1;
    }
    blocks.push({ type: "paragraph", children: inline(paragraph.join("\n"), false, 0, budget) });
  }
  return blocks;
}

/**
 * @param {string[]} lines @param {number} i @param {Block[]} blocks
 * @param {number} depth @param {Budget} budget
 * @returns {number} the next line
 */
function parseList(lines, i, blocks, depth, budget) {
  const first = /** @type {RegExpExecArray} */ (LIST_ITEM.exec(lines[i]));
  const ordered = /\d/.test(first[2]);
  const delimiter = first[2].slice(-1);
  const baseIndent = first[1].length;
  /** @type {ListItem[]} */
  const items = [];

  while (i < lines.length) {
    const marker = LIST_ITEM.exec(lines[i]);
    if (!marker || marker[1].length < baseIndent || marker[1].length > baseIndent + 3 ||
        /\d/.test(marker[2]) !== ordered || marker[2].slice(-1) !== delimiter) break;
    const contentIndent = marker[1].length + marker[2].length + 1;
    const body = [marker[3] ?? ""];
    i += 1;
    while (i < lines.length) {
      spend(budget);
      const line = lines[i];
      if (isBlank(line)) {
        // A blank line stays in the item only if the item goes on after it.
        let next = i + 1;
        while (next < lines.length && isBlank(lines[next])) next += 1;
        if (next < lines.length && indentOf(lines[next]) >= contentIndent) {
          body.push(...lines.slice(i, next).map(() => ""));
          i = next;
          continue;
        }
        break;
      }
      const indent = indentOf(line);
      const nested = LIST_ITEM.exec(line);
      if (indent >= contentIndent || (nested && indent > baseIndent)) {
        body.push(line.slice(Math.min(indent, contentIndent)));
      } else if (nested || interrupts(lines, i)) {
        break;
      } else {
        // A lazy continuation of the item's paragraph.
        body.push(line.trim());
      }
      i += 1;
    }
    const task = TASK.exec(body[0]);
    if (task) body[0] = body[0].slice(task[0].length);
    items.push({ checked: task ? task[1] !== " " : null, children: depth + 1 >= MAX_DEPTH ? [asText(body, budget)] : parseBlocks(body, depth + 1, budget) });
    // A blank line between items keeps the list going.
    let next = i;
    while (next < lines.length && isBlank(lines[next])) next += 1;
    if (next > i && next < lines.length && LIST_ITEM.test(lines[next])) i = next;
  }

  blocks.push({ type: "list", ordered, start: ordered ? Number.parseInt(first[2], 10) : 1, items });
  return i;
}

/** @param {string} row @returns {string[]} */
function splitRow(row) {
  let text = row.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|") && !text.endsWith("\\|")) text = text.slice(0, -1);
  const cells = [];
  let cell = "";
  let inCode = false;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === "\\" && text[i + 1] === "|") {
      cell += "|";
      i += 1;
    } else if (ch === "`") {
      inCode = !inCode;
      cell += ch;
    } else if (ch === "|" && !inCode) {
      cells.push(cell.trim());
      cell = "";
    } else {
      cell += ch;
    }
  }
  cells.push(cell.trim());
  return cells;
}

/** @param {string[]} lines @param {number} i @param {Block[]} blocks @param {Budget} budget @returns {number} the next line */
function parseTable(lines, i, blocks, budget) {
  const header = splitRow(lines[i]);
  /** @type {Align[]} */
  const align = splitRow(lines[i + 1]).map((cell) => {
    const left = cell.startsWith(":");
    const right = cell.endsWith(":");
    return left && right ? "center" : right ? "right" : left ? "left" : null;
  });
  const rows = [];
  i += 2;
  while (i < lines.length && !isBlank(lines[i]) && lines[i].includes("|") && !interrupts(lines, i)) {
    const cells = splitRow(lines[i]);
    rows.push(header.map((_, column) => inline(cells[column] ?? "", false, 0, budget)));
    i += 1;
  }
  blocks.push({ type: "table", align, header: header.map((cell) => inline(cell, false, 0, budget)), rows });
  return i;
}

/**
 * The URL a link may point at, or null. Only absolute http(s) and mailto
 * URLs pass: a relative one would resolve to the extension's own pages.
 * @param {string} raw
 */
export function safeHref(raw) {
  try {
    const url = new URL(raw.trim());
    return url.protocol === "http:" || url.protocol === "https:" || url.protocol === "mailto:" ? url.href : null;
  } catch {
    return null;
  }
}

/** @param {string} src @param {number} i @param {string} ch */
function runLength(src, i, ch) {
  let end = i;
  while (src[end] === ch) end += 1;
  return end - i;
}

/**
 * What the inline scanner knows about one string, built in a single pass so
 * that no lookup rescans the rest of it: where each "[" closes, where each
 * run of backticks starts, and where emphasis could close.
 *
 * @typedef {{
 *   src: string,
 *   inLink: boolean,
 *   depth: number,
 *   budget: Budget,
 *   closeOf: Int32Array,
 *   ticks: Map<number, number[]>,
 *   closers: Map<string, {at: number[], longest: number[]}>,
 *   emphasis: Map<number, {node: Inline, end: number} | null>
 * }} Scan
 */

/** @param {string} src @param {boolean} inLink @param {number} depth @param {Budget} budget @returns {Scan} */
function scan(src, inLink, depth, budget) {
  spend(budget, src.length);
  const closeOf = new Int32Array(src.length).fill(-1);
  /** @type {Map<number, number[]>} */
  const ticks = new Map();
  /** @type {Map<string, {at: number[], longest: number[]}>} */
  const closers = new Map([["*", { at: [], longest: [] }], ["_", { at: [], longest: [] }], ["~", { at: [], longest: [] }]]);
  const open = [];
  for (let i = 0; i < src.length; i += 1) {
    const ch = src[i];
    if (ch === "\\") {
      i += 1;
    } else if (ch === "[") {
      open.push(i);
    } else if (ch === "]") {
      const start = open.pop();
      if (start !== undefined) closeOf[start] = i;
    } else if (ch === "`") {
      const run = runLength(src, i, ch);
      const starts = ticks.get(run);
      if (starts) starts.push(i);
      else ticks.set(run, [i]);
      i += run - 1;
    } else if (ch === "*" || ch === "_" || ch === "~") {
      const run = runLength(src, i, ch);
      if (canClose(src, i, run, ch)) {
        const list = /** @type {{at: number[], longest: number[]}} */ (closers.get(ch));
        list.at.push(i);
        list.longest.push(run);
      }
      i += run - 1;
    }
  }
  // longest[k]: the longest closing run at or after the k-th one.
  for (const { longest } of closers.values()) {
    for (let k = longest.length - 2; k >= 0; k -= 1) longest[k] = Math.max(longest[k], longest[k + 1]);
  }
  return { src, inLink, depth, budget, closeOf, ticks, closers, emphasis: new Map() };
}

/** The first index in ascending `values` greater than `after`. @param {number[]} values @param {number} after */
function firstAfter(values, after) {
  let low = 0;
  let high = values.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (values[mid] > after) high = mid;
    else low = mid + 1;
  }
  return low;
}

/** A code span opening at `i`, if a run of as many backticks closes it. @param {Scan} s @param {number} i */
function codeSpanAt(s, i) {
  const size = runLength(s.src, i, "`");
  const starts = s.ticks.get(size) ?? [];
  const close = starts[firstAfter(starts, i)];
  if (close === undefined) return null;
  let text = s.src.slice(i + size, close).replace(/\n/g, " ");
  if (text.length > 2 && text.startsWith(" ") && text.endsWith(" ") && text.trim()) text = text.slice(1, -1);
  return { text, end: close + size };
}

/** A link `[label](destination "title")` opening at `i`. @param {Scan} s @param {number} i */
function linkAt(s, i) {
  const { src } = s;
  const close = s.closeOf[i];
  if (close < 0 || src[close + 1] !== "(") return null;
  const limit = Math.min(src.length, close + 2 + MAX_DESTINATION);
  let j = close + 2;
  while (src[j] === " ") j += 1;
  let destination = "";
  if (src[j] === "<") {
    const end = src.indexOf(">", j);
    if (end < 0 || end > limit) return null;
    destination = src.slice(j + 1, end);
    j = end + 1;
  } else {
    let parens = 0;
    const start = j;
    while (j < limit && !SPACE.test(src[j])) {
      if (src[j] === "(") parens += 1;
      else if (src[j] === ")" && parens-- === 0) break;
      j += 1;
    }
    spend(s.budget, j - start);
    if (j >= limit && j < src.length) return null;
    destination = src.slice(start, j);
  }
  while (src[j] === " ") j += 1;
  const quote = src[j];
  if (quote === "\"" || quote === "'") {
    const end = src.indexOf(quote, j + 1);
    if (end < 0 || end > j + MAX_DESTINATION) return null;
    j = end + 1;
    while (src[j] === " ") j += 1;
  }
  if (src[j] !== ")") return null;
  return { label: src.slice(i + 1, close), href: safeHref(destination), end: j + 1 };
}

const BARE_URL = /https?:\/\/[^\s<>"]{1,2048}/iy;
const AUTOLINK = /<((?:https?|mailto):[^\s<>]{1,2048})>/iy;

/** A bare http(s) URL starting at `i`. @param {string} src @param {number} i */
function bareUrlAt(src, i) {
  BARE_URL.lastIndex = i;
  const match = BARE_URL.exec(src);
  if (!match) return null;
  let url = match[0].replace(/[.,:;!?'"*_~]+$/, "");
  // Keep a closing parenthesis only if the URL opened one.
  while (url.endsWith(")") && (url.match(/\(/g)?.length ?? 0) < (url.match(/\)/g)?.length ?? 0)) {
    url = url.slice(0, -1).replace(/[.,:;!?'"*_~]+$/, "");
  }
  const href = safeHref(url);
  return href ? { text: url, href, end: i + url.length } : null;
}

/** Whether a run of `ch` at `i` can open emphasis. @param {string} src @param {number} i @param {number} run @param {string} ch */
function canOpen(src, i, run, ch) {
  const after = src[i + run];
  if (after === undefined || SPACE.test(after)) return false;
  return !(ch === "_" && WORD.test(src[i - 1] ?? ""));
}

/** Whether a run of `ch` at `j` can close emphasis. @param {string} src @param {number} j @param {number} run @param {string} ch */
function canClose(src, j, run, ch) {
  const before = src[j - 1];
  if (before === undefined || SPACE.test(before)) return false;
  return !(ch === "_" && WORD.test(src[j + run] ?? ""));
}

/**
 * Emphasis or strikethrough opening at `i`. A closing run may be longer than
 * the opening one: `**bold *and italic***` closes the inner `*` with the
 * first character of `***` and the outer `**` with the rest. Emphasis that
 * opens inside is matched first, so its closer isn't taken for this one's.
 *
 * @param {Scan} s
 * @param {number} i
 */
function emphasisAt(s, i) {
  const known = s.emphasis.get(i);
  if (known !== undefined) return known;
  const found = findEmphasis(s, i);
  s.emphasis.set(i, found);
  return found;
}

/** @param {Scan} s @param {number} i @returns {{node: Inline, end: number} | null} */
function findEmphasis(s, i) {
  const { src } = s;
  const ch = src[i];
  const run = runLength(src, i, ch);
  if (ch === "~" ? run !== 2 : run > 3) return null;
  if (!canOpen(src, i, run, ch) || s.depth >= MAX_INLINE_DEPTH) return null;
  // No run long enough closes anywhere after this: nothing to scan for.
  const closers = /** @type {{at: number[], longest: number[]}} */ (s.closers.get(ch));
  const first = firstAfter(closers.at, i + run - 1);
  if (first >= closers.at.length || closers.longest[first] < run) return null;

  let j = i + run;
  while (j < src.length) {
    spend(s.budget);
    const c = src[j];
    if (c === "\\") {
      j += 2;
      continue;
    }
    if (c === "`") {
      const span = codeSpanAt(s, j);
      j = span ? span.end : j + runLength(src, j, "`");
      continue;
    }
    if (c !== ch) {
      j += 1;
      continue;
    }
    const length = runLength(src, j, ch);
    if (j > i + run && length >= run && (ch !== "~" || length === 2) && canClose(src, j, length, ch)) {
      const children = inline(src.slice(i + run, j), s.inLink, s.depth + 1, s.budget);
      /** @type {Inline} */
      const node = ch === "~" ? { type: "del", children }
        : run === 1 ? { type: "em", children }
        : run === 2 ? { type: "strong", children }
        : { type: "strong", children: [{ type: "em", children }] };
      return { node, end: j + run };
    }
    const inner = canOpen(src, j, length, ch) ? emphasisAt(s, j) : null;
    j = inner ? inner.end : j + length;
  }
  return null;
}

/** @param {string} src @returns {Inline[]} */
export function parseInline(src) {
  return inline(src, false, 0, budgetFor(src));
}

/**
 * @param {string} src
 * @param {boolean} inLink inside a link's text, where links can't nest
 * @param {number} depth how deeply this text is nested in emphasis and links
 * @param {Budget} budget
 * @returns {Inline[]}
 */
function inline(src, inLink, depth, budget) {
  if (depth > MAX_INLINE_DEPTH) return src ? [{ type: "text", text: src }] : [];
  const s = scan(src, inLink, depth, budget);
  /** @type {Inline[]} */
  const out = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ type: "text", text });
    text = "";
  };

  let i = 0;
  while (i < src.length) {
    spend(budget);
    const ch = src[i];
    if (ch === "\\" && src[i + 1] === "\n") {
      flush();
      out.push({ type: "break" });
      i += 2;
    } else if (ch === "\\" && ESCAPABLE.test(src[i + 1] ?? "")) {
      text += src[i + 1];
      i += 2;
    } else if (ch === "\n") {
      text = text.replace(/ +$/, "");
      flush();
      out.push({ type: "break" });
      i += 1;
      while (src[i] === " ") i += 1;
    } else if (ch === "`") {
      const span = codeSpanAt(s, i);
      if (span) {
        flush();
        out.push({ type: "code", text: span.text });
        i = span.end;
      } else {
        const run = runLength(src, i, "`");
        text += src.slice(i, i + run);
        i += run;
      }
    } else if (!inLink && (ch === "[" || (ch === "!" && src[i + 1] === "["))) {
      const image = ch === "!";
      const link = linkAt(s, image ? i + 1 : i);
      if (!link) {
        text += ch;
        i += 1;
        continue;
      }
      flush();
      /** @type {Inline[]} */
      const children = image
        ? [{ type: "text", text: link.label || "Image" }]
        : inline(link.label, true, depth + 1, budget);
      if (link.href) out.push({ type: "link", href: link.href, children });
      else out.push(...children);
      i = link.end;
    } else if (!inLink && ch === "<") {
      AUTOLINK.lastIndex = i;
      const match = AUTOLINK.exec(src);
      const href = match ? safeHref(match[1]) : null;
      if (match && href) {
        flush();
        out.push({ type: "link", href, children: [{ type: "text", text: match[1] }] });
        i += match[0].length;
      } else {
        text += ch;
        i += 1;
      }
    } else if (!inLink && (ch === "h" || ch === "H") && !WORD.test(src[i - 1] ?? "") && bareUrlAt(src, i)) {
      const url = /** @type {{text: string, href: string, end: number}} */ (bareUrlAt(src, i));
      flush();
      out.push({ type: "link", href: url.href, children: [{ type: "text", text: url.text }] });
      i = url.end;
    } else if (ch === "*" || ch === "_" || ch === "~") {
      const emphasis = emphasisAt(s, i);
      if (emphasis) {
        flush();
        out.push(emphasis.node);
        i = emphasis.end;
      } else {
        const run = runLength(src, i, ch);
        text += src.slice(i, i + run);
        i += run;
      }
    } else {
      text += ch;
      i += 1;
    }
  }
  flush();
  return out;
}

/**
 * Renders `source` into `container`, replacing what it held. An answer that
 * is too costly to parse shows as plain text.
 *
 * `interactive: false` is for an answer still typing out, whose nodes are
 * rebuilt every frame: links and copy buttons are drawn but can't take focus
 * or clicks, so nothing a reader is using is replaced from under them. The
 * finished answer is drawn interactive.
 *
 * @param {HTMLElement} container
 * @param {string} source
 * @param {{interactive?: boolean}} [options]
 */
export function renderMarkdown(container, source, { interactive = true } = {}) {
  const doc = container.ownerDocument;
  /** @type {HTMLElement[]} */
  let nodes;
  try {
    nodes = parseMarkdown(source).map((block) => renderBlock(doc, block, interactive));
  } catch {
    const plain = element(doc, "p", "markdown-plain");
    plain.textContent = source;
    nodes = [plain];
  }
  container.replaceChildren(...nodes);
}

/**
 * @param {Document} doc
 * @param {string} tag
 * @param {string} [className]
 */
function element(doc, tag, className) {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  return node;
}

/** @param {Document} doc @param {Block} block @param {boolean} interactive @returns {HTMLElement} */
function renderBlock(doc, block, interactive) {
  switch (block.type) {
    case "paragraph":
      return withInlines(element(doc, "p"), block.children, interactive);
    case "heading":
      // The page owns h1 and h2; an answer's headings sit below them.
      return withInlines(element(doc, `h${Math.min(block.level + 2, 6)}`), block.children, interactive);
    case "code":
      return codeBlock(doc, block.lang, block.text, interactive);
    case "quote": {
      const quote = element(doc, "blockquote");
      quote.append(...block.children.map((child) => renderBlock(doc, child, interactive)));
      return quote;
    }
    case "list": {
      const list = element(doc, block.ordered ? "ol" : "ul");
      if (block.ordered && block.start !== 1) list.setAttribute("start", String(block.start));
      list.append(...block.items.map((item) => listItem(doc, item, interactive)));
      return list;
    }
    case "rule":
      return element(doc, "hr");
    case "table":
      return table(doc, block, interactive);
  }
}

/** @param {Document} doc @param {ListItem} item @param {boolean} interactive */
function listItem(doc, item, interactive) {
  const li = element(doc, "li");
  if (item.checked !== null) {
    li.className = "task";
    const box = element(doc, "input");
    box.setAttribute("type", "checkbox");
    box.setAttribute("disabled", "");
    if (item.checked) box.setAttribute("checked", "");
    li.append(box);
  }
  // A list item's first paragraph sits inline, as in a tight list.
  const [first, ...rest] = item.children;
  if (first?.type === "paragraph") withInlines(li, first.children, interactive);
  else if (first) li.append(renderBlock(doc, first, interactive));
  li.append(...rest.map((child) => renderBlock(doc, child, interactive)));
  return li;
}

/** @param {Document} doc @param {string} lang @param {string} text @param {boolean} interactive */
function codeBlock(doc, lang, text, interactive) {
  const wrapper = element(doc, "div", "code-block");
  const head = element(doc, "div", "code-head");
  const label = element(doc, "span", "code-lang");
  label.textContent = lang || "code";
  const copy = element(doc, "button", "message-copy code-copy");
  copy.setAttribute("type", "button");
  copy.setAttribute("aria-label", lang ? `Copy ${lang} code` : "Copy code");
  copy.textContent = "Copy";
  if (interactive) copy.addEventListener("click", () => { void copyWithFeedback(copy, text); });
  else copy.setAttribute("disabled", "");
  head.append(label, copy);
  const pre = element(doc, "pre");
  const code = element(doc, "code");
  if (/^[\w#+.-]+$/.test(lang)) code.className = `language-${lang}`;
  code.textContent = text;
  pre.append(code);
  wrapper.append(head, pre);
  return wrapper;
}

/** @param {Document} doc @param {Extract<Block, {type: "table"}>} block @param {boolean} interactive */
function table(doc, block, interactive) {
  const wrapper = element(doc, "div", "table-wrap");
  const tableNode = element(doc, "table");
  const head = element(doc, "thead");
  const body = element(doc, "tbody");
  /** @param {Inline[][]} cells @param {"th" | "td"} tag */
  const row = (cells, tag) => {
    const tr = element(doc, "tr");
    tr.append(...cells.map((cell, column) => {
      const node = withInlines(element(doc, tag), cell, interactive);
      const align = block.align[column];
      if (align) node.setAttribute("data-align", align);
      return node;
    }));
    return tr;
  };
  head.append(row(block.header, "th"));
  body.append(...block.rows.map((cells) => row(cells, "td")));
  tableNode.append(head, body);
  wrapper.append(tableNode);
  return wrapper;
}

/** @param {HTMLElement} parent @param {Inline[]} inlines @param {boolean} interactive */
function withInlines(parent, inlines, interactive) {
  const doc = parent.ownerDocument;
  for (const span of inlines) {
    switch (span.type) {
      case "text":
        // Text nodes only: nothing in an answer is parsed as markup.
        parent.append(span.text);
        break;
      case "break":
        parent.append(element(doc, "br"));
        break;
      case "code": {
        const code = element(doc, "code");
        code.textContent = span.text;
        parent.append(code);
        break;
      }
      case "link": {
        if (!interactive) {
          // Looks like the link it will become, but takes no focus yet.
          parent.append(withInlines(element(doc, "span", "link"), span.children, interactive));
          break;
        }
        const link = element(doc, "a");
        link.setAttribute("href", span.href);
        link.setAttribute("target", "_blank");
        link.setAttribute("rel", "noopener noreferrer");
        parent.append(withInlines(link, span.children, interactive));
        break;
      }
      default:
        parent.append(withInlines(element(doc, span.type), span.children, interactive));
    }
  }
  return parent;
}
