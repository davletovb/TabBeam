// Keep this classic content script inert until the popup explicitly requests
// the selection. The service worker independently checks the response size.
const MAX_SELECTION_BYTES = 16 * 1024;
const MAX_PAGE_BYTES = 64 * 1024;
const MAX_PAGE_TEXT_NODES = 5000;

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
  const root = document.querySelector("main, article") ?? document.body;
  if (!root) {
    return { ok: false, reason: "PAGE_EXTRACTION_FAILED" };
  }
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  /** @type {string[]} */
  const parts = [];
  let bytes = 0;
  let inspected = 0;
  /** @type {Node | null} */
  let node;
  while (inspected < MAX_PAGE_TEXT_NODES && (node = walker.nextNode())) {
    inspected += 1;
    const parent = node.parentElement;
    // isContentEditable respects the nearest contenteditable value: a
    // contenteditable="false" island inside an editor is readable.
    if (parent?.isContentEditable || parent?.closest("script,style,noscript,template,svg,nav,footer,aside,form,[hidden],[aria-hidden='true']")) {
      continue;
    }
    const value = node.nodeValue?.replace(/\s+/gu, " ").trim();
    if (!value) {
      continue;
    }
    const separator = parts.length ? "\n" : "";
    const remaining = MAX_PAGE_BYTES - bytes;
    const bounded = boundedText(separator + value, remaining);
    if (bounded.text) {
      parts.push(bounded.text);
      bytes += new TextEncoder().encode(bounded.text).length;
    }
    if (bounded.truncated) {
      return { ok: true, text: parts.join(""), truncated: true, inspected };
    }
  }
  if (!parts.length) {
    return { ok: false, reason: "PAGE_EXTRACTION_FAILED" };
  }
  return {
    ok: true,
    text: parts.join(""),
    truncated: inspected === MAX_PAGE_TEXT_NODES && walker.nextNode() !== null,
    inspected
  };
}

chrome.runtime.onMessage.addListener(
  (
    /** @type {any} */ message,
    /** @type {any} */ _sender,
    /** @type {(response: any) => void} */ sendResponse
  ) => {
    if (message?.type === "pervue.selection.read") {
      const selection = selectedText();
      if (selection.trim() === "") {
        sendResponse({ ok: false, reason: "SELECTION_UNAVAILABLE" });
      } else {
        sendResponse({ ok: true, ...boundedText(selection, MAX_SELECTION_BYTES) });
      }
      return;
    }
    if (message?.type === "pervue.page.read") {
      sendResponse(readablePage());
      return;
    }
    if (message?.type === "pervue.ping") {
      sendResponse({
        ok: true,
        surface: "content",
        page: {
          title: document.title,
          url: window.location.href
        }
      });
    }
  }
);
