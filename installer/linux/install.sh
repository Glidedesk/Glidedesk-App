#!/bin/sh
# Installs Nexpingdesk from the .tar.gz (use the .deb / .rpm when you can).
# Usage: sudo ./install.sh            install
#        sudo ./install.sh --remove   uninstall (your settings are kept)
set -eu
cd "$(dirname "$0")"
if [ "$(id -u)" -ne 0 ]; then echo "Please run with sudo." >&2; exit 1; fi
if [ "${1:-}" = "--remove" ]; then
  rm -f /usr/local/bin/nexpingdesk /usr/share/applications/nexpingdesk.desktop \
        /usr/lib/udev/rules.d/70-nexpingdesk-uinput.rules /usr/lib/modules-load.d/nexpingdesk-uinput.conf
  for s in 32 128 256 512; do rm -f "/usr/share/icons/hicolor/${s}x${s}/apps/nexpingdesk.png"; done
  echo "Nexpingdesk removed. Settings kept in ~/.config/nexpingdesk"
  exit 0
fi
install -Dm755 nexpingdesk /usr/local/bin/nexpingdesk
install -Dm644 nexpingdesk.desktop /usr/share/applications/nexpingdesk.desktop
install -Dm644 70-nexpingdesk-uinput.rules /usr/lib/udev/rules.d/70-nexpingdesk-uinput.rules
install -Dm644 nexpingdesk-uinput.conf /usr/lib/modules-load.d/nexpingdesk-uinput.conf
for s in 32 128 256 512; do install -Dm644 "icons/$s.png" "/usr/share/icons/hicolor/${s}x${s}/apps/nexpingdesk.png"; done
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --name-match=uinput 2>/dev/null || true
echo "Nexpingdesk installed. Start it from your applications menu (it will start at login from then on)."
