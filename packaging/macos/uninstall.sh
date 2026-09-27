#!/bin/sh
# Installed root-owned script; launched by the GUI uninstaller with admin auth.
set -eu
rm -f '/Library/Google/Chrome/NativeMessagingHosts/com.pervue.host.json'
rm -f '/Library/Application Support/Pervue/pervue-host' \
  '/Library/Application Support/Pervue/uninstall.sh' \
  '/Library/Application Support/Pervue/build-info.json'
rm -rf '/Applications/Uninstall Pervue.app'
rmdir '/Library/Google/Chrome/NativeMessagingHosts' \
  '/Library/Google/Chrome' '/Library/Google' \
  '/Library/Application Support/Pervue' 2>/dev/null || true
pkgutil --forget com.pervue.companion >/dev/null 2>&1 || true
