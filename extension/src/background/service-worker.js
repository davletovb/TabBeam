import { ASK_PORT_NAME } from "../shared/ask-port.js";
import { PROVIDER_STATUS_MESSAGE } from "../shared/provider-status.js";
import { DIAGNOSTICS_MESSAGE } from "../shared/diagnostics.js";
import { isExtensionPage } from "./ask-bridge.js";
import { serveConversationAskPort } from "./conversation-bridge.js";
import { createConversationStore } from "./conversation-store.js";
import { checkProviderStatus } from "./status-bridge.js";
import { createDiagnosticsState } from "./diagnostics.js";
import {
  NATIVE_HOST_NAME,
  createNativeConnectionManager
} from "./native-connection.js";
import { handleContextCapture } from "./selection-capture.js";
import {
  MENU_CONSUME_MESSAGE,
  MENU_PAGE_ID,
  MENU_SELECTION_ID,
  createEntryActions
} from "./entry-actions.js";

const entryActions = createEntryActions({
  tabs: chrome.tabs,
  action: chrome.action,
  popupUrl: chrome.runtime.getURL("src/popup/index.html")
});

const nativeConnectionManager = createNativeConnectionManager({
  connectNative: (hostName) => chrome.runtime.connectNative(hostName),
  getLastError: () => chrome.runtime.lastError?.message ?? null
});
const conversations = createConversationStore(chrome.storage.local);
const inFlightConversations = new Set();
const diagnostics = createDiagnosticsState(chrome.runtime.getManifest().version);
nativeConnectionManager.onLifecycleEvent((event) => diagnostics.noteLifecycle(event));

/** @param {string} entry */
async function openFullPage(entry) {
  const url = new URL(chrome.runtime.getURL("src/fullpage/index.html"));
  if (entry) {
    url.searchParams.set("entry", entry);
  }
  await chrome.tabs.create({ url: url.toString() });
}

chrome.runtime.onInstalled.addListener(async () => {
  await chrome.contextMenus.removeAll();
  chrome.contextMenus.create({
    id: MENU_SELECTION_ID,
    title: "Ask Pervue about selection",
    contexts: ["selection"]
  });
  chrome.contextMenus.create({
    id: MENU_PAGE_ID,
    title: "Ask Pervue about this page",
    contexts: ["page"]
  });
});

chrome.contextMenus.onClicked.addListener(
  (/** @type {{menuItemId: string | number, selectionText?: string}} */ info,
    /** @type {{id?: number, url?: string, title?: string} | undefined} */ tab) =>
    entryActions.onMenuClick(info, tab)
);

chrome.runtime.onMessage.addListener(
  (
    /** @type {any} */ message,
    /** @type {any} */ sender,
    /** @type {(response: any) => void} */ sendResponse
  ) => {
    if (message?.type === "pervue.health") {
      sendResponse({
        ok: true,
        surface: "background",
        version: chrome.runtime.getManifest().version
      });
      return;
    }
    if (message?.type === PROVIDER_STATUS_MESSAGE) {
      // Only the extension's own pages may drive the native companion.
      if (!isExtensionPage(sender, chrome.runtime.getURL(""))) {
        return;
      }
      checkProviderStatus({ manager: nativeConnectionManager }).then((response) => {
        diagnostics.noteProvider(response);
        sendResponse(response);
      });
      return true;
    }
    if (message?.type === DIAGNOSTICS_MESSAGE) {
      if (!isExtensionPage(sender, chrome.runtime.getURL(""))) {
        return;
      }
      sendResponse(diagnostics.summary());
      return;
    }
    if (message?.type === MENU_CONSUME_MESSAGE) {
      entryActions.consume(message, sender).then(sendResponse);
      return true;
    }
    if (message?.type === "pervue.conversations.list" ||
        message?.type === "pervue.conversations.get") {
      if (!isExtensionPage(sender, chrome.runtime.getURL(""))) {
        sendResponse({ ok: false, error: "Access denied." });
        return;
      }
      const result = message.type === "pervue.conversations.list"
        ? conversations.list()
        : conversations.get(message.conversation_id);
      result.then((value) => sendResponse({ ok: true, value }),
        () => sendResponse({ ok: false, error: "Conversation history unavailable." }));
      return true;
    }
    return handleContextCapture(
      message,
      sender,
      sendResponse,
      chrome.tabs,
      chrome.runtime.getURL("src/popup/index.html")
    );
  }
);

chrome.runtime.onConnect.addListener(
  (/** @type {PervueChromeRuntimePort} */ port) => {
    if (port.name !== ASK_PORT_NAME) {
      return;
    }
    // Only the extension's own pages may drive the native companion; content
    // scripts run inside web pages.
    if (!isExtensionPage(port.sender, chrome.runtime.getURL(""))) {
      port.disconnect();
      return;
    }
    serveConversationAskPort(port, {
      manager: nativeConnectionManager,
      store: conversations,
      inFlight: inFlightConversations,
      onFailure: (error) => diagnostics.noteFailure(error)
    });
  }
);

export {
  MENU_PAGE_ID,
  MENU_SELECTION_ID,
  NATIVE_HOST_NAME,
  nativeConnectionManager,
  openFullPage
};
