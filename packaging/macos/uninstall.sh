#!/bin/sh
# Installed root-owned script; launched by the GUI uninstaller with admin auth.
set -eu
rm -f '/Library/Google/Chrome/NativeMessagingHosts/com.tabbeam.host.json'
rm -f '/Library/Application Support/TabBeam/tabbeam-host' \
  '/Library/Application Support/TabBeam/uninstall.sh' \
  '/Library/Application Support/TabBeam/build-info.json'
rm -rf '/Applications/Uninstall TabBeam.app'
rmdir '/Library/Google/Chrome/NativeMessagingHosts' \
  '/Library/Google/Chrome' '/Library/Google' \
  '/Library/Application Support/TabBeam' 2>/dev/null || true
pkgutil --forget com.tabbeam.companion >/dev/null 2>&1 || true
