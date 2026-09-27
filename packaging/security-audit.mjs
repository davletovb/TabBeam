import assert from "node:assert/strict";
import fs from "node:fs";

function read(relative) {
  return fs.readFileSync(new URL(relative, import.meta.url), "utf8");
}

const windows = read("./windows/Pervue.iss");
assert.match(windows, /DefaultDirName=\{userpf\}\\Pervue/);
assert.match(windows, /CloseApplications=yes/);
assert.match(windows, /CloseApplicationsFilter=pervue-host\.exe/);
assert.match(windows, /NativeMessagingHosts\\\{#HostName\}/);
assert.match(windows, /Root: HKCU64;/);
assert.match(windows, /Flags: uninsdeletekey/);
assert.doesNotMatch(windows, /Type:\s*filesandordirs/i);
assert.doesNotMatch(windows, /\[Run\]|\[UninstallRun\]/i);
assert.doesNotMatch(windows, /\{param:/i);
assert.match(windows, /CompareText\(WizardDirValue, ExpectedDir\)/);
assert.match(windows, /#ifdef PervueSignTool[\s\S]*SignTool=pervue[\s\S]*SignedUninstaller=yes/);

const windowsBuild = read("./windows/build.ps1");
assert.match(windowsBuild, /ValidatePattern\('\^\[a-p\]\{32\}\$', Options = 'None'\)/);
assert.match(windowsBuild, /--target\s+\$target/);
assert.match(windowsBuild, /x86_64-pc-windows-msvc/);
assert.match(windowsBuild, /Unexpected Native Messaging manifest/);
assert.match(windowsBuild, /PERVUE_WINDOWS_SIGN_COMMAND/);
assert.doesNotMatch(windowsBuild, /OPENAI_API_KEY|CODEX_API_KEY|ANTHROPIC_API_KEY|CLAUDE_API_KEY/);

const mac = read("./macos/build.sh");
assert.match(mac, /\^\[a-p\]\{32\}\$/);
assert.match(mac, /\/Library\/Application Support\/Pervue\/pervue-host/);
assert.match(mac, /\/Library\/Google\/Chrome\/NativeMessagingHosts\/com\.pervue\.host\.json/);

console.log("Packaged trust-boundary audit passed");
