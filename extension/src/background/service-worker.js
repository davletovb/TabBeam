import { ASK_PORT_NAME } from "../shared/ask-port.js";
import { isExtensionPage, serveAskPort } from "./ask-bridge.js";
import {
  NATIVE_HOST_NAME,
  createNativeConnectionManager
} from "./native-connection.js";
import { handleContextCapture } from "./selection-capture.js";

const MENU_ID = "pervue-open-full-page";

const nativeConnectionManager = createNativeConnectionManager({
  connectNative: (hostName) => chrome.runtime.connectNative(hostName),
  getLastError: () => chrome.runtime.lastError?.message ?? null
});

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
    id: MENU_ID,
    title: "Open in Pervue",
    contexts: ["page", "selection"]
  });
});

chrome.commands.onCommand.addListener(
  async (/** @type {string} */ command) => {
    if (command === "open-pervue-full-page") {
      await openFullPage("command");
    }
  }
);

chrome.contextMenus.onClicked.addListener(
  async (/** @type {{ menuItemId: string | number }} */ info) => {
    if (info.menuItemId === MENU_ID) {
      await openFullPage("context-menu");
    }
  }
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
    serveAskPort(port, { manager: nativeConnectionManager });
  }
);

export {
  MENU_ID,
  NATIVE_HOST_NAME,
  nativeConnectionManager,
  openFullPage
};
