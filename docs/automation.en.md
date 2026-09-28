# CLI automation

[简体中文](automation.md) · **English**

The TUI and CLI use the same management functions. People can use hidden input and short commands; AI and scripts can submit public parameters while a trusted local executor supplies credentials and reads structured results. MCP continues to expose only granted service operations.

MDBX edits preserve extension fields and check the original commit. `object_read_only` means this adapter cannot safely modify the type/version; `object_changed` requires a fresh read and review. Generic object disclosure is human TUI only. Blob transfers and segment checkpoints are confirmed together; `blob_unavailable` and `sync_cancelled` require inspecting sync status before resuming. See [MDBX compatibility](mdbx-compatibility.md).

## Discover commands

```sh
monica-pass commands --summary --json
monica-pass commands add --json
monica-pass commands webdav open --json
```

Discovery comes from the actual argument parser, including command names, aliases, arguments, defaults, choices, exclusive groups, and required secret fields. Command paths accept aliases too. JSON discovery uses fixed English descriptions. Omit `--json` for help in your selected interface language.

## Inspect execution semantics

Start with the compact index, then read `monica commands grant --json` before executing a command. The summary is not the full argument schema. Plain `commands --json` still returns the complete tree.

| Full discovery field | Meaning |
| --- | --- |
| `path` / `aliases` | Preferred command path and compatible spellings |
| `execution_command` | Stable command identifier in execution JSON |
| `arguments[].role` / `value_names` | CONNECTION, GRANT, database or category ID |
| `secret_input.required` | Stdin fields supplied by a trusted executor; shared with execution |
| `semantics.target` / `prerequisites` | Target and required preconditions |
| `semantics.effects[]` | `{when, effect}`; for example --install merges client settings |
| `semantics.trust_boundary` | Local inspection, trusted management or grant-scoped broker |
| `semantics.retry` | What to inspect before repeating; renew rotates capability every time |
| `semantics.discovery_grants_authority` | Always false; discovery grants no permission |

Prefer `connections`, `mcp-config`, and `renew`. Legacy `list`, `settings`, `refresh` and aliases remain accepted. Execution JSON retains the identifiers `list`, `settings`, and `refresh`; `execution_command` exposes that mapping.

`mcp-config GRANT` writes a Monica snippet even without --install. `library`, `keys`, and `tiga show` require a password and stop the broker first; they are not pure inspection operations.

`next --grant GRANT --json` reports observed readiness through `steps[].done`; client integration remains `unverified`. Configure the client before foreground serve, and run check in another terminal. Replace placeholders in suggestions and retain the same --config throughout.

## Common options

| Option | Behavior |
| --- | --- |
| `--json` / `-j` | Emit JSON and disable prompts. |
| `--non-interactive` / `--no-prompt` | Disable prompts while retaining normal output formatting. |
| `--secrets-stdin` | Read credential JSON directly from a trusted producer on stdin. |
| `--config FILE` / `-C FILE` | Select the management configuration. |
| `--lang en` / `-l en` | Set the human-facing language; JSON fields and error codes stay stable. |

Global options work before or after subcommands. With no subcommand, normal mode opens the TUI; JSON or non-interactive mode shows status. `tui` rejects automation modes. `mcp` owns its stdio transport and cannot be combined with `--json` or `--secrets-stdin`.

## Management operations

