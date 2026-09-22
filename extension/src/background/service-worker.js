const MENU_ID = "pervue-open-full-page";

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

chrome.commands.onCommand.addListener(async (command) => {
  if (command === "open-pervue-full-page") {
    await openFullPage("command");
  }
});

chrome.contextMenus.onClicked.addListener(async (info) => {
  if (info.menuItemId === MENU_ID) {
    await openFullPage("context-menu");
  }
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === "pervue.health") {
    sendResponse({
      ok: true,
      surface: "background",
      version: chrome.runtime.getManifest().version
    });
  }
});

export { MENU_ID, openFullPage };
