# Database-first redesign

Implemented and tested:

- Launch aliases: `monica`, `monicapass`, `monica-pass` through the Windows installer.
- Default database/category/entry home; nested categories, tree navigation, category rename, entry/category moves. `F3` opens the explicit settings pages.
- `d` lists current and previous databases; `n` creates another database without overwriting the previous one. Switching verifies its password and discards old grants.
- Token creation targets the selected category. Dedicated masked Token replacement preserves the native entry ID and revokes existing grants before replacing the encrypted payload.
- Shared CLI operations: `databases/db`, `use`, `library/tree`, `category/mkdir`, `rename-category`, `move/mv`, `connect --category`, `token`.
- The collection Monica for Android saves new entries into (`nameUUIDFromBytes("monica-root:" + vault id)`, a version-3 id) is seeded when a vault is created and re-created or restored on every unlock, so vaults built by earlier releases become writable on the phone without being rebuilt. Reads never needed the row, which is why such a vault browsed but saved nothing. Deleting or re-parenting it is refused with `protected_collection`; renaming it and filing entries into it stay allowed.
- Without `--json`, `databases`, `library` and `webdav list` print aligned tables with translated headers and the ID each follow-up command needs, and write commands answer with one confirmation line instead of a JSON dump. `--json` envelopes are byte-for-byte unchanged.
- Every AI grant is time-boxed: omitting a duration means the 240-minute default, the configurable range is 1–1440 minutes, and an optional call budget is persisted so a broker restart cannot reset it. Legacy zero windows only survive in older config files. When a window or budget closes, calls return `reauthorization_required` until a person runs `monica refresh <grant>`, which rotates the bearer. Authorization stays revocable and requires an unlocked broker session. The stored credential is permanent until a person replaces it and has no expiry of its own.
- Edit forms separate business fields from the database-password confirmation. Esc from confirmation preserves the draft and clears the password. Every management operation closes its engine session; only summary metadata is cached for up to five minutes. Passwords are not cached to simulate an unlocked vault.
- Settings Token creation remains in settings after saving; the WebDAV/MCP end-to-end test covers this navigation regression.
- EN/ZH quick-start documentation reflects the home/settings split and the bounded AI grant window. The public catalog reports expiry in a separate `authorization` object, so a time box never reads as a credential lifetime.

Verification: 101 Rust tests passed (78 library, 10 binary, 9 CLI management, 3 language, 1 portable), Clippy all-targets with warnings denied, and formatting. Synthetic home renderings at 70×20 and 100×30 were visually inspected. These are Ratatui buffer renderings, not native Windows Terminal screenshots.

That tally is the state at this milestone, not the current suite: the run of 2026-09-22 that added the Android write folder and the human-mode tables reports 225 passed (184 library, 23 binary, 12 CLI management, 2 clipboard, 3 language, 1 portable), with formatting and `clippy --all-targets -D warnings` clean.

The same build was driven end to end against a throwaway vault under `%TEMP%` with synthetic secrets only: `init`, `connect`, `databases`, `library`, `category`, `rename-category`, `rename-entry`, `token`, `move`, a non-empty `delete-category`, and `delete-category` / `move` aimed at the Android folder. The seeded row carries a version-3 id (`713580e8-7409-3cf3-83c6-c9915bebee2d` in that run) and is the only such id in the file, appearing seven times; the two protected operations both answered `protected_collection`. The derivation is additionally pinned in `keys::id` against an independent MD5 implementation, and reproduces the collection id a real Android failure log asked for.

Not measured, and deliberately not claimed:

- what title or position Monica for Android gives that folder — only the id and its derivation are guaranteed to match;
- which field the phone's "unknown version (version 0)" label reads; this CLI writes `format_version = MDBX-2` and `schema_version = 17`;
- the repair across a real phone plus a real WebDAV account — the evidence above is one engine file on one machine;
- the count mismatch between a phone screenshot's "6 entries" and the `entries=8` in that same log.

The separate lifetimes were also demonstrated with the debug binary against a throwaway vault: a one-minute grant returned `reauthorization_required` while `list` and `status` still showed the connection, `refresh` succeeded with the master password only and rejected a payload that also carried a token, and revoking the grant left the stored credential in place.

`monica audit` adds the read side of `gateway.audit.jsonl`, which had been written since the first release but never read by anything in this crate: a trail nobody can open is not a control. It takes `--grant` and `--limit` (1–500, newest first), renders a 6-column table for a person, and keeps `request_id` in `--json` only. The reader projects each line through a 7-field whitelist, so a field a future writer adds cannot reach human or machine output unreviewed; unparsable lines are skipped rather than failing the whole trail. `audit_reader_returns_the_trail_the_gateway_wrote` drives a real `Gateway` (fake upstream, synthetic secrets) and asserts against the file the writer itself produced, including that the token, the vault password and the fixture issue body appear nowhere in it. The table renderer and the command contract are pinned by two further tests. Measured on the real binary: a hand-seeded trail rendered aligned English and zh-CN tables, `--limit 0` and `--limit 501` both exited 2 with the fixed argument-error message, and the reader ran against the developer's own 119-event live trail on a read-only basis. Not measured: the TUI has no audit view, and there is no rotation or retention command — at the 8 MiB cap the broker refuses calls until the operator archives or deletes the file.

The `D:/Apps/MonicaCLI` portable install was refreshed on 2026-09-22 and now reports `monica 0.3.0`, the same number the MCP handshake returns as `serverInfo.version`, so either one tells this build from an older one. Read-only commands were exercised against a throwaway config under `%TEMP%`, never against the real vault.

Scope and limitations:

- [Token format](token-format.md) documents the native encrypted API-token payload and Android integration contract. Portable MDBX roundtrip is tested; Android's current login-only interface still needs a dedicated Token editor. This change does not claim Android UI support.
- Browsing caches public summaries, not an open management session. Saving requires a fresh database password. Cancelling password confirmation preserves the draft; an operation failure after submission currently closes the form.
- The home tree and the connection, grant and WebDAV pages all filter with the same fuzzy subsequence matcher (`/` on the home, a per-page filter elsewhere). It scores only the summaries already on screen, never stored secrets.
- Install only after the application exits; never force-stop a user's session.