| Operation | CLI example | Secret fields |
| --- | --- | --- |
| Create a vault, connection, and read-only grant | `add work --repo org/repo --note "Project purpose"` | `password`, `token` |
| Create a vault separately | `init` | `password` |
| Save a connection only | `connect work --provider github --note "Project purpose"` | `password`, `token` |
| List / inspect connections | `connections` / `show work` | None |
| Edit a purpose note | `note work "Updated purpose"` | `password` |
| Run a generic service API request | `call reader --request request.json` | None; needs an unlocked broker and an explicit API grant |
| Replace a stored Token (revokes its old grants) | `token work` | `password`, `token` |
| List / switch databases | `databases` / `use ID` | None / `password` |
| Browse categories and entries | `library` | `password` |
| Create / rename a category | `category "Title" --parent ID` / `rename-category ID "Title"` | `password` |
| Move an entry or category | `move ID TARGET_CATEGORY_ID` | `password` |
| Issue a grant | `grant reader --connection work --repo org/repo --ttl-minutes 60 --max-calls 200` | `password` |
| Re-authorize a grant (new capability) | `renew reader` / `renew reader --ttl-minutes 60 --max-calls 50` | `password` |
| Put a human approval gate on a grant | `grant reader … --approval write` / `renew reader --approval all` | `password` |
| Revoke a grant | `revoke reader` | None |
| Get and save MCP settings | `mcp-config reader` | None |
| Also write the entry into the AI client's own file | `mcp-config reader --install claude` (`cursor` / `codex` / `vscode`) | None; the file is backed up first and left untouched if the merge is refused |
| Verify MCP discovery | `check reader` or `check --client FILE` | None; the broker must be unlocked |
| Open a managed local MDBX copy | `open vault.mdbx` | `password` |
| Unlock and serve | `serve` | `password` |
| Lock and wait for the broker to stop | `lock` | None |
| Show status | `status` | None |
| Sign in to WebDAV | `webdav login --url https://dav.example.com/monica/ -n user` | `webdav_password` |
| Browse WebDAV | `webdav list [folder]` | `webdav_password` |
| Open a remote MDBX | `webdav open vault.mdbx` | `password`, `webdav_password` |
| Publish / sync MDBX | `webdav publish vault.mdbx` / `webdav sync` | `password`, `webdav_password` |
| Show WebDAV configuration | `webdav status` | None |
| Save a language preference | `language en` | None |

Names must match exactly. `show` selects a connection; `mcp-config`, `check`, `revoke`, and `renew` select a grant. Unknown names never fall back to another record. Quick add uses the same name for the connection and grant, with read-only access for 240 minutes by default; `--ttl` takes 1–1440 minutes and `--max-calls` can cap upstream calls. No authorization is indefinite. `-w` / `--allow-write` explicitly enables Issue creation. Detailed grants use `--op create-issue` to allow writes.

Both `grant` and `renew` stop the broker first and leave it locked afterwards. Unlock it again with `serve`, and restart the AI client's MCP entry so it reads the newly issued capability.

`--approval` (`off|write|all`) does not belong in an unattended pipeline: the gate needs a person answering on the spot, and a broker started with `--secrets-stdin` usually has no terminal to ask in (`--json`, a pipe, or a background process all qualify). Such a broker **fails closed** — a gated call waits out 15 seconds and returns `approval_timeout`, the request is never sent and spends none of the grant's call budget (the per-minute limit is still charged), and a broker that starts while some grant carries a gate says up front that it has no terminal to ask in. Leave automation grants at `off` and narrow the risk with `--max-calls` plus a short `-t`; if you want the gate, run the broker in your own terminal. Quick `add` has no such flag, so its grants are always `off`.

## Secret input contract

`--secrets-stdin` accepts one **UTF-8 JSON object**, at most **16 KiB**, followed by EOF. Only the fields required by the command are accepted. Values must be nonempty strings. Unknown, duplicate, extra or null fields, invalid JSON, and oversized input are rejected. Spaces and Unicode in passwords are preserved.

- `password`: vault master password, with no minimum length. Empty or whitespace-only passwords are rejected. When opening a remote vault, supply that vault's password.
- `token`: service token to store.
- `webdav_password`: WebDAV password or app password, used only in this process.

Human vault creation asks for password confirmation. Programmatic creation uses the producer's single password value and needs no confirmation field. Input buffers and parsed secrets use `Zeroizing`. The command does not create plaintext credential files.

AI can construct a command containing only public parameters:

```sh
monica-pass add work --repo org/repo --note "Project issue tracking" --json --secrets-stdin
```

The trusted executor obtains the secrets and writes directly to the child's stdin. Do not have the model generate JSON containing real credentials, or put secrets in `echo`, shell here-strings, process arguments, or environment variables. Without a trusted input channel, the command fails explicitly rather than asking AI to obtain the raw secret.

