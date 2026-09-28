# Build this repository

The CLI currently uses MDBX workspace path dependencies. Clone the two repositories as siblings:

```sh
git clone --branch codex/cli-android-api-keys https://github.com/Monica-Pass/Monica-cli.git monica-pass-cli
git clone https://github.com/Monica-Pass/Mdbx.git mdbx
git -C mdbx checkout f81d3f1f08589670e534473ab8e0030b796dfdf4
cd monica-pass-cli
cargo test --locked
cargo build --release --locked
```

Use Rust 1.97 or later. In the Windows GNU development environment:

```powershell
cargo +1.97.0-x86_64-pc-windows-gnu test --locked
cargo +1.97.0-x86_64-pc-windows-gnu build --release --locked
```

This CLI feature branch requires the credential-use lease API from MDBX commit `f81d3f1f08589670e534473ab8e0030b796dfdf4`, published on `codex/tiga-proxy-session`. The previous engine revision does not expose that API. Database format and Android API Key fields are unchanged; other clients need not adopt the new API.

The recorded test run used the local engine working tree, which also contained a separate session's uncommitted compatibility fixes. Those fixes are intentionally excluded from this feature's engine commit. See the [proxy-session review](proxy-session-review-2026-09-29.md) for the test scope and remaining integration work; the published commit pair has not been rebuilt after the user's build-cache cleanup.

Database-first interface changes are in progress; see [redesign progress](redesign-progress.md) for implemented behavior and remaining work. This source publication is not a claim that the full redesign is complete.
