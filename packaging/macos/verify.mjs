import assert from "node:assert/strict";
import fs from "node:fs";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { frameNativeMessage, parseNativeFrames } from "../../scripts/protocol-support.mjs";

const extensionId = process.argv[2];
assert.match(extensionId ?? "", /^[a-p]{32}$/);
const host = "/Library/Application Support/Pervue/pervue-host";
const manifest = JSON.parse(fs.readFileSync(
  "/Library/Google/Chrome/NativeMessagingHosts/com.pervue.host.json", "utf8"
));
const build = JSON.parse(fs.readFileSync(
  "/Library/Application Support/Pervue/build-info.json", "utf8"
));
assert.equal(manifest.name, "com.pervue.host");
assert.equal(manifest.path, host);
assert.deepEqual(manifest.allowed_origins, [`chrome-extension://${extensionId}/`]);
assert.equal(build.extension_id, extensionId);
assert.equal(build.architecture, "universal2");
assert.match(build.version, /^\d+\.\d+\.\d+/);
assert.match(build.source_commit, /^[0-9a-f]{40}$/);
assert.ok(fs.statSync(host).isFile());
assert.ok(fs.statSync("/Applications/Uninstall Pervue.app").isDirectory());

// Keep stdin open until the status probe finishes. Closing it earlier tells
// the host to cancel pending work, which hides a provider-present failure.
const child = spawn(host, [`chrome-extension://${extensionId}/`], {
  stdio: ["pipe", "pipe", "pipe"]
});
const exit = once(child, "close");
let stderr = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", chunk => { stderr += chunk; });
const timer = setTimeout(() => child.kill(), 25_000);
try {
  const request = Buffer.from(JSON.stringify({
    version: 1, type: "request", request_id: "package_status",
    method: "provider.status", payload: { provider_id: "codex" }
  }));
  child.stdin.write(frameNativeMessage(request));
  let output = Buffer.alloc(0);
  let events = [];
  for await (const chunk of child.stdout) {
    output = Buffer.concat([output, chunk]);
    assert.ok(output.length < 1024 * 1024, "host output exceeded verification limit");
    try {
      events = parseNativeFrames(output);
    } catch (error) {
      if (!/partial frame (prefix|payload)/.test(String(error))) throw error;
      continue;
    }
    if (events.some(event => event.request_id === "package_status" &&
      ["response.completed", "response.failed"].includes(event.event))) break;
  }
  child.stdin.end();
  const [code, signal] = await exit;
  assert.equal(signal, null, stderr);
  assert.equal(code, 0, stderr);
  assert.equal(events[0]?.event, "host.ready");
  assert.ok(events[0]?.payload?.protocol_versions?.includes(1));
  assert.ok(events.some(event => event.event === "provider.status" &&
    event.request_id === "package_status"));
  assert.equal(events.at(-1)?.event, "response.completed", stderr);
  console.log("Installed universal host, registration and provider status verified");
} finally {
  clearTimeout(timer);
  child.kill();
}
