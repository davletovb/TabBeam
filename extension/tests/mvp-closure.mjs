import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  THEME_STORAGE_KEY,
  applyTheme,
  bindThemeSelect,
  loadTheme,
  normalizeThemePreference
} from "../src/shared/theme.js";
import { createDiagnosticsState } from "../src/background/diagnostics.js";
import { renderDiagnostics } from "../src/popup/diagnostics.js";

class FakeElement {
  constructor() {
    this.textContent = "";
    this.hidden = false;
    this.disabled = false;
    this.open = false;
    this.value = "";
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {Map<string, ((event?: any) => void)[]>} */
    this.listeners = new Map();
  }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  /** @param {string} name */
  removeAttribute(name) { this.attributes.delete(name); }
  /** @param {string} name */
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  /** @param {string} type @param {(event?: any) => void} listener */
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  /** @param {string} type */
  dispatch(type) {
    for (const listener of this.listeners.get(type) ?? []) listener();
  }
}

{
  assert.equal(normalizeThemePreference("system"), "system");
  assert.equal(normalizeThemePreference("light"), "light");
  assert.equal(normalizeThemePreference("dark"), "dark");
  assert.equal(normalizeThemePreference("neon"), "system");

  /** @type {Record<string, any>} */
  const values = {};
  const storage = {
    async get(key) { return { [key]: values[key] }; },
    async set(next) { Object.assign(values, next); }
  };
  const root = new FakeElement();
  const select = new FakeElement();

  assert.equal(applyTheme(/** @type {any} */ (root), "system"), "system");
  assert.equal(root.getAttribute("data-theme"), null);
  await bindThemeSelect(
    /** @type {any} */ (select),
    storage,
    /** @type {any} */ (root)
  );
  assert.equal(select.value, "system");

  select.value = "dark";
  select.dispatch("change");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(values[THEME_STORAGE_KEY], "dark");
  assert.equal(root.getAttribute("data-theme"), "dark");

  const secondRoot = new FakeElement();
  assert.equal(
    await loadTheme(storage, /** @type {any} */ (secondRoot)),
    "dark"
  );
  assert.equal(secondRoot.getAttribute("data-theme"), "dark");
}

{
  const diagnostics = createDiagnosticsState("0.1.0", () => 1234);
  diagnostics.noteLifecycle({
    event: "host.ready",
    payload: { host_version: "0.1.0-dev", protocol_versions: [1] }
  });
  diagnostics.noteProvider({
    provider_id: "codex",
    status: {
      availability: "available",
      authentication: "authenticated",
      capabilities: {}
    }
  });
  diagnostics.noteFailure({
    code: "PROVIDER_FAILED",
    reason: "PROCESS_EXITED",
    message: "secret prompt text must never appear",
    retryable: true,
    prompt: "also-secret"
  });
  const summary = diagnostics.summary();
  assert.equal(summary.extension_version, "0.1.0");
  assert.deepEqual(summary.host, {
    state: "available",
    version: "0.1.0-dev",
    protocol_versions: [1]
  });
  assert.deepEqual(summary.provider, {
    provider_id: "codex",
    availability: "available",
    authentication: "authenticated"
  });
  assert.deepEqual(summary.recent_failure, {
    code: "PROVIDER_FAILED",
    reason: "PROCESS_EXITED",
    retryable: true,
    at: 1234
  });
  assert.ok(!JSON.stringify(summary).includes("secret"));

  diagnostics.noteFailure({
    code: "INTERNAL_ERROR",
    reason: "token=definitely-not-a-safe-reason",
    message: "credential-looking material",
    retryable: false
  });
  assert.equal(diagnostics.summary().recent_failure?.reason, "UNKNOWN");

  const elements = {
    host: new FakeElement(),
    protocol: new FakeElement(),
    provider: new FakeElement(),
    failure: new FakeElement()
  };
  renderDiagnostics(/** @type {any} */ (elements), summary);
  assert.equal(elements.host.textContent, "Companion 0.1.0-dev");
  assert.equal(elements.protocol.textContent, "Extension v1; companion: 1");
  assert.equal(elements.provider.textContent, "Codex: available, authenticated");
  assert.equal(elements.failure.textContent, "PROVIDER_FAILED / PROCESS_EXITED");
}

{
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const popup = fs.readFileSync(path.join(root, "src/popup/index.html"), "utf8");
  const fullpage = fs.readFileSync(path.join(root, "src/fullpage/index.html"), "utf8");
  const popupCss = fs.readFileSync(path.join(root, "src/popup/popup.css"), "utf8");
  const fullpageCss = fs.readFileSync(path.join(root, "src/fullpage/fullpage.css"), "utf8");

  for (const markup of [popup, fullpage]) {
    assert.ok(markup.includes('id="theme-select"'));
    assert.ok(markup.includes('value="system"'));
    assert.ok(markup.includes('value="light"'));
    assert.ok(markup.includes('value="dark"'));
    assert.ok(markup.includes('id="ask-cancel"'));
    assert.ok(markup.includes('id="ask-retry"'));
    assert.ok(markup.includes('aria-live="polite"'));
    assert.ok(markup.includes('aria-atomic="true"'));
  }
  assert.ok(popup.includes('id="diagnostics"'));
  assert.ok(popup.includes('id="companion-setup"'));
  assert.ok(popup.includes("../setup/index.html"));

  for (const css of [popupCss, fullpageCss]) {
    assert.ok(css.includes(':root[data-theme="light"]'));
    assert.ok(css.includes(':root[data-theme="dark"]'));
    assert.ok(css.includes(":focus-visible"));
    assert.ok(css.includes("prefers-reduced-motion"));
  }
}

console.log("EXT-11/EXT-13/OBS-02/EXT-14 MVP closure tests passed");
