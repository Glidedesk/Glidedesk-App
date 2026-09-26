# Security policy

## Reporting a vulnerability

Please **don't open a public issue** for a security problem. Report it privately through
GitHub: **[Security → Report a vulnerability](../../security/advisories/new)**.

Include what an attacker can do, the steps or a proof of concept, and the Nexpingdesk version and
systems involved. You'll get an answer within 7 days. Fixes are released as soon as they are
ready, with an advisory that credits you (unless you'd rather not be named).

## Supported versions

Only the latest release (and the Nightly build of `main`) gets security fixes.

## Threat model

Nexpingdesk shares a keyboard, mouse, clipboard and files between computers **on a local network**.

- Connections are encrypted (QUIC/TLS 1.3). Without a password, **any computer that can reach
  the server on the network can join**, like a device plugged into the same desk. Set a
  **password** (Network → Password) and/or **allowed addresses** (Network → Who may connect) on
  networks you don't fully trust. The password never crosses the network (SPAKE2 bound to the
  TLS session) and only a salted hash is stored.
- Every byte from the network is treated as untrusted: message sizes are limited, peer data must
  never crash the app, received files can't be written outside the destination folder, and the
  local control socket only accepts the same user.
- In scope: anything that lets a peer crash, hang or exhaust a computer, read or write files
  outside what the user shared, bypass the password or network filters, or run code; and
  weaknesses in the installers, update path or release signing.
- Out of scope: a joined peer typing keys or reading the clipboard it was given (that is what the
  app does), and attacks that need an already-compromised computer.

## Verifying downloads

Every release file has a GitHub build-provenance attestation, and `SHA256SUMS` is signed with
minisign ([signing/minisign.pub](signing/minisign.pub)):

```sh
gh attestation verify Nexpingdesk_<ver>_linux-x64.deb --repo <owner>/nexpingdesk
minisign -Vm SHA256SUMS -p minisign.pub && sha256sum -c SHA256SUMS
```
