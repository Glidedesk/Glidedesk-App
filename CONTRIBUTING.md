# Contributing to Glidedesk

Thanks for helping! Bug reports, fixes and ideas are all welcome.

- **Bugs and ideas:** open an [issue](https://github.com/soykot360/glidedesk/issues/new/choose).
- **Security problems:** report them privately — see [SECURITY.md](SECURITY.md).
- **Code:** fork, make a branch, open a pull request against `main`. CI must be green on Linux,
  Windows and macOS, and the maintainer reviews every change before it's merged.

## Building and testing

Everything builds and runs in Docker, so you only need Docker and a POSIX shell:

```sh
scripts/local-ci.sh test       # UI + rustfmt + clippy + all Rust tests + cross-checks
scripts/local-ci.sh            # tests → real agents on a virtual screen → installers in output-build/
docker/run.sh cargo nextest run -p glidedesk-core   # any single command inside the builder image
```

The macOS app and the Mac's real event-tap tests need a Mac (Apple's SDK licence); CI runs them
on GitHub's macOS runners for every pull request.

## Code rules

- Rust edition 2024, `cargo fmt`, and `cargo clippy --all-targets -- -D warnings` (pedantic) clean.
- No `unwrap`/`expect` outside tests. `unsafe` only in the platform backends, with a
  `// SAFETY:` comment on every block.
- **Every network input is untrusted** (there's no mandatory authentication, [PLAN.md §7](PLAN.md)):
  limit sizes, never panic on peer data, keep file paths inside their destination.
- Add a test for every fix and feature. Tests that touch settings use a temporary
  `GLIDEDESK_HOME`, never the real one.
- Keep commits focused, with a message that says what changed and why.

## Licence

Glidedesk is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option. Unless you say otherwise, any contribution you submit is licensed the same way, without
additional terms.
