//! Compact, locale-aware human rendering for read-only CLI output.
//! Only the non-`--json` path uses this; `--json` stays machine-stable.

use chrono::{Local, TimeZone};
use monica_pass_cli::i18n::{Language, human_bytes};
use monica_pass_cli::keys::payload::LOGIN_TYPE_SSH;
use monica_pass_cli::model::Provider;
use monica_pass_cli::tr;
use monica_pass_cli::vault::KeyEntrySummary;
use serde_json::Value;
use std::path::Path;
use unicode_width::UnicodeWidthStr;

fn width(text: &str) -> usize {
    text.width()
}

fn string(item: &Value, key: &str) -> String {
    item[key].as_str().unwrap_or_default().to_string()
}

fn join_array(item: &Value, key: &str) -> String {
    item[key]
        .as_array()
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn pad(text: &str, column: usize) -> String {
    let fill = column.saturating_sub(width(text));
    let mut out = String::with_capacity(text.len() + fill);
    out.push_str(text);
    for _ in 0..fill {
        out.push(' ');
    }
    out
}

fn join_row(cells: &[String], columns: &[usize]) -> String {
    let last = cells.len().saturating_sub(1);
    let mut parts: Vec<String> = Vec::with_capacity(cells.len());
    for (index, cell) in cells.iter().enumerate() {
        if index == last {
            parts.push(cell.clone());
        } else {
            parts.push(pad(cell, columns[index]));
        }
    }
    parts.join("  ").trim_end().to_string()
}

fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut columns: Vec<usize> = headers.iter().map(|header| width(header)).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate().take(columns.len()) {
            columns[index] = columns[index].max(width(cell));
        }
    }
    let header_cells: Vec<String> = headers.iter().map(|header| (*header).to_string()).collect();
    let mut out = join_row(&header_cells, &columns);
    for row in rows {
        out.push('\n');
        out.push_str(&join_row(row, &columns));
    }
    out
}

fn is_custom_base(item: &Value) -> bool {
    let base = item["api_base"].as_str().unwrap_or_default();
    match serde_json::from_value::<Provider>(item["provider"].clone()) {
        Ok(provider) => base != provider.default_api_base(),
        Err(_) => !base.is_empty(),
    }
}

pub fn render_connections(connections: &Value, lang: Language) -> String {
    let items: Vec<&Value> = connections
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if items.is_empty() {
        return tr!(lang, ListNoConnections).to_string();
    }
    let show_api = items.iter().any(|item| is_custom_base(item));
    let mut headers: Vec<&str> = vec![tr!(lang, TableColumnHandle), tr!(lang, TableColumnProvider)];
    if show_api {
        headers.push(tr!(lang, TableColumnApi));
    }
    headers.push(tr!(lang, TableColumnNote));
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(items.len());
    for &item in &items {
        let mut row = vec![string(item, "name"), string(item, "provider")];
        if show_api {
            row.push(string(item, "api_base"));
        }
        row.push(string(item, "note"));
        rows.push(row);
    }
    render_table(&headers, &rows)
}

fn format_expiry(grant: &Value, lang: Language) -> String {
    let timestamp = grant["expires_at_unix"].as_i64().unwrap_or(0);
    if timestamp == 0 {
        return tr!(lang, GrantNeverExpires).to_string();
    }
    let when = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|moment| moment.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string());
    if grant["expired"].as_bool().unwrap_or(false) {
        format!("{when} · {}", tr!(lang, GrantExpiredMarker))
    } else {
        when
    }
}

fn format_calls(grant: &Value, lang: Language) -> String {
    let max = grant["max_calls"].as_i64().unwrap_or(0);
    if max == 0 {
        return tr!(lang, GrantCallsUnlimited).to_string();
    }
    let used = grant["calls_used"].as_i64().unwrap_or(0);
    format!("{used}/{max}")
}

pub fn render_audit(audit: &Value, lang: Language) -> String {
    let events: Vec<&Value> = audit["events"]
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if events.is_empty() {
        return format!("{}\n{}", tr!(lang, AuditEmpty), string(audit, "path"));
    }
    let headers = [
        tr!(lang, TableColumnTime),
        tr!(lang, TableColumnGrant),
        tr!(lang, TableColumnOperation),
        tr!(lang, TableColumnScope),
        tr!(lang, TableColumnStage),
        tr!(lang, TableColumnResult),
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(events.len());
    for &event in &events {
        let stage = string(event, "stage");
        let error = event["error"].as_str();
        let result = match error {
            Some(code) => code.to_string(),
            None if stage == "authorized" => tr!(lang, AuditNoOutcomeYet).to_string(),
            None => tr!(lang, AuditOk).to_string(),
        };
        rows.push(vec![
            audit_time(event["timestamp"].as_i64().unwrap_or_default()),
            dash_if_empty(string(event, "grant")),
            dash_if_empty(string(event, "operation")),
            dash_if_empty(string(event, "repository")),
            match stage.as_str() {
                "authorized" => tr!(lang, AuditStageAuthorized).to_string(),
                "finished" => tr!(lang, AuditStageFinished).to_string(),
                other => other.to_string(),
            },
            result,
        ]);
    }
    let mut out = render_table(&headers, &rows);
    let hidden = audit["total"]
        .as_u64()
        .unwrap_or(0)
        .saturating_sub(audit["shown"].as_u64().unwrap_or(0));
    if hidden > 0 {
        out.push('\n');
        out.push_str(&tr!(lang, AuditHidden, count = hidden));
    }
    out
}

fn audit_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|moment| moment.format("%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn dash_if_empty(value: String) -> String {
    if value.is_empty() {
        "—".to_string()
    } else {
        value
    }
}

pub fn render_connection_detail(connection: &Value, lang: Language) -> String {
    let fields: [(&str, String); 4] = [
        (tr!(lang, TableColumnHandle), string(connection, "name")),
        (
            tr!(lang, TableColumnProvider),
            string(connection, "provider"),
        ),
        (tr!(lang, TableColumnApi), string(connection, "api_base")),
        (tr!(lang, TableColumnNote), string(connection, "note")),
    ];
    let label_width = fields
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in &fields {
        out.push_str(&pad(label, label_width));
        out.push_str("  ");
        out.push_str(value);
        out.push('\n');
    }
    out.push('\n');
    let grants: Vec<&Value> = connection["grants"]
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if grants.is_empty() {
        out.push_str(tr!(lang, ShowNoGrants));
        return out.trim_end().to_string();
    }
    let headers = [
        tr!(lang, TableColumnGrant),
        tr!(lang, TableColumnScope),
        tr!(lang, TableColumnOperations),
        tr!(lang, TableColumnExpires),
        tr!(lang, TableColumnCalls),
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(grants.len());
    for &grant in &grants {
        rows.push(vec![
            string(grant, "name"),
            join_array(grant, "repositories"),
            join_array(grant, "operations"),
            format_expiry(grant, lang),
            format_calls(grant, lang),
        ]);
    }
    out.push_str(&render_table(&headers, &rows));
    out.trim_end().to_string()
}

pub fn render_status(status: &Value, lang: Language) -> String {
    let running = status["broker_running"].as_bool().unwrap_or(false);
    let stopping = status["lock_requested"].as_bool().unwrap_or(false);
    let gateway = if !running {
        tr!(lang, StatusStopped)
    } else if stopping {
        tr!(lang, StatusStopping)
    } else {
        tr!(lang, StatusRunning)
    };
    let webdav = if status["webdav"].is_null() {
        tr!(lang, StatusOff).to_string()
    } else {
        let profile = &status["webdav"]["profile"];
        let mut text = format!(
            "{}@{} · {}",
            string(profile, "username"),
            string(profile, "base_url"),
            string(&status["webdav"], "path")
        );
        if status["safe_remote_replace"].as_bool().unwrap_or(false) {
            text.push_str(&format!(" · {}", tr!(lang, StatusSafeReplace)));
        }
        text
    };
    let fields = [
        (tr!(lang, StatusConfigLabel), string(status, "config")),
        (tr!(lang, VaultHeading), string(status, "vault")),
        (
            tr!(lang, StatusGatewayLabel),
            format!("{} · {}", string(status, "listen"), gateway),
        ),
        ("WebDAV", webdav),
    ];
    let label_width = fields
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in &fields {
        out.push_str(&pad(label, label_width));
        out.push_str("  ");
        out.push_str(value);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&render_connections(&status["connections"], lang));
    out.push('\n');
    let grants: Vec<&Value> = status["grants"]
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if grants.is_empty() {
        out.push('\n');
        out.push_str(tr!(lang, StatusNoGrants));
        return out.trim_end().to_string();
    }
    let headers = [
        tr!(lang, TableColumnGrant),
        tr!(lang, TableColumnHandle),
        tr!(lang, TableColumnScope),
        tr!(lang, TableColumnOperations),
        tr!(lang, TableColumnExpires),
        tr!(lang, TableColumnCalls),
        tr!(lang, ApprovalColumn),
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(grants.len());
    for &grant in &grants {
        rows.push(vec![
            string(grant, "name"),
            string(grant, "connection"),
            join_array(grant, "repositories"),
            join_array(grant, "operations"),
            format_expiry(grant, lang),
            format_calls(grant, lang),
            string(grant, "approval"),
        ]);
    }
    out.push('\n');
    out.push_str(&render_table(&headers, &rows));
    out.trim_end().to_string()
}

/// Peer streams are one row per device generation, and a long-lived remote accumulates
/// dozens, so the summary prints a bounded prefix; `--json` still carries them all.
const MAX_WAITING_ROWS: usize = 8;

/// A segment is named `<sequence>-<64 hex>.mdbxsync`, and only the sequence and a
/// recognisable slice of the digest fit a status line. `--json` keeps the full path.
fn short_segment_name(path: &str) -> String {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    let prefix = if parent.is_empty() {
        String::new()
    } else {
        format!("{parent}/")
    };
    let Some((sequence, digest)) = name.split_once('-') else {
        return path.to_string();
    };
    let digest = digest.strip_suffix(".mdbxsync").unwrap_or(digest);
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return path.to_string();
    }
    format!(
        "{prefix}{sequence}-{}…{}.mdbxsync",
        &digest[..8],
        &digest[56..]
    )
}

/// Segment paths start with the `<vault>.sync/` root that the WebDAV line already names,
/// so the pending line prints from `streams/` on: the device, generation and sequence are
/// what a status line has to add. `--json` keeps the full remote path.
fn relative_segment_path<'a>(binding: &Value, remote: &'a str) -> &'a str {
    let root = format!("{}.sync/", string(binding, "path"));
    remote.strip_prefix(root.as_str()).unwrap_or(remote)
}

