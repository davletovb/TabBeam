import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { frameNativeMessage, parseNativeFrames } from "../../scripts/protocol-support.mjs";

const extensionId = process.argv[2];
assert.match(extensionId ?? "", /^[a-p]{32}$/);

const appDir = path.join(process.env.LOCALAPPDATA ?? "", "Pervue");
assert.ok(path.isAbsolute(appDir), "LOCALAPPDATA must be an absolute path");
const manifestPath = path.join(appDir, "com.pervue.host.json");
const host = path.join(appDir, "pervue-host.exe");
const buildPath = path.join(appDir, "build-info.json");
const registryKey = String.raw`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.pervue.host`;

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
assert.equal(manifest.name, "com.pervue.host");
assert.equal(manifest.path, "pervue-host.exe");
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

const child = spawn(
  host,
  [`chrome-extension://${extensionId}/`, "--parent-window=0"],
  { stdio: ["pipe", "pipe", "pipe"], windowsHide: true }
);
const exit = once(child, "close");
let stderr = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", chunk => { stderr += chunk; });
const timer = setTimeout(() => child.kill(), 25_000);

try {
  const request = Buffer.from(JSON.stringify({
    version: 1,
    type: "request",
    request_id: "package_status",
    method: "provider.status",
    payload: { provider_id: "codex" }
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
    if (events.some(event =>
      event.request_id === "package_status" &&
      ["response.completed", "response.failed"].includes(event.event)
    )) break;
  }

  child.stdin.end();
  const [code, signal] = await exit;
  assert.equal(signal, null, stderr);
  assert.equal(code, 0, stderr);
  assert.equal(events[0]?.event, "host.ready");
  assert.ok(events[0]?.payload?.protocol_versions?.includes(1));
  const status = events.find(event =>
    event.event === "provider.status" && event.request_id === "package_status"
  );
  assert.ok(status, "provider.status was not emitted");
  assert.notEqual(status.payload?.availability, "not_found", stderr);
  assert.equal(events.at(-1)?.event, "response.completed", stderr);
  console.log("Installed Windows host, registry, protocol and provider discovery verified");
} finally {
  clearTimeout(timer);
  child.kill();
}
