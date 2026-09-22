const params = new URLSearchParams(window.location.search);
const entry = params.get("entry");
const entryNode = document.querySelector("#entry");

entryNode.textContent = entry
  ? `Opened from: ${entry}`
  : "Opened directly.";
