//! Trusted human browser management. No HTTP listener, command execution,
//! arbitrary file access, payload reader, bearer export or MCP dispatch.
use crate::credentials::VaultCredentials;
use crate::{
    admin,
    config::ConfigStore,
    databases,
    error::{GatewayError, Result},
    library,
    model::Provider,
    passwords,
    vault::Vault,
};
use base64::Engine;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

mod receipts;
pub const PROTOCOL: u32 = 2;
pub const CAPABILITIES: &[&str] = &[
    "snapshots",
    "mutation_receipts",
    "key_file_bytes",
    "key_management",
    "grant_management",
    "configured_sync",
];

pub const MAX_FRAME: usize = 64 * 1024;
const MAX_REPLY: usize = 900 * 1024;
const IDLE: Duration = Duration::from_secs(300);
const LIFETIME: Duration = Duration::from_secs(900);

pub fn valid_origin(origin: &str) -> bool {
    origin
        .strip_prefix("chrome-extension://")
        .and_then(|s| s.strip_suffix('/'))
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| (b'a'..=b'p').contains(&b)))
}

// Secret-bearing request fields have neither Debug nor Serialize.
#[derive(Default)]
pub struct Secret(Zeroizing<String>);
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(|s| Self(Zeroizing::new(s)))
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello {},
    Unlock {
        password: Secret,
        key_file: Option<Secret>,
    },
    Lock {},
    Snapshot {},
    SwitchDatabase {
        database: String,
        password: Secret,
        key_file: Option<Secret>,
    },
    OperationStatus {
        operation: uuid::Uuid,
    },
    CreateCategory {
        title: String,
        parent: Option<String>,
    },
    RenameCategory {
        entry: String,
        title: String,
    },
    Move {
        entry: String,
        target: String,
    },
    Delete {
        entry: String,
    },
    DeleteCategory {
        entry: String,
    },
    PasswordInfo {
        entry: String,
    },
    CreatePassword {
        identity: uuid::Uuid,
        title: String,
        category: Option<String>,
        fields: Secret,
    },
    EditPassword {
        entry: String,
        head: String,
        title: Option<String>,
        fields: Secret,
    },
    AddConnection {
        name: String,
        title: String,
        provider: Provider,
        category: Option<String>,
        token: Secret,
    },
    RenameConnection {
        name: String,
        title: String,
    },
    ReplaceToken {
        name: String,
        token: Secret,
    },
    Revoke {
        name: String,
    },
    Keys {},
    GenerateSsh {
        title: String,
        category: Option<String>,
        algorithm: String,
        auth: Factors,
    },
    ImportKey {
        title: String,
        category: Option<String>,
        format: KeyFormat,
        text: Secret,
        auth: Factors,
    },
    IssueGrant {
        name: String,
        connection: String,
        repositories: Vec<String>,
        operations: Vec<crate::model::Operation>,
        ttl_minutes: u32,
        requests_per_minute: u32,
        max_calls: u32,
        approval: crate::model::ApprovalPolicy,
        auth: Factors,
    },
    RenewGrant {
        name: String,
        auth: Factors,
    },
    Sync {
        webdav_password: Secret,
        auth: Factors,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyFormat {
    Ssh,
    Openpgp,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Factors {
    password: Secret,
    key_file: Option<Secret>,
}
impl Factors {
    fn credentials(self) -> Result<VaultCredentials> {
        let bytes = self
            .key_file
            .map(|s| {
                if s.0.len() > 44 * 1024 {
                    return Err(GatewayError::InvalidKeyFile);
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(s.0.as_bytes())
                    .map(Zeroizing::new)
                    .map_err(|_| GatewayError::InvalidKeyFile)?;
                if bytes.is_empty() || bytes.len() > 32 * 1024 {
                    return Err(GatewayError::InvalidKeyFile);
                }
                Ok(bytes)
            })
            .transpose()?;
        VaultCredentials::from_key_bytes(self.password.0, bytes)
    }
}
impl Request {
    fn mutation(&self) -> Option<&'static str> {
        Some(match self {
            Self::SwitchDatabase { .. } => "switch_database",
            Self::CreateCategory { .. } => "create_category",
            Self::RenameCategory { .. } => "rename_category",
            Self::Move { .. } => "move",
            Self::Delete { .. } => "delete",
            Self::DeleteCategory { .. } => "delete_category",
            Self::CreatePassword { .. } => "create_password",
            Self::EditPassword { .. } => "edit_password",
            Self::AddConnection { .. } => "add_connection",
            Self::RenameConnection { .. } => "rename_connection",
            Self::ReplaceToken { .. } => "replace_token",
            Self::Revoke { .. } => "revoke",
            Self::GenerateSsh { .. } => "generate_ssh",
            Self::ImportKey { .. } => "import_key",
            Self::IssueGrant { .. } => "issue_grant",
            Self::RenewGrant { .. } => "renew_grant",
            Self::Sync { .. } => "sync",
            _ => return None,
        })
    }
}

#[derive(Deserialize)]
struct Envelope {
    id: u32,
    #[serde(default)]
    protocol: u32,
    operation_id: Option<uuid::Uuid>,
    #[serde(flatten)]
    request: Request,
}

struct Session {
    credentials: VaultCredentials,
    path: PathBuf,
    vault_id: String,
    started: Instant,
    used: Instant,
}
impl AsRef<str> for Session {
    fn as_ref(&self) -> &str {
        self.credentials.as_ref()
    }
}
impl crate::credentials::VaultPassword for Session {
    fn security_key(&self) -> Option<&[u8]> {
        self.credentials.security_key()
    }
    fn expected_vault_id(&self) -> Option<&str> {
        Some(&self.vault_id)
    }
    fn expected_vault_path(&self) -> Option<&std::path::Path> {
        Some(&self.path)
    }
}

#[cfg(test)]
mod tests;
pub struct BrowserHost {
    store: ConfigStore,
    session: Option<Session>,
}
impl BrowserHost {
    pub fn new(store: ConfigStore) -> Self {
        Self {
            store,
            session: None,
        }
    }
    pub fn lock(&mut self) {
        self.session = None;
    }
    fn expire(&mut self) {
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.used.elapsed() >= IDLE || s.started.elapsed() >= LIFETIME)
        {
            self.lock();
        }
    }
    fn with_vault<T>(&self, action: impl FnOnce(&Vault) -> Result<T>) -> Result<T> {
        let session = self.session.as_ref().ok_or(GatewayError::UnlockRequired)?;
        passwords::with_vault(&self.store, session, action)
    }
    fn snapshot(&self) -> Result<Value> {
        self.with_vault(|vault| {
        let tree = vault.library()?;
        // Deliberate projection: never serialize Config, connection bindings or capabilities.
        let status = admin::status(&self.store)?;
        let bindings = self.store.load()?.connections;
        let entries: Vec<_> = tree.entries.iter().map(|entry| {
            let connection = library::entry_connection(entry, &bindings).map(|(name,_)| name);
            json!({"id":entry.id,"category":entry.category,"title":entry.title,"kind":entry.kind,"connection":connection})
        }).collect();
        Ok(json!({"vault_id":vault.vault_id()?,"categories":tree.categories,"entries":entries,
            "databases":databases::list(&self.store)?,"connections":status["connections"],"grants":status["grants"],
            "sync":status["webdav"].as_object().map(|r| json!({"configured":true,"path":r["path"],"last_sync":r["last_sync"]}))}))
        })
    }
    fn reauthenticate(&self, auth: Factors) -> Result<Session> {
        let current = self.session.as_ref().ok_or(GatewayError::UnlockRequired)?;
        let session = Session {
            credentials: auth.credentials()?,
            path: current.path.clone(),
            vault_id: current.vault_id.clone(),
            started: current.started,
            used: Instant::now(),
        };
        passwords::with_vault(&self.store, &session, |_| Ok(()))?;
        Ok(session)
    }
    async fn execute(&mut self, envelope: Envelope) -> Result<Value> {
        if !matches!(envelope.request, Request::Hello {}) && envelope.protocol != PROTOCOL {
            return Err(GatewayError::InvalidRequest);
        }
        let Some(operation) = envelope.request.mutation() else {
            return self.dispatch(envelope.request).await;
        };
        let id = envelope.operation_id.ok_or(GatewayError::InvalidRequest)?;
        self.expire();
        admin::lock_broker(&self.store).await?;
        let vault_id = if let Request::SwitchDatabase {
            database,
            password,
            key_file,
        } = &envelope.request
        {
            // Authenticate the saved target before accepting an intent, including
            // when no source database session exists. Never accept a browser path.
            let target = databases::list(&self.store)?
                .into_iter()
                .find(|d| d.id == *database && !d.current)
                .ok_or(GatewayError::NotFound)?;
            let factors = Factors {
                password: Secret(password.0.clone()),
                key_file: key_file.as_ref().map(|s| Secret(s.0.clone())),
            };
            let vault = Vault::open(&target.path, &factors.credentials()?)?;
            let id = vault.vault_id();
            vault.lock()?;
            id?
        } else {
            self.with_vault(|v| v.vault_id())?
        };
        let mut journal = receipts::Receipts::open(&self.store)?;
        if let Some(receipt) = journal.begin(id, &vault_id, operation)? {
            return Ok(
                json!({"saved":receipt["state"] == "succeeded","receipt":receipt,"replayed":false}),
            );
        }
        let outcome = self.dispatch(envelope.request).await;
        journal.finish(id, &outcome)?;
        outcome.map(|mut value| {
            value["receipt"] = journal.status(id, &vault_id).unwrap_or(Value::Null);
            value
        })
    }
    pub async fn dispatch(&mut self, request: Request) -> Result<Value> {
        self.expire();
        match request {
            Request::Lock {} => {
                self.lock();
                return Ok(json!({"locked":true}));
            }
            Request::Hello {} => {
                return Ok(
                    json!({"protocol":PROTOCOL,"version":env!("CARGO_PKG_VERSION"),"integration_revision":2,"capabilities":CAPABILITIES,"max_frame":MAX_FRAME,"idle_seconds":IDLE.as_secs(),"absolute_seconds":LIFETIME.as_secs(),"unlocked":self.session.is_some(),"databases":databases::list(&self.store)?}),
                );
            }
            Request::Unlock { password, key_file } => {
                self.lock();
                admin::lock_broker(&self.store).await?;
                let path = self.store.load()?.vault;
                let credentials = Factors { password, key_file }.credentials()?;
                let vault_id =
                    passwords::with_vault(&self.store, &credentials, |vault| vault.vault_id())?;
                let now = Instant::now();
                self.session = Some(Session {
                    credentials,
                    path,
                    vault_id,
                    started: now,
                    used: now,
                });
                return self.snapshot();
            }
            Request::SwitchDatabase {
                database,
                password,
                key_file,
            } => {
                self.lock();
                admin::lock_broker(&self.store).await?;
                databases::switch(
                    &self.store,
                    &database,
                    &Factors { password, key_file }.credentials()?,
                )?;
                return Ok(
                    json!({"locked":true,"grants_reset":true,"databases":databases::list(&self.store)?}),
                );
            }
            _ => {}
        }
        if self.session.is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        admin::lock_broker(&self.store).await?;
        self.expire();
        let session = self.session.as_mut().ok_or(GatewayError::UnlockRequired)?;
        session.used = Instant::now();
        let password = &*session;
        match request {
            Request::Snapshot {} => return self.snapshot(),
            Request::OperationStatus { operation } => {
                let vault = self.with_vault(|v| v.vault_id())?;
                return receipts::Receipts::open(&self.store)?.status(operation, &vault);
            }
            Request::Keys {} => {
                // No key payload, note, user ID, comment or existing secret is
                // returned. Native key listing still performs Tiga disclosure.
                let keys = crate::keys::manage::list(&self.store, password)?;
                return Ok(
                    json!({"keys":keys.into_iter().map(|k| json!({"entry_id":k.entry_id,"title":k.title,"kind":k.login_type,"algorithm":k.algorithm,"fingerprint":k.fingerprint})).collect::<Vec<_>>()}),
                );
            }
            Request::GenerateSsh {
                title,
                category,
                algorithm,
                auth,
            } => {
                let auth = self.reauthenticate(auth)?;
                crate::keys::manage::generate_ssh(
                    &self.store,
                    &auth,
                    &title,
                    "",
                    category.as_deref(),
                    &algorithm,
                    "",
                )?;
            }
            Request::ImportKey {
                title,
                category,
                format,
                text,
                auth,
            } => {
                let auth = self.reauthenticate(auth)?;
                if text.0.len() > 32 * 1024 {
                    return Err(GatewayError::KeyPayloadTooLarge);
                }
                match format {
                    KeyFormat::Ssh => {
                        crate::keys::manage::import_ssh_text(
                            &self.store,
                            &auth,
                            &title,
                            "",
                            category.as_deref(),
                            "",
                            &text.0,
                        )?;
                    }
                    KeyFormat::Openpgp => {
                        crate::keys::manage::import_gpg_text(
                            &self.store,
                            &auth,
                            &title,
                            "",
                            category.as_deref(),
                            &text.0,
                        )?;
                    }
                }
            }
            Request::IssueGrant {
                name,
                connection,
                repositories,
                operations,
                ttl_minutes,
                requests_per_minute,
                max_calls,
                approval,
                auth,
            } => {
                if !(1..=1440).contains(&ttl_minutes) {
                    return Err(GatewayError::InvalidRequest);
                }
                let auth = self.reauthenticate(auth)?;
                admin::issue_grant(
                    &self.store,
                    &admin::GrantOptions {
                        name,
                        connection,
                        repositories,
                        operations,
                        ttl_minutes,
                        requests_per_minute,
                        max_calls,
                        approval,
                        out: None,
                    },
                    &auth,
                )?;
            }
            Request::RenewGrant { name, auth } => {
                let auth = self.reauthenticate(auth)?;
                admin::refresh_grant(
                    &self.store,
                    &admin::RefreshOptions {
                        name,
                        window: None,
                        call_cap: None,
                        approval: None,
                    },
                    &auth,
                )?;
            }
            Request::Sync {
                webdav_password,
                auth,
            } => {
                let auth = self.reauthenticate(auth)?;
                let binding = self
                    .store
                    .load()?
                    .webdav
                    .ok_or(GatewayError::RemoteNotConfigured)?;
                let client = crate::webdav::WebDavClient::new(binding.profile, webdav_password.0)?;
                let outcome = crate::sync::synchronize(&self.store, &client, &auth).await?;
                self.lock();
                return Ok(json!({"saved":true,"locked":true,"sync_result":outcome.result}));
            }
            Request::CreateCategory { title, parent } => {
                library::create_category(&self.store, password, &title, parent.as_deref())?;
            }
            Request::RenameCategory { entry, title } => {
                library::rename_category(&self.store, password, &entry, &title)?
            }
            Request::Move { entry, target } => {
                library::move_item(&self.store, password, &entry, &target)?
            }
            Request::DeleteCategory { entry } => {
                library::delete_category(&self.store, password, &entry).map_err(|e| e.error())?;
            }
            Request::Delete { entry } => {
                let config = self.store.load()?;
                if let Some((name, _)) = config
                    .connections
                    .iter()
                    .find(|(_, c)| c.credential_id == entry)
                {
                    admin::delete_connection(&self.store, name, password)?;
                } else {
                    library::delete_entry(&self.store, password, &entry)?;
                }
            }
            Request::PasswordInfo { entry } => {
                return self.with_vault(|vault| Ok(json!(vault.password_summary(&entry)?)));
            }
            Request::CreatePassword {
                identity,
                title,
                category,
                fields,
            } => {
                self.with_vault(|vault| {
                    vault.create_password(identity, category.as_deref(), &title, &fields.0)
                })?;
            }
            Request::EditPassword {
                entry,
                head,
                title,
                fields,
            } => {
                self.with_vault(|vault| {
                    vault.edit_password(&entry, &head, title.as_deref(), &fields.0)
                })?;
            }
            Request::AddConnection {
                name,
                title,
                provider,
                category,
                token,
            } => {
                if provider == Provider::ApiKey {
                    return Err(GatewayError::InvalidRequest);
                }
                admin::add_connection_in_category(
                    &self.store,
                    admin::NewConnection {
                        name: &name,
                        title: &title,
                        provider,
                        base: provider.default_api_base(),
                        note: "",
                        category: category.as_deref(),
                    },
                    password,
                    token.0,
                )?;
            }
            Request::RenameConnection { name, title } => {
                admin::rename_entry(&self.store, &name, &title, password)?
            }
            Request::ReplaceToken { name, token } => {
                admin::update_token(&self.store, &name, password, token.0)?
            }
            Request::Revoke { name } => {
                self.with_vault(|_| Ok(()))?;
                let expected_path = &self
                    .session
                    .as_ref()
                    .ok_or(GatewayError::UnlockRequired)?
                    .path;
                admin::revoke_in_vault(&self.store, &name, Some(expected_path))?;
            }
            _ => return Err(GatewayError::InvalidRequest),
        }
        Ok(json!({"saved":true}))
    }
}

pub async fn serve(
    host: &mut BrowserHost,
    mut input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> Result<()> {
    loop {
        let mut header = [0; 4];
        // The native process itself wipes its session when no messages arrive.
        match tokio::time::timeout(Duration::from_secs(1), input.read_exact(&mut header[..1])).await
        {
            Err(_) => {
                host.expire();
                continue;
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                host.lock();
                return Ok(());
            }
            Ok(Err(_)) => return Err(GatewayError::InvalidRequest),
            Ok(Ok(_)) => {}
        }
        // Once a frame starts, partial input has a hard deadline. read_exact is
        // never cancelled and retried against the middle of a header/body.
        let raw = tokio::time::timeout(Duration::from_secs(10), async {
            input
                .read_exact(&mut header[1..])
                .await
                .map_err(|_| GatewayError::InvalidRequest)?;
            let size = u32::from_le_bytes(header) as usize;
            if size == 0 || size > MAX_FRAME {
                return Err(GatewayError::InvalidRequest);
            }
            let mut bytes = Zeroizing::new(vec![0; size]);
            input
                .read_exact(&mut bytes)
                .await
                .map_err(|_| GatewayError::InvalidRequest)?;
            Ok(bytes)
        })
        .await
        .map_err(|_| GatewayError::InvalidRequest)??;
        let parsed = serde_json::from_slice::<Envelope>(&raw);
        let reply = match parsed {
            Ok(envelope) => {
                let id = envelope.id;
                match host.execute(envelope).await {
                    Ok(data) => json!({"id":id,"ok":true,"data":data}),
                    Err(error) => {
                        if matches!(
                            error,
                            GatewayError::UnlockRequired | GatewayError::ObjectChanged
                        ) {
                            host.lock();
                        }
                        json!({"id":id,"ok":false,"error":error})
                    }
                }
            }
            Err(_) => {
                host.lock();
                json!({"id":0,"ok":false,"error":"invalid_request"})
            }
        };
        let mut bytes = serde_json::to_vec(&reply).map_err(|_| GatewayError::InvalidRequest)?;
        if bytes.len() > MAX_REPLY {
            bytes = serde_json::to_vec(
                &json!({"id":reply["id"],"ok":false,"error":"response_too_large"}),
            )
            .unwrap();
        }
        output
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await
            .map_err(|_| GatewayError::StateUnavailable)?;
        output
            .write_all(&bytes)
            .await
            .map_err(|_| GatewayError::StateUnavailable)?;
        output
            .flush()
            .await
            .map_err(|_| GatewayError::StateUnavailable)?;
    }
}
