# Glidedesk

One keyboard and mouse for your Mac, Windows and Linux computers. Move the cursor off
the edge of the screen and it continues on the next computer; the clipboard and copied
files follow it. One app — every computer can be the server or a client.

| System | Build | File in a release / `output-build/` |
|---|---|---|
| macOS 13+ | Apple Silicon (arm64) | `Glidedesk_<ver>_macos-arm64.dmg` |
| Windows 10 22H2 / 11 / Server 2016+ | x64 | `…_windows-x64-setup.exe`, `…_windows-x64-offline-setup.exe` (includes WebView2), `…_windows-x64-portable.zip` (no install) |
| Linux (X11 or Wayland*) | x64, arm64 | `…_linux-<arch>.deb`, `.rpm`, `.tar.gz` |

\* On Wayland, Linux can be a **client**. Sharing a Linux computer's own keyboard and
mouse (server role) needs an X11 session until Wayland desktops offer a capture API.

## Install

- **macOS:** open the `.dmg` and drag Glidedesk to Applications. First launch: right-click → Open
  (macOS 15+: System Settings → Privacy & Security → Open Anyway). Click **Allow…** and turn
  Glidedesk on under **Accessibility** — that is the only permission needed; the window comes
  back by itself.
- **Windows:** run the setup. The same file installs, **updates** (keeps settings, restarts the app)
  or repairs. Silent: `setup.exe /S [/D=C:\path] [/NORESTART]` (exit code 3 = installed with a
  warning, e.g. firewall rule failed).
- **Windows portable:** unzip `…_windows-x64-portable.zip` anywhere (even a USB stick) and run
  `Glidedesk.exe`. Nothing is installed: settings, logs and received files stay in
  `Glidedesk Data` next to it, and it doesn't start at login unless you turn that on. To
  remove it, quit from the tray and delete the folder.
- **Linux:** `sudo apt install ./Glidedesk_<ver>_linux-<arch>.deb` or
  `sudo dnf install ./Glidedesk_<ver>_linux-<arch>.rpm`, or unpack the `.tar.gz` and run
  `sudo ./install.sh`. The package adds a udev rule so the signed-in user can create a virtual
  keyboard/mouse (needed to be controlled, also on Wayland).

Glidedesk **starts at login** on every system (tray only, no window). Turn it off in
General → Start at login.

## Using it

- **Connect:** clients find the server automatically. You can also type the server's
  **computer name** (e.g. `Studio-Mac`) or IP address under *This computer → Server*.
- **Password (optional):** on the server, *Network → Password*. Clients must enter the same
  password; others can't connect and are never listed. The password never crosses the
  network (SPAKE2 bound to the encrypted connection) and only a salted hash is stored.
- **Keyboard and mouse behave natively:** keys act as on a keyboard plugged into the computer
  you're controlling. Pointer speed and scrolling follow the server by default (same feel and
  scroll direction on every screen, including clients with several monitors at different
  scaling); change them per computer in *Computers → Settings*. Swapping Cmd and Ctrl is an
  option per computer.
- **Files:** copy files, move to another computer and paste with the keyboard shortcut
  (⌘V / Ctrl+V). Nothing is copied before you paste; the paste waits while the files arrive,
  then your file manager pastes them. No size limit (free disk space is checked). To paste
  with the mouse, first choose *Get … now* in the tray or window. Files that were already
  copied when Glidedesk started (or over an hour ago) are not offered; *Clipboard & Files →
  Activity* lists everything that moved, where and why.
- **Network:** IPv4 only. With *Network → Selected interfaces*, a network that goes off shows a
  warning and sharing continues on the others; it is used again when it comes back.

Uninstall: Windows → Settings → Apps (asks whether to keep settings) · macOS → "Uninstall
Glidedesk" in the DMG or Advanced → Uninstall · Linux → your package manager (settings in
`~/.config/glidedesk` are kept).

## Signatures (no paid accounts)

Releases are signed with Glidedesk's own keys (`make signing-keys`, see `signing/`):

- **macOS:** self-signed code-signing identity — the app keeps its permissions across updates.
- **Windows:** self-signed Authenticode. To remove "Unknown publisher" on your own PCs, import
  `signing/glidedesk-codesign.cer` into *Trusted Publishers* and *Trusted Root Certification
  Authorities* (or deploy it with Group Policy).
- **Everything:** `SHA256SUMS` is signed with minisign:
  `minisign -Vm SHA256SUMS -p minisign.pub && sha256sum -c SHA256SUMS`.

## Build (everything runs in Docker)

```sh
make docker-image   # once
make sdk            # once, on a Mac: copies the macOS SDK for cross-compiling
make signing-keys   # once: creates .signing/ (private) and signing/ (public)
make test           # lint + all tests + macOS/Windows cross-checks + UI tests
make release        # macOS arm64 + Windows x64 + Linux (this machine's arch) → output-build/
```

## Building and testing: local Docker only (no GitHub CI)

GitHub only stores the code — there is no GitHub Actions workflow. Every build and test runs
locally in Docker with one command (the same steps the GitHub runners used to do):

```sh
scripts/local-ci.sh            # or: make ci — test → real agents → installers for every platform
scripts/local-ci.sh test       # only the tests
scripts/local-ci.sh package    # only the installers
```

1. **test** — UI typecheck/tests/build, rustfmt, clippy, every Rust test, Windows cross-check
   (and macOS when the SDK is present).
2. **agents** — real server + client agents on a virtual screen: connect, client restarts, self-test.
3. **package** — Linux x64 `.deb`/`.rpm`/`.tar.gz`, Windows installer + offline installer +
   portable zip, macOS `.dmg`, `SHA256SUMS` (signed with `.signing/` keys when present).

A failing step stops the run, so nothing is packaged from code that failed a test. Installers go
to `output-build/` (older files move to `output-build/previous-*/`). The macOS app needs the
macOS SDK, which Apple allows only on Apple hardware: run `make sdk` and `scripts/local-ci.sh`
on the Mac to include it; elsewhere it is reported as skipped. The Mac's real event-tap tests
also need a Mac: `GLIDEDESK_TAP_TESTS=1 cargo test -p glidedesk-input`.

Docs: `PLAN.md` (design), `docs/PROGRESS.md` (status), `docs/TESTING.md` (real-device checklist).
