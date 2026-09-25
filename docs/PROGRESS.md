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
- [x] Phase 20: ECC security/Rust/React reviews — 7 findings fixed (fetch size cap, IPv6 + global
      password throttle, stuck paste key, cancel-safe frame reads, 3 UI config races/a11y);
      121 Rust + 6 UI tests; signed release in output-build/ (checksums, minisign, Authenticode
      timestamp, macOS "Glidedesk Code Signing" verified); GitHub CI and Release green, Nightly
      pre-release published; Graphify refreshed (2510 nodes).
Known gaps (v3): no real Windows/Linux device test from this Mac; Windows virtual-file paste
(Explorer's own network progress) not implemented — files arrive first, then Explorer copies
them from staging; macOS Finder has no "cut" so cut only applies to Windows sources.

## Version 4 (PLAN §15) — task tracker
- [x] C1 IPC `seq` envelope (Forget/Block/Go there/Disconnect/Wake work again), C2 Forget sticks
      (REJOIN), C3 window reloads after Block/Forget, offline edits sync on connect.
- [x] C4 no automatic file fetch, stale files not offered, activity log + transfer details.
- [x] C5 IPv4 only (schema v4), C6 per-network watch/rebind with warnings, client interface fallback.
- [x] C7/C8 multi-monitor clients (Linux desktop refresh, per-monitor speed), protocol 3 (server
      scroll direction, effective settings shown on the client).
- [x] C9 Linux server XI2 grab (found in the lab), C10 responsive UI.
- [x] Lab (`make lab`): real agents on Xvfb (client with 2 monitors) + dummy networks; browser QA
      of all pages at 375/768/1280 px, light and dark; Lighthouse accessibility 100.
Known gaps (v4): still no real Windows device test from this Mac; the lab covers Linux agents only
(macOS server tested on this Mac by the user).

### v4 fix — a restarted client works without restarting the server
- [x] Found (real client log + lab): per-peer state outlived the link it came on. (1) The "don't echo
      back" guard kept the server from sending a client the clipboard it got from it, though the
      restarted client had lost it (X11 clipboards die with the app) — until the server's clipboard
      changed or the server restarted; same on the client after a server restart. (2) A file offer
      from the old link stayed pending: the server held its paste shortcut and the first paste
      failed on the dead link. (3) Old latency / "screen locked" showed until the first pong.
- [x] Fix: `Sync::link_closed` on every link end (server `drop_client`, client session end) drops
      that peer's offer (its placeholder text is never passed on) and its echo guard;
      `Health::connected` starts fresh. Tests: 2 e2e (client restart, server restart) that failed
      before the fix, 2 unit; 137 Rust + 6 UI tests, clippy, Windows cross-check green.
- [x] Lab: client-a restarted with SIGTERM and SIGKILL, server untouched — cursor, pointer
      motion and clipboard work every time; browser QA: offer banner and "Get them now" disappear,
      Activity says why, client back Online. macOS cross-check not run (no SDK on this Linux host).

### OS matrix on GitHub's free runners
- [x] `.github/workflows/os-matrix.yml`: Linux x64, Windows x64 (windows-2025) and macOS ARM64
      (macos-15) each run native clippy, all tests, the app build and `scripts/os-smoke.sh`
      (real server + client agents over loopback: connect, graceful client restart, killed
      client + restart, self-test). Push to main (code changes) or Actions → OS matrix → Run
      (macOS optional, 10× minutes). `make os-smoke` runs the smoke test locally (Linux/Xvfb).
- [x] First native run found 2 test assumptions on macOS (⌘V paste; IPC probe accept error)
      and xtask not building on Windows (excluded, as in ci.yml) — fixed; run 36149212960 green:
      Linux 137, Windows 134, macOS 138 tests; smoke PASSED on all three.

### One pipeline: test on every push, release only when green
- [x] `ci.yml` runs on every push/PR: Linux (fmt, UI typecheck/tests, clippy, tests, app build,
      real agents), Windows x64 and macOS ARM64 (native clippy, tests, app build, real agents).
      `release.yml` is only a reusable workflow now, called by CI with `needs: [linux, native]`
      for main (Nightly), v* tags and manual runs — any red test job stops the release.
      `os-matrix.yml` merged into it. Browser QA via the ecc Chrome MCP removed (plugin
      `.mcp.json` emptied locally, backup `.mcp.json.bak-chrome-devtools`).

### Fix: the Mac's own cursor moved along while a client had control
- [x] Cause: while grabbed, the hidden Mac cursor was only pulled back to the screen centre when it
      came within 120 pt of an edge (at most every 100 ms), so it roamed the whole screen in step
      with the hand — seen whenever macOS showed the cursor again. Pulls were kept rare because
      the first event after each warp was dropped (its deltas contain the jump).
- [x] Fix (`crates/input/src/pin.rs`, used by the macOS tap): pulled back as soon as it drifts
      40 pt; the event after a warp is measured from the warp target, events queued before it keep
      their own delta, so frequent pulls lose no motion. 6 unit tests (run on every OS in CI).

### Fix: extra mouse buttons / copy-paste acted on the Mac while a client had control
- [x] Root cause (proven on the macOS CI runner with a real event tap: red test first):
      mouse utilities (Logi Options+, SteerMouse, …) read extra buttons themselves and post
      what they are set to — ⌘C/⌘V, other shortcuts, scrolls, clicks — at the session level,
      after our HID tap, so it never saw them and they acted on the Mac. Media / special keys
      (system-defined events) were not in the tap mask at all. Buttons 6+ were sent as middle.
- [x] Fix (`crates/input/src/macos.rs`): a second active tap at the annotated-session level
      forwards + swallows everything software posts while grabbed (the HID tap marks the key
      releases it lets through on purpose); system-defined media keys decoded
      (`keymap::mac_aux_key`) and forwarded; buttons 6+ swallowed, not sent as middle.
      Real-tap tests on macOS CI (`GLIDEDESK_TAP_TESTS`): posted ⌘C and volume-up reach the client.
      Limit: actions a utility performs through a private system API (not as events) can't be caught.

### Fix: the Mac's mouse gestures stopped working
- [x] Cause: the session-level tap (added for remapped buttons) ran all the time and asked for
      gesture events; a tap that sees gesture events turns the Mac's gestures off, even when it
      lets them through.
- [x] Fix: the session tap runs only while a client has control (on before `grabbed`, off after);
      gesture + system-defined events are only asked for by it. So on the Mac gestures work as
      before, and on a client gestures / extra buttons / media keys never act on the Mac (they
      can't switch spaces or pull focus back). The "let through" marker is random per run.
      Real-tap test on macOS CI: the session tap is off, on while grabbed, off again.
      Reviewed by the ecc rust-reviewer agent (2 findings, both fixed).

### No GitHub CI: local Docker pipeline for every platform
- [x] GitHub Actions removed (`.github/workflows/` deleted, both workflows disabled with `gh`):
      the private repo ran out of free minutes. GitHub only stores the code.
- [x] `scripts/local-ci.sh` (`make ci`) does locally what the runners did: test (UI, rustfmt,
      clippy, every Rust test, Windows/macOS cross-checks) → real server/client agents on a
      virtual screen → installers (Linux x64, Windows x64, macOS when the SDK is present) +
      signed SHA256SUMS in `output-build/`. Any failure stops it before packaging.
      Not built locally: Linux ARM64 (was GitHub's ARM runner), and the macOS app / its real
      event-tap tests on a non-Mac (Apple SDK licence).