/// Cursors store a stable token so `--json` never has to reword a machine contract. A
/// token this build does not know prints as stored rather than disappearing.
fn segment_reason(lang: Language, token: &str) -> String {
    use monica_pass_cli::segment::{
        WAITING_AFTER_COMPLETION, WAITING_EARLIER_SEGMENT, WAITING_PARENT_COMMIT,
        WAITING_PATH_MISMATCH,
    };
    match token {
        WAITING_EARLIER_SEGMENT => tr!(lang, StatusWaitingEarlierSegment),
        WAITING_PARENT_COMMIT => tr!(lang, StatusWaitingParentCommit),
        WAITING_AFTER_COMPLETION => tr!(lang, StatusWaitingAfterCompletion),
        WAITING_PATH_MISMATCH => tr!(lang, StatusWaitingPathMismatch),
        _ => token,
    }
    .to_string()
}

/// `webdav status` answers without a login, so every line comes from the local
/// configuration and the transport cursor: no request and no vault password.
pub fn render_webdav_status(status: &Value, lang: Language) -> String {
    let binding = &status["sync"];
    let webdav = if binding.is_null() {
        tr!(lang, StatusOff).to_string()
    } else {
        let profile = &binding["profile"];
        let mut text = format!(
            "{}@{} · {}",
            string(profile, "username"),
            string(profile, "base_url"),
            string(binding, "path")
        );
        if status["safe_remote_replace"].as_bool().unwrap_or(false) {
            text.push_str(&format!(" · {}", tr!(lang, StatusSafeReplace)));
        }
        text
    };
    let mut fields = vec![("WebDAV".to_string(), webdav)];
    if !status["profile"].is_null() {
        fields.push((
            tr!(lang, StatusWebDavPasswordLabel).to_string(),
            if status["password_saved"].as_bool().unwrap_or(false) {
                tr!(lang, StatusPasswordRemembered).to_string()
            } else {
                tr!(lang, StatusPasswordNotSaved).to_string()
            },
        ));
    }
    let segments = &status["segments"];
    if !segments.is_null() {
        if segments["tracked"].as_bool().unwrap_or(false) {
            let mut text = match string(segments, "export_base").as_str() {
                "bootstrap" => tr!(lang, StatusSegmentBaseBootstrap),
                "anchored" => tr!(lang, StatusSegmentBaseAnchored),
                _ => tr!(lang, StatusSegmentBaseUnset),
            }
            .to_string();
            if segments["generation_open"].as_bool().unwrap_or(false) {
                text.push_str(&format!(" · {}", tr!(lang, StatusSegmentGenerationOpen)));
            }
            text.push_str(&format!(
                " · {}",
                tr!(
                    lang,
                    StatusSegmentStreams,
                    streams = segments["streams"].as_u64().unwrap_or(0),
                    complete = segments["complete_streams"].as_u64().unwrap_or(0),
                )
            ));
            fields.push((tr!(lang, StatusSegmentsLabel).to_string(), text));
            if segments["usage"].is_object() {
                let usage = &segments["usage"];
                let mut text = tr!(
                    lang,
                    StatusSegmentUsage,
                    bytes = human_bytes(usage["bytes"].as_u64().unwrap_or(0)),
                    segments = usage["segments"].as_u64().unwrap_or(0),
                );
                let unmeasured = usage["unmeasured"].as_u64().unwrap_or(0);
                if unmeasured > 0 {
                    text = format!(
                        "{text} · {}",
                        tr!(lang, StatusSegmentUnmeasured, count = unmeasured)
                    );
                }
                fields.push((tr!(lang, StatusSegmentUsageLabel).to_string(), text));
            }
        } else {
            fields.push((
                tr!(lang, StatusSegmentsLabel).to_string(),
                tr!(lang, StatusSegmentUntracked).to_string(),
            ));
        }
        let pending = &segments["pending_upload"];
        if let Some(remote) = pending["remote"].as_str() {
            fields.push((
                tr!(lang, StatusSegmentPendingLabel).to_string(),
                format!(
                    "{} · {} B",
                    short_segment_name(relative_segment_path(binding, remote)),
                    pending["size"].as_u64().unwrap_or(0)
                ),
            ));
        }
    }
    let label_width = fields
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in &fields {
        out.push_str(&pad(label, label_width));
        out.push_str("  ");
        out.push_str(value);
        out.push('\n');
    }
    let waiting: Vec<&Value> = segments["waiting"]
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if !waiting.is_empty() {
        out.push('\n');
        let headers = [tr!(lang, TableColumnStream), tr!(lang, TableColumnReason)];
        let shown = waiting.len().min(MAX_WAITING_ROWS);
        let rows: Vec<Vec<String>> = waiting[..shown]
            .iter()
            .map(|entry| {
                vec![
                    string(entry, "stream"),
                    segment_reason(lang, &string(entry, "reason")),
                ]
            })
            .collect();
        out.push_str(&render_table(&headers, &rows));
        let hidden = waiting.len() - shown;
        if hidden > 0 {
            out.push('\n');
            out.push_str(&tr!(lang, StatusSegmentMore, count = hidden));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// One line per key entry. `KeyEntrySummary` is already the public projection, so no cell here
/// can carry key material; the private column only says whether the vault holds one.
pub fn render_keys(entries: &[KeyEntrySummary], lang: Language) -> String {
    if entries.is_empty() {
        return tr!(lang, ListNoKeys).to_string();
    }
    let headers = [
        tr!(lang, TableColumnKey),
        tr!(lang, TableColumnKind),
        tr!(lang, TableColumnAlgorithm),
        tr!(lang, TableColumnFingerprint),
        tr!(lang, TableColumnSecret),
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(entries.len());
    for entry in entries {
        let kind = if entry.login_type == LOGIN_TYPE_SSH {
            "SSH"
        } else {
            "GPG"
        };
        let algorithm = match entry.key_size {
            Some(bits) => format!("{} {}", entry.algorithm, bits),
            None => entry.algorithm.clone(),
        };
        rows.push(vec![
            entry.title.clone(),
            kind.to_string(),
            algorithm,
            entry.fingerprint.clone(),
            if entry.has_secret {
                tr!(lang, KeyValueYes).to_string()
            } else {
                tr!(lang, KeyValueNo).to_string()
            },
        ]);
    }
    render_table(&headers, &rows)
}

/// The registered databases, one per line, with the active one starred. The ID column is what
/// `monica-pass use <ID>` takes, so it is printed in full rather than shortened.
pub fn render_databases(databases: &Value, lang: Language) -> String {
    let items: Vec<&Value> = databases
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if items.is_empty() {
        return tr!(lang, DatabasesNone).to_string();
    }
    let headers = [
        tr!(lang, TableColumnDatabase),
        tr!(lang, TableColumnId),
        tr!(lang, TableColumnPath),
    ];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            vec![
                format!(
                    "{}{}",
                    if item["current"].as_bool().unwrap_or(false) {
                        "* "
                    } else {
                        "  "
                    },
                    string(item, "name")
                ),
                string(item, "id"),
                string(item, "path"),
            ]
        })
        .collect();
    let mut out = render_table(&headers, &rows);
    out.push('\n');
    out.push_str(tr!(lang, DatabasesCurrentMark));
    out
}

/// A remote folder listing, so picking a path to open or publish does not mean reading XML-ish
/// JSON. Sizes the server never reported stay blank rather than showing a zero.
pub fn render_webdav_list(entries: &Value, lang: Language) -> String {
    let items: Vec<&Value> = entries
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if items.is_empty() {
        return tr!(lang, WebDavListEmpty).to_string();
    }
    let headers = [
        tr!(lang, TableColumnItem),
        tr!(lang, TableColumnKind),
        tr!(lang, TableColumnSize),
    ];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let directory = item["is_directory"].as_bool().unwrap_or(false);
            let name = item["path"].as_str().unwrap_or_default();
            let name = name.rsplit('/').next().unwrap_or(name);
            let kind = if directory {
                tr!(lang, TableValueFolder)
            } else {
                tr!(lang, TableValueFile)
            };
            vec![
                if directory {
                    format!("{name}/")
                } else {
                    name.to_owned()
                },
                kind.to_string(),
                item["size"].as_u64().map(human_bytes).unwrap_or_default(),
            ]
        })
        .collect();
    render_table(&headers, &rows)
}

