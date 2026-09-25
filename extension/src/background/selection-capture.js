import { MAX_SELECTION_BYTES, utf8ByteLength } from "../shared/limits.js";

export const SELECTION_CAPTURE_MESSAGE = "pervue.selection.capture";
const CONTENT_SELECTION_MESSAGE = "pervue.selection.read";

const ERRORS = Object.freeze({
  PAGE_ACCESS_DENIED: Object.freeze({
    code: "CONTEXT_UNAVAILABLE",
    reason: "PAGE_ACCESS_DENIED",
    message: "Selection can only be inserted from the Pervue popup."
  }),
  PAGE_NOT_SCRIPTABLE: Object.freeze({
    code: "CONTEXT_UNAVAILABLE",
    reason: "PAGE_NOT_SCRIPTABLE",
    message: "This page does not support selected-text capture."
  }),
  SELECTION_UNAVAILABLE: Object.freeze({
    code: "CONTEXT_UNAVAILABLE",
    reason: "SELECTION_UNAVAILABLE",
    message: "Select some text on the page first."
  }),
  CONTEXT_TOO_LARGE: Object.freeze({
    code: "CONTEXT_UNAVAILABLE",
    reason: "CONTEXT_TOO_LARGE",
    message: "The page returned too much selected text."
  })
});

/** @param {keyof typeof ERRORS} reason */
function failure(reason) {
  return { ok: false, error: ERRORS[reason] };
}

/**
 * The caller must be the popup, where a button click initiates this request.
 * No page selection is read during navigation, popup startup, or an Ask submit.
 *
 * @param {any} message
 * @param {{url?: string}} sender
 * @param {(response: any) => void} sendResponse
 * @param {{query(query: object): Promise<{id?: number}[]>, sendMessage(tabId: number, message: any, options: object): Promise<any>}} tabs
 * @param {string} popupUrl
 * @returns {boolean} whether Chrome must keep the response channel open
 */
export function handleSelectionCapture(message, sender, sendResponse, tabs, popupUrl) {
  if (message?.type !== SELECTION_CAPTURE_MESSAGE) {
    return false;
  }
  if (sender?.url !== popupUrl) {
    sendResponse(failure("PAGE_ACCESS_DENIED"));
    return false;
  }
  capture(tabs).then(sendResponse);
  return true;
}

/** @param {{query(query: object): Promise<{id?: number}[]>, sendMessage(tabId: number, message: any, options: object): Promise<any>}} tabs */
async function capture(tabs) {
  try {
    const [tab] = await tabs.query({ active: true, currentWindow: true });
    if (typeof tab?.id !== "number" || !Number.isInteger(tab.id)) {
      return failure("PAGE_NOT_SCRIPTABLE");
    }
    // This script runs only in the top frame; request it explicitly so later
    // frame support cannot turn this capture into an ambiguous first reply.
    const response = await tabs.sendMessage(
      tab.id,
      { type: CONTENT_SELECTION_MESSAGE },
      { frameId: 0 }
    );
    if (response?.ok === false && response.reason === "SELECTION_UNAVAILABLE") {
      return failure("SELECTION_UNAVAILABLE");
    }
    if (response?.ok !== true || typeof response.text !== "string") {
      return failure("PAGE_NOT_SCRIPTABLE");
    }
    if (response.text.trim() === "") {
      return failure("SELECTION_UNAVAILABLE");
    }
    if (utf8ByteLength(response.text) > MAX_SELECTION_BYTES) {
      return failure("CONTEXT_TOO_LARGE");
    }
    return { ok: true, text: response.text, truncated: response.truncated === true };
  } catch {
    // Chrome rejects sendMessage on internal pages and on tabs without our
    // content script. Never return its raw error string or page data.
    return failure("PAGE_NOT_SCRIPTABLE");
  }
}
