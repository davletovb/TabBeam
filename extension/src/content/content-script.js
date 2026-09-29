// Keep this classic content script inert until the popup explicitly requests
// the selection. The service worker independently checks the response size.
const MAX_SELECTION_BYTES = 16 * 1024;
const MAX_PAGE_BYTES = 64 * 1024;
const MAX_PAGE_TEXT_NODES = 5000;
const MAX_PAGE_SCAN_CHARS = 512 * 1024;

function selectedText() {
  const active = /** @type {HTMLInputElement | HTMLTextAreaElement | null} */ (
    document.activeElement
  );
  if (
    active &&
    (active.tagName === "TEXTAREA" ||
      (active.tagName === "INPUT" &&
        /^(text|search|url|tel|email)$/.test(active.type))) &&
    typeof active.selectionStart === "number" &&
    typeof active.selectionEnd === "number" &&
    active.selectionEnd > active.selectionStart
  ) {
    return active.value.slice(active.selectionStart, active.selectionEnd);
  }
  return window.getSelection()?.toString() ?? "";
}

/** Limit UTF-8 bytes without splitting a surrogate pair or allocating an encoded copy. */
/** @param {string} value @param {number} maxBytes */
function boundedText(value, maxBytes) {
  let bytes = 0;
  const characters = [];
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    const width =
      codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    if (bytes + width > maxBytes) {
      return { text: characters.join(""), truncated: true };
    }
    bytes += width;
    // Page-controlled strings may contain a lone UTF-16 surrogate. The host
    // rejects one on the wire, so replace it before returning the text. Its
    // UTF-8 replacement character has the same three-byte width.
    characters.push(codePoint >= 0xd800 && codePoint <= 0xdfff ? "\ufffd" : character);
  }
  return { text: characters.join(""), truncated: false };
}

// Never serialize the DOM or form values. Scan a bounded number of text nodes,
// preferring the page's main content and skipping navigation, hidden content,
// editable fields, scripts, and other non-reading surfaces.
function readablePage() {
  let inspected = 0;
  let scanned = 0;
  const roots = [document.querySelector("main"), document.querySelector("article"), document.body];
  for (const [index, root] of roots.entries()) {
    if (!root || roots.indexOf(root) !== index ||
        inspected >= MAX_PAGE_TEXT_NODES || scanned >= MAX_PAGE_SCAN_CHARS) {
      continue;
    }
    const result = readableFrom(root, MAX_PAGE_TEXT_NODES - inspected, MAX_PAGE_SCAN_CHARS - scanned);
    inspected += result.inspected;
    scanned += result.scanned;
    if (result.ok) {
      return { ok: true, text: result.text, truncated: result.truncated, inspected };
    }
  }
  return { ok: false, reason: "PAGE_EXTRACTION_FAILED" };
}

/** @param {Element} root @param {number} maxNodes @param {number} maxChars */
function readableFrom(root, maxNodes, maxChars) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const hiddenByStyle = new WeakMap();
  /** @type {string[]} */
  const parts = [];
  let bytes = 0;
  let inspected = 0;
  let scanned = 0;
  let clipped = false;
  /** @type {Node | null} */
  let node;
  while (inspected < maxNodes && scanned < maxChars && (node = walker.nextNode())) {
    inspected += 1;
    const parent = node.parentElement;
    // isContentEditable respects the nearest contenteditable value: a
    // contenteditable="false" island inside an editor is readable.
    if (parent?.isContentEditable || parent?.closest("script,style,noscript,template,svg,nav,footer,aside,form,[hidden],[aria-hidden='true']") ||
        (parent && hasHiddenStyle(parent, hiddenByStyle))) {
      continue;
    }
    const raw = node.nodeValue ?? "";
    const slice = raw.slice(0, maxChars - scanned);
    scanned += slice.length;
    clipped = slice.length < raw.length;
    const value = slice.replace(/\s+/gu, " ").trim();
    if (!value) {
      if (clipped) break;
      continue;
    }
    const separator = parts.length ? "\n" : "";
    const remaining = MAX_PAGE_BYTES - bytes;
    const bounded = boundedText(separator + value, remaining);
    if (bounded.text) {
      parts.push(bounded.text);
      bytes += new TextEncoder().encode(bounded.text).length;
    }
    if (bounded.truncated || clipped) {
      return { ok: true, text: parts.join(""), truncated: true, inspected, scanned };
    }
  }
  if (!parts.length) {
    return { ok: false, inspected, scanned };
  }
  return {
    ok: true,
    text: parts.join(""),
    truncated: clipped ||
      ((inspected === maxNodes || scanned === maxChars) && walker.nextNode() !== null),
    inspected,
    scanned
  };
}

/** @param {Element} element @param {WeakMap<Element, boolean>} cache */
function hasHiddenStyle(element, cache) {
  /** @type {Element[]} */
  const visited = [];
  /** @type {Element | null} */
  let current = element;
  let hidden = false;
  while (current) {
    const cached = cache.get(current);
    if (cached !== undefined) {
      hidden = cached;
      break;
    }
    visited.push(current);
    const style = window.getComputedStyle(current);
    if (style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse") {
      hidden = true;
      break;
    }
    current = current.parentElement;
  }
  for (const node of visited) cache.set(node, hidden);
  return hidden;
}

chrome.runtime.onMessage.addListener(
  (
    /** @type {any} */ message,
    /** @type {any} */ _sender,
    /** @type {(response: any) => void} */ sendResponse
  ) => {
    if (message?.type === "tabbeam.selection.read") {
      const selection = selectedText();
      if (selection.trim() === "") {
        sendResponse({ ok: false, reason: "SELECTION_UNAVAILABLE" });
      } else {
        sendResponse({ ok: true, ...boundedText(selection, MAX_SELECTION_BYTES) });
      }
      return;
    }
    if (message?.type === "tabbeam.page.read") {
      sendResponse(readablePage());
      return;
    }
    if (message?.type === "tabbeam.ping") {
      sendResponse({
        ok: true,
        surface: "content"
      });
    }
  }
);
