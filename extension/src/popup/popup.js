const status = document.querySelector("#status");
const fullPageButton = document.querySelector("#open-full-page");

if (!(status instanceof HTMLElement)) {
  throw new Error("popup status element is missing");
}

if (!(fullPageButton instanceof HTMLButtonElement)) {
  throw new Error("popup full-page button is missing");
}

async function checkFoundation() {
  try {
    const response = await chrome.runtime.sendMessage({ type: "pervue.health" });
    status.textContent = response?.ok
      ? `Extension ready · v${response.version}`
      : "Extension background unavailable";
  } catch {
    status.textContent = "Extension background unavailable";
  }
}

fullPageButton.addEventListener("click", async () => {
  await chrome.tabs.create({
    url: chrome.runtime.getURL("src/fullpage/index.html?entry=popup")
  });
});

checkFoundation();
