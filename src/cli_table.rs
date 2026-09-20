//! Compact, locale-aware human rendering for read-only CLI output.
//! Only the non-`--json` path uses this; `--json` stays machine-stable.

use chrono::{Local, TimeZone};
use monica_pass_cli::i18n::Language;
use monica_pass_cli::keys::payload::LOGIN_TYPE_SSH;
use monica_pass_cli::model::Provider;
use monica_pass_cli::tr;
use monica_pass_cli::vault::KeyEntrySummary;
use serde_json::Value;
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
        ]);
    }
    out.push('\n');
    out.push_str(&render_table(&headers, &rows));
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

#[cfg(test)]
mod tests {
    use super::{
        KeyEntrySummary, Value, render_connection_detail, render_connections, render_keys,
        render_status,
    };
    use monica_pass_cli::i18n::Language;
    use monica_pass_cli::keys::payload::{LOGIN_TYPE_GPG, LOGIN_TYPE_SSH};
    use serde_json::json;

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
                 "expires_at_unix":0,"expired":false,"max_calls":0,"calls_used":0,"refresh_required":false},
                {"name":"cap","connection":"gh","repositories":["a/b"],"operations":["get_issue"],
                 "expires_at_unix":1,"expired":true,"max_calls":5,"calls_used":5,"refresh_required":true},
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
            header.contains("Handle") && header.ends_with("Calls"),
            "{out}"
        );
        let read = line_starting(&lines, "read");
        assert!(
            read.contains("never") && read.contains("unlimited"),
            "{read}"
        );
        let cap = line_starting(&lines, "cap");
        assert!(
            cap.contains("1970-01-01") && cap.contains("expired") && cap.ends_with("5/5"),
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
}
