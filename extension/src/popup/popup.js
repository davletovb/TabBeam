/**
 * @param {string} selector
 * @returns {HTMLElement}
 */
function requireElement(selector) {
  const node = document.querySelector(selector);
  if (!(node instanceof HTMLElement)) {
    throw new Error(`popup element is missing: ${selector}`);
  }
  return node;
}

const statusNode = requireElement("#status");
const fullPageButton = requireElement("#open-full-page");

async function checkFoundation() {
  try {
    const response = await chrome.runtime.sendMessage({ type: "pervue.health" });
    statusNode.textContent = response?.ok
      ? `Extension ready · v${response.version}`
      : "Extension background unavailable";
  } catch {
    statusNode.textContent = "Extension background unavailable";
  }
}

fullPageButton.addEventListener("click", async () => {
  await chrome.tabs.create({
    url: chrome.runtime.getURL("src/fullpage/index.html?entry=popup")
  });
});

checkFoundation();

export {};
