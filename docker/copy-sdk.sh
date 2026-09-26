#!/bin/sh
# Copies the newest macOS SDK from this Mac's Command Line Tools that the
# container's linker understands into ~/.cache/nexpingdesk/macos-sdk.
# (Apple licence: the SDK is only used on this Apple machine; never committed.)
set -eu
dest="${NEXPINGDESK_SDK:-$HOME/.cache/nexpingdesk/macos-sdk}"
root=/Library/Developer/CommandLineTools/SDKs
[ -d "$root" ] || { echo "Xcode Command Line Tools not found: run 'xcode-select --install'" >&2; exit 1; }
chosen=""
for sdk in $(ls -d "$root"/MacOSX[0-9]*.sdk 2>/dev/null | sort -rV); do
  tbd="$sdk/System/Library/Frameworks/AppKit.framework/AppKit.tbd"
  [ -f "$tbd" ] || continue
  # LLVM 19's ld64.lld does not know the 'arm64e.x1' arch used by newer SDKs.
  if sed -n '/^targets:/,/\]/p' "$tbd" | grep -q '\.x1-'; then continue; fi
  chosen="$(cd "$sdk" && pwd -P)"; break
done
[ -n "$chosen" ] || { echo "no compatible macOS SDK found in $root" >&2; exit 1; }
mkdir -p "$dest"
rsync -a --delete "$chosen/" "$dest/"
echo "macOS SDK: $(basename "$chosen") -> $dest"
