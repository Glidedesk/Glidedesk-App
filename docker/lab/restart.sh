#!/bin/sh
# Restarts the lab agents (after a rebuild) without touching their settings.
LAB=/lab
BIN=${GLIDEDESK_BIN:-/cache/target/debug/glidedesk}
for n in server client-a client-b; do
  [ -f "$LAB/$n.pid" ] && kill "$(cat "$LAB/$n.pid")" 2>/dev/null
done
sleep 1
start() {
  GLIDEDESK_HOME="$LAB/$1" DISPLAY="$2" RUST_LOG=info,glidedesk_core=debug NO_COLOR=1 \
    nohup "$BIN" --agent >"$LAB/$1.out" 2>&1 &
  echo $! >"$LAB/$1.pid"
}
start server :1
sleep 1
start client-a :2
start client-b :3
echo restarted
