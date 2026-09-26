#!/bin/sh
# Installs Glidedesk from the .tar.gz (use the .deb / .rpm when you can).
# Usage: sudo ./install.sh            install
#        sudo ./install.sh --remove   uninstall (your settings are kept)
set -eu
cd "$(dirname "$0")"
if [ "$(id -u)" -ne 0 ]; then echo "Please run with sudo." >&2; exit 1; fi
if [ "${1:-}" = "--remove" ]; then
  rm -f /usr/local/bin/glidedesk /usr/share/applications/glidedesk.desktop \
        /usr/lib/udev/rules.d/70-glidedesk-uinput.rules /usr/lib/modules-load.d/glidedesk-uinput.conf
  for s in 32 128 256 512; do rm -f "/usr/share/icons/hicolor/${s}x${s}/apps/glidedesk.png"; done
  echo "Glidedesk removed. Settings kept in ~/.config/glidedesk"
  exit 0
fi
install -Dm755 glidedesk /usr/local/bin/glidedesk
install -Dm644 glidedesk.desktop /usr/share/applications/glidedesk.desktop
install -Dm644 70-glidedesk-uinput.rules /usr/lib/udev/rules.d/70-glidedesk-uinput.rules
install -Dm644 glidedesk-uinput.conf /usr/lib/modules-load.d/glidedesk-uinput.conf
for s in 32 128 256 512; do install -Dm644 "icons/$s.png" "/usr/share/icons/hicolor/${s}x${s}/apps/glidedesk.png"; done
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --name-match=uinput 2>/dev/null || true
echo "Glidedesk installed. Start it from your applications menu (it will start at login from then on)."
