# Windows companion release and verification

The Windows package is a per-user installer. It installs the release host, Native Messaging manifest, build provenance, and uninstaller under `%LOCALAPPDATA%\Programs\Pervue`, deliberately separate from Pervue runtime state under `%LOCALAPPDATA%\Pervue`. It registers `com.pervue.host` in the 64-bit `HKCU\Software\Google\Chrome\NativeMessagingHosts` registry view. Provider credentials, provider profiles, conversation/session mappings, and browser storage are neither copied into the package nor removed by uninstall.

The manifest uses `pervue-host.exe` as a path relative to its own directory. Chrome permits relative native-host paths on Windows, while the registry value itself points to the manifest by its full installed path. This keeps the package portable across Windows user-profile locations without generating a manifest from runtime input.

## Build a paired installer

Build from PowerShell 7 with Rust/rustup and Inno Setup 6.3 or newer. CI pins Inno Setup 6.7.3. Use the ID of the exact Chrome extension build that the package will accompany:

```powershell
pwsh -File packaging\windows\build.ps1 -ExtensionId <32-character-extension-id>
```

The script installs the Rust `x86_64-pc-windows-msvc` target when needed and always builds that target explicitly, so an ARM64 build machine cannot silently produce an ARM64 binary labeled as x64. The output is `out\windows\Pervue-<host-version>-<commit-prefix>-windows-x64.exe`.

The build asks the exact release host it just compiled to validate the extension ID and produce the Native Messaging allowlist. It verifies the generated host identity, type, path, and exact origin before changing only the Windows-legal host path to the fixed relative filename `pervue-host.exe`. `build-info.json` contains only version, package version, architecture, extension ID, and source commit.

The installer has a fixed per-user destination. Even though Inno Setup supports `/DIR` and `/LOADINF`, Pervue rejects a resulting install directory other than `%LOCALAPPDATA%\Programs\Pervue` before files are written. Provider discovery remains host-controlled: `PATH`, then `%APPDATA%\npm`; `PERVUE_PROVIDER_PATH` exists only as a local testing/advanced override and is not an installer input.

If local PowerShell policy prevents scripts from running, use a trusted checkout and your normal organization/system PowerShell policy rather than bypassing policy for an untrusted downloaded script.

## Signing and release candidates

Unsigned installers are only for CI/internal mechanics tests. A TST-13 release candidate must be Authenticode signed so Windows shows a trusted publisher instead of an unknown-publisher/SmartScreen flow.

`build.ps1` supports signing through `PERVUE_WINDOWS_SIGN_COMMAND`. The value is an Inno Setup SignTool command and must contain Inno's literal `$f` filename placeholder. When set, the installer, generated uninstaller, and staged native host are signed.

The manual `.github/workflows/windows-release.yml` workflow imports a code-signing certificate from repository secrets, supplies the signing command, verifies the final Authenticode signature, and uploads the installer artifact. Configure:

- `WINDOWS_CODE_SIGN_PFX`: base64-encoded PKCS#12/PFX containing the code-signing identity;
- `WINDOWS_CODE_SIGN_PFX_PASSWORD`: password for that PFX.

The signing certificate remains in the runner's CurrentUser certificate store for the job only; the temporary PFX file is deleted immediately after import. No provider credential is part of the release artifact.

## Automated package gate

The Windows CI package job:

1. rejects a non-canonical uppercase Chrome extension ID before a build starts;
2. builds the explicit x64 release host and real Inno Setup installer;
3. proves a `/DIR` install-location override is rejected before the host is written;
4. installs the artifact as the runner user;
5. checks the HKCU Native Messaging registration and installed manifest;
6. places a fake npm-style `codex.cmd` in the real `%APPDATA%\npm` location, with no `PERVUE_PROVIDER_PATH` override;
7. launches the installed host with Chrome's Windows arguments, checks `host.ready`, and requires `provider.status.status.availability == "available"`;
8. holds the installed native host open, reinstalls over it, and verifies Restart Manager closes the old host and the new installation still works;
9. uninstalls the companion and verifies its install directory, Native Messaging key, and Settings/Installed Apps entry are removed; and
10. verifies a sentinel under `%LOCALAPPDATA%\Pervue` survives uninstall, proving runtime state is not recursively deleted.

This proves PKG-05/PKG-06 packaging mechanics and catches Windows framing, path, registration, default npm discovery, upgrade, and removal regressions. It is not the human clean-machine gate.

## TST-13 clean-machine gate

Record one actual supported Windows run with the matching released Chrome extension, a **signed** Pervue Windows release candidate, and a signed-in supported provider. During the user journey, do not use PowerShell, Command Prompt, Registry Editor, or a developer checkout.

1. Install the released extension through its normal Chrome distribution channel and record its extension ID.
2. Download and open the matching signed Pervue Windows installer normally. Verify Windows identifies the expected publisher; do not treat an unknown-publisher build as a release candidate.
3. Open Pervue setup. Verify the companion is detected. Verify useful missing/signed-out provider states if available, then use the provider's own installation/authentication flow.
4. Ask a question and see text stream, ask a follow-up, and continue the same conversation in the full-page view.
5. Quit Chrome completely and reopen it. Verify Pervue reconnects and a new question works without repair steps.
6. Quit Chrome, then open **Settings → Apps → Installed apps → Pervue Companion → Uninstall**. Complete uninstall normally.
7. Reopen Chrome/Pervue. Verify it reports the companion missing. Remove the extension in Chrome and verify the provider remains signed in and prior provider/Pervue runtime state was not removed by the companion uninstaller.

Record Windows edition/version, architecture, Chrome version, installer SHA-256, Authenticode signer/status, extension ID, provider/tool version, source commit, date, screenshots or screen recording, and pass/fail for every step in the release issue.

Keep TST-13 open until this human evidence exists. CI cannot prove Chrome Web Store installation, real provider authentication, browser restart behavior, publisher trust UI, or the no-terminal journey.
