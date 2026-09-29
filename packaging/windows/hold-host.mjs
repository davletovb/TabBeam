import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { parseNativeFrames } from "../../scripts/protocol-support.mjs";

const extensionId = process.argv[2];
const marker = process.argv[3];
if (!/^[a-p]{32}$/.test(extensionId ?? "") || !marker) {
  throw new Error("usage: hold-host.mjs <extension-id> <ready-marker>");
}

const appDir = path.join(process.env.LOCALAPPDATA ?? "", "Programs", "TabBeam");
const host = path.join(appDir, "tabbeam-host.exe");
const child = spawn(
  host,
  [`chrome-extension://${extensionId}/`, "--parent-window=0"],
  { stdio: ["pipe", "pipe", "pipe"], windowsHide: true }
);
child.stdin.on("error", () => {});

let stderr = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", chunk => { stderr += chunk; });

let output = Buffer.alloc(0);
let ready = false;
const readyTimeout = setTimeout(() => {
  if (!ready) {
    child.kill();
    process.stderr.write(`host did not become ready: ${stderr}\n`);
    process.exitCode = 1;
  }
}, 15_000);

child.stdout.on("data", chunk => {
  if (ready) return;
  output = Buffer.concat([output, chunk]);
  try {
    const events = parseNativeFrames(output);
    if (events.some(event => event.event === "host.ready")) {
      ready = true;
      clearTimeout(readyTimeout);
      fs.writeFileSync(marker, String(child.pid), "utf8");
    }
  } catch (error) {
    if (!/partial frame (prefix|payload)/.test(String(error))) {
      clearTimeout(readyTimeout);
      child.kill();
      throw error;
    }
  }
});

const [code, signal] = await once(child, "close");
clearTimeout(readyTimeout);
if (!ready) {
  throw new Error(`host exited before ready (code=${code}, signal=${signal}): ${stderr}`);
}
// The installer/uninstaller is expected to close this host. Its exact exit
// code is platform-controlled, so the holder's proof is that it was running
// before the operation and is gone afterward.
