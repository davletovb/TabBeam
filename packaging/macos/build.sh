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
    --target "$target" -p tabbeam-host
done
host="$stage/Library/Application Support/TabBeam/tabbeam-host"
manifest="$stage/Library/Google/Chrome/NativeMessagingHosts/com.tabbeam.host.json"
mkdir -p "$(dirname "$host")" "$(dirname "$manifest")" "$stage/Applications"
lipo -create "$root/native/target/aarch64-apple-darwin/release/tabbeam-host" \
  "$root/native/target/x86_64-apple-darwin/release/tabbeam-host" -output "$host"
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
assert manifest["path"].endswith("/Library/Application Support/TabBeam/tabbeam-host")
manifest["path"] = "/Library/Application Support/TabBeam/tabbeam-host"
json.dump(manifest, sys.stdout, indent=2)
print()
' > "$manifest"
chmod 644 "$manifest"

install -m 755 "$root/packaging/macos/uninstall.sh" "$stage/Library/Application Support/TabBeam/uninstall.sh"
osacompile -o "$stage/Applications/Uninstall TabBeam.app" \
  -e 'do shell script (quoted form of "/Library/Application Support/TabBeam/uninstall.sh") with administrator privileges'
# Give the applet a stable bundle identity before signing. Apple's pkgbuild
# analyzer omits bare osacompile applets, so supply its component explicitly.
applet="$stage/Applications/Uninstall TabBeam.app"
info="$applet/Contents/Info.plist"
plutil -replace CFBundleIdentifier -string com.tabbeam.uninstaller "$info"
plutil -replace CFBundleVersion -string "$package_version" "$info"
plutil -replace CFBundleShortVersionString -string "$package_version" "$info"
if [[ -n "${TABBEAM_APP_SIGN_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp --sign "$TABBEAM_APP_SIGN_IDENTITY" "$host"
  codesign --force --options runtime --timestamp --sign "$TABBEAM_APP_SIGN_IDENTITY" "$applet"
else
  # osacompile signs the applet ad hoc; editing Info.plist invalidates that seal.
  codesign --force --sign - "$applet"
fi
codesign --verify --strict "$applet"

python3 - "$stage/Library/Application Support/TabBeam/build-info.json" "$host_version" "$package_version" "$extension_id" "$source_commit" <<'PY'
import json, sys
with open(sys.argv[1], 'w') as target:
    json.dump(dict(version=sys.argv[2], package_version=sys.argv[3],
                   architecture='universal2', extension_id=sys.argv[4],
                   source_commit=sys.argv[5]), target, indent=2)
    target.write('\n')
PY

cat > "$components" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array><dict>
  <key>RootRelativeBundlePath</key><string>Applications/Uninstall TabBeam.app</string>
  <key>BundleIsRelocatable</key><false/>
  <key>BundleIsVersionChecked</key><false/>
  <key>BundleHasStrictIdentifier</key><true/>
  <key>BundleOverwriteAction</key><string>upgrade</string>
</dict></array>
</plist>
PLIST
plutil -lint "$components"
pkg="$output/TabBeam-${host_version}-${source_commit:0:12}-macos-universal.pkg"
if [[ -n "${TABBEAM_INSTALLER_SIGN_IDENTITY:-}" && -z "${TABBEAM_APP_SIGN_IDENTITY:-}" ]]; then
  echo 'Installer signing requires a signed host and uninstaller.' >&2
  exit 64
fi
if [[ -n "${TABBEAM_INSTALLER_SIGN_IDENTITY:-}" ]]; then
  pkgbuild --root "$stage" --identifier com.tabbeam.companion \
    --version "$package_version" --install-location / --ownership recommended \
    --component-plist "$components" \
    --sign "$TABBEAM_INSTALLER_SIGN_IDENTITY" "$pkg"
else
  pkgbuild --root "$stage" --identifier com.tabbeam.companion \
    --version "$package_version" --install-location / --ownership recommended \
    --component-plist "$components" "$pkg"
fi
echo "$pkg"