/// The whole tree with the ids a move or a delete needs. Indentation carries the nesting, so
/// no drawing characters are wasted on it, and the type column stays empty for a category.
pub fn render_library(data: &Value, lang: Language) -> String {
    let categories: Vec<&Value> = data["categories"]
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    let entries: Vec<&Value> = data["entries"]
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    if categories.is_empty() && entries.is_empty() {
        return tr!(lang, LibraryEmpty).to_string();
    }
    let known = |id: &str| categories.iter().any(|c| c["id"].as_str() == Some(id));
    let mut visited: Vec<String> = Vec::with_capacity(categories.len());
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(categories.len() + entries.len());
    for category in categories
        .iter()
        .filter(|category| !known(&string(category, "parent")))
    {
        library_rows(category, &categories, &entries, 0, &mut visited, &mut rows);
    }
    // A pair of categories nested inside each other has no top level to start from. Rather than
    // hide them, print them where they are reached from.
    for category in &categories {
        library_rows(category, &categories, &entries, 0, &mut visited, &mut rows);
    }
    let headers = [
        tr!(lang, TableColumnItem),
        tr!(lang, TableColumnId),
        tr!(lang, TableColumnKind),
    ];
    render_table(&headers, &rows)
}

/// One category, the entries inside it, and then its children one level deeper. `visited` makes
/// a malformed parent cycle terminate instead of recursing forever.
fn library_rows(
    category: &Value,
    categories: &[&Value],
    entries: &[&Value],
    depth: usize,
    visited: &mut Vec<String>,
    rows: &mut Vec<Vec<String>>,
) {
    let id = category["id"].as_str().unwrap_or_default().to_owned();
    if id.is_empty() || visited.contains(&id) {
        return;
    }
    visited.push(id.clone());
    let indent = "    ".repeat(depth);
    rows.push(vec![
        format!("{indent}{}", string(category, "title")),
        id.clone(),
        String::new(),
    ]);
    for entry in entries
        .iter()
        .filter(|entry| entry["category"].as_str().unwrap_or_default() == id)
    {
        rows.push(vec![
            format!("{indent}    {}", string(entry, "title")),
            string(entry, "id"),
            string(entry, "kind"),
        ]);
    }
    for child in categories
        .iter()
        .filter(|child| child["parent"].as_str().unwrap_or_default() == id)
    {
        library_rows(child, categories, entries, depth + 1, visited, rows);
    }
}

