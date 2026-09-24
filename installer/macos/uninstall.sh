#!/bin/sh
# Glidedesk uninstaller for macOS (PLAN §8.3).
# Used by "Uninstall Glidedesk.app" in the DMG and by the app's own
# "Uninstall…" button. Asks whether to keep the settings.
set -u

APP_ID="app.glidedesk.desktop"
SUPPORT="$HOME/Library/Application Support/Glidedesk"
LOGS="$HOME/Library/Logs/Glidedesk"

say() { /usr/bin/osascript -e "$1" 2>/dev/null; }

find_app() {
  for p in "/Applications/Glidedesk.app" "$HOME/Applications/Glidedesk.app"; do
    [ -d "$p" ] && { echo "$p"; return; }
  done
  # Running from inside the bundle (Contents/Resources/uninstall.sh)?
  here="$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd)"
  case "$here" in *.app) echo "$here" ;; esac
}

APP="$(find_app)"

choice="$(say 'button returned of (display dialog "Uninstall Glidedesk?\n\nKeep your settings and layout for a future install?" buttons {"Cancel", "Remove settings", "Keep settings"} default button "Keep settings" cancel button "Cancel" with title "Uninstall Glidedesk" with icon caution)')"
[ -z "$choice" ] && exit 0

# 1. Stop the agent cleanly (releases keys, tells other computers), then the app.
if [ -n "$APP" ] && [ -x "$APP/Contents/MacOS/glidedesk" ]; then
  "$APP/Contents/MacOS/glidedesk" --shutdown >/dev/null 2>&1
fi
/usr/bin/pkill -x glidedesk >/dev/null 2>&1
/usr/bin/pkill -x glidedesk-agent >/dev/null 2>&1

# 2. Remove the login item (created by the app's "Start at login").
for plist in "$HOME/Library/LaunchAgents/Glidedesk.plist" "$HOME/Library/LaunchAgents/$APP_ID.plist"; do
  if [ -f "$plist" ]; then
    /bin/launchctl bootout "gui/$(id -u)" "$plist" >/dev/null 2>&1
    /bin/rm -f "$plist"
  fi
done

# 3. Move the app to the Trash (recoverable), never rm -rf.
if [ -n "$APP" ]; then
  say "tell application \"Finder\" to delete (POSIX file \"$APP\" as alias)" >/dev/null || /bin/mv "$APP" "$HOME/.Trash/" 2>/dev/null
fi

# 4. Settings and logs.
/bin/rm -rf "$HOME/Library/Caches/$APP_ID" "$HOME/Library/WebKit/$APP_ID" "$HOME/Library/Saved Application State/$APP_ID.savedState"
if [ "$choice" = "Remove settings" ]; then
  /bin/rm -rf "$SUPPORT" "$LOGS" "$HOME/Library/Preferences/$APP_ID.plist"
  # Remove the privacy permissions (macOS allows resetting our own entries).
  /usr/bin/tccutil reset Accessibility "$APP_ID" >/dev/null 2>&1
  /usr/bin/tccutil reset ListenEvent "$APP_ID" >/dev/null 2>&1
  say 'display dialog "Glidedesk and its settings were removed." buttons {"OK"} default button "OK" with title "Uninstall Glidedesk"' >/dev/null
else
  /bin/rm -rf "$LOGS"
  say 'display dialog "Glidedesk was removed. Your settings were kept for a future install." buttons {"OK"} default button "OK" with title "Uninstall Glidedesk"' >/dev/null
fi
exit 0
