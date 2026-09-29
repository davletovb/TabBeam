import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { probeInstalledHost } from "../../scripts/installed-host-probe.mjs";

const extensionId = process.argv[2];
assert.match(extensionId ?? "", /^[a-p]{32}$/);

const localAppData = process.env.LOCALAPPDATA ?? "";
assert.ok(path.isAbsolute(localAppData), "LOCALAPPDATA must be an absolute path");
const appDir = path.join(localAppData, "Programs", "TabBeam");
const manifestPath = path.join(appDir, "com.tabbeam.host.json");
const host = path.join(appDir, "tabbeam-host.exe");
const buildPath = path.join(appDir, "build-info.json");
const registryKey = String.raw`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.tabbeam.host`;

const registry = spawnSync(
  "reg.exe",
  ["query", registryKey, "/ve", "/reg:64"],
  { encoding: "utf8", windowsHide: true }
);
assert.equal(registry.status, 0, registry.stderr || registry.stdout);
assert.ok(
  registry.stdout.toLowerCase().includes(manifestPath.toLowerCase()),
  `Native Messaging registry value does not point to ${manifestPath}`
);

const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const build = JSON.parse(fs.readFileSync(buildPath, "utf8"));
assert.deepEqual(Object.keys(manifest).sort(), [
  "allowed_origins", "description", "name", "path", "type"
]);
assert.equal(manifest.name, "com.tabbeam.host");
assert.equal(manifest.path, "tabbeam-host.exe");
assert.equal(manifest.type, "stdio");
assert.deepEqual(manifest.allowed_origins, [`chrome-extension://${extensionId}/`]);
assert.equal(path.resolve(path.dirname(manifestPath), manifest.path), host);

assert.deepEqual(Object.keys(build).sort(), [
  "architecture", "extension_id", "package_version", "source_commit", "version"
]);
assert.equal(build.extension_id, extensionId);
assert.equal(build.architecture, "windows-x86_64");
assert.match(build.version, /^\d+\.\d+\.\d+/);
assert.match(build.package_version, /^\d+\.\d+\.\d+$/);
assert.match(build.source_commit, /^[0-9a-f]{40}$/);
assert.ok(fs.statSync(host).isFile());

const { code, signal, stderr, events } = await probeInstalledHost(
  host,
  [`chrome-extension://${extensionId}/`, "--parent-window=0"],
  { windowsHide: true }
);
assert.equal(signal, null, stderr);
assert.equal(code, 0, stderr);
assert.equal(events[0]?.event, "host.ready");
assert.ok(events[0]?.payload?.protocol_versions?.includes(1));
const status = events.find(event =>
  event.event === "provider.status" && event.request_id === "package_status"
);
assert.ok(status, "provider.status was not emitted");
assert.equal(status.payload?.status?.availability, "available", stderr);
assert.equal(events.at(-1)?.event, "response.completed", stderr);
console.log("Installed Windows host, registry, protocol and provider discovery verified");
