import assert from "node:assert/strict";
import fs from "node:fs";

function read(relative) {
  return fs.readFileSync(new URL(relative, import.meta.url), "utf8");
}

const windows = read("./windows/TabBeam.iss");
assert.match(windows, /DefaultDirName=\{userpf\}\\TabBeam/);
assert.match(windows, /CloseApplications=yes/);
assert.match(windows, /CloseApplicationsFilter=tabbeam-host\.exe/);
assert.match(windows, /NativeMessagingHosts\\\{#HostName\}/);
assert.match(windows, /Root: HKCU64;/);
assert.match(windows, /Flags: uninsdeletekey/);
assert.doesNotMatch(windows, /Type:\s*filesandordirs/i);
assert.doesNotMatch(windows, /\[Run\]|\[UninstallRun\]/i);
assert.doesNotMatch(windows, /\{param:/i);
assert.match(windows, /CompareText\(WizardDirValue, ExpectedDir\)/);
assert.match(windows, /#ifdef TabBeamSignTool[\s\S]*SignTool=tabbeam[\s\S]*SignedUninstaller=yes/);

// The installer's AppId names its uninstall key, and CI checks that key by
// name. A typo in either would leave that check looking at the wrong key.
const appId = /^AppId=\{\{([0-9A-F]{8}(?:-[0-9A-F]{4}){3}-[0-9A-F]{12})\}$/im.exec(windows)?.[1];
assert.ok(appId, "TabBeam.iss must declare a well-formed AppId GUID");
// Pervue's development installer used this AppId. Reusing it would make a
// locally installed Pervue build look like a TabBeam upgrade.
assert.notEqual(appId.toUpperCase(), "D7A1D4E8-774F-4E2B-A745-04B43E51A93C");
assert.ok(
  fs.existsSync(new URL("../.github/workflows/ci.yml", import.meta.url)),
  "this audit reads .github/workflows/ci.yml, so run it from a full checkout"
);
const ci = read("../.github/workflows/ci.yml");
// Only the assignment the Windows job runs counts, not a comment or a mention.
const uninstallKeys = [
  ...ci.matchAll(/^[ \t]*\$uninstallKey\s*=\s*'[^']*\\Uninstall\\\{([0-9A-F-]{36})\}_is1'/gim)
].map((match) => match[1]);
assert.ok(uninstallKeys.length > 0, "CI must check the installer's uninstall key");
for (const guid of uninstallKeys) {
  assert.equal(guid.toUpperCase(), appId.toUpperCase(), "CI's uninstall key must match TabBeam.iss's AppId");
}

const windowsBuild = read("./windows/build.ps1");
assert.match(windowsBuild, /ValidatePattern\('\^\[a-p\]\{32\}\$', Options = 'None'\)/);
assert.match(windowsBuild, /--target\s+\$target/);
assert.match(windowsBuild, /x86_64-pc-windows-msvc/);
assert.match(windowsBuild, /Unexpected Native Messaging manifest/);
assert.match(windowsBuild, /TABBEAM_WINDOWS_SIGN_COMMAND/);
assert.doesNotMatch(windowsBuild, /OPENAI_API_KEY|CODEX_API_KEY|ANTHROPIC_API_KEY|CLAUDE_API_KEY/);

const mac = read("./macos/build.sh");
assert.match(mac, /\^\[a-p\]\{32\}\$/);
assert.match(mac, /\/Library\/Application Support\/TabBeam\/tabbeam-host/);
assert.match(mac, /\/Library\/Google\/Chrome\/NativeMessagingHosts\/com\.tabbeam\.host\.json/);

console.log("Packaged trust-boundary audit passed");
