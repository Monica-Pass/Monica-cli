# Build this repository

The CLI currently uses MDBX workspace path dependencies. Clone the two repositories as siblings:

```sh
git clone --branch v1.0.101 https://github.com/Monica-Pass/Monica-cli.git monica-pass-cli
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

This release requires the credential-use lease API from MDBX commit `f81d3f1f08589670e534473ab8e0030b796dfdf4`. The previous engine revision does not expose that API. Database format and Android API Key fields are unchanged; other clients need not adopt the new API.

The release source archive includes both repositories and vendored, locked registry dependencies. From its `monica-pass-cli/` directory, run `cargo build --release --locked --offline` after installing Rust 1.97.0 and a native C toolchain. The Windows release uses the MSVC target with `RUSTFLAGS="-C target-feature=+crt-static"` and `--target x86_64-pc-windows-msvc`; Linux uses Ubuntu 22.04 with `--target x86_64-unknown-linux-gnu`. `BUILD-INFO.json` in each binary package records the exact revisions and toolchain.

The earlier proxy-session test run used a local engine working tree containing another session's uncommitted fixes. A subsequent direct-configuration validation used an isolated source snapshot of the exact published MDBX commit above, without those fixes, in `wsl -d homoos`. CLI regressions, strict Clippy and a Linux release build passed; Codex and Claude Code also completed live direct requests using generated configurations. See the [direct-configuration review](direct-config-review-2026-09-29.md) for scope and limitations. This validation does not establish live proxy compatibility or Windows/macOS release readiness.

Database-first interface changes are in progress; see [redesign progress](redesign-progress.md) for implemented behavior and remaining work. This source publication is not a claim that the full redesign is complete.
