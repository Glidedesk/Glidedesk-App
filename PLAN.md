# Project Plan — **Glidedesk** · Cross-Platform Keyboard & Mouse Sharing

> Status: **v2 built** (2026-09-25) — see §13 and `docs/PROGRESS.md`. Progress per phase is tracked in `docs/PROGRESS.md`.
> Name: **Glidedesk** — your cursor glides from desk to desk. Private app, not for public distribution.
> Date: 2026-09-25

---

## 1. Goal

One app for **macOS and Windows** that lets one computer's keyboard and mouse (the **server**) control other computers (the **clients**) on the local network, by moving the cursor off a screen edge. Clipboard and files follow the cursor. It installs and uninstalls like normal software on both systems.

Similar products we take ideas from: Synergy / Deskflow / Input Leap / Barrier, ShareMouse, Microsoft Mouse Without Borders, Apple Universal Control, Logitech Flow, lan-mouse.

### Requirements from the brief

| # | Requirement | Where it is handled |
|---|---|---|
| R1 | Runs on macOS and Windows; any mix of server and client (Mac→Win, Win→Mac, Mac→Mac, Win→Win) | §4, §5 |
| R2 | Server and client roles | §4.1 |
| R3 | Server settings set where each client sits (placement / direction) | §6.1 |
| R4 | Config saved in a protected location, with export (and import) | §6.5 |
| R5 | Clipboard sharing can be turned on/off | §6.3 |
| R6 | Files can be copied/cut and pasted (can be turned on/off) | §6.4 |
| R7 | Clipboard and files of **any size**, copy or cut, following each OS's normal behaviour | §6.3, §6.4 |
| R8 | Server network interface selection: pick interfaces and/or IPs; one, several, or all | §6.2 |
| R9 | No authentication, no password — client and server just connect | §7 |
| R10 | Latest technology only, nothing outdated | §5 |
| R11 | Normal installer and uninstaller on both OSes | §8 |
| R12 | Uninstaller asks whether to keep the configuration | §8.3 |
| R13 | Lightweight and low-resource, but full-featured | §6.10, §10 |
| R14 | Everything customisable on both server and client | §6.9 |
| R15 | Server keeps checking every client; offline clients show as **Offline** | §6.7 |
| R16 | System tray / menu bar on both OSes with **Stop, Restart, Quit** | §6.8 |
| R17 | UI designed after the best paid and free apps | §6.11 |
| R18 | Build both the Mac and Windows apps on a Mac | §8.4 |
| R19 | **All builds and code tests run in a Docker container** (on this Mac) | §8.4, §10 |
| R20 | **Real tests run on real Mac and Windows machines** | §10.2 |
| R21 | Server with 2+ monitors: choose **which monitor(s)** hand over to each client, or **all monitors** on the client's side | §6.1.1 |
| R22 | Windows installer **detects an existing install and upgrades it in place** (keeps config, applies changes); installs fresh when absent. Same idea on macOS | §8.2.1, §8.1 |
| R23 | Finished installers/app files are placed in **`output-build/`** | §8.4 |

---

## 2. Terms

- **Server** — the computer with the physical keyboard and mouse. It captures input and sends it to clients.
- **Client** — a computer that receives input and injects it as if typed locally.
- **Screen** — one computer (which may have several monitors) placed in the layout.
- **Layout** — the 2D arrangement of screens that decides which edge leads where.
- **Daemon** — the background process that does the real work (capture, network, clipboard). Runs without the UI open.
- **UI** — the settings/tray app that controls the daemon.

---

## 3. Feature list

### 3.1 Core (MVP — must ship in v1.0)

1. **Role choice** — one app; each machine is set as Server or Client at first run (can be changed later).
2. **Auto-discovery** — clients and servers find each other on the LAN via mDNS/DNS-SD. Manual "connect to IP:port" as a fallback.
3. **Visual layout editor** (server) — drag computer tiles around a grid; snap left/right/top/bottom; shows the real monitor shapes and sizes of each client; supports offsets (e.g. client sits half-way down the right edge).
4. **Edge switching** — cursor crosses to the neighbour screen at the shared edge, with correct position mapping between different resolutions and DPI scales.
5. **Full keyboard support** — scancode-based so layouts/languages work; all modifiers, media keys, function keys; Unicode fallback.
6. **Full mouse support** — buttons 1–5, high-resolution and smooth (trackpad) scrolling, horizontal scroll.
7. **Clipboard sharing** — on/off switch; text, rich text (RTF/HTML), images, files; any size (lazy transfer, §6.3).
8. **File copy/cut and paste** — on/off switch; any size; progress, cancel, resume (§6.4).
9. **Network interface / IP binding** — select one, several, or all (§6.2).
10. **Config storage** — protected per-user location, export/import (§6.5).
11. **Tray / menu-bar app** — status, every client with online/offline state, quick toggles, **Stop / Start, Restart, Quit** (§6.8).
12. **Start at login** — optional, on by default.
13. **Auto-reconnect + health checks** — clients reconnect on network drop, sleep/wake, IP change; the server checks every client continuously and shows Offline clients (§6.7).
14. **Installer + uninstaller** on both OSes with "keep configuration?" prompt (§8).
15. **Encrypted traffic** — always on, zero setup, no password (§7).

### 3.2 Best features borrowed from similar apps (v1.x) — summary; full list in §3.4

| Feature | Inspired by | Notes |
|---|---|---|
| Drag-and-drop files across screens (drag a file from server desktop onto a client window) | Mouse Without Borders, ShareMouse, Universal Control | Phase 5 |
| Modifier remapping per client (e.g. Cmd↔Ctrl when a Mac controls Windows) | Deskflow, ShareMouse | Default: sensible Mac↔Win mapping, editable |
| Per-client mouse speed and scroll direction/speed | ShareMouse | Natural scrolling differs Mac vs Win |
| Hotkey: **lock cursor to current screen** (for games / full-screen apps) | Synergy, Deskflow | Plus auto-lock when a full-screen game is focused |
| Hotkeys: jump straight to screen N; cycle screens | Synergy | Configurable |
| Switch guards: delay before switching, double-tap edge, hold modifier, dead corners | Synergy / Barrier | Stops accidental switching |
| Dim inactive screens | ShareMouse | Optional, adjustable level |
| Cursor locator — brief highlight/animation on the screen the cursor lands on | Universal Control | Optional |
| Sync lock screen / screensaver / sleep across machines | Deskflow, Logitech Flow | Optional per client |
| Wake-on-LAN for sleeping clients | ShareMouse | Needs client MAC stored |
| Multi-monitor aware layout (each monitor is a separate tile inside a computer) | Universal Control | Edges between monitors of different machines |
| Clipboard history (last N items, local only) | ShareMouse | Optional, off by default |
| Transfer centre — list of active/finished transfers with speed, ETA, retry | — | Phase 5 |
| Diagnostics page — latency, packet loss, per-interface status, log export | Deskflow | Helps support |
| Auto-update from a private release location | — | Update packages verified with the Tauri updater key; can be disabled |
| Dark/light mode, localisation-ready, screen-reader accessible UI | — | WCAG 2.2 AA |

### 3.4 Market research — every similar app (September 2026)

Researched on 2026-09-25. **Release** column: **v1** = in 1.0 · **v1.x** = Phase 5 · **Later** = after 1.0 · **No** = out of scope (reason given).

#### 3.4.1 The apps

| App | Type | Platforms | Status (Sept 2026) | Known for | Weak points we avoid |
|---|---|---|---|---|---|
| **Synergy 3** (Symless) | Paid, one-time | Win, Mac, Linux | Active (v3.6.3) | Original software KVM; TLS, hotkeys, key swapping, layout editor, drag-and-drop | Background service left running after the app is deleted on macOS 26; reconnect loops; database version clashes between versions |
| **ShareMouse** | Freemium / Standard / Pro | Win, Mac | Active | Most polished Mac↔Win app: drag-and-drop files and folders, dimming, auto monitor detection, location-based profiles, portable mode, remote Windows login, Ctrl+Alt+Del, UAC | Closed source; AES optional and password-based |
| **Multiplicity 4** (Stardock) | Paid (4 tiers, 5–40 PCs) | Windows only | Active | Audio sharing, "Seamless Display" (a laptop used as an extra monitor), AES-256, Wake-on-LAN | Windows only |
| **Input Director** | Free for personal use | Windows only | Active | Most power-user options: macros, input mirroring, LED sync, coordinated shutdown/sleep, host allow-list, command-line tool, screen-name overlay | Windows only; dated UI |
| **Mouse Without Borders** (PowerToys) | Free | Windows only, max 4 PCs | Active | Service mode (lock screen + admin windows), wrap mouse, easy-mouse modifier, fullscreen-app guard, lock-all hotkey, broadcast input, status colours | File copy limited to **one file ≤ 100 MB**; no folders |
| **Logitech Flow** (Options+) | Free (needs Logitech mouse) | Win, Mac, max 3 | Active | Copy/paste text, images, files, folders; simple setup | Tied to Logitech hardware |
| **Apple Universal Control** | Free (built in) | Mac + iPad, max 3 | Active | Best feel: push-through edge, drag-and-drop, arrangement canvas | Apple devices only |
| **Cursr** | Freemium (2 devices free) | Win, Mac, Linux | Active | Border linking and segmenting, auto-load layouts per display setup, use any machine's keyboard/mouse | Closed source |
| **CursorHop** | Paid, one-time ($10–35, 2–10 PCs) | Win, Mac | Active (new) | Noise-protocol encryption, Cmd/Ctrl translation, file transfer with no size limit (~70 Mbps) | Slow file transfer; full clipboard only on higher tiers |
| **across** | Paid | Win, Mac, Linux, Android, iOS | Active | Bluetooth HID emulation, AirPlay screen mirroring, built-in file server, trackpad gestures | Text clipboard limited to 1,023 bytes in some modes |
| **EasySwitch** | Free for personal use | Win, Mac, Linux | Active | Keyboard, mouse, clipboard and file transfer | Closed source |
| **GiMeSpace KVMShare Pro** | Paid | Windows | Active | Shares windows/video with low bandwidth | Windows only |
| **Deskflow** | Free, open source | Win, Mac, Linux, BSD | Active (upstream of Synergy) | TLS by default, Wayland, compatible with Synergy 1/Barrier/Input Leap, huge option set | Qt UI; file transfer weak |
| **Input Leap** | Free, open source | Win, Mac, Linux, BSD | **Archived July 2026** | Barrier fork | Dead project |
| **Barrier** | Free, open source | Win, Mac, Linux, BSD | **Unmaintained** (no security fixes) | Synergy 1 fork | Dead project |
| **Lan Mouse** | Free, open source (Rust) | Win, Mac, Linux | Active | Rust, DTLS encryption, device fingerprints, CLI/daemon modes | Deliberately no clipboard or file transfer |
| **ShareCursor** | Free, open source (Rust, MIT/Apache) | Win, Mac | Early development | UDP input, X25519 + ChaCha20-Poly1305, mDNS, image clipboard | Early; needs a passphrase |
| **InputShare** | Free, open source | Windows → Android | Active | Controls an Android phone over ADB | Android only |

