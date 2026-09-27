#!/bin/bash
# Run on macOS: ./packaging/macos/build.sh <extension-id> [output-directory]
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
extension_id="${1:?pass the ID of the extension paired with this installer}"
if [[ ! "$extension_id" =~ ^[a-p]{32}$ ]]; then
  echo 'Expected a Chrome extension ID: exactly 32 lowercase letters a-p.' >&2
  exit 64
fi
output="${2:-$root/out/macos}"
output="$(mkdir -p "$output" && cd "$output" && pwd)"
stage="$(mktemp -d)"
components="$(mktemp)"
trap 'rm -rf "$stage"; rm -f "$components"' EXIT

# Ship one universal host so Installer cannot put the wrong architecture on a Mac.
rustup target add aarch64-apple-darwin x86_64-apple-darwin
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --manifest-path "$root/native/Cargo.toml" --release --locked \
    --target "$target" -p pervue-host
done
host="$stage/Library/Application Support/Pervue/pervue-host"
manifest="$stage/Library/Google/Chrome/NativeMessagingHosts/com.pervue.host.json"
mkdir -p "$(dirname "$host")" "$(dirname "$manifest")" "$stage/Applications/Pervue"
lipo -create "$root/native/target/aarch64-apple-darwin/release/pervue-host" \
  "$root/native/target/x86_64-apple-darwin/release/pervue-host" -output "$host"
chmod 755 "$host"
host_version="$("$host" --version)"
package_version="${host_version%%-*}"
if [[ ! "$package_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Unsupported package version: $host_version" >&2
  exit 64
fi
source_commit="$(git -C "$root" rev-parse HEAD)"

# The host validates the Chrome ID and prints the exact allowlist. Only its
# build-time staging path is replaced with its final, system-wide path.
"$host" --print-manifest "$extension_id" | python3 -c '
import json, sys
manifest = json.load(sys.stdin)
assert manifest["path"].endswith("/Library/Application Support/Pervue/pervue-host")
manifest["path"] = "/Library/Application Support/Pervue/pervue-host"
json.dump(manifest, sys.stdout, indent=2)
print()
' > "$manifest"
chmod 644 "$manifest"

install -m 755 "$root/packaging/macos/uninstall.sh" "$stage/Library/Application Support/Pervue/uninstall.sh"
osacompile -o "$stage/Applications/Pervue/Uninstall Pervue.app" \
  -e 'do shell script (quoted form of "/Library/Application Support/Pervue/uninstall.sh") with administrator privileges'
if [[ -n "${PERVUE_APP_SIGN_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp --sign "$PERVUE_APP_SIGN_IDENTITY" "$host"
  codesign --force --options runtime --timestamp --sign "$PERVUE_APP_SIGN_IDENTITY" \
    "$stage/Applications/Pervue/Uninstall Pervue.app"
fi

python3 - "$stage/Library/Application Support/Pervue/build-info.json" "$host_version" "$package_version" "$extension_id" "$source_commit" <<'PY'
import json, sys
with open(sys.argv[1], 'w') as target:
    json.dump(dict(version=sys.argv[2], package_version=sys.argv[3],
                   architecture='universal2', extension_id=sys.argv[4],
                   source_commit=sys.argv[5]), target, indent=2)
    target.write('\n')
PY

pkgbuild --analyze --root "$stage" "$components"
/usr/libexec/PlistBuddy -c 'Print :0:RootRelativeBundlePath' "$components" | \
  grep -F 'Applications/Pervue/Uninstall Pervue.app' >/dev/null
/usr/libexec/PlistBuddy -c 'Set :0:BundleIsRelocatable false' "$components"
pkg="$output/Pervue-${host_version}-${source_commit:0:12}-macos-universal.pkg"
if [[ -n "${PERVUE_INSTALLER_SIGN_IDENTITY:-}" && -z "${PERVUE_APP_SIGN_IDENTITY:-}" ]]; then
  echo 'Installer signing requires a signed host and uninstaller.' >&2
  exit 64
fi
if [[ -n "${PERVUE_INSTALLER_SIGN_IDENTITY:-}" ]]; then
  pkgbuild --root "$stage" --identifier com.pervue.companion \
    --version "$package_version" --install-location / --ownership recommended \
    --component-plist "$components" \
    --sign "$PERVUE_INSTALLER_SIGN_IDENTITY" "$pkg"
else
  pkgbuild --root "$stage" --identifier com.pervue.companion \
    --version "$package_version" --install-location / --ownership recommended \
    --component-plist "$components" "$pkg"
fi
echo "$pkg"
