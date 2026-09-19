//! Compact, locale-aware human rendering for read-only CLI output.
//! Only the non-`--json` path uses this; `--json` stays machine-stable.

use chrono::{Local, TimeZone};
use monica_pass_cli::i18n::Language;
use monica_pass_cli::model::Provider;
use monica_pass_cli::tr;
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

#[cfg(test)]
mod tests {
    use super::{Value, render_connection_detail, render_connections, render_status};
    use monica_pass_cli::i18n::Language;
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