#### 3.4.2 Every feature found → Glidedesk

**Input & switching**

| Feature | Seen in | Release |
|---|---|---|
| Edge switching with correct mapping between different resolutions/DPI | All | v1 |
| Multi-monitor layout, each monitor its own tile | Universal Control, ShareMouse, Cursr | v1 |
| **Edge segments / border linking** — split one edge into several neighbours; link any two borders | Cursr | v1 |
| Auto-place a new computer on a free side (zero-config layout) | ShareMouse | v1 |
| Grid layout (not just a row) with offsets | Synergy, Deskflow, MWB (2×2) | v1 |
| **Wrap mouse** — leaving the last screen brings you back to the first | MWB | v1 |
| Switch guards: delay, double-tap edge, hold modifier (Shift/Ctrl), **dead corners** | Synergy, Deskflow, MWB, Input Director | v1 |
| **Block switching while a full-screen app is focused**, with an "allowed apps" list | MWB | v1 |
| Lock cursor to current screen (hotkey + tray) | Synergy, Deskflow | v1 |
| Hotkeys: jump to computer N, next/previous, move in a direction | MWB, Synergy, Input Director, Multiplicity | v1 |
| **Relative mouse mode** (for games, odd DPI setups) | MWB, Deskflow | v1 |
| **Draw a cursor on machines with no mouse plugged in** (Windows hides it otherwise — common on servers) | MWB | v1 |
| Hide the server's cursor at the edge while controlling a client | MWB | v1 |
| Full keyboard incl. multimedia keys, function keys, international layouts | ShareMouse, Across, all | v1 |
| **Cmd↔Ctrl and other modifier translation** Mac↔Win, per client | CursorHop, Synergy, Deskflow | v1 |
| Per-client mouse speed, scroll speed, scroll direction | ShareMouse | v1 |
| High-resolution and smooth trackpad scrolling | Across, Universal Control | v1 |
| **Caps/Num/Scroll Lock LED sync** | Input Director | v1 |
| Any stuck key released when a link drops | Lan Mouse (roadmap), all | v1 |
| **Broadcast mode** — type into several/all computers at once | MWB, Input Director | v1.x |
| Per-client key remapping (any key → any key) | Input Director | v1.x |
| Keyboard macros (record/play) | Input Director | Later |
| Trackpad gestures (swipe, pinch) Mac→Mac | Across, Universal Control | Later (experimental) |
| Use *any* computer's own keyboard/mouse to control the others | ShareMouse, Cursr | Local input on a client always works; **full two-way control: No** (you chose one server) |

**Clipboard & files**

