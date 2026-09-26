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

/** @param {string} source @returns {Block[]} */
export function parseMarkdown(source) {
  const lines = source.replace(/\r\n?/g, "\n").split("\n").map(expandIndent);
  return parseBlocks(lines);
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

/** @param {string[]} lines @returns {Block[]} */
function parseBlocks(lines) {
  /** @type {Block[]} */
  const blocks = [];
  let i = 0;
  while (i < lines.length) {
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
      blocks.push({ type: "heading", level: heading[1].length, children: parseInline(heading[2] ?? "") });
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
      blocks.push({ type: "quote", children: parseBlocks(inner) });
      continue;
    }

    const item = LIST_ITEM.exec(line);
    if (item && item[1].length <= 3) {
      i = parseList(lines, i, blocks);
      continue;
    }

    if (isTableStart(lines, i)) {
      i = parseTable(lines, i, blocks);
      continue;
    }

    const paragraph = [line.trim()];
    i += 1;
    while (i < lines.length && !isBlank(lines[i]) && !interrupts(lines, i)) {
      paragraph.push(lines[i].trim());
      i += 1;
    }
    blocks.push({ type: "paragraph", children: parseInline(paragraph.join("\n")) });
  }
  return blocks;
}

/** @param {string[]} lines @param {number} i @param {Block[]} blocks @returns {number} the next line */
function parseList(lines, i, blocks) {
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
    items.push({ checked: task ? task[1] !== " " : null, children: parseBlocks(body) });
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

/** @param {string[]} lines @param {number} i @param {Block[]} blocks @returns {number} the next line */
function parseTable(lines, i, blocks) {
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
    rows.push(header.map((_, column) => parseInline(cells[column] ?? "")));
    i += 1;
  }
  blocks.push({ type: "table", align, header: header.map((cell) => parseInline(cell)), rows });
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

/** A code span opening at `i`, if it closes. @param {string} src @param {number} i */
function codeSpanAt(src, i) {
  const size = runLength(src, i, "`");
  let j = i + size;
  while (j < src.length) {
    if (src[j] !== "`") {
      j += 1;
      continue;
    }
    const run = runLength(src, j, "`");
    if (run === size) {
      let text = src.slice(i + size, j).replace(/\n/g, " ");
      if (text.length > 2 && text.startsWith(" ") && text.endsWith(" ") && text.trim()) text = text.slice(1, -1);
      return { text, end: j + size };
    }
    j += run;
  }
  return null;
}

/** A link `[label](destination "title")` opening at `i`. @param {string} src @param {number} i */
function linkAt(src, i) {
  let depth = 0;
  let close = -1;
  for (let j = i; j < src.length; j += 1) {
    if (src[j] === "\\") j += 1;
    else if (src[j] === "[") depth += 1;
    else if (src[j] === "]" && --depth === 0) {
      close = j;
      break;
    }
  }
  if (close < 0 || src[close + 1] !== "(") return null;
  let j = close + 2;
  while (src[j] === " ") j += 1;
  let destination = "";
  if (src[j] === "<") {
    const end = src.indexOf(">", j);
    if (end < 0) return null;
    destination = src.slice(j + 1, end);
    j = end + 1;
  } else {
    let parens = 0;
    const start = j;
    while (j < src.length && !SPACE.test(src[j])) {
      if (src[j] === "(") parens += 1;
      else if (src[j] === ")" && parens-- === 0) break;
      j += 1;
    }
    destination = src.slice(start, j);
  }
  while (src[j] === " ") j += 1;
  const quote = src[j];
  if (quote === "\"" || quote === "'") {
    const end = src.indexOf(quote, j + 1);
    if (end < 0) return null;
    j = end + 1;
    while (src[j] === " ") j += 1;
  }
  if (src[j] !== ")") return null;
  return { label: src.slice(i + 1, close), href: safeHref(destination), end: j + 1 };
}

/** A bare http(s) URL starting at `i`. @param {string} src @param {number} i */
function bareUrlAt(src, i) {
  const match = /^https?:\/\/[^\s<>"]+/i.exec(src.slice(i));
  if (!match) return null;
  let url = match[0].replace(/[.,:;!?'"*_~]+$/, "");
  // Keep a closing parenthesis only if the URL opened one.
  while (url.endsWith(")") && (url.match(/\(/g)?.length ?? 0) < (url.match(/\)/g)?.length ?? 0)) {
    url = url.slice(0, -1).replace(/[.,:;!?'"*_~]+$/, "");
  }
  const href = safeHref(url);
  return href ? { text: url, href, end: i + url.length } : null;
}

/** Emphasis or strikethrough opening at `i`. @param {string} src @param {number} i @param {boolean} inLink */
function emphasisAt(src, i, inLink) {
  const ch = src[i];
  const run = runLength(src, i, ch);
  if (ch === "~" ? run !== 2 : run > 3) return null;
  const after = src[i + run];
  if (after === undefined || SPACE.test(after)) return null;
  if (ch === "_" && WORD.test(src[i - 1] ?? "")) return null;

  let j = i + run;
  while (j < src.length) {
    const c = src[j];
    if (c === "\\") {
      j += 2;
      continue;
    }
    if (c === "`") {
      const span = codeSpanAt(src, j);
      j = span ? span.end : j + runLength(src, j, "`");
      continue;
    }
    if (c !== ch) {
      j += 1;
      continue;
    }
    const closing = runLength(src, j, ch);
    const before = src[j - 1];
    if (closing === run && j > i + run && !SPACE.test(before) &&
        !(ch === "_" && WORD.test(src[j + closing] ?? ""))) {
      const children = parseInline(src.slice(i + run, j), inLink);
      /** @type {Inline} */
      const node = ch === "~" ? { type: "del", children }
        : run === 1 ? { type: "em", children }
        : run === 2 ? { type: "strong", children }
        : { type: "strong", children: [{ type: "em", children }] };
      return { node, end: j + closing };
    }
    j += closing;
  }
  return null;
}

/**
 * @param {string} src
 * @param {boolean} [inLink] inside a link's text, where links can't nest
 * @returns {Inline[]}
 */
export function parseInline(src, inLink = false) {
  /** @type {Inline[]} */
  const out = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ type: "text", text });
    text = "";
  };

  let i = 0;
  while (i < src.length) {
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
      const span = codeSpanAt(src, i);
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
      const link = linkAt(src, image ? i + 1 : i);
      if (!link) {
        text += ch;
        i += 1;
        continue;
      }
      flush();
      /** @type {Inline[]} */
      const children = image
        ? [{ type: "text", text: link.label || "Image" }]
        : parseInline(link.label, true);
      if (link.href) out.push({ type: "link", href: link.href, children });
      else out.push(...children);
      i = link.end;
    } else if (!inLink && ch === "<" && /^<(?:https?|mailto):[^\s<>]+>/i.test(src.slice(i))) {
      const end = src.indexOf(">", i);
      const target = src.slice(i + 1, end);
      const href = safeHref(target);
      if (href) {
        flush();
        out.push({ type: "link", href, children: [{ type: "text", text: target }] });
      } else {
        text += src.slice(i, end + 1);
      }
      i = end + 1;
    } else if (!inLink && (ch === "h" || ch === "H") && !WORD.test(src[i - 1] ?? "") && bareUrlAt(src, i)) {
      const url = /** @type {{text: string, href: string, end: number}} */ (bareUrlAt(src, i));
      flush();
      out.push({ type: "link", href: url.href, children: [{ type: "text", text: url.text }] });
      i = url.end;
    } else if (ch === "*" || ch === "_" || ch === "~") {
      const emphasis = emphasisAt(src, i, inLink);
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
 * Renders `source` into `container`, replacing what it held.
 *
 * @param {HTMLElement} container
 * @param {string} source
 */
export function renderMarkdown(container, source) {
  const doc = container.ownerDocument;
  container.replaceChildren(...parseMarkdown(source).map((block) => renderBlock(doc, block)));
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

/** @param {Document} doc @param {Block} block @returns {HTMLElement} */
function renderBlock(doc, block) {
  switch (block.type) {
    case "paragraph":
      return withInlines(element(doc, "p"), block.children);
    case "heading":
      // The page owns h1 and h2; an answer's headings sit below them.
      return withInlines(element(doc, `h${Math.min(block.level + 2, 6)}`), block.children);
    case "code":
      return codeBlock(doc, block.lang, block.text);
    case "quote": {
      const quote = element(doc, "blockquote");
      quote.append(...block.children.map((child) => renderBlock(doc, child)));
      return quote;
    }
    case "list": {
      const list = element(doc, block.ordered ? "ol" : "ul");
      if (block.ordered && block.start !== 1) list.setAttribute("start", String(block.start));
      list.append(...block.items.map((item) => listItem(doc, item)));
      return list;
    }
    case "rule":
      return element(doc, "hr");
    case "table":
      return table(doc, block);
  }
}

/** @param {Document} doc @param {ListItem} item */
function listItem(doc, item) {
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
  if (first?.type === "paragraph") withInlines(li, first.children);
  else if (first) li.append(renderBlock(doc, first));
  li.append(...rest.map((child) => renderBlock(doc, child)));
  return li;
}

/** @param {Document} doc @param {string} lang @param {string} text */
function codeBlock(doc, lang, text) {
  const wrapper = element(doc, "div", "code-block");
  const head = element(doc, "div", "code-head");
  const label = element(doc, "span", "code-lang");
  label.textContent = lang || "code";
  const copy = element(doc, "button", "message-copy code-copy");
  copy.setAttribute("type", "button");
  copy.setAttribute("aria-label", lang ? `Copy ${lang} code` : "Copy code");
  copy.textContent = "Copy";
  copy.addEventListener("click", () => { void copyWithFeedback(copy, text); });
  head.append(label, copy);
  const pre = element(doc, "pre");
  const code = element(doc, "code");
  if (/^[\w#+.-]+$/.test(lang)) code.className = `language-${lang}`;
  code.textContent = text;
  pre.append(code);
  wrapper.append(head, pre);
  return wrapper;
}

/** @param {Document} doc @param {Extract<Block, {type: "table"}>} block */
function table(doc, block) {
  const wrapper = element(doc, "div", "table-wrap");
  const tableNode = element(doc, "table");
  const head = element(doc, "thead");
  const body = element(doc, "tbody");
  /** @param {Inline[][]} cells @param {"th" | "td"} tag */
  const row = (cells, tag) => {
    const tr = element(doc, "tr");
    tr.append(...cells.map((cell, column) => {
      const node = withInlines(element(doc, tag), cell);
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

/** @param {HTMLElement} parent @param {Inline[]} inlines */
function withInlines(parent, inlines) {
  const doc = parent.ownerDocument;
  for (const inline of inlines) {
    switch (inline.type) {
      case "text":
        // Text nodes only: nothing in an answer is parsed as markup.
        parent.append(inline.text);
        break;
      case "break":
        parent.append(element(doc, "br"));
        break;
      case "code": {
        const code = element(doc, "code");
        code.textContent = inline.text;
        parent.append(code);
        break;
      }
      case "link": {
        const link = element(doc, "a");
        link.setAttribute("href", inline.href);
        link.setAttribute("target", "_blank");
        link.setAttribute("rel", "noopener noreferrer");
        parent.append(withInlines(link, inline.children));
        break;
      }
      default:
        parent.append(withInlines(element(doc, inline.type), inline.children));
    }
  }
  return parent;
}
