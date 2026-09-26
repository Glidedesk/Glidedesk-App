## What and why

<!-- What does this change, and which problem does it solve? Link issues: "Fixes #123". -->

## How it was tested

<!-- CI runs on Linux, Windows and macOS. Say what you checked by hand, and on which systems. -->

- [ ] `scripts/local-ci.sh test` passes (or CI is green)
- [ ] New behaviour has a test; `cargo clippy -- -D warnings` is clean
- [ ] Network input stays untrusted: sizes are limited and peer data can't panic (see SECURITY.md)
- [ ] Docs updated if behaviour or settings changed
