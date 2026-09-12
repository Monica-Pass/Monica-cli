# Token storage and Android integration

Monica CLI stores credentials as native MDBX3 `api-token` entries. A Token is not a password/login record and is not stored in a public note or custom display field.

The encrypted JSON payload has this versioned contract (synthetic example):

```json
{
  "schema": "monica.gateway.credential.v1",
  "provider": "gitlab",
  "api_base": "https://gitlab.example.test/api/v4/",
  "note": "Project issue tracking",
  "token": "synthetic-token-example"
}
```

- `provider`: `github` or `gitlab`. `api_base` is a validated HTTPS API root.
- `note`: optional public context, at most 1024 UTF-8 bytes; never a place for secrets.
- `token`: required secret, available only through an authorized engine disclosure. Mask it in editors and exclude it from logs, previews and MCP output.
- Entry ID, title and collection ID use native MDBX metadata. Category parent IDs use the collection's `group_id`. Moving or renaming a category does not change the Token's identity.
- Recovery scans typed API tokens in all categories. The schema must match exactly; unrelated `api-token` payloads are not interpreted as gateway connections.

For future Android support, recognize the native type plus schema, show a dedicated masked Token editor, and write back the same type, entry identity, category and payload fields. Do not convert these records to `login` or silently drop unrecognized records. The existing Android login-only presentation does not yet provide this editor; Android UI interoperability is not claimed by this release.

Sync transfers an engine-produced encrypted portable MDBX snapshot, not a JSON credential export. CLI tests verify nested category creation, Token movement, category renaming, portable reopen, and recovery of the original credential binding. They use only synthetic credentials. Android runtime/build validation remains a separate integration task.

Replacing a Token locally preserves its entry ID and revokes old connection grants before writing the replacement. After switching databases, old grants are discarded and must be explicitly recreated.
