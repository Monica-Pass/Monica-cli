//! The security profile a vault runs on: what it decides today, and how a person moves it.
use mdbx_core::tiga::{PolicyCompliance, ResolvedTigaPolicy};
use serde::Serialize;

use crate::admin::TigaLevel;
use crate::config::ConfigStore;
use crate::error::Result;
use crate::upstream::reject_secret_value;
use crate::vault::Vault;

/// Reported by both commands. The stored name and the policy in force are kept apart because
/// lowering a profile leaves the higher name in the vault header and records an exception
/// beside it, so one field alone would hide that the vault is currently running reduced.
#[derive(Debug, Serialize)]
pub struct Report {
    pub default_profile: String,
    #[serde(flatten)]
    pub resolved: ResolvedTigaPolicy,
}

impl Report {
    /// The profile the vault actually answers to right now.
    pub fn effective_profile(&self) -> String {
        self.resolved.policy.profile.to_string()
    }

    /// True when the vault is running below the policy its stored name promises.
    pub fn reduced(&self) -> bool {
        self.default_profile != self.effective_profile()
    }

    /// True when the vault runs the full policy but the unlock methods on it are
    /// too weak for that policy. The profile itself is not reduced here, so this
    /// has to stay a separate statement: a Power vault created with a password
    /// alone lands in it, and `default_profile == effective_profile` there.
    pub fn needs_remediation(&self) -> bool {
        self.resolved.compliance == PolicyCompliance::RemediationRequired
    }
}

fn report(vault: &Vault) -> Result<Report> {
    Ok(Report {
        default_profile: vault.tiga_default()?.to_string(),
        resolved: vault.tiga_policy()?,
    })
}

pub fn show(store: &ConfigStore, password: &str) -> Result<Report> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = report(&vault);
    vault.lock()?;
    result
}

/// Move the vault to another profile and report what it runs under afterwards.
///
/// A reason only means something when lowering, and the engine refuses a lowering without one,
/// so nobody can quietly weaken a vault they were merely handed. Typing one while raising would
/// read later as if it had been recorded, so that is refused too.
pub fn set(
    store: &ConfigStore,
    password: &str,
    level: TigaLevel,
    reason: Option<&str>,
) -> Result<Report> {
    if let Some(reason) = reason {
        reject_secret_value(&serde_json::json!([reason]), password)?;
    }
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault
        .set_tiga_policy(level.mode(), reason)
        .and_then(|()| report(&vault));
    vault.lock()?;
    result
}

#[cfg(test)]
mod tests {
    use super::{ConfigStore, TigaLevel, report, set, show};
    use crate::admin::{NewVault, initialize_with};
    use crate::error::GatewayError;
    use crate::test_support::PASSWORD;
    use crate::vault::Vault;
    use mdbx_core::tiga::{PolicyCompliance, TigaMode};

    fn store_with(level: TigaLevel) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        initialize_with(
            &store,
            &directory.path().join("vault.mdbx"),
            47841,
            PASSWORD,
            PASSWORD,
            &NewVault {
                name: None,
                tiga: level,
            },
        )
        .unwrap();
        directory
    }

    fn store_in(directory: &std::path::Path) -> ConfigStore {
        ConfigStore::new(directory.join("gateway.json"))
    }

    /// The four transitions a password-only terminal can carry out, in the order a person
    /// would walk them. Measured against real vaults, including what a lowering leaves behind.
    #[test]
    fn raising_is_plain_and_lowering_is_a_recorded_exception() {
        let directory = store_with(TigaLevel::Sky);
        let store = store_in(directory.path());
        let report = set(&store, PASSWORD, TigaLevel::Multi, None).unwrap();
        assert_eq!(report.default_profile, "multi");
        assert_eq!(report.effective_profile(), "multi");
        assert!(!report.reduced());

        assert_eq!(
            set(&store, PASSWORD, TigaLevel::Sky, None).unwrap_err(),
            GatewayError::TigaReasonRequired,
            "a lowering has to say why"
        );
        assert_eq!(
            set(&store, PASSWORD, TigaLevel::Sky, Some("   ")).unwrap_err(),
            GatewayError::TigaReasonRequired,
            "padding is not a reason"
        );
        assert_eq!(
            set(&store, PASSWORD, TigaLevel::Power, Some("raising anyway")).unwrap_err(),
            GatewayError::TigaReasonNotApplicable,
            "a raising records nothing, so a reason would read later as if it had"
        );

        let report = set(
            &store,
            PASSWORD,
            TigaLevel::Sky,
            Some("the vendor app cannot hold two factors"),
        )
        .unwrap();
        assert_eq!(report.default_profile, "multi", "the higher name stays");
        assert_eq!(report.effective_profile(), "sky");
        assert_eq!(report.resolved.compliance, PolicyCompliance::Exception);
        assert!(report.resolved.exception_id.is_some());
        assert!(report.reduced());

        // Raising back to the stored name is what clears the exception again.
        let report = set(&store, PASSWORD, TigaLevel::Multi, None).unwrap();
        assert!(!report.reduced());
        assert_eq!(report.resolved.exception_id, None);
        assert_eq!(report.resolved.compliance, PolicyCompliance::Compliant);
        let report = show(&store, PASSWORD).unwrap();
        assert_eq!(report.default_profile, "multi");
        assert_eq!(report.effective_profile(), "multi");
    }

    /// A Power vault is the one profile this CLI can move into but not out of: Power asks for
    /// two factors and hardware-backed device assurance, and a password-unlocked terminal has
    /// neither, so the engine's own authorization gate refuses the lowering. Measured, not
    /// assumed — as is the heaviest Argon2id cost the same profile puts on every unlock.
    #[test]
    fn a_power_vault_refuses_to_be_lowered_from_here() {
        let directory = store_with(TigaLevel::Power);
        let vault = Vault::open(&directory.path().join("vault.mdbx"), PASSWORD).unwrap();
        assert_eq!(vault.tiga_default().unwrap(), TigaMode::Power);
        let resolved = vault.tiga_policy().unwrap();
        assert_eq!(resolved.policy.profile, TigaMode::Power);
        assert_eq!(
            resolved.compliance,
            PolicyCompliance::RemediationRequired,
            "a Power vault created here never meets its own assurance bar from a password-only session"
        );
        // The flag is about how the vault is unlocked, not about running reduced:
        // the name and the policy in force are the same here, so the reduced
        // sentence must not fire and claim a power vault is running something else.
        let report = report(&vault).unwrap();
        assert_eq!(report.default_profile, "power");
        assert_eq!(report.effective_profile(), "power");
        assert!(!report.reduced());
        assert!(report.needs_remediation());
        assert_eq!(
            vault
                .set_tiga_policy(
                    TigaMode::Multi,
                    Some("the hardware requirement is unworkable")
                )
                .unwrap_err(),
            GatewayError::TigaChangeDenied
        );
    }
}