| Feature | Seen in | Release |
|---|---|---|
| Text, rich text (RTF/HTML), images | All major apps | v1 |
| **Files and folders, any size, many at once** (MWB: 1 file ≤ 100 MB) | ShareMouse, Logitech Flow, CursorHop | v1 |
| Cut & paste files | ShareMouse, Flow | v1 |
| **Clipboard goes only where you paste** (doesn't overwrite every machine) | ShareMouse | v1 (lazy transfer) |
| Drag-and-drop files between screens | Universal Control, ShareMouse, MWB, Synergy 3, CursorHop | v1.x |
| Transfer speed at full network speed (CursorHop ≈ 70 Mbps) | — | v1 (target ≥ 90% of link) |
| Clipboard history | ShareMouse | v1.x |
| Shared folders / network drive | Across | **No** (Finder/Explorer copy-paste covers it) |

**Connection, discovery & status**

| Feature | Seen in | Release |
|---|---|---|
| Auto-discovery (mDNS) | ShareMouse, CursorHop, ShareCursor, Deskflow | v1 |
| Manual IP / hostname connect | All | v1 |
| **Detailed status colours** (resolving, connecting, connected, timeout, error) | MWB | v1 (§6.7) |
| Online/offline + "back online" notifications | Input Director, MWB | v1 |
| **"Reconnect all" button + hotkey** | MWB | v1 |
| Latency / bandwidth display | Lan Mouse (roadmap), Diagnostics | v1 |
| **"Same subnet only"** option | MWB | v1 |
| IP / subnet / hostname allow-list | Input Director | v1 |
| One-click **firewall rule repair** | MWB | v1 |
| Wake-on-LAN | Input Director, ShareMouse, Multiplicity | v1.x |
| Compatible with Deskflow/Synergy clients (e.g. to add Linux later) | Deskflow | Later |

**System sync & power**

| Feature | Seen in | Release |
|---|---|---|
| **Lock all computers** at once (hotkey) | MWB, Input Director, ShareMouse | v1.x |
| Screensaver sync / **stop screensavers on other machines** | ShareMouse, MWB, Input Director | v1.x |
| **Shut down / sleep / hibernate all** together | Input Director | v1.x |
| Control Windows **lock screen, login screen, UAC prompts, Ctrl+Alt+Del** (SYSTEM service) | MWB service mode, ShareMouse, Synergy | v1.x (Phase 6) |
| Fast User Switching (follow the active Windows user) | ShareMouse, Input Director | v1.x (Phase 6) |

**Settings, profiles & management**

| Feature | Seen in | Release |
|---|---|---|
| Export/import settings | Input Director (command line) | v1 |
| Profiles (named setting sets) | Input Director, ShareMouse | v1.x |
| **Auto-switch profile by network** (office vs home) | ShareMouse | v1.x |
| **Auto-load layout when the monitor setup changes** | Cursr | v1.x |
| **Lock client settings** so only the server can change them | Input Director (admin-only config) | v1.x |
| **Identify screens** — big name overlay on every computer | Input Director | v1 |
| Cursor locator / ripple on arrival | Input Director, Universal Control | v1 |
| Dim inactive screens | ShareMouse | v1.x |
| Command-line tool (status, switch, reconnect, apply layout) | Input Director, Lan Mouse | v1.x |
| Portable mode (run from USB, no install) | ShareMouse | **No** (you want a normal installer) |

**Beyond keyboard/mouse (out of scope)**

| Feature | Seen in | Release |
|---|---|---|
| Audio sharing | Multiplicity | Later |
| Screen streaming / use a laptop as a monitor | Multiplicity, Across, GiMeSpace | **No** (different product) |
| Bluetooth HID emulation; Android/iOS clients | Across, InputShare | **No** (Mac + Windows only) |

#### 3.4.3 Where Glidedesk beats all of them

1. **Files and clipboard of any size**, many files and folders, at full network speed. MWB is limited to one 100 MB file, Across to 1 KB of text in some modes, and Lan Mouse has none.
2. **Choose network interfaces and exact IPs** (one, several, all). No competitor offers this.
3. **Live Online/Offline tracking** with "last seen", latency, and the cursor refusing to enter an offline machine.
4. **Uninstall that asks to keep the configuration**, and never leaves a service running (Synergy's macOS 26 bug).
5. **Modern and maintained**: Rust + QUIC. Barrier is unmaintained and Input Leap was archived in July 2026.
6. **Mac + Windows + Windows Server 2016+**. The strongest Windows-only feature sets (MWB, Input Director, Multiplicity) become available on the Mac too.

Sources: [Synergy](https://symless.com/synergy), [Synergy 3.6.0 notes](https://symless.com/synergy/download/release-notes/43), [ShareMouse features](https://www.sharemouse.com/features/), [Multiplicity 4](https://www.stardock.com/edgerunner), [Input Director](https://www.inputdirector.com/), [Mouse Without Borders docs](https://learn.microsoft.com/en-us/windows/powertoys/mouse-without-borders), [Logitech Flow](https://www.logitech.com/en-us/software/flow), [Universal Control](https://support.apple.com/en-us/102459), [Cursr](https://github.com/bitgapp/Cursr), [CursorHop](https://cursorhop.com/), [across](https://www.acrosscenter.com/), [Deskflow](https://github.com/deskflow/deskflow), [Lan Mouse](https://github.com/feschber/lan-mouse), [ShareCursor](https://sharecursor.com/), [ShareCursor comparison](https://sharecursor.com/synergy-alternatives.html), [AlternativeTo list](https://alternativeto.net/software/synergy/).

### 3.3 Later / nice-to-have (not in v1)

- Linux support (Wayland via libei / X11). Architecture keeps this possible.
- Control from the Windows login / UAC secure desktop and macOS login window (needs SYSTEM service / LaunchDaemon — see §11 risks).
- Audio sharing, trackpad gesture forwarding, keyboard macros.
- Compatibility with Deskflow/Synergy clients.

---

## 4. Architecture

### 4.1 Process model

```
┌──────────────────────────── one machine ────────────────────────────┐
│                                                                      │
│   UI app (Tauri 2)  ◄── local IPC (Unix socket / named pipe) ──►  Daemon (Rust)
│   - tray/menu bar                                                 - input capture / inject
│   - settings, layout editor                                       - network (QUIC)
│   - transfer centre                                               - clipboard + file transfer
│                                                                   - discovery (mDNS)
│                                                                   - config store
└──────────────────────────────────────────────────────────────────────┘
                 ▲                                         ▲
                 │  QUIC over UDP (encrypted), LAN only    │
                 ▼                                         ▼
           other machines (same app, server or client role)
```

- **Same binary set on every machine.** Role is a setting.
- The **daemon** is separate from the UI so input sharing keeps working when the window is closed, and a UI crash never drops input.
- Local IPC is restricted to the current user (socket file mode `0600` on macOS; named pipe with a per-user security descriptor on Windows).
- The **tray/menu bar lives in the small UI process**. The settings window (webview) is created only when opened and **destroyed when closed**, so nothing heavy stays in memory while the app sits in the tray (§6.10).

### 4.2 Code layout (Cargo workspace + UI)

```
glidedesk/
├─ Cargo.toml                    # workspace, Rust 2024 edition
├─ crates/
│  ├─ proto/        # wire messages, versioning, (de)serialisation
│  ├─ net/          # QUIC transport, interface enumeration, binding, mDNS discovery
│  ├─ layout/       # screen graph, edge mapping, DPI/coordinate maths (pure, heavily tested)
│  ├─ input/        # trait InputCapture + InputInjector
│  │  ├─ macos/     # CGEventTap capture, CGEventPost inject
│  │  └─ windows/   # Raw Input + low-level hooks capture, SendInput inject
│  ├─ clipboard/    # trait + macOS NSPasteboard / Windows OLE clipboard, lazy formats
│  ├─ transfer/     # chunked file/clipboard streaming, hashing, resume, temp store
│  ├─ config/       # schema, load/save, migration, export/import, secure paths
│  ├─ ipc/          # daemon <-> UI protocol
│  ├─ platform/     # permissions checks, autostart, power/lock events, WoL
│  └─ daemon/       # the background binary; wires everything together
├─ app/             # Tauri 2 shell (Rust side) + UI
│  └─ ui/           # React 19 + TypeScript + Vite
├─ installer/
│  ├─ macos/        # pkg scripts, uninstaller app, LaunchAgent plist
│  └─ windows/      # NSIS templates/hooks (install + uninstall pages)
├─ docker/         # glidedesk-builder Dockerfile, compose file, SDK mount notes
├─ xtask/          # build/package automation (macOS .app/.dmg/.pkg in Docker, release)
├─ Makefile        # make test / build-win / build-mac / release — all run in Docker
├─ docs/           # incl. TESTING.md real-device checklist
└─ .github/workflows/  # CI: build, test, package, private release
```

### 4.3 Data flow — input

1. Server daemon captures every input event at OS level.
2. `layout` checks: is the cursor at an edge with a neighbour? Are switch guards met?
3. If switching: server **hides and freezes** its own cursor (keeps it captured), sends `Enter{screen, x, y}` to the client, then streams events.
4. Mouse moves go as **QUIC datagrams** (lowest latency; a lost move is replaced by the next one). Keys, buttons and scroll go on an **ordered reliable stream** (never lose a key-up).
5. Client injects events. When the cursor hits a client edge, client reports `Leave{edge, pos}` and the server moves control on.
6. Safety: if the connection drops while a client is active, the server immediately gets its cursor back and sends/release-all keys so no key is left "stuck".

### 4.4 Data flow — clipboard & files (lazy)

1. On copy, the owning machine sends only an **announcement**: formats available, sizes, file list (names, sizes, tree) — not the data.
2. The other side puts **promised / delayed-render** entries on its own clipboard.
3. Only when the user **pastes**, the data is pulled over a dedicated QUIC stream, chunked, streamed to disk (files) or memory (small text), verified with **BLAKE3**.
4. This is what makes "any size" possible without freezing or copying gigabytes that are never pasted.

---

## 5. Technology choices (latest stable at start of work — no legacy stacks)

| Area | Choice | Why |
|---|---|---|
| Core language | **Rust (2024 edition, latest stable)** | Memory-safe, native speed, first-class macOS + Windows APIs, tiny binaries. Same choice as modern lan-mouse. |
| Async runtime | **Tokio** | Standard, mature |
| Transport | **QUIC via `quinn`** (TLS 1.3 via `rustls`) | Built-in encryption, multiplexed streams (input, control, clipboard, files never block each other), unreliable datagrams for mouse motion, fast reconnection/connection migration |
| Serialisation | **`postcard` + `serde`** with explicit protocol version | Compact, fast, no-std friendly; versioned envelope for compatibility |
| Compression | **zstd** (adaptive; skipped for already-compressed files) | Best speed/ratio |
| Hashing | **BLAKE3** | Very fast integrity checks, supports incremental/resume |
| Discovery | **mDNS/DNS-SD (`mdns-sd`)** | Zero-config LAN discovery, restricted to selected interfaces |
| Interface enumeration | `netdev` / `if-addrs` + native APIs for friendly names | Lists name, type (Wi-Fi/Ethernet/VPN), IPv4/IPv6 |
| macOS input | `CGEventTap` (capture), `CGEventPost` / `CGEventCreate*` (inject) via `objc2` / `core-graphics` crates | Current Apple APIs |
| Windows input | **Raw Input** + `WH_KEYBOARD_LL`/`WH_MOUSE_LL` (capture/block), **`SendInput`** (inject) via the official **`windows` crate** | Microsoft's current Rust bindings |
| macOS clipboard/files | `NSPasteboard` + `NSPasteboardItemDataProvider` (lazy) + **`NSFilePromiseProvider`** (lazy files) via `objc2` | Native lazy paste; Finder shows real copy progress |
| Windows clipboard/files | OLE clipboard with **delayed rendering** + **`CFSTR_FILEDESCRIPTORW` / `CFSTR_FILECONTENTS` virtual files (IStream)** | Explorer pulls file data only on paste, any size, with native progress |
| UI shell | **Tauri 2** | Native webview, small footprint, built-in tray, updater, autostart, single-instance plugins |
| UI | **React 19 + TypeScript (strict) + Vite + Tailwind CSS v4** + a headless accessible component library (Radix-based) | Modern, typed, accessible |
| Layout editor canvas | `dnd-kit` or plain pointer events on SVG | Accessible drag with keyboard support |
| Config format | **TOML** (human-readable) + JSON schema for validation | Easy to export/share/edit |
| Logging | `tracing` + rotating files | Structured logs, exportable |
| Tests | `cargo nextest`, `proptest` (layout maths, protocol), `insta` snapshots, Vitest + Playwright for UI | |
| Quality gates | `clippy -D warnings`, `rustfmt`, `cargo-deny` (advisories, banned crates), `cargo-audit`, ESLint + TypeScript strict | |
| CI/CD | **GitHub Actions** matrix: `macos-latest` (arm64 + x86_64 → universal), `windows-latest` (x64 + ARM64); private repository | |
| Signing | **No paid certificates.** macOS: ad-hoc signature (`codesign -s -`, required to run on Apple Silicon). Windows: unsigned. | Private app: first-launch Gatekeeper / SmartScreen warnings are accepted (§8) |

Supported OS (confirmed):

- **macOS 13 Ventura or later** (Apple Silicon and Intel)
- **Windows 10 22H2, Windows 11** (x64 and ARM64)
- **Windows Server 2016 or later** (2016 / 2019 / 2022 / 2025, x64). Server 2016/2019 do not ship the WebView2 runtime, so the installer includes it (§8.2). Server Core (no desktop) is not supported — the app needs a desktop session.

---

## 6. Feature design details

### 6.1 Client placement (server settings) — R3

- Layout editor shows the server in the centre; clients appear as tiles (auto-discovered ones in a "waiting" tray).
- Drag a tile to any side of any other tile: **left, right, top, bottom**, including diagonal chains (A→B→C) and multiple rows.
- Each tile shows its real monitors (sizes, arrangement read from the client), so edges line up per monitor.
- **Offset slider** per link (e.g. client covers only the lower 60% of the server's right edge).
- Keyboard-accessible: select tile, arrow keys to move, `Enter` to confirm.
- Validation: no overlaps; warns on unreachable screens.
- Changes apply live; stored in config (§6.5).

#### 6.1.1 Multi-monitor handover — which monitor leads to the client (R21)

When the server (or a client) has **two or more monitors**, each link to a neighbour has a **handover monitor** setting:

| Mode | What happens | Example: server has Monitor 1 (top) and Monitor 2 (bottom) stacked, client placed on the **right** |
|---|---|---|
| **Single monitor** | Only the chosen monitor's outer edge on that side leads to the client. Other monitors' edges stay walls. | Only Monitor 2's right edge goes to the client |
| **Selected monitors** | Tick any set of monitors; each ticked monitor's outer edge on that side leads to the client. | Monitors 1 and 2 both lead to the client (same as All here) |
| **All monitors (whole side)** | Every monitor edge that faces the client's side and is on the **outside of the desktop** leads to the client. Default. | Right edges of both monitors go to the client |

- Only **outer** edges can hand over. An edge that touches another of the server's own monitors always moves between those monitors, so it can never hand over.
- **How positions map** when several monitor edges lead to one client:
  - **Continuous** (default): the ticked edges are treated as one long edge. Top of Monitor 1 → top of the client, bottom of Monitor 2 → bottom of the client.
  - **Per monitor**: each monitor's edge maps to the full height (or width) of the client, so any monitor lands anywhere on the client.
- **Same on the client side**: when a client has several monitors, pick which of its monitors the cursor **enters on** and **leaves from** when going back (single / selected / all).
- **In the layout editor**: open a computer tile to show its monitors. Click the edge segments to turn them on or off; active segments are highlighted in that client's colour. A side panel offers the same choice as a radio list (Single / Selected / All) for keyboard users.
- The **Return path** follows the same rule automatically. Leaving the client toward the server lands on the monitor the cursor came from, or on the matching monitor in Continuous mode.
- **Monitor changes** (plug/unplug, resolution change, laptop lid closed): monitors are recognised by a stable ID (display serial/EDID where available, else position and size). If a chosen monitor disappears, the link falls back to **All monitors** until it comes back, and a warning appears in the tray.
- Works together with offsets and edge segments (§3.4.2): e.g. only the lower half of Monitor 2's right edge goes to the client.
- Stored per link in `config.toml`:
  ```toml
  [[layout.link]]
  from = "server"          # this computer
  to = "office-pc"
  side = "right"
  handover = "selected"    # single | selected | all
  monitors = ["mon:DELL-U2723QE:5CD1234", "mon:builtin"]
  mapping = "continuous"   # continuous | per-monitor
  offset = { start = 0.0, end = 1.0 }
  ```

### 6.2 Network interface & IP selection — R8

Server settings → *Network*:

- List of all interfaces: friendly name, type (Ethernet / Wi-Fi / VPN / virtual), status, and each IPv4/IPv6 address.
- Three modes:
  - **All** — listen on all interfaces (`0.0.0.0` + `::`), including ones added later.
  - **Selected interfaces** — tick one or more interfaces; all their addresses are used, and follow address changes (DHCP).
  - **Selected IPs** — tick exact addresses (one or many).
- Port: default fixed port (e.g. `24800` family, final number TBD), editable.
- mDNS announcements go out **only** on selected interfaces.
- Live status per binding (listening / error: address in use / interface down), with auto-retry when an interface comes back.
- Client side: optional "use this interface to reach the server" selector.
- Optional **allow-list / block-list of client IPs or subnets** (not authentication — just a filter, off by default).

### 6.2.1 Implementation note (2026-09-25) — clipboard & files as built

The lazy / promised-data design below was replaced during Phase 3–4 by a simpler model that works the same way on macOS and Windows (the approach Synergy and Mouse Without Borders use):

- **The clipboard follows the cursor.** When control moves to another computer, the side whose clipboard changed sends it on its own QUIC stream (input is never delayed). Echo back to the sender is suppressed.
- **Files and folders of any size** are streamed straight to disk into a private staging folder (bounded memory, BLAKE3 per file, names sanitised, paths cannot escape). Only when every file is verified are they put on that computer's clipboard as real files, so Ctrl+V / Cmd+V in Explorer or Finder pastes them. A paste can never see a partial file.
- **Cut & paste:** Windows marks cut files; the receiver's clipboard gets them as "move". When the user pastes (the staged copy is moved away), the receiver tells the sender, which moves the originals to the Recycle Bin / Trash — only if they are unchanged. Finder's ⌥⌘V (move) works the same way.
- Not done yet: resuming an interrupted file transfer (it restarts on the next switch), and drag-and-drop between screens (v1.x).

### 6.3 Clipboard — R5, R7

- Global on/off switch (tray + settings); optional per-client on/off; optional direction (server→clients, clients→server, both).
- Formats: plain text, RTF, HTML, images (PNG/TIFF/DIB converted as needed), file lists (handled by §6.4).
- **Any size**: lazy/promised transfer (§4.4). Small items (< ~1 MB, configurable) are pushed eagerly for instant paste; large ones are pulled on paste.
- Optional max-size limit (off by default = unlimited).
- Clipboard is synced when the cursor moves to another screen (like Synergy) **and** on change, configurable.

### 6.4 Files — copy / cut / paste, any size — R6, R7

- On/off switch (separate from clipboard).
- **Copy**: user copies files/folders in Finder/Explorer → paste on the other machine in Finder/Explorer → OS-native progress window; data streams on demand.
  - macOS target: `NSFilePromiseProvider`.
  - Windows target: virtual-file clipboard formats with `IStream`.
- **Cut**: follows each OS's normal rule:
  - **Windows** marks cut with `Preferred DropEffect = MOVE`; after the destination confirms a verified, complete transfer, the source deletes the originals (sent to Recycle Bin, never permanent delete).
  - **macOS** Finder has no real "cut" — it uses Copy then **⌥⌘V** "Move Item Here". We detect move-paste on a Mac target and, after verified transfer, move the source files to Trash on the source machine.
  - Deletion **only after hash-verified success**; any error = originals untouched.
- **Any size**: 64 MiB-windowed chunk streaming, straight to disk (never full file in RAM), BLAKE3 per chunk + whole file, zstd where it helps, **resume after disconnect**, multiple files in parallel streams.
- Keeps folder structure, timestamps, and (where possible) permissions; handles names illegal on the other OS (e.g. `:` on Windows) by safe renaming with a notice.
- Transfer centre in UI: progress, speed, ETA, pause/cancel/retry.
- Drag-and-drop across screens (v1.x): drag a file off the server edge, drop on client desktop/window.

### 6.5 Configuration storage, export, import — R4

**Location (per user, protected):**

| OS | Path | Protection |
|---|---|---|
| macOS | `~/Library/Application Support/<AppId>/config.toml` | Folder `0700`, file `0600`; atomic write (temp file + rename); anything sensitive (e.g. a future device identity key) in the **macOS Keychain** |
| Windows | `%APPDATA%\<AppName>\config.toml` | Folder ACL limited to the user + SYSTEM; atomic write; sensitive values protected with **DPAPI** (user scope) |

- Machine-wide defaults (optional, for IT): macOS `/Library/Application Support/<AppId>/`, Windows `%ProgramData%\<AppName>\`.
- Config has a `schema_version`; automatic migration on upgrade; last 5 versions kept as backups.
- **Export**: Settings → *Export configuration…* → save a `.glidedesk.toml` file anywhere (choose: full config, or layout only). Machine-specific items (e.g. interface IDs) are marked so import on another machine can map or skip them.
- **Import**: validate against schema, show a diff/summary, confirm, apply. Invalid files are rejected with a clear message.
- UI "Reset to defaults" (keeps a backup).

### 6.6 Permissions and first-run

- macOS needs **Accessibility** and **Input Monitoring** permission. First-run wizard checks each, deep-links to the correct System Settings pane, and re-checks live.
- macOS local network privacy prompt (Sonoma+) handled with a proper `NSLocalNetworkUsageDescription` and Bonjour service types in `Info.plist`.
- Windows: firewall rule added by installer for the selected port (UDP) on Private networks; wizard warns if the current network is marked Public.
- Windows elevated windows (UIPI): daemon runs with `uiAccess` or as a helper service so the mouse/keyboard also work over admin windows (details in Phase 6).

---

### 6.7 Client health monitoring & offline status — R15

- The server **remembers every client it has ever been set up with** (stored in config), so a client that is off still appears in the list and the layout — marked **Offline** — instead of disappearing.
- **Heartbeat**: the server sends a ping on the control stream every **1 s** (adjustable 0.5–10 s). The client answers with a pong plus a small status report (screen setup, locked/unlocked, asleep soon, RDP session disconnected, app version).
- **Offline rule**: 3 missed heartbeats in a row (adjustable) → **Offline**. A closed connection or an mDNS "goodbye" marks it Offline **immediately**. QUIC idle timeout (10 s) is the last safety net.
- **States** shown everywhere (tray, client list, layout tiles):

  | State | Meaning |
  |---|---|
  | 🟢 Online | Connected, healthy; shows latency (ms) |
  | 🟡 Degraded | Connected, but high latency or packet loss (thresholds adjustable) |
  | 🔒 Locked / Asleep | Connected, but the screen is locked or the machine is going to sleep |
  | 🔄 Connecting / Reconnecting | Finding the address, connecting or handshaking (shown as a detail line), or trying again after a drop |
  | ⚪ Offline | Not reachable; shows **"last seen"** time |
  | — Never connected | Added to the layout but has not connected yet |

- The cursor **never enters an Offline client** — its edge acts like a wall until it comes back.
- If the active client goes offline while the user is on it, control snaps back to the server instantly and all keys are released (§4.3).
- Optional notifications: "Office-PC went offline" / "back online" (per client, off/on).
- Clients reconnect on their own (backoff 0.5 s → max 5 s). "Wake" button in the tray/client list sends Wake-on-LAN to an offline client.
- The **client** also shows the server's state (Connected / Searching / Offline) in its own tray.
- Heartbeat history (latency graph, disconnect log) is in the Diagnostics page.

### 6.8 System tray / menu bar — R16

Same menu on **macOS (menu bar)** and **Windows (notification area)**, native menus (no webview needed to show it).

**Server menu:**
```
Glidedesk — Server · Sharing (3 of 4 online)
─────────────────────────────────────────
Computers                              ▸  🟢 Office-PC      2 ms
                                          🟢 MacBook-Air    4 ms
                                          🔒 Studio-Mac     locked
                                          ⚪ Lab-Server     offline · last seen 14:02  [Wake]
─────────────────────────────────────────
✓ Share clipboard
✓ Share files
  Lock cursor to this screen          ⌃⌥L
─────────────────────────────────────────
■ Stop sharing        (becomes ▶ Start sharing)
↻ Restart Glidedesk
⟳ Reconnect all
─────────────────────────────────────────
Open Glidedesk…
Transfers…
Settings…
─────────────────────────────────────────
Quit Glidedesk
```

**Client menu:** status line (Connected to *Office-Mac* · 2 ms / Searching / Offline), server address, the same clipboard/files toggles (local permission), Stop / Start, Restart, Open, Settings, Quit.

| Action | What it does |
|---|---|
| **Stop sharing** | Releases all keys, returns the cursor, closes network connections and stops listening. The daemon stays alive (idle, ~0% CPU) so **Start** is instant. Clients show the server as Offline. |
| **Start sharing** | Re-binds the selected interfaces, resumes discovery, clients reconnect. |
| **Restart** | Fully restarts the background daemon (fresh config load, re-bind network, re-check permissions). Used after changes or to recover from errors. |
| **Quit** | Stops the daemon and the tray app completely. Optional confirmation ("Input sharing will stop"). Autostart still starts it again at next login if enabled. |

- **Tray icon states**: sharing / stopped / warning (a client is Offline or Degraded) / error (e.g. permission missing, port in use). Follows light/dark menu bar and Windows taskbar theme.
- Windows: single-click opens the menu, double-click opens the main window. macOS: click opens the menu.
- Every tray action also has an optional global hotkey.

### 6.9 Customisation model (server & client) — R14

**Everything that has an option is a setting** — in the UI, in `config.toml`, and exportable. Settings page has a **search box**.

Three scopes:

| Scope | Where edited | Examples |
|---|---|---|
| **Server global** | Server app | Network interfaces/IPs/port, layout, hotkeys, switch guards (delay, double-tap, dead corners), clipboard/files defaults, health intervals & thresholds, notifications, dimming, cursor locator, theme, language, logging |
| **Per-client (on server)** | Server app → Computers → client | Placement & offset, modifier remap, mouse speed, scroll direction/speed, clipboard/files allowed + direction, lock/sleep sync, Wake-on-LAN MAC, notifications for this client |
| **Client local** | Client app | Server address (auto/manual), network interface to use, file receive folder, accept clipboard/files (local veto), local overrides for scroll/speed/key remap, start at login, tray, notifications, theme, language, logging |

Rules when both sides set the same thing:
- **Privacy toggles (clipboard, files)** — *both* sides must allow it; the stricter one wins.
- **Feel settings (speed, scroll, key remap)** — client local override › server per-client › server global default.
- The UI shows the **effective value and where it comes from** ("Set by server" / "Overridden on this computer"), with a reset button.
- Settings apply **live** — no restart needed (except port/interface changes, which re-bind automatically).
- Profiles: save/load named setting sets (e.g. "Office", "Gaming") via export/import (§6.5).

### 6.10 Lightweight by design — R13

- **Rust daemon, event-driven**: no busy loops, no polling except the heartbeat; idle when nothing moves. Client does nothing until it receives input.
- **No Electron, no Node.js at runtime.** UI uses the OS's own webview (WKWebView / WebView2) through Tauri — and only while a window is open.
- Release builds: LTO, `codegen-units = 1`, stripped symbols, size-optimised UI bundle (tree-shaken, no heavy UI frameworks beyond React + headless components).
- File/clipboard transfer uses a **fixed memory budget** (bounded buffers, streamed to disk), so a 100 GB copy uses the same RAM as a 1 MB one.
- Logs rotate (default 5 × 5 MB).

**Budgets (checked in CI/perf tests):**

| Metric | Target |
|---|---|
| Daemon RAM idle / active | < 20 MB / < 40 MB |
| Tray process with no window open | < 30 MB |
| CPU idle | ≈ 0% (< 0.2%) |
| CPU while moving the mouse across | < 2% of one core |
| Input latency p95 | < 5 ms wired, < 15 ms Wi-Fi |
| File transfer speed | ≥ 90% of link speed |
| macOS `.dmg` / Windows standard installer | < 15 MB / < 10 MB |
| Cold start to tray ready | < 1 s |

### 6.11 UI design — R17

Designed by taking the best parts of leading paid and free apps:

| App | Paid/Free | What we take |
|---|---|---|
| Apple Universal Control / macOS Displays settings | Free (built-in) | Drag-to-arrange canvas with real monitor shapes; cursor "pushes through" edge feel |
| Synergy 3 | Paid | Clean computer cards with status; one-click "add" of discovered computers |
| ShareMouse | Paid | Tray-first design, per-client settings, screen dimming, visible transfer progress |
| Stardock Multiplicity | Paid | Per-computer quick actions, hotkeys to jump to a machine |
| Logitech Flow | Free | Simple onboarding, cursor locator animation |
| Mouse Without Borders (PowerToys) | Free | Well-grouped settings page with short descriptions under each toggle (Fluent style) |
| Deskflow / Input Leap / Barrier | Free (open source) | Nothing hidden — every advanced option available under "Advanced" |
| lan-mouse | Free (open source) | Minimal, fast, modern Rust core |

**Look & feel**
- Native-feeling on each OS: macOS — SF Pro, sidebar with vibrancy, rounded controls. Windows — Fluent 2, Segoe UI Variable, **Mica** background (Windows 11; plain on 10/Server).
- Follows system light/dark mode and accent colour; manual override available.
- Keyboard navigable, screen-reader labels, WCAG 2.2 AA contrast.

**Main window (server)**
```
┌──────────────┬─────────────────────────────────────────────────────┐
│ ● Sharing    │  Layout                               [■ Stop] [↻] │
│              │  ┌────────┐┌──────────────┐┌────────┐               │
│ Layout       │  │MacBook ││   THIS MAC   ││Office- │               │
│ Computers  4 │  │ 🟢 4ms  ││   (server)   ││PC 🟢2ms│               │
│ Clipboard &  │  └────────┘└──────────────┘└────────┘               │
│   Files      │             ┌──────────────┐                        │
│ Keyboard &   │             │ Lab-Server ⚪ │  ← greyed, offline      │
│   Mouse      │             └──────────────┘                        │
│ Network      │  Waiting to be placed:  [Studio-Mac 🔒]  (drag in)  │
│ Transfers    │─────────────────────────────────────────────────────│
│ General      │  Clipboard ✓   Files ✓   3/4 online   0 transfers   │
│ Advanced     │                                                     │
│ 🔍 Search    │                                                     │
└──────────────┴─────────────────────────────────────────────────────┘
```

**Pages**: Layout (home) · Computers (list: status, latency, last seen, OS, version; click → per-client settings drawer) · Clipboard & Files · Keyboard & Mouse (hotkeys, switch guards, remaps, speed) · Network (interfaces/IPs, port, allow-list) · Transfers · General (startup, tray, notifications, theme, language) · Advanced (health check timings, logs, diagnostics, export/import, reset).
**Client window**: smaller — connection card (server, status, latency), local permissions, local overrides, General, Advanced.
**First-run wizard**: 1) Server or Client → 2) Permissions (macOS) / firewall (Windows) → 3) Network → 4) Place computers (server) or pick server (client).

The real UI was built directly (no separate prototype round) so you can review the working app; changes are cheap to make.

## 7. Security model — R9 (v3: optional password, see §14.1)

- **No password, no pairing code, no account.** Any client that can reach the server can connect.
- Traffic is still **encrypted** (QUIC/TLS 1.3 with self-generated certificates, not verified). This stops passive sniffing of keystrokes on the LAN at zero setup cost. It does **not** stop an active attacker on the same network — that is the accepted trade-off of "no auth".
- Mitigations kept by default, all without passwords:
  - Listen **only on selected interfaces** (§6.2); never on interfaces marked public unless the user opts in.
  - Discovery limited to the local subnet.
  - Optional IP allow-list.
  - UI shows every connected client with a **Disconnect / Block** button, and a notification when a new client connects.
- Code rule: protocol parser is fuzzed (`cargo-fuzz`) so bad packets cannot crash or exploit the daemon.
- *Recommendation (optional, not required):* a one-click "Trust this device" toggle later, if the user ever wants it. Off by default.

---

## 8. Install / uninstall — R11, R12

### 8.1 macOS

- Deliverable: ad-hoc-signed **universal `.dmg`** containing `Glidedesk.app` (drag to Applications), plus a **`.pkg`** for scripted installs.
- First launch of an un-notarised app: macOS 13–14 right-click → **Open**; macOS 15+ **System Settings → Privacy & Security → Open Anyway** (or `xattr -dr com.apple.quarantine /Applications/Glidedesk.app`). The install guide documents this.
- Note: macOS ties Accessibility / Input Monitoring permission to the code signature. With ad-hoc signing, permission may need re-granting after an update; the first-run wizard detects this and guides the user. (A self-signed local certificate kept stable across builds avoids it — Phase 6 will use one.)
- Background daemon registered with **`SMAppService`** (modern login item / LaunchAgent API, macOS 13+) — no manual plist copying.
- **Upgrade (R22)**: the `.pkg` installs over the existing app (same bundle ID), its `preinstall` script asks the running app to quit cleanly, and `postinstall` restarts it for the logged-in user. Dragging a new `.app` from the DMG replaces the old one; on next launch the app notices the version change, migrates the config and restarts its daemon. Config is never touched by an upgrade.

### 8.2 Windows

- Deliverable: **NSIS installer (`.exe`)** from our own NSIS script (built in Docker with `makensis`), x64 and ARM64. Tauri is used as a library; its bundler is not needed.
- Unsigned: SmartScreen shows "Windows protected your PC" on first run → **More info → Run anyway**. Documented in the install guide.
- **WebView2 runtime** bundled (offline installer) so it installs on Windows Server 2016/2019 and machines without internet.
- Windows Server notes: works in a normal console or RDP desktop session. Input injection does not work in a disconnected/minimised RDP session (Windows limitation) — shown as a warning in the client UI.
- Appears in **Settings → Apps / Add or Remove Programs** with icon, version, publisher, uninstall.
- Installs daemon + UI, Start-menu shortcut, optional desktop shortcut, firewall rule, autostart entry.

- Two Windows installers: **Standard** (small; installs WebView2 only if missing, needs internet for that) and **Offline** (bundles the WebView2 runtime; for servers without internet). Both support silent install `/S` for scripted deployment, so the MSI is dropped.

#### 8.2.1 Install vs upgrade — one installer does the right thing (R22)

The same `Glidedesk_<ver>_x64-setup.exe` is used for a new install **and** for updating. It is our own NSIS script (full control, built in Docker) with a fixed product ID, so Windows always sees one product.

On start the installer reads `HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Glidedesk` (`DisplayVersion`, `InstallLocation`) and picks a mode:

| Found | Mode | What happens |
|---|---|---|
| Nothing | **Install** | Normal wizard: location, shortcuts, start at login, firewall rule |
| Older version | **Upgrade** | Short page "Upgrade Glidedesk 1.2 → 1.3". Same folder and options as before, no questions about settings |
| Same version | **Repair** | Re-copies files, re-creates shortcuts, firewall rule and autostart |
| Newer version | **Downgrade blocked** | Explains and stops. `/ALLOWDOWNGRADE` overrides |
| Different architecture (x64 ↔ ARM64) | **Replace** | Removes the other build (keeping config), then installs |

**Upgrade steps (safe order):**
1. Ask the running app to quit through its local control channel (releases keys, closes connections cleanly); wait up to 10 s; only then force-close leftover processes.
2. Rename the current program folder to a backup (`<InstallDir>.old`).
3. Copy new files, update the registry entry (version, size, icon), shortcuts, firewall rule and autostart.
4. If any step fails → restore the backup folder and restart the old version (**automatic rollback**). If it succeeds → delete the backup.
5. Start the app again if it was running before (or always, after an interactive install).
6. On first start the app sees the new version, backs up `config.toml` and **migrates it** to the new schema (§6.5).

- Configuration and logs live in `%APPDATA%` / `%LOCALAPPDATA%`, never in the program folder, so upgrades never touch them.
- Silent use for servers and scripts: `setup.exe /S` installs or upgrades automatically; `/D=<path>` sets the folder on first install; `/NORESTART` stops the app from starting afterwards; exit code 0 = success, non-zero = failure (rolled back).
- Per-machine install in `C:\Program Files\Glidedesk` (needed for the firewall rule and to serve all users on servers); the installer asks for admin rights once.
- An upgrade never shows the "keep configuration?" question; it only appears on a real uninstall.
- The in-app updater (Phase 6) downloads the same installer and runs it with `/S`, so there is only one upgrade path to test.

### 8.3 Uninstall — "Keep your configuration?"

- **Windows**: the uninstaller shows a page: *"Keep your settings and layout for a future install?"* **[Keep] [Remove]** (default: Keep). Silent uninstall (`/S`) supports `/KEEPCONFIG=0|1`. It stops the daemon, removes firewall rule, autostart, files, and — if Remove — `%APPDATA%\<AppName>`, `%LOCALAPPDATA%\<AppName>`, and ProgramData defaults.
- **macOS** (no system uninstaller exists, so we provide one):
  - Menu → **"Uninstall Glidedesk…"** in the app, and an **Uninstall Glidedesk** helper inside the DMG / pkg.
  - Same dialog: Keep / Remove configuration. It unregisters the `SMAppService` item, stops the daemon, moves the app to Trash, removes caches/logs, and — if Remove — deletes `~/Library/Application Support/<AppId>`, preferences, and Keychain items.
  - Also offers to open System Settings to remove the Accessibility / Input Monitoring entries (macOS does not let apps remove these themselves; `tccutil reset` used where allowed).
  - Dragging the app to the Trash still works; the leftover config is then kept (safe default), and the in-app uninstaller can still be run later to remove it.

### 8.4 Builds and code tests in Docker (on this Mac) — R18, R19

**Every build and every automated code test runs inside a Docker container** on this Mac. Nothing needs to be installed on the Mac itself except Docker (already installed: Docker 29.8, Linux arm64 VM). The same container can run on any other machine or CI later.

**Container image `glidedesk-builder`** (Linux, pinned versions, `docker/Dockerfile`):
- Rust stable (2024 edition) + targets `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, and the Linux host target for tests.
- **Windows**: `cargo-xwin` (downloads the Windows SDK/CRT — Microsoft licence terms apply), `nsis` (makensis), `llvm` / `lld` / `llvm-rc`. Tauri officially supports building the Windows NSIS installer this way.
- **macOS**: `osxcross` toolchain with a **macOS SDK copied from this Mac's Xcode Command Line Tools** (Apple's licence allows the SDK only on Apple hardware — fine here because Docker runs on this Mac; the SDK is kept out of git and out of the image registry). `llvm-lipo` for universal binaries, **`rcodesign`** for the ad-hoc signature, `libdmg-hfsplus` for `.dmg`, `xar` + `bomutils` for `.pkg`.
- Node LTS + pnpm for the UI; Playwright browsers for UI tests.
- Cache volumes for `~/.cargo`, `target/`, pnpm store and the xwin SDK so rebuilds are fast.

**Commands (all run the container):**

| Command | Does |
|---|---|
| `make docker-image` | Builds the builder image (once, or when tools change) |
| `make test` | fmt check, clippy, all unit/property/integration tests, UI tests — inside Docker |
| `make build-win` | Windows x64 + ARM64 installers (standard + offline) |
| `make build-mac` | macOS universal `.app`, `.dmg`, `.pkg` |
| `make release` | Everything → `output-build/` |

Output in **`output-build/`**: `Glidedesk_<ver>_universal.dmg`, `Glidedesk_<ver>_universal.pkg`, `Glidedesk_<ver>_x64-setup.exe`, `_x64-offline-setup.exe`, `_arm64-setup.exe`, plus `SHA256SUMS`.

**Important details:**
- Tauri's own bundler only makes `.app`/`.dmg` on a macOS host. In the container the Rust binaries are cross-compiled with osxcross, and a small **`xtask` packager** (Rust, part of the repo) builds the `.app` folder (Info.plist, icons, entitlements, daemon + UI binaries), then signs and packages it. The UI files are embedded in the binary by Tauri at compile time, so nothing else is needed.
- **Fallback, only if macOS packaging in Docker ever fails**: the same `xtask` can run natively on the Mac for the final `.dmg` step. All compiling and testing still happens in Docker.
- **What the container can test**: all platform-independent code (protocol, layout maths, config, transfer, health checks, network with two daemons on loopback using mock input backends, UI with mocked IPC). The Windows and macOS code is **compile-checked and linted** for every target in the container.
  - Windows-specific unit tests are run under Wine where it works (x86_64 container through Docker's Rosetta emulation). This is best-effort; the Windows test machines are the real check.
- **What only real machines can test** (§10.2): real keyboard/mouse capture and injection, permissions, clipboard/file handover with Finder/Explorer, tray, installers.
- **Docker resources**: raise Docker's limits to **8 CPUs / 10 GB RAM** (currently 4 CPUs / 6 GB). Rust + Tauri builds are much slower with less.
- Limits: WiX MSI needs Windows (dropped — NSIS covers it). Builds are unsigned/ad-hoc (fine for this private app).

---

## 9. Delivery phases

Each phase ends with working, tested software on **both** OSes.

| Phase | Scope | Exit criteria |
|---|---|---|
| **0. Foundations** | `glidedesk-builder` Docker image + `make` targets (§8.4), workspace, lint/test gates in Docker, project Claude hooks (auto `cargo fmt`/`clippy` inside the container after edits), `proto` + `config` crates, logging, Graphify graph of the repo | `make release` produces Mac + Windows installers in `output-build/` from the container; `make test` passes in Docker; first installers start on a real Mac and Windows PC; config load/save/migrate/export/import tested |
| **1. Input + network MVP** | `input` (mac + win capture/inject) incl. every §3.4.2 **v1** input item (wrap, guards, relative mode, cursor drawing, LED sync, modifier translation), `net` (QUIC, interface/IP binding all/some/one), `layout` maths incl. multi-monitor handover (§6.1.1), heartbeat + Online/Offline tracking (§6.7), Stop/Start/Restart in daemon, headless daemon with TOML layout | Client going offline is shown within 3 s; mouse + keyboard cross between a Mac and a Windows PC both directions; no stuck keys on disconnect; latency < 5 ms p95 on wired LAN |
| **2. UI + layout editor** | Clickable HTML prototype → your approval (§6.11), Tauri app, tray/menu bar with Stop/Restart/Quit (§6.8), all settings pages + client/server scopes (§6.9), first-run wizard + permission checks, drag-and-drop layout editor, network settings page, IPC | Placement changes apply live; interface selection UI works; tray works on both OSes; tray-only RAM < 30 MB |
| **3. Clipboard** | Text/RTF/HTML/image, lazy transfer, on/off, direction, size limit | 1 GB image/text paste works without UI freeze; toggle respected instantly |
| **4. Files** | Copy + cut via Finder/Explorer, virtual files / file promises, resume, verify, transfer centre | 50 GB folder copy OK; cut deletes source only after verify; resume after cable pull |
| **5. Borrowed features** | Every §3.4.2 item marked **v1.x**: broadcast mode, key remap, drag-and-drop files, clipboard history, WoL, lock-all, screensaver/power sync, profiles + auto-switch, settings lock, dimming, CLI | Every v1.x row in §3.4.2 done on both OSes |
| **6. Packaging** | DMG/pkg + NSIS (standard + offline) with install/upgrade/repair/rollback (§8.2.1), all built in Docker, uninstall with keep-config prompt, auto-update, WebView2 bundling, UIPI/elevated window handling | Clean install → use → upgrade → uninstall (keep & remove) passes on fresh VMs of macOS 13+, Windows 10/11 and Windows Server 2016/2022 |
| **7. Hardening + release** | Fuzzing, security review, performance pass, accessibility audit, docs, 1.0 | No high/critical findings; release notes; 1.0 tagged |

---

## 10. Quality & testing plan

### 10.1 Automated tests — in Docker (R19)

- **TDD** for pure logic (layout maths incl. multi-monitor handover, protocol, config migration, transfer resume, health state machine) — target ≥ 80% coverage on these crates.
- **Property tests**: coordinate mapping across any resolution/DPI/monitor combination never produces off-screen positions; the return path always lands on a valid monitor.
- **Integration tests**: two or more daemons on loopback with mock input backends; offline detection timing; clipboard/file transfer with fault injection (drops, disconnects, disk full).
- **Fuzzing**: protocol decoder, config parser.
- **Cross-target checks**: `cargo check` + `clippy` for Windows x64/ARM64 and macOS arm64/x64 on every change.
- **UI**: Vitest (components), Playwright (screens against a mock daemon).
- **Performance**: benchmarks for encode/decode and transfer throughput; §6.10 budgets that can be measured in a container.

### 10.2 Real-device tests — on Mac and Windows (R20)

Run on real hardware (VMs only where noted) after each phase, using the installers built in Docker:

- **Machines**: this Mac (Apple Silicon, macOS 27) + at least one more Mac if possible (macOS 13 for the minimum); a Windows 11 PC; a Windows 10 22H2 machine or x64 VM; a **Windows Server 2016** and **2022/2025** machine or x64 VM; a Windows 11 ARM VM on this Mac for ARM64 builds.
- **`glidedesk selftest`** command (built into the app): checks permissions, injects and reads back test input, tests clipboard read/write, lists monitors and interfaces, and measures loopback latency. It writes a report file so real-machine checks are repeatable.
- **Scripted test checklist** (`docs/TESTING.md`) per phase: install → first-run → layout (incl. multi-monitor handover modes) → keyboard/mouse both directions → clipboard/files (small, 10 GB, folders, cut) → unplug network (offline within 3 s, no stuck keys) → tray Stop/Start/Restart/Quit → sleep/wake → upgrade → uninstall (keep and remove config).
- **Matrix**: macOS 13/14/15/26/27 × Windows 10 22H2 / 11 / Server 2016 / 2019 / 2022 / 2025, Intel/Apple Silicon/ARM64, console and RDP sessions, mixed DPI, 1–3 monitors on each side, Wi-Fi vs Ethernet vs VPN.
- **Performance budgets** (§6.10) confirmed on real machines: latency, CPU, RAM, transfer speed.

### Tooling from the ECC toolkit during implementation

| When | ECC agent / skill |
|---|---|
| Architecture checks per phase | `ecc:architect`, `ecc:planner` |
| Writing tests first | `ecc:tdd-guide` |
| Every Rust change | `ecc:rust-reviewer`; build failures → `ecc:rust-build-resolver` |
| Docker image / build scripts | `ecc:security-reviewer` (image contents, SDK kept out of git), `ecc:silent-failure-hunter` (scripts fail loudly) |
| Every UI change | `ecc:react-reviewer`, `ecc:typescript-reviewer`, `ecc:a11y-architect` |
| Network / input / file / uninstall code | `ecc:security-reviewer`, `ecc:silent-failure-hunter` |
| Performance pass | `ecc:performance-optimizer` |
| UI end-to-end tests | `ecc:e2e-runner` |
| Docs | `ecc:doc-updater` |
| Up-to-date library docs (Tauri, quinn, windows crate, objc2) | `ecc:docs-lookup` (Context7 MCP — installed in Phase 0 if missing) |
| Knowledge graph | **Graphify** — `graphify update .` after each phase; queried before design changes |

---

## 11. Risks & open questions

| Risk | Plan |
|---|---|
| macOS permission prompts confuse users | Wizard with live checks and screenshots; clear re-grant steps after updates |
| Windows secure desktop (UAC, lock screen) cannot be controlled by a normal user app | v1: document the limit; Phase 6+: SYSTEM service helper for UAC / login screen |
| "Cut" differs between OSes | Follow each OS's native gesture (§6.4); never delete before verified copy; send to Trash/Recycle Bin |
| No authentication = anyone on the LAN can connect | Accepted by requirement; mitigated by interface binding, allow-list, visible connection list, encryption (§7) |
| Unsigned builds show first-run OS warnings; macOS permissions may reset after updates | Accepted (private app); documented steps; stable self-signed cert for macOS builds |
| Windows Server 2016 is older (build 14393) | Test early in Phase 1; avoid Windows APIs newer than 1607 or gate them at runtime |
| Game anti-cheat may block injected input | Document; lock-cursor mode for games |
| Windows can be built but not fully tested on the Mac | Windows 11 ARM VM for daily checks; real x64 PC / VM for Windows 10 + Server 2016+ |
| macOS packaging from a Linux container is not supported by Tauri's bundler | Own `xtask` packager (osxcross + rcodesign + libdmg-hfsplus); native fallback for the last step only |
| Container cannot test real input, permissions, tray or installers | Split tests: Docker for code (§10.1), real machines for behaviour (§10.2) with `glidedesk selftest` |
| Monitor IDs change (docks, adapters without serials) | Fallback matching by position/size; fall back to "All monitors" + warning (§6.1.1) |

**Decisions (confirmed 2026-09-25):**

1. **Name** — Glidedesk.
2. **Signing** — no paid certificates; first-run security warnings are fine (private app).
3. **OS support** — macOS 13+, Windows 10 22H2 / 11, Windows Server 2016 or later.
4. **Licence** — private, closed source; no public package managers (Homebrew/winget dropped).
5. **Servers** — one server with many clients; no multi-server profiles.

---

## 12. Next step

When you say **"start"**, work begins with **Phase 0** (repo setup, CI for macOS + Windows, `proto` and `config` crates with tests), then continues phase by phase (building and code-testing in Docker, real tests on Mac and Windows), updating this plan and the Graphify graph as decisions are made.

---

## 13. Version 2 plan (2026-09-25)

New requirements:

| # | Requirement |
|---|---|
| R24 | Supported builds: **macOS ARM64**, **Windows x64**, **Linux ARM64 + x64** (Intel Mac and Windows ARM64 dropped) |
| R25 | Starts automatically at login on all three systems |
| R26 | Fully professional, polished GUI |
| R27 | **One app** — no separate agent program; the same app is server or client |
| R28 | Find and fix all bugs and vulnerabilities; update both apps |
| R29 | Signed without any paid account |
| R30 | ~~GitHub Actions~~ → local Docker pipeline `scripts/local-ci.sh` (GitHub only stores the code) |

### 13.1 Decisions

**One executable (R27).** `glidedesk` / `Glidedesk.exe` contains everything. The tray/settings process starts a second copy of *itself* in background mode (`--agent`), which does the input sharing. You install and see one app. The background copy keeps input sharing alive if the window crashes, and on macOS both share one identity, so permissions are granted once. `--selftest` and `--shutdown` move to the same executable.

**Linux (R24).**
| Area | Choice |
|---|---|
| Keyboard/mouse injection (client) | **uinput** kernel device (works on X11 *and* Wayland); XTest fallback. The package installs a udev rule giving the logged-in user access to `/dev/uinput` |
| Capture (server) | **X11**: XInput2 raw events + pointer grab + XFixes cursor hide (via `x11rb`, pure Rust). **Wayland server role is not supported yet** — needs the new InputCapture portal; the app says so clearly and offers X11 or Client role |
| Monitors | RandR (also available through XWayland on Wayland desktops) |
| Clipboard | `arboard` (X11 + Wayland): text, HTML, images; files via `text/uri-list` |
| Trash / autostart / tray | freedesktop Trash, XDG autostart, AppIndicator tray |
| Packages | `.deb` and `.rpm` (built with `nfpm`) + `.tar.gz`, for x64 and ARM64 |

**Start at login (R25).** It's on by default and turned on at first launch:
- macOS: a LaunchAgent.
- Windows: the per-user `Run` key, also written by the installer.
- Linux: XDG autostart, also installed by the package.

Each starts the app with `--background` (tray only, no window).

**Signing without an account (R29).**
| OS | How | Effect |
|---|---|---|
| macOS | Stable **self-signed code-signing certificate** (generated once, kept outside the repo / as a GitHub secret), signed with `rcodesign`; ad-hoc if no certificate | Permissions survive updates; Gatekeeper still asks once (no Apple account) |
| Windows | Self-signed **Authenticode** certificate, `osslsigncode` + free RFC 3161 timestamp | Signature and publisher visible; importing `glidedesk-codesign.cer` into Trusted Publishers (by hand or via Group Policy) removes "Unknown publisher" on your own PCs |
| Linux | `SHA256SUMS` signed with **minisign** (public key in the repo) | Anyone can verify downloads |
`scripts/generate-signing-keys.sh` creates all three keys once. Nothing secret is ever committed.

**No GitHub CI any more (R30 replaced, 2026-09-26).** GitHub Actions ran out of the free
private-repo minutes, so the workflows were removed and disabled: GitHub only stores the code.
The same pipeline runs locally in Docker — `scripts/local-ci.sh` (`make ci`): tests (UI, rustfmt,
clippy, every Rust test, cross-checks) → real server/client agents on a virtual screen → installers
for Linux x64, Windows x64 and (with the macOS SDK, i.e. on the Mac) macOS, plus signed
`SHA256SUMS` in `output-build/`. A failing step stops it before anything is packaged. Linux ARM64
packages (built on GitHub's ARM runner before) are not built locally.

The macOS build must run on a Mac runner: Apple's SDK licence forbids it on Linux hosts.

### 13.2 Phases
| Phase | Scope | Exit criteria |
|---|---|---|
| 7 | Scope change: arm64-only macOS, x64-only Windows; single executable | `make release` makes one app per OS; installers and uninstallers updated |
| 8 | Bug & vulnerability sweep (ECC Rust, silent-failure, React, security reviewers) | Every verified finding fixed with a test where possible |
| 9 | Linux support | Linux x64/ARM64 build, tests pass, `.deb/.rpm/.tar.gz` produced |
| 10 | Start at login everywhere | Default on; installers register it; toggle works |
| 11 | Professional UI | New design (icons, sidebar, cards, toasts, empty states, native window styles), reviewed |
| 12 | Signing | Keys script; signed Mac app, Windows exe/installer, Linux checksums |
| 13 | GitHub Actions | `ci.yml` + `release.yml`, validated with `actionlint` |
| 14 | Final release build, real Mac test, docs, Graphify update | Files in `output-build/` |

## 14. Version 3 plan (2026-09-25) — field bugs, password, GitHub

Reported after real use on a Mac server with PC clients. Each item names the root cause
found in the code, and the fix.

| # | Report | Root cause | Fix |
|---|---|---|---|
| B1 | macOS permission prompt has no app name | Permissions were requested by the background `--agent` copy. At login it was started directly by `launchd` (the plugin's LaunchAgent runs the binary, not the app), so macOS had no app to name | The window/tray process asks (`AXIsProcessTrustedWithOptions` prompt); the login item opens the **app bundle** through LaunchServices (`open -g -a`) |
| B2 | After granting permission, "the app closes" | The app is menu-bar only (no Dock icon). When System Settings closes, the window is hidden behind other windows with no way to switch back. Input Monitoring also triggers macOS's "Quit & Reopen" | While a window is open, Glidedesk shows in the Dock and ⌘-Tab. Only **Accessibility** is needed (an active event tap needs Accessibility, not Input Monitoring), so there's no Quit & Reopen. The agent restarts capture when permissions change, and the window comes back to the front when it has been granted |
| B3 | Connect by computer name | Only DNS lookups, which fail for plain computer names on many networks | Server address accepts the computer name: first matched against servers found on the network (mDNS name, case-insensitive), then `name.local`, then DNS |
| B4 | Password protection | No authentication by design (old §7) | Optional server password (see §14.1). With a password set, a client can't connect without it |
| B5 | While on a client, the server's own mouse also moves and clicks | macOS: dropping mouse-move events in an event tap doesn't stop the cursor, and "disconnect mouse from cursor" only works for foreground apps. The server cursor kept moving under the hidden pointer, and gestures/pressure events weren't blocked | While grabbed, the Mac cursor is **re-pinned to the centre of the main screen** after every motion (Deskflow technique); gesture, pressure and tablet events are swallowed too. Windows: when a hook misses input (elevated window in front), Raw Input re-pins the cursor. A notice is shown when macOS Secure Keyboard Entry stops key capture |
| B6 | Connected clients don't show in Computers | The settings window loaded its settings once. New clients added by the server never reached it, and **saving any setting sent the stale list back, which disconnected and removed the new client** | The agent sends a `config-changed` event and the UI reloads. The agent never drops a known client because a save lacked it (only **Forget** removes one). Computers also lists live clients from status |
| B7 | Returning to the server sticks at the edge | Same as B5: the pinned Mac cursor reached the edge of the server screen, so no more movement arrived and the virtual cursor on the client stopped short | Fixed by centre re-pinning (B5), plus a regression test in the engine |
| B8 | Keyboard/mouse should behave natively on each OS | Cmd↔Ctrl was swapped by default and scroll direction was forwarded after macOS's "natural" inversion | Default modifier mapping is **native** (like plugging the keyboard into that computer), with swap as an option. Scroll is sent in device direction; each OS applies its own natural-scrolling setting |
| B9 | Copy files as large as possible | Clipboard payload cap 512 MB; files already had no size cap | Files: no limit, checked against free disk space first. Clipboard text/images keep a 512 MB cap (held in memory) |
| B10 | Paste shows old files; files copy in the background; files copy by themselves | Files were pushed eagerly every time the cursor crossed, and the clipboard changed only after the transfer ended | **Nothing is copied until you paste.** Crossing sends only the file list (an *offer*). Windows: pasted as virtual files, so **Explorer does the copy with its own progress window**. macOS/Linux: the paste waits while the files arrive, with a progress notice, then the file manager pastes them. The old clipboard content is replaced immediately, so nothing stale is pasted |
| B11 | Tray menu closes by itself | The native menu was rebuilt on every status change (latency, "last seen") | Menu items are updated in place; the menu is rebuilt only when its structure changes |
| B12 | GitHub | — | Repo `soykot360/glidedesk` (private), `.gitignore` for project files only, CI secrets set with `gh`, CI/release verified on GitHub |

### 14.1 Password (replaces "no auth" in §7)

- Optional: the server's **Network → Password** setting. Empty means open, as before.
- **PAKE:** SPAKE2 (Ed25519 group) on a key derived from the password. The password never crosses the network, and a captured exchange can't be brute-forced offline.
- **Channel binding:** both sides prove the shared key with an HMAC over the QUIC/TLS exporter, so a man-in-the-middle who terminates TLS can't relay the proof. This is mutual: a client with a password refuses a server that can't prove it too.
- **Storage:** the server stores only an Argon2id-derived verifier plus a salt, not the password. The client stores its copy in its private config file (0600 / owner-only ACL).
- **Throttling:** 5 failures from one IP → blocked for 60 s, doubling up to 1 h. Failures are logged and shown in the UI.
- A client that fails auth is never added to Computers.
- Protocol version 2; version-1 peers get a clear "update Glidedesk" message.

### 14.2 Files on paste (B10)

| Step | What happens |
|---|---|
| Copy on A | Nothing is sent |
| Cursor moves to B | A sends an **offer**: names, sizes and a set id. No file data |
| B's clipboard | Windows: a virtual-file data object (`FileGroupDescriptorW` + `FileContents` streams, fetched from A on demand). macOS/Linux: the old content is removed at once, and a paste is intercepted |
| Paste on B | Windows: Explorer pulls the streams and shows its own copy dialog. Cut is honoured through `Preferred/Performed DropEffect`. macOS/Linux: ⌘V/Ctrl+V is held, files arrive in staging with a progress notice, the clipboard is set to them, then the paste is replayed so Finder/Files copies them |
| Cut | A moves its originals to the Trash only after B reports the paste (unchanged rule: only the recipient can release a set) |

### 14.3 Phases

| Phase | Scope |
|---|---|
| 15 | Repo, `.gitignore`, first push, CI secrets |
| 16 | B6 config sync, B11 tray, B1/B2 macOS permissions and lifecycle |
| 17 | B5/B7 grab and edge fixes, B8 native keyboard/scroll |
| 18 | B3 name resolution, B4 password (PAKE) |
| 19 | B9/B10 files on paste (offer protocol; Windows virtual files; macOS/Linux paste hold) |
| 20 | Reviews (Rust, security, React, silent failures), tests, release build, GitHub CI green, docs, Graphify |

## 15. Version 4 plan (2026-09-25) — field bugs round 2

| # | Report | Cause found | Fix |
|---|---|---|---|
| C1 | Forget, Block, "Go there", Disconnect, Wake do nothing | The IPC envelope's `id` (request number) collided with the request's own `id` (computer): the agent could not parse those requests and answered under number 0, so the button waited forever | Envelope key is now `seq`; unparsable requests are answered under their `seq`; round-trip test for every request that names a computer |
| C2 | A forgotten computer comes straight back | It reconnects a second later and was auto-added again | Forgotten ids are kept (agent state); such a client is refused with `Forgotten` until its user presses Reconnect (`Features::REJOIN`) |
| C3 | Settings of an offline computer / after Block "don't stick" | Block/Forget changed settings behind the window, whose older copy undid them on its next save | The agent tells windows to reload after Block/Forget; offline edits are saved and applied on connect (tested) |
| C4 | Old files "copy by themselves", no details | A client holding an unpasted offer downloaded the files when the cursor left, to pass them on; a Linux server fetched offers at once | Offers are never fetched without a paste or "Get them now"; files already on the clipboard when Glidedesk started, or copied over an hour ago, are not offered; an **activity log** (window) and transfer details say what moved, where and why |
| C5 | IPv6 must be off completely | — | IPv4 only: no IPv6 listing, binding, mDNS, DNS results or typed addresses; schema v4 drops the IPv6 switches |
| C6 | One chosen network off → should warn but keep working | Chosen interfaces were resolved once at start | The server re-checks every 3 s, drops/adds per-address sockets without touching other connections, and warns per network; it starts even when every chosen network is off. A client whose chosen interface is off uses any other and says so |
| C7 | Client with several monitors | Linux injector cached the desktop size at start; speed used one scale for all screens | Desktop re-read every 2 s; per-monitor speed factor (Windows per-monitor scaling) |
| C8 | Mouse/scroll speed should come from the server | Scrolling followed each computer's own direction | Protocol 3: wheel input travels in the server's direction; `ClientSettings` carries the effective speed/scroll/keys and the client window shows them ("Set by the server: 1.5×") |
| C9 | (found in the lab) Linux server: nothing moves after crossing | A core pointer grab stops XInput raw events for the grabbing client | XI2 grab with raw event masks |
| C10 | UI on phones/tablets | Fixed 228 px sidebar, rows that don't wrap | Responsive shell (top bar + drawer below 768 px), wrapping rows/inputs, stacked layout editor |

Security note: Forget and Block act on the device id a client declares, so they tidy up the list
but don't stop a modified client; the server **password** (§14.1) is what keeps others out.

Testing: `docker/lab` runs a real server and two Linux clients (one with two screens) on virtual X
displays and dummy networks, with the settings UI in a browser through a dev-only agent bridge
(`make lab`). See docs/TESTING.md.
