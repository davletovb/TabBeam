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
trap 'rm -rf "$stage"' EXIT

version="$(python3 - "$root/native/Cargo.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], 'rb') as source:
    print(tomllib.load(source)['workspace']['package']['version'].split('-')[0])
PY
)"

cargo build --manifest-path "$root/native/Cargo.toml" --release --locked -p pervue-host
host="$stage/Library/Application Support/Pervue/pervue-host"
manifest="$stage/Library/Google/Chrome/NativeMessagingHosts/com.pervue.host.json"
mkdir -p "$(dirname "$host")" "$(dirname "$manifest")" "$stage/Applications/Pervue"
install -m 755 "$root/native/target/release/pervue-host" "$host"

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
  codesign --force --timestamp --sign "$PERVUE_APP_SIGN_IDENTITY" \
    "$stage/Applications/Pervue/Uninstall Pervue.app"
fi

python3 - "$stage/Library/Application Support/Pervue/build-info.json" "$version" "$extension_id" "$(git -C "$root" rev-parse HEAD)" <<'PY'
import json, sys
with open(sys.argv[1], 'w') as target:
    json.dump(dict(version=sys.argv[2], extension_id=sys.argv[3],
                   source_commit=sys.argv[4]), target, indent=2)
    target.write('\n')
PY

arch="$(uname -m)"
pkg="$output/Pervue-${version}-macos-${arch}.pkg"
if [[ -n "${PERVUE_INSTALLER_SIGN_IDENTITY:-}" ]]; then
  pkgbuild --root "$stage" --identifier com.pervue.companion \
    --version "$version" --install-location / --ownership recommended \
    --sign "$PERVUE_INSTALLER_SIGN_IDENTITY" "$pkg"
else
  pkgbuild --root "$stage" --identifier com.pervue.companion \
    --version "$version" --install-location / --ownership recommended "$pkg"
fi
echo "$pkg"