/// The security profile a vault runs under, and what it costs day to day.
///
/// The name the vault stores and the profile in force are one line while they agree and two
/// once they do not, because lowering a profile deliberately leaves the higher name in the
/// vault header. Printing only one of them would hide that the vault runs reduced.
pub fn render_tiga(data: &Value, lang: Language) -> String {
    let stored = string(data, "default_profile");
    let policy = &data["policy"];
    let effective = string(policy, "profile");
    let mut fields: Vec<(&str, String)> = Vec::with_capacity(3);
    if stored == effective {
        fields.push((tr!(lang, TigaProfileLabel), effective.clone()));
    } else {
        fields.push((tr!(lang, TigaStoredProfile), stored));
        fields.push((tr!(lang, TigaEffectiveProfile), effective.clone()));
    }
    fields.push((
        tr!(lang, TigaCompliance),
        tiga_compliance(&string(data, "compliance"), lang),
    ));

    let unlock = &policy["unlock"];
    let session = &policy["session"];
    let disclosure = &policy["disclosure"];
    let egress = &policy["egress"];
    let rows = vec![
        vec![
            tr!(lang, TigaSettingUnlock).to_string(),
            tr!(
                lang,
                TigaUnlockLine,
                factors = unlock["minimum_auth_factors"].as_u64().unwrap_or(0),
                key = yes_no(
                    unlock["security_key_required"].as_bool().unwrap_or(false),
                    lang
                )
            ),
        ],
        vec![
            tr!(lang, TigaSettingSession).to_string(),
            tr!(
                lang,
                TigaSessionLine,
                idle = tiga_duration(session["idle_timeout_secs"].as_u64().unwrap_or(0)),
                max = tiga_duration(session["max_lifetime_secs"].as_u64().unwrap_or(0))
            ),
        ],
        vec![
            tr!(lang, TigaSettingClipboard).to_string(),
            if disclosure["clipboard_allowed"].as_bool().unwrap_or(false) {
                tr!(
                    lang,
                    TigaClipboardLine,
                    ttl = tiga_duration(disclosure["clipboard_ttl_secs"].as_u64().unwrap_or(0)),
                    secure = yes_no(
                        disclosure["secure_clipboard_required"]
                            .as_bool()
                            .unwrap_or(false),
                        lang
                    )
                )
            } else {
                tr!(lang, TigaClipboardBlocked).to_string()
            },
        ],
        vec![
            tr!(lang, TigaSettingEgress).to_string(),
            format!(
                "{} / {}",
                yes_no(egress["export_allowed"].as_bool().unwrap_or(false), lang),
                yes_no(egress["print_allowed"].as_bool().unwrap_or(false), lang)
            ),
        ],
        vec![
            tr!(lang, TigaSettingDevice).to_string(),
            tiga_device(&string(policy, "minimum_device_assurance"), lang),
        ],
        vec![
            tr!(lang, TigaSettingAudit).to_string(),
            tiga_audit(&string(policy, "audit_level"), lang),
        ],
    ];
    let headers = [tr!(lang, TigaColumnSetting), tr!(lang, TigaColumnValue)];

    let label_width = fields
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in &fields {
        out.push_str(&pad(label, label_width));
        out.push_str("  ");
        out.push_str(value);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&render_table(&headers, &rows));
    // Engine wording is not translated, so it goes under a translated heading rather than
    // inline, and `--json` keeps the full list.
    let warnings: Vec<&str> = data["warnings"]
        .as_array()
        .map(|array| array.iter().filter_map(|value| value.as_str()).collect())
        .unwrap_or_default();
    if !warnings.is_empty() {
        out.push('\n');
        out.push_str(tr!(lang, TigaWarningsLabel));
        out.push(':');
        for warning in warnings.iter().take(MAX_WARNING_ROWS) {
            out.push('\n');
            out.push_str(warning);
        }
        let remaining = warnings.len() - MAX_WARNING_ROWS.min(warnings.len());
        if remaining > 0 {
            out.push('\n');
            out.push_str(&tr!(lang, StatusSegmentMore, count = remaining));
        }
    }
    out.trim_end().to_string()
}

/// A lowered profile reports one warning naming every field it weakens, so a handful is enough
/// to read and the rest belongs to `--json`.
const MAX_WARNING_ROWS: usize = 3;

/// Engine seconds are policy values, not prose, so the smallest unit that reads cleanly wins.
fn tiga_duration(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    if secs.is_multiple_of(3600) {
        return format!("{}h", secs / 3600);
    }
    if secs.is_multiple_of(60) {
        return format!("{}m", secs / 60);
    }
    format!("{}s", secs)
}

fn yes_no(value: bool, lang: Language) -> &'static str {
    if value {
        tr!(lang, KeyValueYes)
    } else {
        tr!(lang, KeyValueNo)
    }
}

fn tiga_compliance(token: &str, lang: Language) -> String {
    match token {
        "exception" => tr!(lang, TigaComplianceException).to_string(),
        "remediation-required" => tr!(lang, TigaComplianceRemediation).to_string(),
        _ => tr!(lang, TigaComplianceCompliant).to_string(),
    }
}

fn tiga_device(token: &str, lang: Language) -> String {
    match token {
        "trusted-hardware" => tr!(lang, TigaDeviceTrusted).to_string(),
        "standard" => tr!(lang, TigaDeviceStandard).to_string(),
        _ => tr!(lang, TigaDeviceUnknown).to_string(),
    }
}

fn tiga_audit(token: &str, lang: Language) -> String {
    match token {
        "all-decisions" => tr!(lang, TigaAuditAll).to_string(),
        "sensitive-operations" => tr!(lang, TigaAuditSensitive).to_string(),
        _ => tr!(lang, TigaAuditSecurityChanges).to_string(),
    }
}

