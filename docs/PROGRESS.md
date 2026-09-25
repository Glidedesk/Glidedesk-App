# Glidedesk — progress log

Source of truth for scope: `PLAN.md`. This file records what is done and verified.

## Phase 0 — Foundations ✅ (2026-09-25)
- `docker/Dockerfile` → image `glidedesk-builder` (Rust 1.98, Node 24 + pnpm, cargo-xwin (clang mode),
  NSIS 3.11, lld/ld64.lld, rcodesign, libdmg-hfsplus `dmg`, xorriso).
- `docker/run.sh` runs any command in the image with caches + macOS SDK mounted.
- `docker/copy-sdk.sh` (`make sdk`) copies the newest SDK LLVM 19 can link (26.5; 27.0's
  `arm64e.x1` TBD arch is unsupported by ld64.lld 19).
- Verified: Windows x64 + ARM64 and macOS arm64 + x86_64 binaries (with C deps + AppKit) link in
  Docker; the macOS universal binary runs natively on this Mac.
- Workspace lints: clippy pedantic `-D warnings`, no unwrap/expect outside tests.
- Claude project hook: `.claude/hooks/rustfmt-edited.sh` formats edited Rust files in the container.

## Phase 1 — Input + network MVP (in progress)
Done and tested in Docker:
- `glidedesk-proto`: wire messages, framing with per-stream size limits, fuzz-ish decode test.
- `glidedesk-layout`: outer-edge maths, multi-monitor handover (single/selected/all, continuous /
  per-monitor mapping, return to origin monitor), switch guards (delay, double-tap, modifier, dead
  corners, full-screen), wrap, lock, offline walls, speed factor. Property tests.
- `glidedesk-config`: schema, protected atomic storage (0700/0600), migration framework, repair on
  load, read-only for newer schema, corrupt-file recovery, backups (5), export/import.
- `glidedesk-net`: QUIC (quinn + rustls/ring, TLS 1.3, no auth by design), bind all / interfaces /
  exact IPs with per-address status, admission filter (allow/block/same-subnet), mDNS advertise/browse.
- `glidedesk-input`: HID↔macOS↔Windows key tables, hotkeys, Cmd↔Ctrl remap, key gate (no stuck
  local keys on grab), macOS backend (CGEventTap/CGEventPost, displays, permissions, background
  cursor hide), Windows backend (LL hooks + Raw Input edge push, SendInput, per-monitor DPI,
  veil window cursor hiding, Mouse Keys cursor for mouse-less servers, LED sync). Cross-lint clean.
- `glidedesk-core`: server hub (engine, clients, heartbeat health, hotkeys, auto-placement,
  coalescing writers, back-pressure), client runtime (discovery/address, reconnect back-off,
  injection thread, prefs, monitor change reports). E2E tests over real QUIC.

Bugs found by tests and fixed: QUIC uni stream invisible until first write; endpoint mapping
off-by-one between different edge lengths.

- `glidedesk-platform` (host name, logs, lock/RDP session state, Wake-on-LAN, move-to-trash),
  `glidedesk-ipc` (private socket / owner-only named pipe, JSON protocol), `glidedesk-agent`
  (background process, `--selftest`, `--shutdown`).

Real Mac test (2026-09-25): agent self-test — capture OK, inject OK, loopback QUIC p50 0.5 ms.

## Phase 2 — UI + tray ✅
- Tauri 2 app: tray/menu bar with computers + status, Stop/Start, Restart, Reconnect, Identify,
  clipboard/files toggles, Quit; agent supervisor (spawns/restarts the agent); notifications;
  identify overlay; start-at-login sync; single instance; window created on demand.
- React 19 + TS 7 UI: setup wizard, Layout editor (drag, keyboard, multi-monitor handover panel),
  Computers, Clipboard & Files (+ Transfers), Keyboard & Mouse, Network (all/interfaces/IPs,
  allow/block lists), General, Advanced (export/import with preview, reset, self-test, logs).
- Real Mac test: DMG mounts, ad-hoc signature valid, universal binary; app + agent start;
  agent RSS 8 MB, 0 % CPU; permission error reported clearly; Quit exits both processes.

## Phase 3 + 4 — Clipboard & files ✅ (see PLAN §6.2.1 for the as-built design)
- `glidedesk-clipboard`: macOS NSPasteboard, Windows clipboard thread (text, HTML, RTF, PNG↔DIB,
  file lists + cut flag); portable conversions tested.
- `glidedesk-transfer`: manifest, sanitised names, BLAKE3-verified streaming, staging.
- Core sync: clipboard follows the cursor, echo suppression, direction/size/both-sides rules,
  cut → trash after paste. E2E test over QUIC.
- Not yet: resume of interrupted transfers; drag-and-drop between screens.

## Phase 6 — Packaging (in progress)
- `cargo xtask package macos` → universal `.dmg` (+ `.app.tar.gz`), uninstaller app.
- `cargo xtask package windows` → NSIS installers x64/ARM64 (standard + offline WebView2) with
  install / upgrade / repair / replace / rollback and keep-config uninstall.

## Hardening (2026-09-25)
- ECC security review (network, IPC, transfer, app, installers): one HIGH finding fixed — a
  different client could release another client's *cut* file set (originals to Trash early).
  Cut sets are now bound to their recipient and use random 64-bit ids; test added.
- Full-screen guard implemented (macOS window list / Windows foreground window) with an
  allow list; cached 500 ms.
- `make test`: 102 Rust tests + UI tests + clippy pedantic + cross-checks (macOS arm64/x64,
  Windows x64/ARM64) all green.

