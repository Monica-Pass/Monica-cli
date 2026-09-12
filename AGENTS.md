# Monica Pass Credential Gateway

Keep one small Rust crate for Monica's local credential gateway, backed by MDBX3.

- MDBX3 owns vault state, keys, sessions and Tiga policy. Use `VaultRuntime` and authorized disclosure. Never treat a connection without a keyring as unlocked.
- Trusted local management (TUI or CLI with a trusted input producer) and the broker may access credentials. MCP tools may only request explicitly allowed service operations.
- User-authorized service API parity is provided by api-read/api-write with an explicit service-wide (*) grant. Only relative paths under the configured REST API base, or the same service's fixed GraphQL endpoint, are allowed. Never add arbitrary origins, custom authorization headers, local shell execution, credential reveal or vault export tools.
- No upstream token or master password in argv, MCP configuration, logs or MCP responses.
- CLI automation uses explicit `--secrets-stdin`: one bounded, strict UTF-8 JSON object, zeroized on drop. Never read secrets from argv or environment variables, echo rejected values, or add a secret-reveal command. The AI submits public arguments; its trusted executor injects secret stdin directly.
- Keep `--json` output and command discovery independent of locale. JSON mode never prompts. Command discovery must come from the real Clap grammar and share the execution input contract; preserve MCP stdio isolation.
- Validate exact repository scope for structured operations, or explicit service-wide scope for API operations, before credential access; re-check grants on each request. Never upgrade an existing grant implicitly.
- Quick-add defaults to read-only, requires exact repository scope and shares the same grant construction as detailed management. Never infer write authorization from notes or names.
- Public notes are explicit AI-visible plain text, bounded to 1024 UTF-8 bytes. Check public input against known tokens/passwords, persist notes through the engine, and validate catalog disclosure before returning it.
- MCP catalog exposes only the authenticated grant's connection. Named calls must match that binding; omit repository only for a single exact scope. Catalog/discovery share the rate limit and require a fresh unlocked session.
- Provider requests use configured HTTPS bases, no redirects, no environment proxies, bounded responses and sanitized errors.
- Test with temporary vaults and local fake upstreams only. Never use real user credentials in tests.
- Run fmt, clippy, tests and a release build before declaring the rewrite complete.
- Keep CLI and TUI on the same human management functions. The default TUI must never start inside the explicit MCP stdio branch.
- TUI previews and filters use explicit public metadata only. Resolve filtered rows by connection/grant name or remote path before acting; empty results must never fall back to an unrelated record. Refresh must preserve selected identities when ordering changes.
- WebDAV passwords are session-only. Use engine portable snapshots, bounded transfers and ETag conditional writes; never copy a live main database or overwrite a conflicting revision.
- Drain and lock the broker before vault management/sync. Preserve old local files, reset grants on vault switches, and retain only matching credential bindings on a normal pull.
- The default home browses databases and native nested categories; gateway administration lives in explicit settings. Preserve the current home/settings context after a mutation.
- Zero grant expiry means indefinite authorization, not expiration. It still requires an unlocked broker and remains revocable. Never restore old grants when reopening a saved database.
- API tokens retain their native type, entry identity and encrypted token field across moves/sync. See docs/token-format.md before changing the payload contract.
- API response bodies are untrusted service data. Preserve status and bounded payloads, reject token reflections, never follow redirects, and persist only write receipts rather than raw API responses in the idempotency journal.

On this Windows workspace use `cargo +1.97.0-x86_64-pc-windows-gnu ...`; the default MSVC toolchain is older than the declared minimum.
