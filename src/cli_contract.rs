//! Public semantics shared by discovery, documentation and error recovery.
//! Discovering a management command never grants permission to execute it.
use serde_json::{Value, json};

pub fn execution_name(name: &str) -> &str {
    match name {
        "connections" => "list",
        "mcp-config" => "settings",
        "renew" => "refresh",
        other => other,
    }
}

pub fn canonical_name(name: &str) -> &str {
    match name {
        "list" => "connections",
        "settings" => "mcp-config",
        "refresh" => "renew",
        other => other,
    }
}

/// Effect codes are stable, public facts, not an authorization decision.
/// `when` describes options or runtime conditions, never supplied values.
pub fn describe(name: &str) -> Option<Value> {
    let name = canonical_name(name);
    let (target, prerequisites, effects): (&str, &[&str], &[(&str, &str)]) = match name {
        "" => (
            "command",
            &[],
            &[
                ("interactive_default", "terminal_ui"),
                ("non_interactive_default", "read_public_metadata"),
            ],
        ),
        "commands" => ("command", &[], &[("always", "read_command_grammar")]),
        "next" => ("grant", &[], &[("always", "read_public_metadata")]),
        "status" | "connections" => (
            "vault",
            &["configured_vault"],
            &[("always", "read_public_metadata")],
        ),
        "show" => (
            "connection",
            &["configured_vault", "existing_connection"],
            &[("always", "read_public_metadata")],
        ),
        "databases" => ("database", &[], &[("always", "read_public_metadata")]),
        "init" => (
            "database",
            &["new_vault_path", "broker_stopped", "secure_password_input"],
            &[
                ("always", "create_vault"),
                ("always", "write_local_config"),
                ("always", "switch_vault"),
                ("always", "reset_grants"),
            ],
        ),
        "add" => (
            "connection_and_same_named_grant",
            &[
                "new_connection_and_grant",
                "exact_repository_scope",
                "secure_password_and_token_input",
            ],
            &[
                ("always", "lock_broker"),
                ("vault_missing", "create_vault"),
                ("always", "write_vault"),
                ("always", "create_grant"),
                ("always", "write_capability_file"),
                ("always", "write_mcp_snippet"),
                ("--serve", "foreground_broker"),
            ],
        ),
        "connect" => (
            "connection",
            &[
                "configured_vault",
                "new_connection",
                "secure_password_and_token_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "write_vault"),
                ("always", "write_local_config"),
            ],
        ),
        "grant" => (
            "grant",
            &[
                "configured_vault",
                "existing_connection",
                "explicit_repository_scope",
                "new_grant",
                "secure_password_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "create_grant"),
                ("always", "write_capability_file"),
                ("service_tool_grant", "print_mcp_snippet"),
                ("model_proxy_grant", "print_proxy_config_hint"),
            ],
        ),
        "renew" => (
            "grant",
            &[
                "configured_vault",
                "existing_grant",
                "secure_password_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "rotate_capability"),
                ("always", "reset_grant_window_and_budget"),
                ("always", "write_capability_file"),
                ("service_tool_grant", "print_mcp_snippet"),
                ("model_proxy_grant", "print_proxy_config_hint"),
                ("--approval", "change_approval_policy"),
            ],
        ),
        "bind" => (
            "connection",
            &[
                "configured_vault",
                "existing_api_key_entry",
                "secure_password_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "bind_existing_api_key"),
                ("always", "write_local_config"),
                ("--replace", "revoke_connection_grants"),
                ("android_root_missing", "restore_android_root"),
            ],
        ),
        "proxy-config" => (
            "grant",
            &["configured_vault", "existing_grant"],
            &[("always", "write_proxy_client_config")],
        ),
        "direct-config" => ("subcommand", &[], &[]),
        "direct-config manual" => (
            "client_file",
            &["secure_token_input", "explicit_client_file"],
            &[
                ("always", "write_direct_client_config"),
                ("changed_existing_file", "backup_client_config"),
            ],
        ),
        "direct-config saved" => (
            "connection",
            &[
                "configured_vault",
                "existing_connection",
                "secure_password_input",
                "tiga_export_allowed",
                "explicit_client_file",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "write_direct_client_config"),
                ("changed_existing_file", "backup_client_config"),
            ],
        ),
        "unbind" => (
            "connection",
            &[
                "configured_vault",
                "existing_connection",
                "secure_password_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "remove_api_key_binding"),
                ("always", "revoke_connection_grants"),
            ],
        ),
        "revoke" => (
            "grant",
            &["configured_vault", "existing_grant"],
            &[("always", "revoke_grant")],
        ),
        "token" => (
            "connection",
            &[
                "configured_vault",
                "existing_connection",
                "secure_password_and_token_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "replace_token"),
                ("always", "revoke_connection_grants"),
            ],
        ),
        "note" | "rename-entry" => (
            "connection",
            &[
                "configured_vault",
                "existing_connection",
                "secure_password_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "write_public_metadata"),
            ],
        ),
        "use" => (
            "database_id",
            &["saved_database", "secure_password_input"],
            &[
                ("always", "lock_broker"),
                ("always", "switch_vault"),
                ("always", "reset_grants"),
            ],
        ),
        "open" => (
            "vault_file",
            &["readable_mdbx_file", "secure_password_input"],
            &[
                ("always", "lock_broker"),
                ("always", "create_managed_copy"),
                ("always", "switch_vault"),
                ("always", "reset_grants"),
            ],
        ),
        "library" | "keys" | "tiga show" => (
            "vault",
            &["configured_vault", "secure_password_input"],
            &[
                ("always", "lock_broker"),
                ("always", "read_vault_metadata"),
                ("android_root_missing", "restore_android_root"),
            ],
        ),
        "category" => (
            "new_category",
            &["configured_vault", "secure_password_input"],
            &[("always", "lock_broker"), ("always", "write_vault")],
        ),
        "rename-category" => (
            "category_id",
            &[
                "configured_vault",
                "existing_category",
                "secure_password_input",
            ],
            &[("always", "lock_broker"), ("always", "write_vault")],
        ),
        "move" => (
            "entry_or_category_id",
            &[
                "configured_vault",
                "existing_source_and_target_category",
                "unprotected_source",
                "compatible_adapter_for_entry",
                "secure_password_input",
            ],
            &[("always", "lock_broker"), ("always", "write_vault")],
        ),
        "delete" => (
            "connection_or_entry_id",
            &[
                "configured_vault",
                "compatible_adapter_for_entry",
                "verified_target_and_confirmation_or_force",
                "secure_password_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "tombstone_entry"),
                ("connection_target", "revoke_connection_grants"),
            ],
        ),
        "delete-category" => (
            "category_id",
            &[
                "configured_vault",
                "empty_unprotected_category",
                "verified_target_and_confirmation_or_force",
                "secure_password_input",
            ],
            &[("always", "lock_broker"), ("always", "delete_category")],
        ),
        "keys ssh" | "keys gpg" => (
            "new_key_entry",
            &[
                "configured_vault",
                "supported_key_material",
                "secure_password_input",
            ],
            &[("always", "lock_broker"), ("always", "write_vault")],
        ),
        "keys edit" => (
            "key_entry",
            &[
                "configured_vault",
                "existing_key_entry",
                "compatible_adapter_for_entry",
                "secure_password_input",
            ],
            &[("always", "lock_broker"), ("always", "write_vault")],
        ),
        "keys delete" => (
            "key_entry",
            &[
                "configured_vault",
                "existing_key_entry",
                "compatible_adapter_for_entry",
                "verified_target_and_confirmation_or_force",
                "secure_password_input",
            ],
            &[("always", "lock_broker"), ("always", "tombstone_entry")],
        ),
        // Intentionally absent: keys export is not part of machine discovery.
        "mcp-config" => (
            "grant",
            &["configured_vault", "existing_grant"],
            &[
                ("always", "write_mcp_snippet"),
                ("--install", "merge_client_config"),
            ],
        ),
        "check" => (
            "grant_or_client_file",
            &[
                "existing_grant_or_client_file",
                "unlocked_broker",
                "usable_grant",
            ],
            &[
                ("always", "authenticated_discovery"),
                ("always", "consume_rate_limit"),
            ],
        ),
        "call" => (
            "grant",
            &[
                "existing_grant",
                "public_tool_call_file",
                "unlocked_broker",
                "usable_grant",
                "allowed_operation_and_scope",
            ],
            &[
                ("always", "consume_rate_limit"),
                ("upstream_operation", "consume_call_budget"),
                ("upstream_operation", "upstream_request"),
                ("write_operation", "upstream_write"),
                ("write_operation", "write_receipt"),
            ],
        ),
        "mcp" => (
            "client_file",
            &[
                "readable_client_file",
                "unlocked_broker_for_calls",
                "usable_grant_for_calls",
            ],
            &[
                ("always", "mcp_stdio"),
                ("authorized_tool_call", "scoped_broker_operation"),
            ],
        ),
        "serve" => (
            "broker",
            &[
                "configured_vault",
                "secure_password_input",
                "available_loopback_port",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "foreground_broker"),
                ("always", "bounded_broker_session"),
                ("--proxy-grant", "authorize_local_model_session"),
            ],
        ),
        "lock" => (
            "broker",
            &["configured_vault"],
            &[("always", "drain_and_lock_broker")],
        ),
        "audit" => (
            "grant",
            &["configured_vault"],
            &[("always", "read_audit_trail")],
        ),
        "language" => (
            "language_preference",
            &[],
            &[
                ("always", "read_language_preference"),
                ("language_argument", "write_language_preference"),
            ],
        ),
        "tui" => (
            "vault",
            &["human_terminal"],
            &[
                ("always", "terminal_ui"),
                ("user_action", "trusted_management"),
            ],
        ),
        "tiga" | "mdbx" | "webdav" => ("subcommand", &["subcommand_required"], &[]),
        "tiga set" => (
            "vault_profile",
            &[
                "configured_vault",
                "secure_password_input",
                "vault_policy_allows_change",
                "reason_when_lowering",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "change_security_profile"),
            ],
        ),
        "mdbx check" | "mdbx files" => (
            "vault_file",
            &["explicit_file_or_configured_vault"],
            &[("always", "read_file_metadata")],
        ),
        "webdav status" => ("webdav_profile", &[], &[("always", "read_public_metadata")]),
        "webdav login" => (
            "webdav_profile",
            &["valid_https_webdav_url", "secure_webdav_password_input"],
            &[
                ("always", "webdav_request"),
                ("success", "save_webdav_profile"),
                ("successful_typed_password", "save_os_credential"),
            ],
        ),
        "webdav list" => (
            "remote_path",
            &["webdav_profile", "secure_webdav_password_input"],
            &[
                ("always", "webdav_request"),
                ("successful_typed_password", "save_os_credential"),
            ],
        ),
        "webdav forget-password" => (
            "webdav_profile",
            &["webdav_profile"],
            &[("always", "delete_os_credential")],
        ),
        "webdav open" => (
            "remote_vault_path",
            &[
                "webdav_profile",
                "remote_mdbx_vault",
                "secure_password_and_webdav_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "webdav_request"),
                ("always", "create_managed_copy"),
                ("always", "switch_vault"),
                ("always", "reset_grants"),
                ("successful_typed_password", "save_os_credential"),
            ],
        ),
        "webdav publish" => (
            "new_remote_path",
            &[
                "configured_vault",
                "webdav_profile",
                "new_remote_path",
                "secure_password_and_webdav_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "webdav_write"),
                ("success", "save_sync_binding"),
                ("successful_typed_password", "save_os_credential"),
            ],
        ),
        "webdav sync" => (
            "sync_binding",
            &[
                "configured_vault",
                "webdav_sync_binding",
                "nonconflicting_revisions",
                "secure_password_and_webdav_input",
            ],
            &[
                ("always", "lock_broker"),
                ("always", "webdav_request"),
                ("local_changes", "webdav_write"),
                ("remote_changes", "write_vault"),
                ("remote_changes", "retain_matching_grants_only"),
                ("success", "save_sync_binding"),
                ("successful_typed_password", "save_os_credential"),
            ],
        ),
        _ => return None,
    };
    let boundary = match name {
        "commands" | "next" | "status" | "connections" | "show" | "databases" | "audit"
        | "mdbx check" | "mdbx files" | "webdav status" => "public_local_inspection",
        "call" | "check" | "mcp" => "grant_scoped_broker",
        "tui" => "human_terminal",
        "tiga" | "mdbx" | "webdav" | "direct-config" => "command_group",
        _ => "trusted_local_management",
    };
    let retry = match name {
        "commands" | "next" | "status" | "connections" | "show" | "databases" | "audit"
        | "mdbx check" | "mdbx files" | "webdav status" => "repeatable_read",
        "call" | "mcp" => "same_request_id_for_same_write_inspect_unknown_outcome",
        "renew" => "not_idempotent_rotates_capability",
        "serve" | "tui" => "check_running_process_before_restart",
        "webdav publish" | "webdav sync" => "compare_revisions_before_retry",
        _ => "inspect_state_before_retry",
    };
    Some(json!({
        "schema_version": 1,
        "target": target,
        "prerequisites": prerequisites,
        "effects": effects.iter().map(|(when, effect)| json!({"when":when,"effect":effect})).collect::<Vec<_>>(),
        "trust_boundary": boundary,
        "discovery_grants_authority": false,
        "mcp_tool": false,
        "retry": retry,
    }))
}

/// Roles describe public arguments; never include their runtime values.
pub fn argument_role(name: &str, arg: &clap::Arg) -> Option<String> {
    if arg.is_global_set() || !arg.get_action().takes_values() {
        return None;
    }
    if arg.get_id() == "name" && name != "init" {
        return describe(name).and_then(|v| v["target"].as_str().map(str::to_owned));
    }
    Some(
        arg.get_value_names()
            .and_then(|v| v.first())
            .map(|v| v.to_ascii_lowercase())
            .unwrap_or_else(|| arg.get_id().as_str().to_owned()),
    )
}
