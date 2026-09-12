# Build this repository

The CLI currently uses MDBX workspace path dependencies. Clone the two repositories as siblings:

```sh
git clone https://github.com/Monica-Pass/Monica-cli.git monica-pass-cli
git clone https://github.com/Monica-Pass/Mdbx.git mdbx
git -C mdbx checkout 5cf8af697727095654884e0a76bec6ca12d0ce6b
cd monica-pass-cli
cargo test --locked
cargo build --release --locked
```

Use Rust 1.97 or later. In the Windows GNU development environment:

```powershell
cargo +1.97.0-x86_64-pc-windows-gnu test --locked
cargo +1.97.0-x86_64-pc-windows-gnu build --release --locked
```

The MDBX revision above is the engine revision used for this initial CLI source publication. Engine updates should be verified alongside CLI tests.

Database-first interface changes are in progress; see [redesign progress](redesign-progress.md) for implemented behavior and remaining work. This source publication is not a claim that the full redesign is complete.
