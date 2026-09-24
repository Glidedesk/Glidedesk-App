#!/bin/sh
# Run a command inside glidedesk-builder with the repo, caches and the macOS SDK mounted.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
sdk="${GLIDEDESK_SDK:-$HOME/.cache/glidedesk/macos-sdk}"
sdk_mount=""
[ -d "$sdk/usr/lib" ] && sdk_mount="-v $sdk:/opt/macos-sdk:ro"
# Local signing keys (make signing-keys) are used automatically when present.
sign_env=""
if [ -f "$root/.signing/password" ]; then
  sign_env="-e GLIDEDESK_MAC_P12=/src/.signing/mac.p12 -e GLIDEDESK_WIN_PFX=/src/.signing/win.p12 -e GLIDEDESK_MINISIGN_KEY=/src/.signing/minisign.key"
  sign_env="$sign_env -e GLIDEDESK_MAC_P12_PASSWORD=$(cat "$root/.signing/password") -e GLIDEDESK_WIN_PFX_PASSWORD=$(cat "$root/.signing/password")"
fi
tty=""
[ -t 0 ] && [ -t 1 ] && tty="-it"
# shellcheck disable=SC2086
exec docker run --rm $tty \
  -v "$root:/src" \
  -v gd-cargo-registry:/usr/local/cargo/registry \
  -v gd-cargo-git:/usr/local/cargo/git \
  -v gd-cache:/cache \
  $sdk_mount \
  $sign_env \
  -e CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}" \
  -e GLIDEDESK_VERSION \
  -w /src \
  glidedesk-builder:latest "$@"
