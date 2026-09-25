// Keep this classic content script inert until the popup explicitly requests
// the selection. The service worker independently checks the response size.
const MAX_SELECTION_BYTES = 16 * 1024;

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
/** @param {string} value */
function boundedSelection(value) {
  let bytes = 0;
  const characters = [];
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    const width =
      codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    if (bytes + width > MAX_SELECTION_BYTES) {
      return { text: characters.join(""), truncated: true };
    }
    bytes += width;
    characters.push(character);
  }
  return { text: characters.join(""), truncated: false };
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
        sendResponse({ ok: true, ...boundedSelection(selection) });
      }
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
