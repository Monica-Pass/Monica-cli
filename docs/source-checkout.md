# Build this repository

The CLI currently uses MDBX workspace path dependencies. Clone the two repositories as siblings:

```sh
git clone --branch main https://github.com/Monica-Pass/Monica-cli.git monica-pass-cli
git clone https://github.com/Monica-Pass/Mdbx.git mdbx
git -C mdbx checkout f004673ef07a04c31702eaf1015e062f29730537
cd monica-pass-cli
cargo test --locked -- --test-threads=1
cargo build --release --locked
```

Use Rust 1.97 or later. In the Windows GNU development environment:

```powershell
cargo +1.97.0-x86_64-pc-windows-gnu test --locked -- --test-threads=1
cargo +1.97.0-x86_64-pc-windows-gnu build --release --locked
```

Current main and CI pin MDBX `f004673ef07a04c31702eaf1015e062f29730537`, integrating credential-use leases, lossless JSON, native Glitter support and the upstream Android Build ID fix. Monica CLI deliberately rejects Glitter even with correct factors; Sky / Multi / Power retain combined password and key-file support, including binding, direct configuration and broker sessions. Tests use synthetic credentials and serial threads to bound memory while creating 512 MiB-KDF Glitter fixtures. No client is automatically migrated or downgraded.

The published `v1.0.101` tag and its existing release artifacts remain unchanged and use the earlier engine `f81d3f1f08589670e534473ab8e0030b796dfdf4`. To reproduce that release, check out the tag and use the source instructions stored at that tag. The integration on main is not a new release.

The release source archive includes both repositories and vendored, locked registry dependencies. From its `monica-pass-cli/` directory, run `cargo build --release --locked --offline` after installing Rust 1.97.0 and a native C toolchain. The Windows release uses the MSVC target with `RUSTFLAGS="-C target-feature=+crt-static"` and `--target x86_64-pc-windows-msvc`; Linux uses Ubuntu 22.04 with `--target x86_64-unknown-linux-gnu`. `BUILD-INFO.json` in each binary package records the exact revisions and toolchain.

The earlier proxy-session test run used a local engine working tree containing another session's uncommitted fixes. A subsequent direct-configuration validation used an isolated source snapshot of the published MDBX `f81d3f1` commit, without those fixes, in `wsl -d homoos`. CLI regressions, strict Clippy and a Linux release build passed; Codex and Claude Code also completed live direct requests using generated configurations. See the [direct-configuration review](direct-config-review-2026-09-29.md) for scope and limitations. That historical validation does not establish live proxy compatibility or Windows/macOS release readiness. Current integration results are recorded in the [Glitter client-boundary validation](glitter-client-disabled-2026-10-06.md).

Database-first interface changes are in progress; see [redesign progress](redesign-progress.md) for implemented behavior and remaining work. This source publication is not a claim that the full redesign is complete.
