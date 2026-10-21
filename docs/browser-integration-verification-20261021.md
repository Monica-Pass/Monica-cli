# Browser integration verification — 2026-10-21

Protocol 2 adds version/capability negotiation, durable mutation receipts, bounded key-file input, SSH/OpenPGP management, explicit AI grant creation/renewal and configured WebDAV sync. Integration remains off by default and marked incomplete. Tests use synthetic portable vaults and installed Windows Edge, without user credentials or Android devices.

## Validation

- `fmt --check` passed. Strict all-target Clippy and release build passed, producing both CLI and browser helper. `cargo +1.97.0-x86_64-pc-windows-gnu test --release --all-targets --locked -- --test-threads=1` exited 0: 372 passed, 0 failed, 3 ignored. Totals: 294 library, 39 CLI grammar/output, 22 CLI management and 17 compatibility/clipboard/language/portable tests passed. The three existing ignored tests require independent Android synthetic fixture preparation or returned output; they were not claimed as executed. All 15 browser/receipt regressions passed.
- CLI transport/recovery/localization and global localization: 40 tests passed. Scoped completeness checks cover direct labels and dynamic error/operation/heading maps, preserving global validation and the TypeScript target.
- The final production extension build passed, including scoped localization integration; the shared build identity is d9854431c308eac1c210.
- Actual temporary installer checks passed: compatible metadata, portable setup state, independent Edge/Chrome manifests, missing helper, wrong origin and updating Edge without changing Chrome. Exact registry state restored. Evidence: shared workspace `.codex-tasks/20261021-cli-browser-recovery/raw/installer-result.json`.

## Actual Edge

Run with `MONICA_CLI_INTEROP=1`, optionally `MONICA_CLI_BIN=../monica-pass-cli/target/release`, then `npx playwright test tests/e2e/cli-management.spec.ts`.

Final release-host acceptance passed: 2 tests in about 1 minute, using the frozen production extension build. It also covered binary key-file unlock, fresh-factor SSH creation and focus-loss clearing. Earlier debug-host acceptance passed in 5.8 minutes. A real successful mutation reply was discarded; reconnect and authentication retrieved its durable receipt without replay. The operation was sent once and the folder existed once. A stale edit remained unconfirmed until explicit human acknowledgement. Grant creation defaulted to read-only scopes; renewal rotated the client credential and retained scope/call cap. SSH generation, private OpenPGP import and metadata-only listing passed. Existing selective password replacement, stale-head protection, revocation, page departure and tab-switch locking remained covered. The default-off opt-in/disable test also passed.

Twelve additional lock/unlock cycles with deliberate settings navigation every third cycle preserved CLI context. Two earlier baseline Edge runs passed. The historical trace does show an unlock attempt ending on settings with opt-in still enabled, but this symptom was not reproduced. No root cause or fix is claimed; this task did not change App.vue. Public navigation labels and operation IDs are recorded without inputs or secrets.

Final release evidence: `%TEMP%/monica-cli-edge-g0RQ4v/`; an additional final run passed in 42 seconds at `%TEMP%/monica-cli-edge-8ihTPc/`, with the configuration-secret assertion checked before database replacement; earlier debug evidence: `%TEMP%/monica-cli-edge-NESMth/` (`public-navigation.json`, `result.json`, `native-host-evidence.json`). Registry restoration verified; profiles and copied helper removed. Local editable Canvas: extension `docs/design/cli-recovery-20261021.m3e.json` and its adjacent local project URL file. Screenshots/designs remain local, unstaged.

## Coverage and limits

Regressions cover restart deduplication, interrupted intent non-replay, ID scope, cross-process exclusive locking, full capacity without eviction, protocol/ID requirements, fresh factors, GPG secret exclusion, binary key-file authentication and portable sync with pinned vault identity/factors. Existing tests cover native heads, protected folders, unknown/precise-number fields, expiry, origins/framing and Token revocation.

Chrome registration was tested; Chrome itself and other operating systems were not browser-tested. Fake TLS WebDAV verifies the shared sync engine's pinned identity and key factors. The browser sync form has not received live-service acceptance. Programmatic file selection and explicit blur tests do not certify all native OS file choosers. Key files/imported text are limited to 32 KiB. No existing-secret reader, MCP tool, arbitrary path/origin, shell operation, listener or export was added.

The 11.14 GiB Rust target cache moved from C to `E:/CodexBuildCache/monica-pass-cli`; the original target is a junction. C free space increased from about 35.25 to 44.93 GB immediately after migration. Builds reuse that cache without a second target tree or cargo clean. That browser-acceptance stage did not commit, push, tag or release; subsequent CI and source publication are described in [ci.md](ci.md).

See [browser-integration.md](browser-integration.md) for setup, protocol and receipt limits.
