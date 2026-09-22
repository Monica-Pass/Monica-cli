# Database-first redesign

Implemented and tested:

- Launch aliases: `monica`, `monicapass`, `monica-pass` through the Windows installer.
- Default database/category/entry home; nested categories, tree navigation, category rename, entry/category moves. `F3` opens the explicit settings pages.
- `d` lists current and previous databases; `n` creates another database without overwriting the previous one. Switching verifies its password and discards old grants.
- Token creation targets the selected category. Dedicated masked Token replacement preserves the native entry ID and revokes existing grants before replacing the encrypted payload.
- Shared CLI operations: `databases/db`, `use`, `library/tree`, `category/mkdir`, `rename-category`, `move/mv`, `connect --category`, `token`.
- Every AI grant is time-boxed: omitting a duration means the 240-minute default, the configurable range is 1–1440 minutes, and an optional call budget is persisted so a broker restart cannot reset it. Legacy zero windows only survive in older config files. When a window or budget closes, calls return `reauthorization_required` until a person runs `monica refresh <grant>`, which rotates the bearer. Authorization stays revocable and requires an unlocked broker session. The stored credential is permanent until a person replaces it and has no expiry of its own.
- Edit forms separate business fields from the database-password confirmation. Esc from confirmation preserves the draft and clears the password. Every management operation closes its engine session; only summary metadata is cached for up to five minutes. Passwords are not cached to simulate an unlocked vault.
- Settings Token creation remains in settings after saving; the WebDAV/MCP end-to-end test covers this navigation regression.
- EN/ZH quick-start documentation reflects the home/settings split and the bounded AI grant window. The public catalog reports expiry in a separate `authorization` object, so a time box never reads as a credential lifetime.

Verification: 101 Rust tests passed (78 library, 10 binary, 9 CLI management, 3 language, 1 portable), Clippy all-targets with warnings denied, and formatting. Synthetic home renderings at 70×20 and 100×30 were visually inspected. These are Ratatui buffer renderings, not native Windows Terminal screenshots.

That tally is the state at this milestone, not the current suite: the run of 2026-09-22 reports 221 passed (183 library, 20 binary, 12 CLI management, 2 clipboard, 3 language, 1 portable).

The separate lifetimes were also demonstrated with the debug binary against a throwaway vault: a one-minute grant returned `reauthorization_required` while `list` and `status` still showed the connection, `refresh` succeeded with the master password only and rejected a payload that also carried a token, and revoking the grant left the stored credential in place.

The `D:/Apps/MonicaCLI` portable install was refreshed on 2026-09-22 and now reports `monica 0.3.0`, the same number the MCP handshake returns as `serverInfo.version`, so either one tells this build from an older one. Read-only commands were exercised against a throwaway config under `%TEMP%`, never against the real vault.

Scope and limitations:

- [Token format](token-format.md) documents the native encrypted API-token payload and Android integration contract. Portable MDBX roundtrip is tested; Android's current login-only interface still needs a dedicated Token editor. This change does not claim Android UI support.
- Browsing caches public summaries, not an open management session. Saving requires a fresh database password. Cancelling password confirmation preserves the draft; an operation failure after submission currently closes the form.
- The home tree and the connection, grant and WebDAV pages all filter with the same fuzzy subsequence matcher (`/` on the home, a per-page filter elsewhere). It scores only the summaries already on screen, never stored secrets.
- Install only after the application exits; never force-stop a user's session.
