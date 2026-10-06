# Token storage and Android integration

Monica CLI creates GitHub/GitLab credentials as native MDBX3 `api-token` entries. Their tokens are not stored in a public note or custom display field. It can also bind existing Android API Keys stored as v1 `login` (`kind=password`, `login_type=API_KEY`) or v1 `api-token` (`monica.api-token.v1` / `monica.gateway.credential.v1`) without converting or rewriting their payloads. See [the local model proxy guide](model-proxy.md).

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

That public-note rule applies to CLI-created gateway credentials. A bound Android object's note/custom fields remain private: `bind --note` explicitly supplies separate public context. Local bindings pin the native UUID, format and head commit; source changes require rebinding and new grants. Upstream keys have no grant TTL, and unbinding never deletes the source object.
- `token`: required secret, available only through an authorized engine disclosure. Mask it in editors and exclude it from logs, previews and MCP output.
- Entry ID, title and collection ID use native MDBX metadata. Category parent IDs use the collection's `group_id`. Moving or renaming a category does not change the Token's identity.
- Recovery scans typed API tokens in all categories. Native type `api-token`, payload version `1` and schema `monica.gateway.credential.v1` must all match. Other formats and records over the Gateway adapter's 16 KiB limit remain in the library without becoming Gateway connections.

Edits patch the original JSON, preserving unknown fields, nested values, precise numbers and the difference between missing, null and empty fields. Renaming changes only the title. Every edit checks the original head commit, native type/version and collection inside the engine transaction; stale edits fail. A future native or inner schema is read-only. Android now has an API Token presentation; support for arbitrary future fields in every Android adapter must be verified separately against its packaged runtime.

Local import uses an engine-produced portable copy plus encrypted Blob sidecars. WebDAV uses conditional snapshots for existing single-file remotes, or immutable Android-compatible segments and content-addressed Blobs when the `.sync` tree exists or attachments require it. Referenced Blobs are verified before sync acknowledgement. See [the compatibility contract](mdbx-compatibility.md) for limits, human viewing and validation details.

Replacing a Token locally preserves its entry ID and revokes old connection grants before writing the replacement. After switching databases, old grants are discarded and must be explicitly recreated.
