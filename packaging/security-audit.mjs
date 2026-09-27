import assert from "node:assert/strict";
import fs from "node:fs";

const extension = JSON.parse(fs.readFileSync("extension/manifest.json", "utf8"));
assert.equal(extension.manifest_version, 3);
assert.deepEqual([...extension.permissions].sort(), [
  "activeTab", "contextMenus", "nativeMessaging", "storage"
]);
assert.equal("host_permissions" in extension, false);
assert.equal("optional_host_permissions" in extension, false);
assert.equal("externally_connectable" in extension, false);
assert.deepEqual(extension.content_scripts?.map(script => script.matches), [
  ["http://*/*", "https://*/*"]
]);

const windows = fs.readFileSync("packaging/windows/Pervue.iss", "utf8");
assert.match(windows, /DefaultDirName=\{localappdata\}\\Pervue/);
assert.match(windows, /NativeMessagingHosts\\\{#HostName\}/);
assert.match(windows, /Root: HKCU64;/);\nassert.match(windows, /Flags: uninsdeletekey/);
assert.doesNotMatch(windows, /\[Run\]|\[UninstallRun\]/i);
assert.doesNotMatch(windows, /\{param:/i);

const windowsBuild = fs.readFileSync("packaging/windows/build.ps1", "utf8");
assert.match(windowsBuild, /\[ValidatePattern\('\^\[a-p\]\{32\}\$'\)\]/);
assert.match(windowsBuild, /\$manifest\.path = 'pervue-host\.exe'/);
assert.doesNotMatch(windowsBuild, /OPENAI_API_KEY|CODEX_API_KEY|ANTHROPIC_API_KEY|CLAUDE_API_KEY/);

const mac = fs.readFileSync("packaging/macos/build.sh", "utf8");
assert.match(mac, /\^\[a-p\]\{32\}\$/);
assert.match(mac, /\/Library\/Application Support\/Pervue\/pervue-host/);
assert.match(mac, /\/Library\/Google\/Chrome\/NativeMessagingHosts\/com\.pervue\.host\.json/);

console.log("Packaged trust-boundary and extension-permission audit passed");
