# CLI automation

[简体中文](automation.md) · **English**

The TUI and CLI use the same management functions. People can use hidden input and short commands; AI and scripts can submit public parameters while a trusted local executor supplies credentials and reads structured results. MCP continues to expose only granted service operations.

## Discover commands

```sh
monica-pass cmds --json
monica-pass cmds add --json
monica-pass cmds dav open --json
```

Discovery comes from the actual argument parser, including command names, aliases, arguments, defaults, choices, exclusive groups, and required secret fields. Command paths accept aliases too. JSON discovery uses fixed English descriptions. Omit `--json` for help in your selected interface language.

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
| Create a vault, connection, and read-only grant | `a work -r org/repo -n "Project purpose"` | `password`, `token` |
| Create a vault separately | `n` | `password` |
| Save a connection only | `c work -p github -n "Project purpose"` | `password`, `token` |
| List / inspect connections | `ls` / `show work` | None |
| Edit a purpose note | `e work "Updated purpose"` | `password` |
| Run a generic service API request | `call reader --request request.json` | None; needs an unlocked broker and an explicit API grant |
| Replace a stored Token (revokes its old grants) | `token work` | `password`, `token` |
| List / switch databases | `db` / `use ID` | None / `password` |
| Browse categories and entries | `tree` | `password` |
| Create / rename a category | `mkdir "Title" --parent ID` / `rename-category ID "Title"` | `password` |
| Move an entry or category | `mv ID TARGET_CATEGORY_ID` | `password` |
| Issue a grant | `g reader -c work -r org/repo -t 60 --max-calls 200` | `password` |
| Re-authorize a grant (new capability) | `rf reader` / `rf reader -t 60 --max-calls 50` | `password` |
| Revoke a grant | `rv reader` | None |
| Get and save MCP settings | `m reader` | None |
| Verify MCP discovery | `ck reader` or `check --client FILE` | None; the broker must be unlocked |
| Open a managed local MDBX copy | `o vault.mdbx` | `password` |
| Unlock and serve | `u` | `password` |
| Lock and wait for the broker to stop | `lk` | None |
| Show status | `st` | None |
| Sign in to WebDAV | `dav in -u https://dav.example.com/monica/ -n user` | `webdav_password` |
| Browse WebDAV | `dav ls [folder]` | `webdav_password` |
| Open a remote MDBX | `dav o vault.mdbx` | `password`, `webdav_password` |
| Publish / sync MDBX | `dav p vault.mdbx` / `dav s` | `password`, `webdav_password` |
| Show WebDAV configuration | `dav st` | None |
| Save a language preference | `lang en` | None |

Names must match exactly. `show` selects a connection; `m`, `ck`, `rv`, and `rf` select a grant. Unknown names never fall back to another record. Quick add uses the same name for the connection and grant, with read-only access for 240 minutes by default; `--ttl` takes 1–1440 minutes and `--max-calls` can cap upstream calls. No authorization is indefinite. `-w` / `--allow-write` explicitly enables Issue creation. Detailed grants use `--op create-issue` to allow writes.

Both `grant` and `refresh` stop the broker first and leave it locked afterwards. Unlock it again with `u`, and restart the AI client's MCP entry so it reads the newly issued capability.

## Secret input contract

`--secrets-stdin` accepts one **UTF-8 JSON object**, at most **16 KiB**, followed by EOF. Only the fields required by the command are accepted. Values must be nonempty strings. Unknown, duplicate, extra or null fields, invalid JSON, and oversized input are rejected. Spaces and Unicode in passwords are preserved.

- `password`: vault master password, with no minimum length. Empty or whitespace-only passwords are rejected. When opening a remote vault, supply that vault's password.
- `token`: service token to store.
- `webdav_password`: WebDAV password or app password, used only in this process.

Human vault creation asks for password confirmation. Programmatic creation uses the producer's single password value and needs no confirmation field. Input buffers and parsed secrets use `Zeroizing`. The command does not create plaintext credential files.

AI can construct a command containing only public parameters:

```sh
monica-pass a work -r org/repo -n "Project issue tracking" --json --secrets-stdin
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
    ["monica-pass", "a", "work", "-r", "org/repo",
     "-n", "Project issue tracking", "--json", "--secrets-stdin"],
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

`serve` / `u` stays running. It emits `event: "ready"` immediately after startup and `event: "stopped"` on shutdown, each on its own flushed JSON line. `add --serve` emits its creation result before broker events. If broker startup subsequently fails, the created connection and grant still exist; handle the completed step separately.

Management that accesses the vault, including sync, requests a broker lock and waits for in-flight operations to drain. Failure to acquire the lock is explicit. The broker stays locked after these operations; `a -s` starts it immediately after adding. Metadata queries and revocation do not stop the broker. Unlock sessions last five minutes and do not extend grants. Once a grant's window or call budget is spent, the proxy answers `reauthorization_required` and only a human `rf GRANT` restores it; `st` reports each grant's `calls_used`, `max_calls`, `expired`, and `refresh_required`.

A `--secrets-stdin` password does not persist across CLI processes and never enters the local credential manager: supply it for each network operation, and command exit ends that session. A password you type at the hidden prompt is stored in this computer's credential manager once a request using it succeeds, so later operations no longer ask for it; `dav forget-password` removes it. TUI `:logout` ends only its own WebDAV session. URLs and usernames can be saved, and `dav st` reports them plus whether a password is stored.

`status` and `dav st` expose `safe_remote_replace`: `null` without a connected vault, `false` when it has no strong ETag, and `true` when a strong ETag is available. With `false`, reading, checking sync state, and publishing a new filename remain available, but local changes cannot replace the remote file. Use `dav p NEW_NAME.mdbx`. The TUI dashboard and WebDAV preview show the same limitation.
