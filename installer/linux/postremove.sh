#!/bin/sh
# Settings in ~/.config/glidedesk are kept (like any Linux package).
# Remove them yourself if you want: rm -rf ~/.config/glidedesk ~/.local/share/glidedesk
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
fi
exit 0
