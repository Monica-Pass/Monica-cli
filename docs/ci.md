# Continuous integration

Pushes to `main`, pull requests targeting `main`, and manual runs use:

| Workflow | Checks |
| --- | --- |
| Code quality | actionlint for every workflow, Rust format, strict Clippy for all targets |
| Dependency security | Cargo.lock against the current RustSec database; also runs every Monday |
| Build and verify release packages | Serial release-mode tests and builds on Ubuntu 22.04 / Windows 2022; extracted CLI discovery, native-host diagnostics and framed hello; Windows temporary installation; Linux offline source bundle |
| Deploy the command practice book | Publishes committed documentation when `docs/` or the Pages workflow changes |

Rust builds pin toolchain 1.97.0 and sibling MDBX revision
`f004673ef07a04c31702eaf1015e062f29730537`. All actions are pinned to
commit SHAs; Dependabot checks GitHub Actions weekly. CI does not create a
GitHub Release or a tag. Package artifacts expire after 14 days.

Cargo cache restores use an exact OS/lockfile key and save only successful
`main` builds. Only compressed registry downloads and the registry index are
cached. Compiled `target/`, expanded crate sources, vaults and configuration
files are excluded. Tests use the same optimized profile as packaging, with
one test thread to bound KDF memory; debug and release test suites are not
built together in the packaging job. Each runner is discarded after its job.

The security job installs a prebuilt cargo-audit rather than compiling another
Rust toolchain. `.cargo/audit.toml` exempts only `RUSTSEC-2023-0071`, with the
local-only RSA generation/parsing rationale in [SECURITY.md](../SECURITY.md).
All other vulnerability advisories fail the job. Informational warnings such
as the engine's unmaintained `bincode` dependency remain visible in the log.

For local Windows work, use the GNU toolchain described in
[source-checkout.md](source-checkout.md). On this development machine the
repository's `target` junction points to `E:\CodexBuildCache\monica-pass-cli`;
reuse it rather than creating additional build trees on C:.
