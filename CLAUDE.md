# Glidedesk — working notes for Claude

- Plan: `PLAN.md` (source of truth). Progress: `docs/PROGRESS.md`.
- Knowledge graph: `graphify-out/` — query it first (`graphify query "..."`), update after each phase
  (`graphify extract . --backend claude-cli && graphify cluster-only . --backend=claude-cli`).
- **Never build or test on the host.** Everything runs in Docker: `make lint`, `make test-rust`,
  `make check-cross`, `make build-win`, `make build-mac`, `make release`, or `docker/run.sh <cmd>`.
- Output installers go to `output-build/`.
- macOS SDK comes from `make sdk` (copied to `~/.cache/glidedesk/macos-sdk`, never committed).
- Rust: edition 2024, `clippy -D warnings` with pedantic, no `unwrap`/`expect` outside tests,
  `unsafe` only in platform backends with a `// SAFETY:` comment per block.
- Security: no auth by design (PLAN §7) → every network input is untrusted: size-limit frames,
  never panic on peer data, filter by interface/allow-list.
