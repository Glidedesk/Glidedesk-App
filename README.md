# Glidedesk

One keyboard and mouse for your Mac, Windows and Linux computers. Move the cursor off
the edge of the screen and it continues on the next computer; the clipboard and copied
files follow it. One app — every computer can be the server or a client.

| System | Build | File in a release / `output-build/` |
|---|---|---|
| macOS 13+ | Apple Silicon (arm64) | `Glidedesk_<ver>_macos-arm64.dmg` |
| Windows 10 22H2 / 11 / Server 2016+ | x64 | `…_windows-x64-setup.exe`, `…_windows-x64-offline-setup.exe` (includes WebView2) |
| Linux (X11 or Wayland*) | x64, arm64 | `…_linux-<arch>.deb`, `.rpm`, `.tar.gz` |

\* On Wayland, Linux can be a **client**. Sharing a Linux computer's own keyboard and
mouse (server role) needs an X11 session until Wayland desktops offer a capture API.

## Install

- **macOS:** open the `.dmg` and drag Glidedesk to Applications. First launch: right-click → Open
  (macOS 15+: System Settings → Privacy & Security → Open Anyway). Allow **Accessibility**
  (and **Input Monitoring** on the server) when asked.
- **Windows:** run the setup. The same file installs, **updates** (keeps settings, restarts the app)
  or repairs. Silent: `setup.exe /S [/D=C:\path] [/NORESTART]` (exit code 3 = installed with a
  warning, e.g. firewall rule failed).
- **Linux:** `sudo apt install ./Glidedesk_<ver>_linux-<arch>.deb` or
  `sudo dnf install ./Glidedesk_<ver>_linux-<arch>.rpm`, or unpack the `.tar.gz` and run
  `sudo ./install.sh`. The package adds a udev rule so the signed-in user can create a virtual
  keyboard/mouse (needed to be controlled, also on Wayland).

Glidedesk **starts at login** on every system (tray only, no window). Turn it off in
General → Start at login.

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

## GitHub Actions

- **Every push / pull request:** lint, tests, UI tests, Windows cross-check (`ci.yml`).
- **Push to `main`:** Windows x64 + Linux x64/arm64 installers → **Nightly** pre-release.
- **Tag `vX.Y.Z`:** all of the above **plus macOS** → a normal release with every file,
  checksums and signature: `git tag v0.2.0 && git push --tags`.

Designed for the GitHub Free private-repo allowance (2,000 min/month): all builds except macOS
run on Linux runners (1×), macOS (10×) only runs for tags. A normal month (~30 pushes,
2 releases) uses about 1,300 minutes. Add the four secrets from `.signing/github-secrets.env`
to sign CI builds; without them builds are ad-hoc/unsigned but still work.

Docs: `PLAN.md` (design), `docs/PROGRESS.md` (status), `docs/TESTING.md` (real-device checklist).
