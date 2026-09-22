const params = new URLSearchParams(window.location.search);
const entry = params.get("entry");
const entryNode = document.querySelector("#entry");

if (!(entryNode instanceof HTMLElement)) {
  throw new Error("full-page entry status element is missing");
}

entryNode.textContent = entry
  ? `Opened from: ${entry}`
  : "Opened directly.";
