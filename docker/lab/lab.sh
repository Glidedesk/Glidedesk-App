#!/bin/sh
# Starts the lab inside glidedesk-lab (see docker/lab/Dockerfile):
#   server   on X display :1 (one 1920x1080 screen), listening on gd0 + gd1
#   client-a on X display :2 (two screens: 1920x1080 + 1280x1024 on its right) via gd0
#   client-b on X display :3 (one 1600x900 screen) via gd1
# plus `vite dev` with the agent bridge on :5173 for browser QA.
# Every instance has its own GLIDEDESK_HOME under /lab — nothing real is touched.
set -eu
BIN=${GLIDEDESK_BIN:-/cache/target/debug/glidedesk}
LAB=/lab
mkdir -p "$LAB"

# Two dummy networks, so "one network goes off" can be tested for real.
ip link add gd0 type dummy 2>/dev/null || true
ip link add gd1 type dummy 2>/dev/null || true
ip addr add 10.77.0.1/24 dev gd0 2>/dev/null || true
ip addr add 10.78.0.1/24 dev gd1 2>/dev/null || true
ip link set gd0 up
ip link set gd1 up

# One at a time: started together they race to create /tmp/.X11-unix.
screen() { # display geometry
  Xvfb ":$1" -screen 0 "$2" -nolisten tcp -noreset >/dev/null 2>&1 &
  i=0
  while [ ! -S "/tmp/.X11-unix/X$1" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
}
screen 1 1920x1080x24
screen 2 3200x1080x24
screen 3 1600x900x24
# client-a: split its X screen into two monitors of different sizes.
DISPLAY=:2 xrandr --setmonitor A 1920/508x1080/286+0+0 screen >/dev/null
DISPLAY=:2 xrandr --setmonitor B 1280/339x1024/271+1920+0 none >/dev/null

conf() { # name role extra-toml
  mkdir -p "$LAB/$1/config"
  chmod 700 "$LAB/$1" "$LAB/$1/config"
  [ -f "$LAB/$1/config/config.toml" ] || printf 'schema_version = 4\n[device]\nrole = "%s"\nname = "%s"\n%s\n' "$2" "$1" "$3" >"$LAB/$1/config/config.toml"
}
conf server server '[server.network]
mode = "interfaces"
interfaces = ["gd0", "gd1"]
discovery = false'
conf client-a client '[client]
server_address = "10.77.0.1"'
conf client-b client '[client]
server_address = "10.78.0.1"'

start() { # name display
  GLIDEDESK_HOME="$LAB/$1" DISPLAY="$2" RUST_LOG=info,glidedesk_core=debug \
    nohup "$BIN" --agent >"$LAB/$1.out" 2>&1 &
  echo $! >"$LAB/$1.pid"
}
start server :1
sleep 1
start client-a :2
start client-b :3

if [ "${LAB_UI:-1}" = 1 ]; then
  cd /src/app/ui
  GLIDEDESK_DEV_AGENTS="server=$LAB/server/config/agent.sock,client-a=$LAB/client-a/config/agent.sock,client-b=$LAB/client-b/config/agent.sock" \
    nohup pnpm exec vite --host 0.0.0.0 --port 5173 --strictPort >"$LAB/vite.out" 2>&1 &
fi
echo "lab up"
