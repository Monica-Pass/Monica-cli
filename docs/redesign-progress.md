# Database-first redesign

Implemented and tested:

- Launch aliases: `monica`, `monicapass`, `monica-pass` through the Windows installer.
- Default database/category/entry home; nested categories, tree navigation, category rename, entry/category moves. `F3` opens the explicit settings pages.
- `d` lists current and previous databases; `n` creates another database without overwriting the previous one. Switching verifies its password and discards old grants.
- Token creation targets the selected category. Dedicated masked Token replacement preserves the native entry ID and revokes existing grants before replacing the encrypted payload.
- Shared CLI operations: `databases/db`, `use`, `library/tree`, `category/mkdir`, `rename-category`, `move/mv`, `connect --category`, `token`.
- Grant expiry is optional: blank or zero means no expiry, including quick setup. Authorization remains revocable and requires an unlocked broker session.
- Edit forms separate business fields from the database-password confirmation. Esc from confirmation preserves the draft and clears the password. Every management operation closes its engine session; only summary metadata is cached for up to five minutes. Passwords are not cached to simulate an unlocked vault.
- Settings Token creation remains in settings after saving; the WebDAV/MCP end-to-end test covers this navigation regression.
- EN/ZH quick-start documentation reflects the home/settings split and indefinite grant default.

Verification: 82 Rust tests passed (64 library, 6 binary, 8 CLI management, 3 language, 1 portable), Clippy all-targets with warnings denied, and formatting. Synthetic home renderings at 70×20 and 100×30 were visually inspected. These are Ratatui buffer renderings, not native Windows Terminal screenshots.

Scope and limitations:

- [Token format](token-format.md) documents the native encrypted API-token payload and Android integration contract. Portable MDBX roundtrip is tested; Android's current login-only interface still needs a dedicated Token editor. This change does not claim Android UI support.
- Browsing caches public summaries, not an open management session. Saving requires a fresh database password. Cancelling password confirmation preserves the draft; an operation failure after submission currently closes the form.
- Settings support search/filtering; the home uses category navigation.
- Install only after the application exits; never force-stop a user's session.
