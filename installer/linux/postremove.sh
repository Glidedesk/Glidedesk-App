#!/bin/sh
# Settings in ~/.config/nexpingdesk are kept (like any Linux package).
# Remove them yourself if you want: rm -rf ~/.config/nexpingdesk ~/.local/share/nexpingdesk
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
fi
exit 0
