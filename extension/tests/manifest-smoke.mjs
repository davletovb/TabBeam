import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifestPath = path.join(root, "manifest.json");
/** @type {any} */
const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));

assert.equal(manifest.manifest_version, 3);
assert.equal(manifest.action.default_popup, "src/popup/index.html");
assert.equal(manifest.background.service_worker, "src/background/service-worker.js");
assert.equal(manifest.background.type, "module");
assert.ok(manifest.permissions.includes("contextMenus"));
assert.ok(manifest.permissions.includes("nativeMessaging"));
assert.ok(manifest.commands["open-pervue-full-page"]);

const contentScript = manifest.content_scripts[0];
assert.deepEqual(contentScript.matches, ["http://*/*", "https://*/*"]);
assert.ok(!contentScript.matches.includes("<all_urls>"));

const referencedFiles = [
  manifest.action.default_popup,
  manifest.background.service_worker,
  ...contentScript.js,
  "src/background/native-connection.js",
  "src/fullpage/index.html",
  "src/popup/popup.js",
  "src/popup/popup.css",
  "src/fullpage/fullpage.js",
  "src/fullpage/fullpage.css"
];

for (const file of referencedFiles) {
  assert.ok(fs.existsSync(path.join(root, file)), `missing extension file: ${file}`);
}

/** @type {any} */
const listeners = {
  installed: [],
  messages: [],
  commands: [],
  contextMenuClicks: []
};

let nativeConnectCalls = 0;

globalThis.chrome = /** @type {any} */ ({
  runtime: {
    onInstalled: {
      addListener: (/** @type {any} */ fn) => listeners.installed.push(fn)
    },
    onMessage: {
      addListener: (/** @type {any} */ fn) => listeners.messages.push(fn)
    },
    getURL: (/** @type {string} */ relative) =>
      `chrome-extension://test/${relative}`,
    getManifest: () => manifest,
    connectNative: () => {
      nativeConnectCalls += 1;
      throw new Error("smoke test should not connect eagerly");
    }
  },
  commands: {
    onCommand: {
      addListener: (/** @type {any} */ fn) => listeners.commands.push(fn)
    }
  },
  contextMenus: {
    onClicked: {
      addListener: (/** @type {any} */ fn) =>
        listeners.contextMenuClicks.push(fn)
    },
    removeAll: async () => {},
    create: () => 1
  },
  tabs: {
    create: async () => ({ id: 1 })
  }
});

await import(
  `${pathToFileURL(path.join(root, manifest.background.service_worker)).href}?smoke=1`
);

assert.equal(nativeConnectCalls, 0);
assert.equal(listeners.installed.length, 1);
assert.equal(listeners.messages.length, 1);
assert.equal(listeners.commands.length, 1);
assert.equal(listeners.contextMenuClicks.length, 1);

/** @type {any} */
let healthResponse;
listeners.messages[0](
  { type: "pervue.health" },
  {},
  (/** @type {any} */ response) => {
    healthResponse = response;
  }
);

assert.deepEqual(healthResponse, {
  ok: true,
  surface: "background",
  version: manifest.version
});

console.log("EXT-01/EXT-02 manifest and service-worker smoke checks passed");