/// The header a vault file carries for itself, plus how much disk it takes.
///
/// `Schema` carries the target of this build beside the stored value because the two are the
/// whole reason a client will rewrite the file the next time something opens it for writing.
pub fn render_mdbx_check(data: &Value, lang: Language) -> String {
    let format_version = string(data, "format_version");
    let reader = string(data, "min_reader_version");
    let writer = string(data, "min_writer_version");
    let schema = data["schema_version"].as_u64();
    let target_schema = data["target_schema_version"].as_u64().unwrap_or(0);
    let mut fields: Vec<(&str, String)> = vec![
        (tr!(lang, MdbxSettingFile), string(data, "path")),
        (
            tr!(lang, MdbxSettingSize),
            human_bytes(data["size_bytes"].as_u64().unwrap_or(0)),
        ),
        (
            tr!(lang, MdbxSettingModified),
            stored_time(data["modified_unix"].as_i64().unwrap_or(0)),
        ),
        (
            tr!(lang, MdbxSettingFormat),
            value_or_none(&format_version, lang),
        ),
        (
            tr!(lang, MdbxSettingSchema),
            match schema {
                Some(schema) => tr!(
                    lang,
                    MdbxSchemaLine,
                    schema = schema,
                    target = target_schema
                ),
                None => tr!(lang, MdbxValueNone).to_string(),
            },
        ),
        (
            tr!(lang, MdbxSettingCompat),
            if reader.is_empty() && writer.is_empty() {
                tr!(lang, MdbxValueNone).to_string()
            } else {
                tr!(
                    lang,
                    MdbxCompatLine,
                    reader = value_or_none(&reader, lang),
                    writer = value_or_none(&writer, lang)
                )
            },
        ),
        (
            tr!(lang, MdbxSettingUpgrade),
            if data["requires_upgrade"].as_bool().unwrap_or(false) {
                tr!(
                    lang,
                    MdbxUpgradePending,
                    from = schema.unwrap_or(target_schema),
                    to = target_schema
                )
            } else {
                tr!(lang, MdbxUpgradeNone).to_string()
            },
        ),
    ];
    if let Some(profile) = data["declared_tiga_profile"].as_str() {
        fields.push((tr!(lang, MdbxDeclaredTiga), profile.to_owned()));
    }
    let label_width = fields
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in &fields {
        out.push_str(&pad(label, label_width));
        out.push_str("  ");
        out.push_str(value);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Every file the vault is made of, named by what it is for.
///
/// The attachment store is what single-file sync refuses to carry, so the listing is also the
/// answer to "why did sync say no". The write-ahead-log pair is here too, and a reader has to
/// know that an empty log means nothing more than "this file has been opened" — any WAL
/// connection creates the pair, including the one that printed this table.
pub fn render_mdbx_files(data: &Value, lang: Language) -> String {
    let entries: Vec<&Value> = data
        .as_array()
        .map(|array| array.iter().collect())
        .unwrap_or_default();
    let headers = [
        tr!(lang, TableColumnKind),
        tr!(lang, TableColumnItem),
        tr!(lang, TableColumnSize),
        tr!(lang, MdbxSettingModified),
    ];
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(entries.len());
    let mut total = 0u64;
    for entry in &entries {
        total += entry["size_bytes"].as_u64().unwrap_or(0);
        rows.push(vec![
            mdbx_role(string(entry, "role"), lang),
            Path::new(&string(entry, "path"))
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            entry["size_bytes"]
                .as_u64()
                .map(human_bytes)
                .unwrap_or_else(|| tr!(lang, MdbxSizeUncounted).to_string()),
            stored_time(entry["modified_unix"].as_i64().unwrap_or(0)),
        ]);
    }
    let directory = entries
        .first()
        .map(|entry| {
            let path = string(entry, "path");
            Path::new(&path)
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut out = format!("{}  {}\n\n", tr!(lang, MdbxDirectoryLabel), directory);
    out.push_str(&render_table(&headers, &rows));
    out.push('\n');
    out.push_str(&tr!(
        lang,
        MdbxTotalLine,
        count = entries.len(),
        bytes = human_bytes(total)
    ));
    out.trim_end().to_string()
}

/// A stored value a fresh or unreadable header leaves blank.
fn value_or_none(value: &str, lang: Language) -> String {
    if value.is_empty() {
        tr!(lang, MdbxValueNone).to_string()
    } else {
        value.to_string()
    }
}

fn mdbx_role(token: String, lang: Language) -> String {
    match token.as_str() {
        "vault" => tr!(lang, MdbxRoleVault).to_string(),
        "wal" => tr!(lang, MdbxRoleWal).to_string(),
        "shm" => tr!(lang, MdbxRoleShm).to_string(),
        "journal" => tr!(lang, MdbxRoleJournal).to_string(),
        "blobs" => tr!(lang, MdbxRoleBlobs).to_string(),
        _ => token,
    }
}

/// Last write, in local time and with the year, because a vault untouched since last year is
/// the point of asking.
fn stored_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|moment| moment.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        KeyEntrySummary, MAX_WAITING_ROWS, Value, render_audit, render_connection_detail,
        render_connections, render_databases, render_keys, render_library, render_mdbx_check,
        render_mdbx_files, render_status, render_tiga, render_webdav_list, render_webdav_status,
        short_segment_name,
    };
    use monica_pass_cli::i18n::Language;
    use monica_pass_cli::keys::payload::{LOGIN_TYPE_GPG, LOGIN_TYPE_SSH};
    use monica_pass_cli::tr;
    use serde_json::json;

    /// Values as the engine reported them for a `multi` vault lowered to `sky`.
    fn tiga_report(compliance: &str, stored: &str, effective: &str) -> Value {
        let base = match effective {
            "sky" => json!({
                "profile": "sky",
                "unlock": {"minimum_auth_factors": 1, "security_key_required": false},
                "session": {"idle_timeout_secs": 1800, "max_lifetime_secs": 43200},
                "disclosure": {
                    "clipboard_allowed": true,
                    "clipboard_ttl_secs": 60,
                    "secure_clipboard_required": false
                },
                "egress": {"export_allowed": true, "print_allowed": true},
                "minimum_device_assurance": "unknown",
                "audit_level": "security-changes"
            }),
            "power" => json!({
                "profile": "power",
                "unlock": {"minimum_auth_factors": 2, "security_key_required": true},
                "session": {"idle_timeout_secs": 120, "max_lifetime_secs": 900},
                "disclosure": {
                    "clipboard_allowed": true,
                    "clipboard_ttl_secs": 10,
                    "secure_clipboard_required": true
                },
                "egress": {"export_allowed": false, "print_allowed": false},
                "minimum_device_assurance": "trusted-hardware",
                "audit_level": "all-decisions"
            }),
            _ => json!({
                "profile": "multi",
                "unlock": {"minimum_auth_factors": 1, "security_key_required": false},
                "session": {"idle_timeout_secs": 600, "max_lifetime_secs": 7200},
                "disclosure": {
                    "clipboard_allowed": true,
                    "clipboard_ttl_secs": 30,
                    "secure_clipboard_required": false
                },
                "egress": {"export_allowed": true, "print_allowed": true},
                "minimum_device_assurance": "standard",
                "audit_level": "sensitive-operations"
            }),
        };
        let warnings = if compliance == "exception" {
            json!([
                "policy exception 1616a51d weakens: idle_timeout_secs, max_lifetime_secs, audit_level"
            ])
        } else {
            json!([])
        };
        json!({
            "default_profile": stored,
            "policy": base,
            "compliance": compliance,
            "exception_id": null,
            "warnings": warnings
        })
    }

    #[test]
    fn tiga_reports_one_profile_until_the_vault_runs_reduced() {
        let plain = render_tiga(&tiga_report("compliant", "multi", "multi"), Language::En);
        assert_eq!(
            plain
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["Profile", "multi"],
            "a vault that answers to its own name gets one line, not two"
        );
        assert!(
            !plain.contains("Stored"),
            "a second row with no gap to explain is noise: {plain}"
        );
        assert!(plain.contains("idle 10m · max 2h"), "{plain}");
        assert!(plain.contains("30s · secure no"), "{plain}");
        assert!(plain.contains("Export / print  yes / yes"), "{plain}");
        assert!(!plain.contains("Policy warnings"), "{plain}");

        let reduced = render_tiga(&tiga_report("exception", "multi", "sky"), Language::ZhCn);
        assert!(reduced.contains("登记等级  multi"), "{reduced}");
        assert!(reduced.contains("生效等级  sky"), "{reduced}");
        assert!(
            reduced.contains("因已记录的例外而降低"),
            "the gap between the two names is the point: {reduced}"
        );
        assert!(
            reduced.contains("idle_timeout_secs"),
            "which fields the exception weakened is worth reading: {reduced}"
        );

        let power = render_tiga(&tiga_report("compliant", "power", "power"), Language::En);
        assert!(
            power.contains("Export / print  no / no")
                && power.contains("trusted hardware")
                && power.contains("2 factor(s), security key yes"),
            "{power}"
        );
    }
    #[test]
    fn connections_align_cjk_and_hide_the_default_api_base() {
        let connections = json!([
            {"name":"gitlab","provider":"gitlab","api_base":"https://gitlab.com/api/v4/","note":"管理 Issue"},
            {"name":"gh","provider":"github","api_base":"https://api.github.com/","note":"ok"},
        ]);
        let out = render_connections(&connections, Language::En);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Handle  Provider  Note");
        assert_eq!(lines[1], "gitlab  gitlab    管理 Issue");
        assert_eq!(lines[2], "gh      github    ok");
    }

    #[test]
    fn custom_api_base_adds_a_column() {
        let connections = json!([
            {"name":"self","provider":"gitlab","api_base":"https://git.example/","note":"x"},
        ]);
        let out = render_connections(&connections, Language::En);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Handle  Provider  API                   Note");
        assert_eq!(lines[1], "self    gitlab    https://git.example/  x");
    }

    #[test]
    fn empty_connections_list_reads_as_a_sentence() {
        assert_eq!(
            render_connections(&json!([]), Language::En),
            "No connections yet."
        );
    }

    fn key_entry(
        title: &str,
        login_type: &str,
        algorithm: &str,
        key_size: Option<i64>,
        fingerprint: &str,
        has_secret: bool,
    ) -> KeyEntrySummary {
        KeyEntrySummary {
            entry_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            logical_id: "password:22222222-2222-2222-2222-222222222222".to_owned(),
            collection_id: "33333333-3333-3333-3333-333333333333".to_owned(),
            title: title.to_owned(),
            login_type: login_type.to_owned(),
            algorithm: algorithm.to_owned(),
            key_size,
            fingerprint: fingerprint.to_owned(),
            comment: "comment-never-shown".to_owned(),
            public_key: "ssh-ed25519 AAAAC3NzaC1l never-shown".to_owned(),
            notes: "notes-never-shown".to_owned(),
            has_secret,
            chunk_count: 3,
        }
    }

    #[test]
    fn keys_table_projects_only_public_metadata() {
        let entries = vec![
            key_entry(
                "工作机",
                LOGIN_TYPE_SSH,
                "ed25519",
                None,
                "SHA256:aaaa",
                true,
            ),
            key_entry(
                "laptop",
                LOGIN_TYPE_GPG,
                "RSA",
                Some(2048),
                "1234ABCD5678EF90",
                false,
            ),
        ];
        let out = render_keys(&entries, Language::En);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            "Key     Type  Algorithm  Fingerprint       Private"
        );
        assert_eq!(lines[1], "工作机  SSH   ed25519    SHA256:aaaa       yes");
        assert_eq!(lines[2], "laptop  GPG   RSA 2048   1234ABCD5678EF90  no");
        assert_eq!(render_keys(&[], Language::En), "No key entries yet.");

        let zh = render_keys(&entries, Language::ZhCn);
        assert!(zh.contains("有"), "{zh}");
        assert!(zh.contains("无"), "{zh}");
        for material in ["never-shown", "comment-never", "notes-never"] {
            assert!(
                !out.contains(material) && !zh.contains(material),
                "{material}"
            );
        }
    }

    #[test]
    fn audit_table_marks_stage_and_outcome() {
        let audit = json!({
            "path":"/tmp/gateway.audit.jsonl",
            "order":"newest_first",
            "total":4,
            "shown":3,
            "events":[
                {"timestamp":1777000000,"grant":"probe","operation":"list_issues","repository":"a/b","request_id":"req-9f","stage":"finished","error":"permission_denied"},
                {"timestamp":1777000001,"grant":"probe","operation":"api_write","repository":"*","request_id":"req-8e","stage":"authorized","error":null},
                {"timestamp":1777000002,"grant":"probe","stage":"finished","error":null}
            ]
        });
        let out = render_audit(&audit, Language::En);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5, "{out}");
        let headers: Vec<&str> = lines[0].split_whitespace().collect();
        assert_eq!(
            headers,
            ["Time", "Grant", "Operation", "Scope", "Stage", "Result"]
        );
        assert!(lines[1].contains("permission_denied"), "{out}");
        // An `authorized` row is the pre-dispatch write: the outcome is not known yet.
        assert!(lines[2].contains("pending"), "{out}");
        assert!(lines[3].contains('—'), "{out}");
        assert_eq!(lines[4], "Older events not shown: 1.");
        assert!(!out.contains("req-9f"), "{out}");
        assert!(!out.contains("newest_first"), "{out}");

        let zh = render_audit(&audit, Language::ZhCn);
        assert!(zh.contains("时间") && zh.contains("阶段") && zh.contains("结果"));
        assert!(zh.contains("待回执"), "{zh}");
        assert!(zh.contains("更早的 1 条未显示。"), "{zh}");

        assert_eq!(
            render_audit(
                &json!({"path":"/tmp/gateway.audit.jsonl", "events":[], "total":0, "shown":0}),
                Language::En
            ),
            format!(
                "{}\n/tmp/gateway.audit.jsonl",
                tr!(Language::En, AuditEmpty)
            )
        );
    }

    #[test]
    fn show_detail_labels_and_grant_markers() {
        let connection = json!({
            "name":"gitlab","provider":"gitlab","api_base":"https://gitlab.com/api/v4/","note":"管理 Issue",
            "grants":[
                {"name":"probe","connection":"gitlab","repositories":["a/b","c/d"],"operations":["list_issues","get_issue"],"expires_at_unix":0,"expired":false,"max_calls":0,"calls_used":0,"refresh_required":false},
                {"name":"w","connection":"gitlab","repositories":["*"],"operations":["api_write"],"expires_at_unix":1,"expired":true,"max_calls":10,"calls_used":10,"refresh_required":true},
            ]
        });
        let out = render_connection_detail(&connection, Language::En);
        assert_eq!(out.lines().next().unwrap(), "Handle    gitlab");
        assert!(out.contains("Note      管理 Issue"));
        assert!(out.contains("a/b, c/d"));
        assert!(out.contains("list_issues, get_issue"));
        assert!(out.contains("never"));
        assert!(out.contains("unlimited"));
        assert!(out.contains("10/10"));
        assert!(out.contains("expired"));
        assert!(out.contains("1970-01-01"));
    }

    fn status_fixture() -> Value {
        json!({
            "config": "C:\\x\\gateway.json", "vault": "/tmp/v.mdbx", "listen": "127.0.0.1:47831",
            "webdav": null, "safe_remote_replace": null,
            "lock_requested": false, "broker_running": true,
            "connections": [{"name":"gh","provider":"github","api_base":"https://api.github.com/","note":"ok"}],
            "grants": [
                {"name":"read","connection":"gh","repositories":["a/b"],"operations":["list_issues"],
                 "expires_at_unix":0,"expired":false,"max_calls":0,"calls_used":0,"refresh_required":false,"approval":"off"},
                {"name":"cap","connection":"gh","repositories":["a/b"],"operations":["get_issue"],
                 "expires_at_unix":1,"expired":true,"max_calls":5,"calls_used":5,"refresh_required":true,"approval":"write"},
            ]
        })
    }

    fn line_starting<'a>(lines: &[&'a str], prefix: &str) -> &'a str {
        lines
            .iter()
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting with {prefix}: {lines:?}"))
    }

    #[test]
    fn status_leads_with_gateway_state_and_lists_every_grant_budget() {
        let out = render_status(&status_fixture(), Language::En);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Config   C:\\x\\gateway.json");
        assert_eq!(lines[1], "Vault    /tmp/v.mdbx");
        assert_eq!(lines[2], "Gateway  127.0.0.1:47831 · running");
        assert_eq!(lines[3], "WebDAV   off");
        let header = line_starting(&lines, "Grant");
        assert!(
            header.contains("Handle") && header.ends_with("Gate"),
            "{out}"
        );
        let read = line_starting(&lines, "read");
        assert!(
            read.contains("never") && read.contains("unlimited") && read.ends_with("off"),
            "{read}"
        );
        let cap = line_starting(&lines, "cap");
        assert!(
            cap.contains("1970-01-01") && cap.contains("expired") && cap.ends_with("write"),
            "{cap}"
        );
    }

    #[test]
    fn status_names_the_broker_state_and_says_so_when_nothing_is_granted() {
        let mut status = status_fixture();
        status["broker_running"] = json!(false);
        status["lock_requested"] = json!(true);
        assert!(
            render_status(&status, Language::ZhCn).contains("127.0.0.1:47831 · 已停止"),
            "{status}"
        );
        status["broker_running"] = json!(true);
        let stopping = render_status(&status, Language::En);
        assert!(
            stopping.contains("127.0.0.1:47831 · stopping"),
            "{stopping}"
        );
        status["grants"] = json!([]);
        let empty = render_status(&status, Language::En);
        assert!(empty.ends_with("No AI grants yet."), "{empty}");
        assert!(!empty.contains("for this connection"), "{empty}");
    }

    fn webdav_status_fixture(waiting: usize) -> Value {
        json!({
            "profile": {"username":"joy","base_url":"https://dav.example/dav"},
            "sync": {
                "profile": {"username":"joy","base_url":"https://dav.example/dav"},
                "path": "vault.mdbx",
                "vault_id": "0f6f0f6f-0f6f-4f6f-8f6f-0f6f0f6f0f6f",
                "etag": "\"abc\"",
                "remote_sha256": "a".repeat(64),
                "local_sha256": "b".repeat(64),
                "last_sync": 0,
            },
            "password_saved": false,
            "safe_remote_replace": true,
            "segments": {
                "tracked": true,
                "export_base": "anchored",
                "generation_open": false,
                "pending_upload": {
                    "remote": format!(
                        "vault.mdbx.sync/streams/dev/1/segments/0000000001-{}.mdbxsync",
                        "c".repeat(64)
                    ),
                    "size": 4096,
                },
                "streams": 12,
                "complete_streams": 5,
                "usage": {"segments": 34, "bytes": 4404019, "unmeasured": 0},
                "waiting": (0..waiting)
                    .map(|index| json!({
                        "stream": format!("dev{index}/generation"),
                        "reason": monica_pass_cli::segment::WAITING_PARENT_COMMIT,
                    }))
                    .collect::<Vec<_>>(),
            },
        })
    }

    #[test]
    fn databases_star_the_active_one_and_keep_the_id_a_switch_needs() {
        let out = render_databases(
            &json!([
                {"id": "current", "name": "Monicacli", "path": "D:/Apps/MonicaCLI/data/vault.mdbx", "current": true},
                {"id": "11111111-1111-1111-1111-111111111111", "name": "Phone", "path": "/tmp/phone.mdbx", "current": false},
            ]),
            Language::En,
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("Database"), "{out}");
        assert!(lines[1].starts_with("* Monicacli"), "{out}");
        assert_eq!(
            lines[0].find("ID"),
            lines[1].find("current"),
            "the ID column has to sit under its header: {out}"
        );
        assert!(
            lines[2].contains("Phone") && lines[2].contains("11111111-1111-1111-1111-111111111111"),
            "{out}"
        );
        assert!(
            !lines[2].starts_with('*'),
            "only the database in use carries the mark: {out}"
        );
        assert!(lines[3].contains("monica-pass use"), "{out}");
        assert!(
            render_databases(&json!([]), Language::ZhCn).contains("尚未登记"),
            "an empty registry still has to say what is missing"
        );
    }

    #[test]
    fn library_indents_nesting_and_lands_the_id_beside_every_row() {
        let out = render_library(
            &json!({
                "categories": [
                    {"id": "root-id", "parent": null, "title": "Monica"},
                    {"id": "work-id", "parent": null, "title": "Work"},
                    {"id": "sub-id", "parent": "work-id", "title": "Projects"},
                ],
                "entries": [
                    {"id": "e1", "category": "work-id", "title": "demo", "kind": "api-token"},
                    {"id": "e2", "category": "sub-id", "title": "kid", "kind": "login"},
                ],
            }),
            Language::En,
        );
        let row = |needle: &str| {
            out.lines()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle} in {out}"))
                .trim_end()
                .to_owned()
        };
        assert!(
            row("Monica").starts_with("Monica") && row("Monica").contains("root-id"),
            "{out}"
        );
        assert!(row("Projects").starts_with("    Projects"), "{out}");
        assert!(row("demo").starts_with("    demo"), "{out}");
        assert!(row("kid").starts_with("        kid"), "{out}");
        assert!(row("demo").ends_with("api-token"), "{out}");
        assert!(
            !row("Monica").trim().ends_with("folder"),
            "a category has no type to repeat: {out}"
        );
        let zh = render_library(
            &json!({"categories": [{"id": "c", "parent": "c", "title": "循环"}], "entries": []}),
            Language::ZhCn,
        );
        assert!(
            zh.contains("循环"),
            "a parent cycle still lists itself: {zh}"
        );
        assert_eq!(zh.lines().count(), 2, "{zh}");
        assert!(
            render_library(&json!({"categories": [], "entries": []}), Language::ZhCn)
                .contains("暂无"),
            "an empty vault says so instead of printing a header"
        );
    }

    #[test]
    fn a_remote_folder_reads_as_names_sizes_and_slashes() {
        let out = render_webdav_list(
            &json!([
                {"path": "Backup/vault.mdbx", "is_directory": false, "size": 2048, "etag": "\"x\""},
                {"path": "Backup/Logs", "is_directory": true, "size": null, "etag": null},
            ]),
            Language::En,
        );
        assert!(out.contains("vault.mdbx"), "{out}");
        assert!(out.contains("2.0 KiB"), "{out}");
        let logs = out
            .lines()
            .find(|line| line.contains("Logs"))
            .unwrap_or_else(|| panic!("no row for Logs in {out}"));
        assert!(
            logs.starts_with("Logs/") && logs.contains("folder"),
            "a directory is marked by a slash and its type: {out}"
        );
        assert!(!out.contains("Backup/"), "only the name is a column: {out}");
        assert!(
            render_webdav_list(&json!([]), Language::ZhCn).contains("为空"),
            "an empty folder still answers"
        );
    }

    #[test]
    fn webdav_status_shows_the_cursor_and_bounds_a_long_wait_list() {
        let out = render_webdav_status(&webdav_status_fixture(10), Language::En);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines[0].ends_with("joy@https://dav.example/dav · vault.mdbx · safe replace"),
            "{out}"
        );
        assert!(lines[1].starts_with("Saved password"), "{out}");
        assert!(lines[1].ends_with("not saved"), "{out}");
        assert!(lines[2].starts_with("Segments"), "{out}");
        assert!(
            lines[2].contains("anchored · 12 stream(s), 5 complete"),
            "{}",
            lines[2]
        );
        assert!(lines[3].starts_with("Remote usage"), "{out}");
        assert!(
            lines[3].contains("4.2 MiB across 34 segment(s)"),
            "{}",
            lines[3]
        );
        assert!(lines[4].starts_with("Pending upload"), "{out}");
        assert!(
            lines[4]
                .ends_with("streams/dev/1/segments/0000000001-cccccccc…cccccccc.mdbxsync · 4096 B"),
            "{out}"
        );
        assert_eq!(
            lines[0].find("joy@").unwrap(),
            lines[2].find("anchored").unwrap(),
            "values share one column whatever the label widths"
        );
        assert_eq!(
            lines[0].find("joy@").unwrap(),
            lines[3].find("4.2 MiB").unwrap(),
            "the measured footprint is a value like every other one"
        );
        assert_eq!(
            lines[0].find("joy@").unwrap(),
            lines[4].find("streams/").unwrap(),
            "the longest label still has to keep that column"
        );
        assert_eq!(
            lines[0].find("joy@").unwrap(),
            lines[1].find("not saved").unwrap(),
            "{out}"
        );
        let mut remembered = webdav_status_fixture(0);
        remembered["password_saved"] = json!(true);
        assert!(
            render_webdav_status(&remembered, Language::En).contains("Saved password"),
            "the remembered state has to be visible"
        );
        assert!(
            render_webdav_status(&remembered, Language::En).contains("this computer only"),
            "{remembered}"
        );
        assert!(
            !lines[4].contains("vault.mdbx.sync"),
            "the WebDAV line already names the sync root: {out}"
        );
        assert_eq!(
            short_segment_name("streams/dev-a/segments/7-not-a-digest.mdbxsync"),
            "streams/dev-a/segments/7-not-a-digest.mdbxsync",
            "an unexpected name must print in full rather than guess at its shape"
        );
        assert_eq!(
            out.matches("waiting for a parent commit").count(),
            MAX_WAITING_ROWS,
            "{out}"
        );
        assert!(out.ends_with("… and 2 more (see --json)"), "{out}");

        let zh = render_webdav_status(&webdav_status_fixture(1), Language::ZhCn);
        assert!(zh.contains("分段同步") && zh.contains("已锚定"), "{zh}");
        assert!(zh.contains("12 个流，5 个已完成"), "{zh}");
        assert!(zh.contains("34 个分段共 4.2 MiB"), "{zh}");
        assert!(!zh.contains("Waiting"), "{zh}");
        assert!(zh.contains("等待父提交"), "{zh}");
        assert!(!zh.contains("waiting_for_parent_commit"), "{zh}");

        let mut unmeasured = webdav_status_fixture(0);
        unmeasured["segments"]["usage"]["unmeasured"] = json!(3);
        assert!(
            render_webdav_status(&unmeasured, Language::En)
                .contains("3 without a reported size (total is a floor)"),
            "a server that hides lengths must not let the total read as exact"
        );
        assert!(
            render_webdav_status(&unmeasured, Language::ZhCn).contains("另有 3 个未报大小"),
            "{unmeasured}"
        );

        let mut future = webdav_status_fixture(1);
        future["segments"]["waiting"][0]["reason"] = json!("reason_from_a_newer_client");
        assert!(
            render_webdav_status(&future, Language::ZhCn).contains("reason_from_a_newer_client"),
            "an unknown token must still reach the user"
        );

        let mut unstarted = webdav_status_fixture(1);
        unstarted["segments"]["tracked"] = json!(false);
        unstarted["segments"]["pending_upload"] = json!(null);
        unstarted["segments"]["waiting"] = json!([]);
        let idle = render_webdav_status(&unstarted, Language::En);
        assert!(
            idle.contains("Segments") && idle.contains("no segment run yet"),
            "{idle}"
        );
        assert!(!idle.contains("Pending upload"), "{idle}");
        assert!(
            !idle.contains("Remote usage"),
            "nothing was ever measured, so no number may appear: {idle}"
        );

        unstarted["sync"] = json!(null);
        unstarted["segments"] = json!(null);
        assert_eq!(
            render_webdav_status(&unstarted, Language::ZhCn)
                .lines()
                .next()
                .unwrap(),
            "WebDAV    未启用"
        );
    }

    /// A header sheet as `mdbx::check` serializes it: this build reads schema `target`, the file
    /// carries `stored`. Measured against the engine: a file that was created but never
    /// initialized reports no format, no schema and no compatibility floor at all.
    fn mdbx_check_fixture(stored: Option<u64>, target: u64) -> Value {
        json!({
            "path": "D:\\Apps\\MonicaCLI\\data\\vault.mdbx",
            "size_bytes": 24_576,
            "modified_unix": 1_777_000_000,
            "initialized": stored.is_some(),
            "format_version": stored.map(|_| "MDBX-2"),
            "schema_version": stored,
            "min_reader_version": stored.map(|_| "0.12.0"),
            "min_writer_version": stored.map(|_| "0.13.0"),
            "requires_upgrade": stored.unwrap_or(target) != target,
            "unknown_critical_extensions": false,
            "target_format_version": "MDBX-2",
            "target_schema_version": target,
        })
    }

    /// A label sheet, so every row is read as `label → value`.
    fn sheet(rendered: &str) -> Vec<(String, String)> {
        rendered
            .lines()
            .map(|line| {
                let (label, value) = line.split_once("  ").unwrap_or((line, ""));
                (label.trim_end().to_string(), value.trim_start().to_string())
            })
            .collect()
    }

    #[test]
    fn the_vault_header_sheet_keeps_the_stored_schema_apart_from_this_build() {
        let rendered = render_mdbx_check(&mdbx_check_fixture(Some(12), 12), Language::En);
        let mut rows = sheet(&rendered);
        // The last write is local time and the runner's zone is not ours, so it is pulled out
        // of the sheet and checked on its own below.
        let modified = std::mem::replace(&mut rows[2], ("Modified".into(), String::new()));
        assert_eq!(
            rows,
            vec![
                (
                    "File".into(),
                    r"D:\Apps\MonicaCLI\data\vault.mdbx".to_string()
                ),
                ("Size".into(), "24.0 KiB".to_string()),
                ("Modified".into(), String::new()),
                ("Format".into(), "MDBX-2".to_string()),
                ("Schema".into(), "12 · this build 12".to_string()),
                (
                    "Readable by".into(),
                    "readers ≥ 0.12.0, writers ≥ 0.13.0".to_string()
                ),
                ("Upgrade".into(), "not needed".to_string()),
            ],
            "{rendered}"
        );
        assert!(modified.1.starts_with("2026-04-2"), "{rendered}");
        assert!(
            !modified.1.starts_with("1777"),
            "an epoch count is not an answer: {rendered}"
        );

        // The gap between the two schema numbers is the whole warning, so it stays visible.
        let lowered = render_mdbx_check(&mdbx_check_fixture(Some(11), 12), Language::En);
        assert_eq!(
            sheet(&lowered)[6],
            ("Upgrade".to_string(), "yes, 11 → 12".to_string()),
            "{lowered}"
        );
        let zh = render_mdbx_check(&mdbx_check_fixture(Some(11), 12), Language::ZhCn);
        assert!(zh.contains("需要，11 → 12"), "{zh}");
        assert!(zh.contains("11 · 当前引擎 12"), "{zh}");
    }

    /// A database file that was created but never initialized has no header at all. Blank rows
    /// would read as a renderer bug, so every absent value says `none`.
    #[test]
    fn a_vault_without_a_header_renders_blanks_as_none() {
        let rendered = render_mdbx_check(&mdbx_check_fixture(None, 12), Language::En);
        let rows = sheet(&rendered);
        assert_eq!(
            rows.iter()
                .map(|(label, value)| (label.as_str(), value.as_str()))
                .skip(3)
                .collect::<Vec<_>>(),
            vec![
                ("Format", "none"),
                ("Schema", "none"),
                ("Readable by", "none"),
                ("Upgrade", "not needed"),
            ],
            "{rendered}"
        );
        assert_eq!(
            sheet(&render_mdbx_check(
                &mdbx_check_fixture(None, 12),
                Language::ZhCn
            ))[3]
                .1,
            "无"
        );
    }

    fn mdbx_files_fixture() -> Value {
        json!([
            {
                "role": "vault",
                "path": "/home/m/.monica/vault.mdbx",
                "directory": false,
                "size_bytes": 24_576,
                "modified_unix": 1_777_000_000
            },
            {
                "role": "wal",
                "path": "/home/m/.monica/vault.mdbx-wal",
                "directory": false,
                "size_bytes": 0,
                "modified_unix": 1_777_000_000
            },
            {
                "role": "shm",
                "path": "/home/m/.monica/vault.mdbx-shm",
                "directory": false,
                "size_bytes": 32_768,
                "modified_unix": 1_777_000_000
            },
            {
                "role": "blobs",
                "path": "/home/m/.monica/vault.mdbx.blobs",
                "directory": true,
                "size_bytes": null,
                "modified_unix": 1_777_000_000
            },
        ])
    }

    #[test]
    fn the_file_listing_names_every_part_and_sums_only_what_it_measured() {
        let rendered = render_mdbx_files(&mdbx_files_fixture(), Language::En);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[0], "Directory  /home/m/.monica");
        assert_eq!(lines[1], "");
        assert_eq!(
            lines[2].split_whitespace().collect::<Vec<_>>(),
            ["Type", "Item", "Size", "Modified"],
            "{rendered}"
        );
        let body = &lines[3..7];
        for row in body {
            assert_eq!(
                super::width(row),
                super::width(body[0]),
                "a ragged row: {row}"
            );
        }
        assert!(body[0].starts_with("vault file"), "{rendered}");
        // The column is padded, so the basename is followed by blanks rather than by the next
        // path component: only the name is repeated, the directory is stated once above.
        assert!(body[0].contains("vault.mdbx "), "basename only: {rendered}");
        assert!(!body[0].contains("/home/"), "{rendered}");
        assert!(
            body[1].contains("0 B"),
            "an empty log is a fact: {rendered}"
        );
        assert!(body[3].starts_with("attachments"), "{rendered}");
        assert!(body[3].contains("not counted"), "{rendered}");
        assert_eq!(lines[7], "4 items, 56.0 KiB on disk");
        assert_eq!(rendered.lines().count(), 8, "{rendered}");

        let zh = render_mdbx_files(&mdbx_files_fixture(), Language::ZhCn);
        assert!(zh.starts_with("所在目录  /home/m/.monica"), "{zh}");
        assert!(zh.contains("共 4 项，占用 56.0 KiB"), "{zh}");
    }
}
