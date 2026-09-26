import { bindThemeSurface } from "../shared/theme.js";

void bindThemeSurface(
  chrome.storage.local,
  document.documentElement,
  chrome.storage.onChanged
);
