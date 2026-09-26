#!/usr/bin/env bash
# Real-agent smoke test on the machine it runs on (Linux, macOS, Windows/Git Bash):
# a server agent and a client agent talk over loopback, each with its own
# GLIDEDESK_HOME (nothing real is touched). Checks:
#   1. the client connects;
#   2. the client restarts gracefully (--shutdown) and reconnects — the server keeps running;
#   3. the client is killed (no goodbye) and reconnects;
#   4. --selftest runs and prints its report.
# Usage: scripts/os-smoke.sh path/to/glidedesk[.exe]
# Exit 0 = passed; 2 = the OS refused keyboard/mouse access (reported, e.g. a
# macOS runner without Accessibility); anything else = failure.
set -u
BIN=${1:?usage: os-smoke.sh path/to/glidedesk}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
PORT=${GLIDEDESK_SMOKE_PORT:-24871}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/gd-smoke.XXXXXX")
SRV=$WORK/server
CLI=$WORK/client
PIDS=()

say() { printf '\n== %s\n' "$*"; }
dump() {
  for f in "$SRV.out" "$CLI.out"; do
    [ -f "$f" ] && { printf -- '--- %s (last 60 lines)\n' "$(basename "$f")"; tail -n 60 "$f"; }
  done
}
fail() {
  printf '\n!! FAILED: %s\n' "$*"
  dump
  exit 1
}
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  wait 2>/dev/null
}
trap cleanup EXIT

conf() { # dir role body
  mkdir -p "$1/config"
  chmod 700 "$1" "$1/config" 2>/dev/null
  printf 'schema_version = 4\n[device]\nrole = "%s"\nname = "%s"\n%s\n' "$2" "smoke-$2" "$3" >"$1/config/config.toml"
}
conf "$SRV" server "[server.network]
mode = \"addresses\"
addresses = [\"127.0.0.1\"]
port = $PORT
discovery = false"
conf "$CLI" client "[client]
server_address = \"127.0.0.1:$PORT\""

start() { # home logfile
  GLIDEDESK_HOME="$1" RUST_LOG="info,glidedesk_core=debug" NO_COLOR=1 "$BIN" --agent >>"$2" 2>&1 &
  LAST=$!
  PIDS+=("$LAST")
}
count() { grep -c "$2" "$1" 2>/dev/null || true; }
# wait_more file pattern old-count seconds
wait_more() {
  local i=0
  while [ "$i" -lt $(($4 * 4)) ]; do
    [ "$(count "$1" "$2")" -gt "$3" ] && return 0
    sleep 0.25
    i=$((i + 1))
  done
  return 1
}
refused() { grep -qiE "missing permission|access to the keyboard and mouse|Accessibility" "$1" 2>/dev/null; }
refused_exit() {
  dump
  printf '\n!! The OS refused keyboard/mouse access to the %s (expected on CI without it).\n' "$1"
  exit 2
}

"$BIN" --version || fail "the binary does not run"

say "1. server + client connect over loopback"
start "$SRV" "$SRV.out"
SRV_PID=$LAST
if ! wait_more "$SRV.out" "server running" 0 30; then
  refused "$SRV.out" && refused_exit server
  fail "server did not start"
fi
start "$CLI" "$CLI.out"
CLI_PID=$LAST
if ! wait_more "$SRV.out" "client connected" 0 30; then
  refused "$CLI.out" && refused_exit client
  fail "client did not connect"
fi
wait_more "$CLI.out" "connected server=" 0 10 || fail "client does not report the link"
echo "ok"

say "2. graceful client restart (server keeps running)"
n=$(count "$SRV.out" "client connected")
GLIDEDESK_HOME="$CLI" "$BIN" --shutdown || true
wait_more "$SRV.out" "client disconnected" 0 15 || fail "server did not see the client leave"
wait "$CLI_PID" 2>/dev/null
start "$CLI" "$CLI.out"
CLI_PID=$LAST
wait_more "$SRV.out" "client connected" "$n" 30 || fail "client did not reconnect after a graceful restart"
echo "ok"

say "3. client killed without a goodbye, then restarted"
n=$(count "$SRV.out" "client connected")
kill -9 "$CLI_PID" 2>/dev/null || kill "$CLI_PID"
wait "$CLI_PID" 2>/dev/null
start "$CLI" "$CLI.out"
CLI_PID=$LAST
wait_more "$SRV.out" "client connected" "$n" 30 || fail "client did not reconnect after being killed"
kill -0 "$SRV_PID" 2>/dev/null || fail "the server stopped"
echo "ok"

say "4. self-test"
GLIDEDESK_HOME="$WORK/selftest" "$BIN" --selftest >"$WORK/selftest.json" 2>&1
head -c 4000 "$WORK/selftest.json"
echo
[ -s "$WORK/selftest.json" ] || fail "self-test printed nothing"

say "shutting down"
GLIDEDESK_HOME="$CLI" "$BIN" --shutdown || true
GLIDEDESK_HOME="$SRV" "$BIN" --shutdown || true
sleep 1
grep -hE "WARN|ERROR" "$SRV.out" "$CLI.out" | grep -viE "clipboard|mDNS|arboard" | head -20
echo
echo "PASSED: connect, graceful restart, kill + restart, self-test"
