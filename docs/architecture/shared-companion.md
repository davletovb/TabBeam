# Shared Seatline integration

Seatline is installed once for the user's account. The default TabBeam native
build routes real provider execution through that shared companion. TabBeam's
conversation policy, protocol and product state remain in this repository.
Conclave and future neutral-protocol clients use the same broker without loading
their app engines into Seatline.

For this development integration, first build/install `seatline-companion`
following Seatline's companion README. Build TabBeam's own native integration:

```sh
cargo build --release --locked --manifest-path native/Cargo.toml -p tabbeam-host
```

Keep that binary at a permanent location, then authorize TabBeam's exact Chrome
extension ID and register the compatibility adapter:

```sh
seatline-companion install
seatline-companion authorize tabbeam codex,claude,gemini,grok chrome-extension://EXTENSION_ID/ --cache-title=TabBeam --allow-provider-default
seatline-companion register-native tabbeam /absolute/path/to/tabbeam-host
```

`--allow-provider-default` is required for TabBeam's plain questions. A question
with no page context asks the provider to apply its own tool configuration
(`ToolPolicy::ProviderDefault`), which Seatline refuses unless the grant allows
it; without the option those questions fail with `PROVIDER_DEFAULT_TOOLS_DENIED`
(the extension shows what to do). Questions that carry page context, and web
search, never need it.

Use the absolute path to `tabbeam-host.exe` on Windows. The setup page fills in
the current extension ID. Chrome connects to `com.seatline.host`; Seatline selects
the approved TabBeam integration by the exact caller origin. Its own provider
requests use authenticated IPC with the shared broker. There is no local HTTP
listener. App adapters are an optional compatibility path for existing native
protocols; neutral-protocol extensions need no extra executable.

Do not register TabBeam's `--print-manifest` output over Seatline's shared Chrome
registration. Let Seatline's `install` and `authorize` commands manage that host
and the full set of approved extension origins.

`seatline-companion revoke tabbeam` removes TabBeam's authorization without
uninstalling Seatline or affecting other apps. Reauthorizing rotates credentials
and resets adapter registration; repeat `register-native` afterward. Provider
CLIs and their sign-ins are still managed separately.

## Two builds that do not interoperate yet

This branch produces two different products, and neither works with the other:

| | Extension connects to | Native side |
| --- | --- | --- |
| Shared-host build (default) | `com.seatline.host` | Seatline companion, plus the registered `tabbeam-host` adapter |
| Standalone packages (macOS pkg, Windows installer) | `com.tabbeam.host` | The self-contained `tabbeam-host`, built with `--no-default-features` |

An extension built from this branch does not talk to a companion installed from
a TabBeam release, and the released installers do not talk to this extension.
The shared-host build is a development installation until a signed installer
that registers `com.seatline.host` exists, and it should not be offered as the
default distribution path before then. The contract records both names
(`host_name`, `legacy_host_name` in `docs/protocol/native-messaging-v1.json`),
and CI tests both configurations.

The existing macOS/Windows packaging scripts explicitly build the legacy
standalone host with `--no-default-features`. Those packages serve extensions
using `com.tabbeam.host`; they are not installers for this shared-host extension
build. Signed shared-integration packaging and distribution remain release work.
Provider warming is unchanged: adapters continue to launch one process per turn.
