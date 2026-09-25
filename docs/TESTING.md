# Real-device test checklist (PLAN §10.2)

Automated tests run in Docker (`make test`). These checks need real computers.
Use the installers in `output-build/`.

## Lab (no hardware needed)
`make lab` starts, in one container, a real server and two Linux clients on virtual X screens
(client-a has two monitors of different sizes) connected over two dummy networks (`gd0`, `gd1`).
The settings UI of each is at `http://localhost:5173/?agent=server` (`client-a`, `client-b`) —
a dev-only bridge to each agent's socket, never part of a build. Useful checks:
- move the cursor: `docker exec gd-lab sh -c 'DISPLAY=:1 xdotool mousemove 1900 300 mousemove_relative 60 0'`,
  read it on a client: `DISPLAY=:2 xdotool getmouselocation`;
- a network off/on: `docker exec gd-lab ip link set gd0 down` (warning, client-b stays online), `… up`;
- restart agents after a rebuild: `docker exec gd-lab /src/docker/lab/restart.sh`; stop: `make lab-stop`.

## Before you start
- One **server** (the computer whose keyboard/mouse you use) and at least one **client**.
- Same local network. Windows: allow Glidedesk on *Private* networks when asked.
- macOS: grant **Accessibility** (the only permission needed). The prompt and the list in
  System Settings must show **Glidedesk** with its icon; after granting it, the Glidedesk
  window comes back to the front (it shows in the Dock while open).

## 1. Install
- [ ] macOS: open the `.dmg`, drag Glidedesk to Applications, open it
      (macOS 15+: System Settings → Privacy & Security → Open Anyway).
- [ ] Windows: run `Glidedesk_<ver>_windows-x64-setup.exe` (SmartScreen: More info → Run anyway).
- [ ] Windows Server / no internet: use `…_windows-x64-offline-setup.exe`.
- [ ] First window shows the setup: choose Server or Client.

## 2. Keyboard & mouse
- [ ] Client appears on the server (Computers page, with the window already open) within a few
      seconds and is placed on a free side. Changing any setting afterwards keeps it.
- [ ] Client: type the server's computer name (not its IP) as the address → connects.
- [ ] Server Network → Password set: a client without it shows "needs a password" and is not
      listed; with the right one it connects; 5 wrong tries block it for a minute.
- [ ] Moving the cursor across that edge moves it onto the client; typing works.
- [ ] Keys are native: on a Windows client, the Mac's ⌘ key is the Windows key and Ctrl+C copies;
      Computers → Cmd / Ctrl → "Swap" makes ⌘C copy instead.
- [ ] Scrolling on each computer follows its own direction setting (natural on the Mac only).
- [ ] While the cursor is on the client, the server's own cursor stays hidden and **nothing moves
      or clicks on the server** (also with trackpad gestures; also with an admin window in front
      on a Windows server).
- [ ] Crossing a wide client and coming back works on the **first** push at the edge.
- [ ] Hold Shift while crossing: nothing stays stuck on either side.
- [ ] Move back: the cursor returns where it left.
- [ ] Layout page: drag the client to another side; the new edge works immediately.
- [ ] Two monitors on the server: set "One monitor" for the link; only that edge leads to the client.
- [ ] Ctrl+Alt+L locks the cursor; again unlocks.

## 3. Health
- [ ] Unplug the client's network cable (or turn Wi-Fi off): the server shows it **Offline** within ~3 s,
      the cursor cannot enter it, and no key is stuck.
- [ ] Reconnect: it comes back **Online** by itself.

## 4. Clipboard & files
- [ ] Copy text on the server, move to the client, paste.
- [ ] Copy on the client, move back, paste on the server.
- [ ] Copy a screenshot/image both ways.
- [ ] Copy a folder with a large file (e.g. 5 GB), move across: **nothing is transferred** yet and
      pasting with the mouse can't paste old files. Press ⌘V / Ctrl+V in a folder: the paste
      waits while the files arrive (Transfers shows progress), then the file manager pastes them.
- [ ] Without copying anything new, move back and forth several times: no transfer starts.
- [ ] Windows: **Cut** a file, move to the other computer, paste → original goes to the Recycle Bin.
- [ ] Turn off "Share clipboard" in the tray: nothing is sent.

## 5. Tray
- [ ] Open the menu and leave it open for 30 s while clients are connected: it stays open.
- [ ] Stop sharing → clients show the server offline; Start → they reconnect.
- [ ] Restart Glidedesk → back within a few seconds.
- [ ] Quit → both the tray icon and `glidedesk-agent` exit.

## 6. Upgrade & uninstall (Windows)
- [ ] Install version A, then run the installer of version B: it says "Update", keeps settings,
      and Glidedesk restarts by itself.
- [ ] Run the same version again: "Repair".
- [ ] Silent: `setup.exe /S` upgrades without questions; exit code 0.
- [ ] Uninstall → "Keep my settings" → reinstall → layout is still there.
- [ ] Uninstall → "Remove my settings" → `%APPDATA%\Glidedesk` is gone.

## 7. Linux
- [ ] `sudo apt install ./Glidedesk_<ver>_linux-<arch>.deb` (or `dnf install` the .rpm); open Glidedesk from the menu.
- [ ] Client role on **Wayland** (GNOME/KDE): the server can move the pointer, click and type here.
- [ ] Server role on an **X11** session: moving off the edge reaches the other computer; on Wayland
      the app explains that the server role needs X11.
- [ ] Log out and in: Glidedesk starts by itself (tray icon).
- [ ] Clipboard text/image both ways; copy a folder in Files/Dolphin and paste on the other computer.

## 8. Uninstall (macOS)
- [ ] Run "Uninstall Glidedesk" from the DMG → Keep / Remove settings both work.

## Self-test
`Advanced → Run self-test` (or `glidedesk-agent --selftest`) prints permissions, monitors,
interfaces, capture/inject checks and encrypted loopback latency. Attach it to bug reports.

## Version 4 checks
- [ ] Computers → Settings → Block / Unblock / Forget / Go there / Disconnect all react at once.
- [ ] Forget an online computer: it disappears and stays away; on it, Reconnect joins again.
- [ ] Change an offline computer's mouse speed; when it comes online it uses the new speed
      (its window shows "Set by the server: …").
- [ ] Copy files, restart Glidedesk, move to a client: they are **not** offered (Clipboard & Files →
      Activity says why). Copy again: offered; nothing downloads until you paste there.
- [ ] Network → Selected interfaces with two networks; switch one off: a warning, the other keeps working;
      switch it on: listening again within a few seconds.
- [ ] A client with two monitors: the cursor reaches both, speed feels the same on each.
- [ ] Scrolling on a client goes the same way as on the server (natural scrolling included).
