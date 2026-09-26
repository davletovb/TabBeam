import {
  MAX_CONTEXT_URL_BYTES,
  MAX_PAGE_BYTES,
  MAX_SELECTION_BYTES,
  boundedUtf8Text,
  utf8ByteLength
} from "../shared/limits.js";

export const CONTEXT_CAPTURE_MESSAGE = "pervue.context.capture";
const CONTENT_MESSAGES = Object.freeze({
  selection: "pervue.selection.read",
  page: "pervue.page.read"
});

const ERRORS = Object.freeze({
  PAGE_ACCESS_DENIED: "Pervue cannot access this tab. Allow site access and try again.",
  PAGE_NOT_SCRIPTABLE: "This page does not support context capture.",
  SELECTION_UNAVAILABLE: "Select some text on the page first.",
  PAGE_EXTRACTION_FAILED: "No readable text was found on this page.",
  CONTEXT_TOO_LARGE: "The page returned more context than Pervue can accept."
});

/** @param {keyof typeof ERRORS} reason @param {"denied" | "unsupported" | "granted"} permission */
function failure(reason, permission = "granted") {
  return {
    ok: /** @type {false} */ (false),
    permission,
    error: { code: "CONTEXT_UNAVAILABLE", reason, message: ERRORS[reason], retryable: false }
  };
}

/** @param {{query(query: object): Promise<{id?: number, url?: string, title?: string}[]>}} tabs */
export async function getActiveTabMetadata(tabs) {
  try {
    const [tab] = await tabs.query({ active: true, currentWindow: true });
    return metadataForTab(tab);
  } catch {
    return failure("PAGE_NOT_SCRIPTABLE", "unsupported");
  }
}

/** @param {{id?: number, url?: string, title?: string} | undefined} tab */
export function metadataForTab(tab) {
  try {
    if (typeof tab?.id !== "number" || !Number.isInteger(tab.id)) {
      return failure("PAGE_NOT_SCRIPTABLE", "unsupported");
    }
    if (typeof tab.url !== "string") {
      return failure("PAGE_ACCESS_DENIED", "denied");
    }
    const url = new URL(tab.url);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return failure("PAGE_NOT_SCRIPTABLE", "unsupported");
    }
    // Omit credentials, query, and fragment before page metadata can reach a provider.
    url.username = "";
    url.password = "";
    url.search = "";
    url.hash = "";
    if (utf8ByteLength(url.href) > MAX_CONTEXT_URL_BYTES) {
      return failure("CONTEXT_TOO_LARGE");
    }
    const title = typeof tab.title === "string"
      ? [...tab.title.replace(/[\uD800-\uDFFF]/gu, "\ufffd")].slice(0, 256).join("")
      : url.hostname;
    return { ok: /** @type {true} */ (true), permission: "granted", tabId: tab.id, page: { title, url: url.href } };
  } catch {
    return failure("PAGE_NOT_SCRIPTABLE", "unsupported");
  }
}

/**
 * Only the popup's explicit action can request content. Context is ephemeral:
 * no content or per-site permission state is stored here.
 *
 * @param {any} message
 * @param {{url?: string}} sender
 * @param {(response: any) => void} sendResponse
 * @param {{query(query: object): Promise<{id?: number, url?: string, title?: string}[]>, sendMessage(tabId: number, message: any, options: object): Promise<any>}} tabs
 * @param {string} popupUrl
 * @returns {boolean}
 */
export function handleContextCapture(message, sender, sendResponse, tabs, popupUrl) {
  if (message?.type !== CONTEXT_CAPTURE_MESSAGE) {
    return false;
  }
  if (sender?.url !== popupUrl || message.intent !== "user_click") {
    sendResponse(failure("PAGE_ACCESS_DENIED", "denied"));
    return false;
  }
  if (message.mode !== "selection" && message.mode !== "page") {
    sendResponse(failure("PAGE_NOT_SCRIPTABLE", "unsupported"));
    return false;
  }
  captureContext(tabs, message.mode).then(sendResponse);
  return true;
}

/**
 * Shared capture path for popup clicks and browser context-menu clicks. A menu
 * selection comes from Chrome's click event (including selections in frames),
 * while a popup selection is read from the top-frame content script.
 * @param {Parameters<typeof handleContextCapture>[3]} tabs
 * @param {"selection" | "page"} mode
 * @param {{id?: number, url?: string, title?: string}} [tab]
 * @param {string} [menuSelection]
 */
export async function captureContext(tabs, mode, tab, menuSelection) {
  const metadata = tab ? metadataForTab(tab) : await getActiveTabMetadata(tabs);
  if (!metadata.ok) {
    return metadata;
  }
  if (mode === "selection" && menuSelection !== undefined) {
    if (menuSelection.trim() === "") {
      return failure("SELECTION_UNAVAILABLE");
    }
    const bounded = boundedUtf8Text(menuSelection, MAX_SELECTION_BYTES);
    return {
      ok: true,
      permission: "granted",
      context: { mode, ...bounded, page: metadata.page }
    };
  }
  try {
    const response = await tabs.sendMessage(
      metadata.tabId,
      { type: CONTENT_MESSAGES[mode] },
      { frameId: 0 }
    );
    if (response?.ok === false) {
      if (mode === "selection" && response.reason === "SELECTION_UNAVAILABLE") {
        return failure("SELECTION_UNAVAILABLE");
      }
      return failure("PAGE_EXTRACTION_FAILED");
    }
    if (response?.ok !== true || typeof response.text !== "string") {
      return failure("PAGE_EXTRACTION_FAILED");
    }
    if (response.text.trim() === "") {
      return failure(mode === "selection" ? "SELECTION_UNAVAILABLE" : "PAGE_EXTRACTION_FAILED");
    }
    const limit = mode === "selection" ? MAX_SELECTION_BYTES : MAX_PAGE_BYTES;
    if (utf8ByteLength(response.text) > limit) {
      return failure("CONTEXT_TOO_LARGE");
    }
    return {
      ok: true,
      permission: "granted",
      context: {
        mode,
        text: response.text,
        truncated: response.truncated === true,
        page: metadata.page
      }
    };
  } catch {
    return failure("PAGE_ACCESS_DENIED", "denied");
  }
}
