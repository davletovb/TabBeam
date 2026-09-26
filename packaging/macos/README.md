# macOS companion release and verification

The package installs the release host at `/Library/Application Support/Pervue/pervue-host` and its Chrome system Native Messaging manifest at `/Library/Google/Chrome/NativeMessagingHosts/com.pervue.host.json`. It also installs a Finder uninstaller at `/Applications/Pervue/Uninstall Pervue.app`. No user profile, provider credentials, or browser storage are touched by uninstall.

## Build a paired package

The Chrome extension ID is an input to the package, not a guessed value. Use the ID of the exact published extension build. Chrome only connects a native host when its manifest allowlist contains that ID. If an unpacked development extension has another ID, build a separate development package.

On a Mac with Rust and Xcode command line tools:

```sh
packaging/macos/build.sh <32-character-extension-id>
```

The output is `out/macos/Pervue-<version>-macos-<architecture>.pkg`. The package embeds its version, extension ID, and source commit in `build-info.json`. It contains no provider session or API credential. Builds are per architecture; make and verify both `arm64` and `x86_64` if both are distributed. The unsigned package is for CI and internal testing; release packages need Developer ID signatures and notarization.

The `macos-release.yml` manual workflow builds a signed, notarized, stapled package for an explicit extension ID. Configure these repository secrets: `MACOS_DEVELOPER_ID_APP_P12` and `MACOS_DEVELOPER_ID_INSTALLER_P12` (base64 PKCS#12), `MACOS_CERT_PASSWORD`, `MACOS_DEVELOPER_ID_APP_IDENTITY`, `MACOS_DEVELOPER_ID_INSTALLER_IDENTITY`, and `MACOS_NOTARY_KEY_P8`, `MACOS_NOTARY_KEY_ID`, `MACOS_NOTARY_ISSUER` (App Store Connect API key). The workflow publishes an artifact for review; publishing a release remains a separate action. Signing identities and notary keys are read only from CI secrets and removed from temporary files after use.

## macOS package gate

CI installs an unsigned test package on a macOS runner, verifies the manifest's exact Chrome origin and absolute host path, launches the installed host using the Chrome origin, checks `host.ready` and a `provider.status` round trip, runs the uninstaller script, and checks removal of its files. This exercises the actual `pkgbuild` payload and system registration. The extension suite separately checks the protocol gate before any queued request reaches an incompatible host.

For **TST-12**, additionally record an actual clean Mac run with a signed/notarized release candidate, matching Chrome extension, and a signed-in supported provider:

1. In Chrome, install the released extension through its user-facing distribution channel. Check its ID matches `build-info.json` in the companion package.
2. Open the matching `.pkg` in Finder and install it through macOS Installer. Do not use Terminal during this journey.
3. Open Pervue setup; verify host detection and that missing Codex/Claude tools and signed-out providers show useful states. Follow the provider's own installation and authentication flow.
4. Ask a question; see text stream, ask a follow-up, and open that conversation in the full-page view.
5. Quit and reopen Chrome; verify the extension reconnects and a new question works.
6. Run `Applications → Pervue → Uninstall Pervue`, authorize the macOS prompt, and verify Pervue reports the companion missing. Remove the extension in Chrome. Verify provider credentials are unaffected.

Record macOS version/architecture, Chrome version, package artifact checksum, extension ID, provider/tool version, date, and pass/fail evidence in the release issue. CI cannot establish the browser UI, provider authentication, or a no-terminal human journey; keep TST-12 open until that evidence exists.