This Python launcher can run in a local human terminal. It prompts invisibly, sends the secret input directly to Monica, and forwards only Monica's results. Replace the two `getpass` calls inside the trusted executor when integrating a credential provider:

```python
import getpass
import json
import subprocess
import sys

credentials = {
    "password": getpass.getpass("Vault password: "),
    "token": getpass.getpass("Service token: "),
}
result = subprocess.run(
    ["monica-pass", "add", "work", "--repo", "org/repo",
     "--note", "Project issue tracking", "--json", "--secrets-stdin"],
    input=json.dumps(credentials, ensure_ascii=False).encode("utf-8"),
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    check=False,
)
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
raise SystemExit(result.returncode)
```

The launcher is trusted infrastructure and must not return `credentials` or the input JSON to the model. Ordinary AI service access uses only an MCP capability; that capability does not authorize local management. AI tools with arbitrary file access, environment inspection or debugging permissions require additional OS account or sandbox isolation. See [SECURITY.md](../SECURITY.md) for the detailed boundary.

## Results and exit status

A finite JSON command emits one JSON object on stdout. Success and failure results contain no human prompts. For example:

```json
{"ok":true,"command":"note","data":{"name":"work","note":"Project issue tracking"}}
```

Failures return `ok: false`, `error.code`, and `error.message`. Missing secrets also return `error.required`, such as `["password", "token"]`. Exit codes are `0` for success, `1` for execution errors, and `2` for argument errors. Rejected argument values and secret input are not echoed in errors.

`serve` stays running. It emits `event: "ready"` immediately after startup and `event: "stopped"` on shutdown, each on its own flushed JSON line. `add --serve` emits its creation result before broker events. If broker startup subsequently fails, the created connection and grant still exist; handle the completed step separately.

Management that accesses the vault, including sync, requests a broker lock and waits for in-flight operations to drain. Failure to acquire the lock is explicit. The broker stays locked after these operations; `add -s` starts it immediately after adding. Metadata queries and revocation do not stop the broker. Unlock sessions last five minutes and do not extend grants. Once a grant's window or call budget is spent, the proxy answers `reauthorization_required` and only a human `renew GRANT` restores it; `status` reports each grant's `calls_used`, `max_calls`, `expired`, `refresh_required`, and `approval` gate. A call the gate stops answers `approval_denied` or `approval_timeout`: the request was not sent, nothing was written to the replay journal or the `authorized` audit row, so an identical retry does not duplicate an upstream write. Still, after `approval_denied`, stop and await new instructions from the person; after `approval_timeout`, obtain their agreement before retrying with the same `request_id` and identical arguments. Retrying never bypasses the question.

A `--secrets-stdin` password does not persist across CLI processes and never enters the local credential manager: supply it for each network operation, and command exit ends that session. A password you type at the hidden prompt is stored in this computer's credential manager once a request using it succeeds, so later operations no longer ask for it; `webdav forget-password` removes it. TUI `:logout` ends only its own WebDAV session. URLs and usernames can be saved, and `webdav status` reports them plus whether a password is stored.

`status` and `webdav status` expose `safe_remote_replace`: `null` without a connected vault, `false` when it has no strong ETag, and `true` when a strong ETag is available. With `false`, reading, checking sync state, and publishing a new filename remain available, but local changes cannot replace the remote file. Use `webdav publish NEW_NAME.mdbx`. The TUI dashboard and WebDAV preview show the same limitation.


CLI errors add `error.recovery`: a fixed retry policy, suggested command templates, `automatic_retry: false`, and `use_same_config`. These do not echo rejected arguments or authorize running the suggestions. Missing secrets go back to the trusted executor; missing confirmation never permits automatically adding --force. Expired grants require human renewal. Inspect upstream state and audit before retrying write_outcome_unknown / request_id_conflict; do not blindly write with a new request_id. Parser errors also provide a discovery entry point. MCP stdio retains its own protocol and never includes these CLI hints.
