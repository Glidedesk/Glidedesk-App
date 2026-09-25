# Glidedesk — working notes for Claude

- Plan: `PLAN.md` (source of truth). Progress: `docs/PROGRESS.md`.
- Knowledge graph: `graphify-out/` — query it first (`graphify query "..."`), update after each phase
  (`graphify extract . --backend claude-cli && graphify cluster-only . --backend=claude-cli`).
- **Never build or test on the host.** Everything runs in Docker: `scripts/local-ci.sh` (= `make ci`:
  tests → real agents → installers for every platform), or single steps with `make lint`,
  `make test-rust`, `make check-cross`, `make build-win`, `make build-mac`, `docker/run.sh <cmd>`.
  (`make` isn't installed on this Linux host: call the script / `docker/run.sh` directly.)
- **No GitHub Actions.** GitHub only stores the code (workflows removed and disabled). Build and
  test locally in Docker before every push; never add a workflow back without being asked.
- Output installers go to `output-build/`.
- macOS SDK comes from `make sdk` (copied to `~/.cache/glidedesk/macos-sdk`, never committed).
- Rust: edition 2024, `clippy -D warnings` with pedantic, no `unwrap`/`expect` outside tests,
  `unsafe` only in platform backends with a `// SAFETY:` comment per block.
- Security: no auth by design (PLAN §7) → every network input is untrusted: size-limit frames,
  never panic on peer data, filter by interface/allow-list.
