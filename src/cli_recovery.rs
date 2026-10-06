//! CLI-only recovery hints. Fixed templates never interpolate rejected argv,
//! paths, credentials or upstream messages. MCP retains its own wire contract.
use monica_pass_cli::error::GatewayError;
use monica_pass_cli::i18n::{Language, Message};
use serde_json::{Value, json};

pub fn recovery(error: GatewayError, command: &str) -> (Value, Message) {
    use GatewayError::*;
    let command = crate::cli_contract::canonical_name(command);
    let (retry, actions, message): (&str, Vec<String>, Message) = match error {
        SecretInputRequired | InvalidSecretInput | HumanTerminalRequired => (
            "after_trusted_secret_input",
            vec![format!("monica {command} --help")],
            Message::RecoverySecret,
        ),
        SetupRequired => (
            "after_setup",
            vec!["monica next".into()],
            Message::RecoverySetup,
        ),
        BrokerUnavailable | UnlockRequired => (
            "after_human_unlock",
            vec!["monica status".into(), "monica serve".into()],
            Message::RecoveryUnlock,
        ),
        BrokerAlreadyRunning | ListenUnavailable => (
            "inspect_running_process",
            vec!["monica status".into()],
            Message::RecoveryBusy,
        ),
        ReauthorizationRequired => (
            "after_human_reauthorization",
            vec!["monica status".into(), "monica renew <GRANT>".into()],
            Message::RecoveryRenew,
        ),
        Unauthorized | PermissionDenied => (
            "after_authorization_review",
            vec!["monica status".into()],
            Message::RecoveryAuthority,
        ),
        ApprovalDenied => ("do_not_retry", vec![], Message::RecoveryDenied),
        GlitterUnavailable => (
            "use_compatible_client",
            vec![],
            Message::ErrorGlitterUnavailable,
        ),
        KeyFileRequired => (
            "after_key_file_input",
            vec![format!("monica {command} --help")],
            Message::ErrorKeyFileRequired,
        ),
        InvalidKeyFile => (
            "after_valid_key_file_input",
            vec![],
            Message::ErrorInvalidKeyFile,
        ),
        ApprovalTimeout => (
            "after_human_approval_same_request",
            vec![],
            Message::RecoveryApproval,
        ),
        WriteOutcomeUnknown | RequestIdConflict => (
            "inspect_outcome_no_new_request_id",
            vec!["monica audit --grant <GRANT>".into()],
            Message::RecoveryWrite,
        ),
        SyncOutcomeUnknown
        | SyncConflict
        | SyncStateMissing
        | RemoteVersionRequired
        | RemoteProtocolUnsupported
        | BlobUnavailable
        | SyncCancelled
        | SyncSegmentCorrupt => (
            "compare_local_and_remote_first",
            vec!["monica webdav status".into()],
            Message::RecoverySync,
        ),
        ConfirmationRequired => (
            "after_human_target_verification",
            vec![format!("monica {command} --help")],
            Message::RecoveryConfirm,
        ),
        ObjectReadOnly | ObjectPayloadTooLarge | AttachmentMoveUnsupported => (
            "requires_compatible_client",
            vec![],
            Message::RecoveryInspect,
        ),
        ObjectChanged => (
            "reread_before_edit",
            vec!["monica library".into()],
            Message::RecoveryTarget,
        ),
        RateLimited | BrokerBusy => ("later_same_request", vec![], Message::RecoveryWait),
        NotFound | AlreadyExists => {
            let list = match command {
                "use" | "open" | "init" => "databases",
                "show" | "note" | "token" | "connect" | "rename-entry" => "connections",
                _ => "status",
            };
            (
                "inspect_target_first",
                vec![format!("monica {list}"), format!("monica {command} --help")],
                Message::RecoveryTarget,
            )
        }
        _ => (
            "inspect_before_retry",
            vec![format!("monica {command} --help")],
            Message::RecoveryInspect,
        ),
    };
    (
        json!({
            "retry":retry,"commands":actions,"automatic_retry":false,
            "use_same_config":true,
            "command_contract":crate::cli_contract::describe(command),
        }),
        message,
    )
}

pub fn render(value: &Value, message: Message, language: Language) -> String {
    let mut out = language.text(message).to_owned();
    for command in value["commands"].as_array().into_iter().flatten() {
        if let Some(command) = command.as_str() {
            out.push_str("\n  ");
            out.push_str(command);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncertain_writes_and_refusals_never_invite_blind_retries() {
        for error in [
            GatewayError::WriteOutcomeUnknown,
            GatewayError::RequestIdConflict,
        ] {
            let (value, _) = recovery(error, "call");
            assert_eq!(value["retry"], "inspect_outcome_no_new_request_id");
            assert_eq!(value["automatic_retry"], false);
            assert_eq!(value["commands"], json!(["monica audit --grant <GRANT>"]));
        }
        let (value, _) = recovery(GatewayError::ApprovalDenied, "call");
        assert_eq!(value["retry"], "do_not_retry");
        assert_eq!(value["commands"], json!([]));
        let (value, _) = recovery(GatewayError::ConfirmationRequired, "delete");
        assert!(!value["commands"].to_string().contains("--force"));
    }

    #[test]
    fn glitter_refusal_never_recommends_creation_or_opening_in_this_client() {
        let (value, message) = recovery(GatewayError::GlitterUnavailable, "webdav sync");
        assert_eq!(value["retry"], "use_compatible_client");
        assert_eq!(value["automatic_retry"], false);
        assert_eq!(value["commands"], json!([]));
        assert_eq!(
            serde_json::to_value(GatewayError::GlitterUnavailable).unwrap(),
            "glitter_unavailable"
        );
        let english = render(&value, message, Language::En);
        let chinese = render(&value, message, Language::ZhCn);
        assert!(english.contains("client that explicitly supports Glitter"));
        assert!(chinese.contains("明确支持 Glitter 的客户端"));
        for rendered in [&english, &chinese] {
            assert!(!rendered.contains("Monica for Android"));
            assert!(!rendered.contains("monica open"));
            assert!(!rendered.contains("monica init"));
        }
    }
}
