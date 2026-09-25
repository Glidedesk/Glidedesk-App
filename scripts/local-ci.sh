#!/bin/sh
# Glidedesk's pipeline, run locally in Docker (GitHub only stores the code; there
# is no GitHub Actions CI). The same steps the GitHub runners used to do:
#   test     builder image: UI typecheck + tests + build, rustfmt, clippy, every
#            Rust test, Windows cross-check (and macOS, when the SDK is present)
#   agents   lab image, virtual X screen: real server + client agents connect,
#            survive client restarts, run the self-test (scripts/os-smoke.sh)
#   package  builder image: Linux x64 .deb/.rpm/.tar.gz, Windows installer +
#            offline installer + portable zip, macOS .dmg (needs the macOS SDK:
#            `make sdk` on the Mac), SHA256SUMS; signed with .signing/ keys when present
# A failing step stops everything: nothing is packaged from code that failed a test.
# Installers land in output-build/ (earlier files are moved to output-build/previous-*/).
#
# Usage: scripts/local-ci.sh [all|test|agents|package]      (default: all)
# macOS: the Mac app is built only where the SDK is (Apple licence: Apple hardware);
# on another machine it is reported as skipped. Its real event-tap tests need a Mac:
#   GLIDEDESK_TAP_TESTS=1 cargo test -p glidedesk-input   (on the Mac)
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
RUN=docker/run.sh
sdk="${GLIDEDESK_SDK:-$HOME/.cache/glidedesk/macos-sdk}"
have_sdk=false
[ -d "$sdk/usr/lib" ] && have_sdk=true
what="${1:-all}"
started=$(date +%s)
summary=""

step() { printf '\n==== %s\n' "$*"; }
note() { summary="$summary\n  $*"; }
fail() {
  printf '\n!! FAILED: %s — stopping (nothing packaged).\n' "$*"
  exit 1
}

images() {
  if ! docker image inspect glidedesk-builder:latest >/dev/null 2>&1; then
    step "building the builder image (first run only, ~20 min)"
    docker build -f docker/Dockerfile -t glidedesk-builder:latest . || fail "builder image"
  fi
  if [ "$what" = all ] || [ "$what" = agents ]; then
    docker build -q -t glidedesk-lab -f docker/lab/Dockerfile docker/lab >/dev/null || fail "lab image"
  fi
}

run_tests() {
  step "test: UI (typecheck, tests, build)"
  $RUN sh -c 'cd app/ui && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test && pnpm build' || fail "UI"
  step "test: rustfmt, clippy, all Rust tests"
  $RUN sh -c 'cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo nextest run --workspace --locked --no-fail-fast' ||
    fail "Rust lint/tests"
  targets="x86_64-pc-windows-msvc"
  $have_sdk && targets="$targets aarch64-apple-darwin"
  step "test: cross-checks ($targets)"
  # shellcheck disable=SC2086
  $RUN cargo xtask check-cross $targets || fail "cross-check"
  note "tests:    passed (UI, rustfmt, clippy, Rust tests, cross-check: $targets)"
}

run_agents() {
  step "agents: real server + client on a virtual screen"
  $RUN cargo build -p glidedesk-app --locked || fail "app build"
  docker run --rm -v "$root:/src" -v gd-cargo-registry:/usr/local/cargo/registry -v gd-cargo-git:/usr/local/cargo/git \
    -v gd-cache:/cache -w /src glidedesk-lab:latest \
    sh -c 'Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; DISPLAY=:99 bash scripts/os-smoke.sh /cache/target/debug/glidedesk' ||
    fail "real agents"
  note "agents:   passed (connect, graceful restart, kill + restart, self-test)"
}

run_package() {
  step "package: installers for every platform"
  old=$(find output-build -maxdepth 1 -type f 2>/dev/null | head -n 1)
  if [ -n "$old" ]; then
    keep="output-build/previous-$(date +%Y%m%d-%H%M%S)"
    mkdir -p "$keep"
    find output-build -maxdepth 1 -type f -exec mv {} "$keep/" \;
    echo "earlier files moved to $keep"
  fi
  $RUN cargo xtask package linux || fail "Linux packages"
  note "linux:    x64 .deb .rpm .tar.gz"
  $RUN cargo xtask package windows || fail "Windows packages"
  note "windows:  x64 installer, offline installer, portable zip"
  if $have_sdk; then
    $RUN cargo xtask package macos || fail "macOS package"
    note "macos:    Apple Silicon .dmg"
  else
    note "macos:    SKIPPED — no macOS SDK here (run 'make sdk' and this script on the Mac)"
  fi
  $RUN cargo xtask checksums || fail "checksums"
  note "sums:     output-build/SHA256SUMS$([ -f .signing/minisign.key ] && echo ' + minisign signature')"
}

case "$what" in
  all) images; run_tests; run_agents; run_package ;;
  test) images; run_tests ;;
  agents) images; run_agents ;;
  package) images; run_package ;;
  *) echo "usage: $0 [all|test|agents|package]" >&2; exit 2 ;;
esac

printf '\n==== done in %s min' "$((($(date +%s) - started) / 60))"
printf '%b\n' "$summary"
for f in output-build/*; do [ -f "$f" ] && echo "  ${f#output-build/}"; done
exit 0
