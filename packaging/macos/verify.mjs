import assert from "node:assert/strict";
import fs from "node:fs";
import { probeInstalledHost } from "../../scripts/installed-host-probe.mjs";

const extensionId = process.argv[2];
assert.match(extensionId ?? "", /^[a-p]{32}$/);
const host = "/Library/Application Support/TabBeam/tabbeam-host";
const manifest = JSON.parse(fs.readFileSync(
  "/Library/Google/Chrome/NativeMessagingHosts/com.tabbeam.host.json", "utf8"
));
const build = JSON.parse(fs.readFileSync(
  "/Library/Application Support/TabBeam/build-info.json", "utf8"
));
assert.equal(manifest.name, "com.tabbeam.host");
assert.equal(manifest.path, host);
assert.deepEqual(manifest.allowed_origins, [`chrome-extension://${extensionId}/`]);
assert.equal(build.extension_id, extensionId);
assert.equal(build.architecture, "universal2");
assert.match(build.version, /^\d+\.\d+\.\d+/);
assert.match(build.source_commit, /^[0-9a-f]{40}$/);
assert.ok(fs.statSync(host).isFile());
assert.ok(fs.statSync("/Applications/Uninstall TabBeam.app").isDirectory());

const { code, signal, stderr, events } = await probeInstalledHost(
  host,
  [`chrome-extension://${extensionId}/`]
);
assert.equal(signal, null, stderr);
assert.equal(code, 0, stderr);
assert.equal(events[0]?.event, "host.ready");
assert.ok(events[0]?.payload?.protocol_versions?.includes(1));
assert.ok(events.some(event =>
  event.event === "provider.status" && event.request_id === "package_status"
));
assert.equal(events.at(-1)?.event, "response.completed", stderr);
console.log("Installed universal host, registration and provider status verified");
