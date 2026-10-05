# Build this repository

The CLI currently uses MDBX workspace path dependencies. Clone the two repositories as siblings:

```sh
git clone --branch codex/android-password-alignment https://github.com/Monica-Pass/Monica-cli.git monica-pass-cli
git clone --branch codex/password-lossless-json https://github.com/Monica-Pass/Mdbx.git mdbx
git -C mdbx checkout 51d5f75551d1c0f576f3da8f9ccea6b904dcf343
cd monica-pass-cli
cargo test --locked
cargo build --release --locked
```

Use Rust 1.97 or later. In the Windows GNU development environment:

```powershell
cargo +1.97.0-x86_64-pc-windows-gnu test --locked
cargo +1.97.0-x86_64-pc-windows-gnu build --release --locked
```

This password-interoperability checkpoint is based on CLI commit `d777af1`; it does not include later main-branch model proxy/direct-configuration releases. The pinned MDBX revision provides the lossless JSON parser required by the password adapter. Both checkpoint branches remain separate from the released main branches. Engine updates should be verified alongside CLI tests.

Database-first interface changes are in progress; see [redesign progress](redesign-progress.md) for implemented behavior and remaining work. This source publication is not a claim that the full redesign is complete.
