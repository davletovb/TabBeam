# Windows companion release and verification

The Windows package is a per-user installer. It installs the release host, Native Messaging manifest, build provenance, and uninstaller under `%LOCALAPPDATA%\Pervue`. It registers `com.pervue.host` in the 64-bit `HKCU\Software\Google\Chrome\NativeMessagingHosts` registry view. No provider credential, provider profile, or browser storage is copied into the package or removed by uninstall.

The manifest uses `pervue-host.exe` as a path relative to its own directory. Chrome permits relative native-host paths on Windows, while the registry value itself points to the manifest by its full installed path. This keeps the package portable across Windows user-profile locations without generating a manifest from runtime input.

## Build a paired installer

Use the ID of the exact Chrome extension build that the package will accompany:

```powershell
packaging\windows\build.ps1 -ExtensionId <32-character-extension-id>
```

The build requires Rust and Inno Setup 6. It produces `out\windows\Pervue-<host-version>-<commit-prefix>-windows-x64.exe`. The build asks the host to validate the extension ID and produce the allowlist, then changes only the Windows-legal host path to the fixed relative filename `pervue-host.exe`. `build-info.json` contains only version, package version, architecture, extension ID, and source commit.

The installer does not accept a provider path, executable path, Native Messaging registry path, or extension origin from the installed browser or a web page. Its install directory, host filename, manifest filename, and registry key are compile-time constants. Provider discovery remains the host's platform-controlled lookup: `PATH`, then `%APPDATA%\npm`, or the local testing override `PERVUE_PROVIDER_PATH`.

## Automated package gate

The Windows CI package job:

1. builds the release host and installer;
2. installs the real Inno Setup artifact silently as the runner user;
3. checks the HKCU Native Messaging registration and installed manifest;
4. launches the installed host with Chrome's Windows arguments, including `--parent-window=0`;
5. checks `host.ready` and a `provider.status` round trip while a fake npm-style `codex.cmd` is discoverable;
6. runs the real uninstaller; and
7. verifies that the files and registry key are removed.

This proves PKG-05/PKG-06 packaging mechanics and catches Windows framing, path, registration, and npm-shim discovery regressions. It is not the human clean-machine gate.

## TST-13 clean-machine gate

Record one actual supported Windows run with the matching released Chrome extension and a signed-in supported provider. During the user journey, do not use PowerShell, Command Prompt, Registry Editor, or a developer checkout.

1. Install the released extension through its normal Chrome distribution channel and record its extension ID.
2. Download and open the matching Pervue Windows installer normally. Complete the installer UI.
3. Open Pervue setup. Verify the companion is detected. Verify useful missing/signed-out provider states if available, then use the provider's own installation/authentication flow.
4. Ask a question and see text stream, ask a follow-up, and continue the same conversation in the full-page view.
5. Quit Chrome completely and reopen it. Verify Pervue reconnects and a new question works without repair steps.
6. Open **Settings → Apps → Installed apps → Pervue Companion → Uninstall**. Complete uninstall normally.
7. Reopen Pervue. Verify it reports the companion missing. Remove the extension in Chrome and verify the provider remains signed in.

Record Windows edition/version, architecture, Chrome version, installer SHA-256, extension ID, provider/tool version, source commit, date, screenshots or screen recording, and pass/fail for every step in the release issue.

Keep TST-13 open until this human evidence exists. CI cannot prove Chrome Web Store installation, real provider authentication, browser restart behavior, or the no-terminal journey.
