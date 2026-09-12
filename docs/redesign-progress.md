# Database-first redesign — work in progress

Full requested scope remains: executable aliases; clear database/category home with nested folders; separate settings; coherent unlock flow; optional unlimited grants; dedicated Token fields compatible with Android; CLI parity; visual verification; installation after verification.

Implemented in current source:

- `library.rs`: bounded unlocked metadata browsing, native nested categories and moves; shared CLI `library/tree`, `category/mkdir`, `move/mv`.
- `tui/home.rs`: default library page with category navigation, entry details, locked state, explicit F3 settings; n category, m move, c token.
- Native category persistence and cycle rejection test passes. Home navigation/three terminal sizes test passes. 14 settings TUI tests pass after explicit settings entry in fixtures.
- Forms have three-row field spacing and focused hints. New category/move forms currently still ask for password; coherent unlock workflow remains required.
- Unlimited grants use expires_at=0. Existing expiry validation/authentication/UI updated; CLI help corrected. Dedicated unlimited grant regression tests still needed.
- Installer copies executable aliases and detects all alias processes. Do not kill running user apps to install. Previous installation occurred before current home changes: source is ahead of D:/Apps/MonicaCLI.
- Token records remain encrypted native api-token objects with monica.gateway.credential.v1 schema and token field. Inventory now scans API tokens in all native categories so moved tokens can recover. Verify move + reopen + gateway recovery explicitly.

Outstanding:

- Multi-database selection/registration, real interactive category tree focus, search, rename; preserve identities across refresh and mutation.
- Unlock flow separate from editing, avoiding repeated password fields while respecting engine session policy; home cache must be cleared on lock/expiry/vault change. Current browsing unlock closes engine and retains metadata for 300 seconds, so UI must not imply a persistent unlocked engine session.
- Token creation in selected category, editing Token with dedicated field, Android display/roundtrip adapter in current Android MDBX client. Android source located under Monica-main/Monica for Android; inspect its AGENTS.md before editing. Current Mdbx2Repository uses groupId as parent ID.
- Polish visual hierarchy and modal behavior; render actual screenshots using existing scripts/render_tui.py and tests harness. Current home is an initial implementation, not approved final design.
- Verify complete fmt/clippy/tests/release after final changes, update README/automation docs and AGENTS to match actual behavior, then install when program is closed (never force-stop user UI).

Last checks: cargo check passed; clippy all-targets -D warnings passed before last small edits; native nested category persistence test passed; 14 settings tests passed; home test passed. No claim of final completion.