## Known gaps (honest list)
- Not tested on a real Windows machine yet (built and cross-checked only) — see docs/TESTING.md.
- Interrupted file transfers restart instead of resuming; no drag-and-drop between screens yet.
- Windows secure desktop (UAC prompt / lock screen) cannot be controlled (needs a SYSTEM service).
- Phase 5 v1.x items not done: broadcast typing, lock-all/sleep-all, screensaver sync, profiles,
  dimming, cursor locator animation, CLI, auto-update.
- macOS: the macOS 27 SDK cannot be linked by the container's LLVM 19; builds use SDK 26.5
  (fully compatible with macOS 13–27).

## Version 2 (PLAN §13) — task tracker
Done:
- [x] Phase 7: single executable — agent is a library (`crates/agent/src/lib.rs`: `run_agent`,
      `run_selftest`, `shutdown_running_agent`, `VERSION`); `app/src/main.rs` dispatches `--agent`,
      `--selftest`, `--shutdown`, `--version`; `app/src/link.rs` spawns `current_exe --agent`.
- [x] NSIS: single exe, StopApp waits (un./install variants), firewall/WebView2 failures → exit 3.
- [x] macOS uninstall.sh uses `glidedesk --shutdown`.
- [x] xtask rewritten: macOS arm64 only, Windows x64 only, Linux deb/rpm/tgz via nfpm,
      optional signing (GLIDEDESK_MAC_P12/_PASSWORD, GLIDEDESK_WIN_PFX/_PASSWORD via
      `gd-sign-windows`, GLIDEDESK_MINISIGN_KEY), checksums verify sha256sum status.
To do:
- [x] installer/linux files; NSIS SIGN hooks; Docker tools (osslsigncode, minisign, nfpm, GTK/WebKit);
      docker/bin/gd-sign-windows; Makefile build-linux + 2 cross targets; app lints on Linux too.
- [x] Phase 9 Linux: input (x11rb XInput2 capture + uinput/XTest inject + RandR + EWMH fullscreen),
      clipboard (arboard), trash (gio/kioclient), app open/uninstall on Linux.
      Real test: .deb installs on clean Debian trixie; on Xvfb capture/inject/monitors OK.
- [x] Windows installer writes HKCU Run on fresh install (autostart); app syncs afterwards.
- [ ] Phase 8 fixes: silent-failure #3 done (xtask), #4 clipboard init notice, #5 UI buttons errors,
      #6 store initial status retry, #7 link.rs subscribe backoff; React #1 reload cancels pending,
      #2 generation guard, #3 textarea controlled, #5 unplaced drag pointer capture, #6 space key;
      Rust review: #1 Windows grab pins cursor at primary-monitor centre ✅, #2 Mouse Keys restored
      on drop ✅, #3 subscribe backoff ✅, #4 socket race mitigated by 0700 dir (no change).
- [x] Phase 11 UI redesign (all pages patched; mac overlay title bar; click-vs-drag fix).
- [x] Phase 12 signing: scripts/generate-signing-keys.sh (make signing-keys) → .signing/ (private,
      git-ignored) + signing/ (public). docker/run.sh passes keys automatically.
- [~] (old note) Done: components/icons.tsx, lib/config.tsx (ConfigProvider, generation
      guard, commit), lib/toast.tsx (ToastProvider, run()), lib/store.ts (retry, HMR unlisten), ui.tsx
      (Button icons/focus ring, Card, EmptyState, Spinner, Callout icons, Modal focus), App.tsx shell,
      ComputersPage cards, AdvancedPage (commit, formatted self-test). TODO: LayoutPage/ClientHome/
      Wizard use toast.run + commit; InputPage controlled textarea; LayoutEditor unplaced pointer
      capture + Space; mac window titleBarStyle Overlay (app/src/windows.rs); typecheck/build.
- [x] Phase 13 .github/workflows/ci.yml + release.yml — actionlint clean.
- [x] Phase 14: make test green (103 tests); signed release built (macOS arm64 dmg, Windows x64
      standard+offline, Linux arm64 deb/rpm/tgz; Linux x64 is built by CI); minisign + Windows
      timestamped signatures verified; macOS app signed by "Glidedesk Code Signing".
- Incident: a local smoke test overwrote and deleted the user's real Glidedesk settings (their
  installed 0.1.0 was running). Added `GLIDEDESK_HOME` isolation (config, logs, socket/pipe,
  staging) — all future tests must use it.

## Version 3 (PLAN §14) — task tracker
- [x] Phase 15: repo `soykot360/glidedesk` (private), `.gitignore` (project files only), signing
      secrets set with `gh secret set`, first push; CI fixed (disk space, no debug info).
- [x] Phase 16: B6 settings sync (config-changed event + replay of pending edits; stale saves
      can't remove clients — only Forget), B11 tray updated in place, B1/B2 macOS Accessibility
      prompt from the app process, Accessibility-only capture, LaunchServices login item, Dock
      icon while a window is open, capture restarts on permission change.
- [x] Phase 17: B5/B7 macOS cursor re-pinned at screen centre, gestures swallowed, Secure
      Keyboard Entry warning; Windows raw-input re-pin; B8 native keys (schema v2) and scroll.
- [x] Phase 18: B4 password (SPAKE2 + TLS exporter, Argon2id verifier, lockout, protocol 2),
      B3 connect by computer name (mDNS name/host match, .local, DNS).
- [x] Phase 19: B9/B10 files move only on paste (offer → held paste → fetch → replay), relay
      across 3 computers, free-space check, "Get … now" in tray/window.
- [ ] Phase 20: reviews, release build, GitHub CI/Release green, Graphify.
Known gaps (v3): no real Windows/Linux device test from this Mac; Windows virtual-file paste
(Explorer's own network progress) not implemented — files arrive first, then Explorer copies
them from staging; macOS Finder has no "cut" so cut only applies to Windows sources.
