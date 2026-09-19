//! All network and vault fixtures are local, disposable and use synthetic credentials.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer};
use zeroize::Zeroizing;

use crate::config::{
    Config, ConfigStore, Grant, capability_hash, connection_fingerprint, new_capability,
};
use crate::gateway::Gateway;
use crate::model::{Operation, Provider, ToolCall};
use crate::vault::Vault;

pub const PASSWORD: &str = "Synthetic vault passphrase for tests only 7283!";
pub const TOKEN: &str = "synthetic-upstream-token-must-stay-inside-broker";
pub const REPOSITORY: &str = "example/project";

#[derive(Clone, Debug)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub location: Option<String>,
    pub disconnect: bool,
    pub omit_length: bool,
    pub delay: Duration,
    pub headers: Vec<(String, String)>,
}

impl Reply {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: serde_json::to_vec(&value).unwrap(),
            location: None,
            disconnect: false,
            omit_length: false,
            delay: Duration::ZERO,
            headers: Vec::new(),
        }
    }

    pub fn disconnected() -> Self {
        Self {
            disconnect: true,
            ..Self::json(Value::Null)
        }
    }
}

pub struct FakeUpstream {
    pub port: u16,
    pub client: reqwest::Client,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    observed: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeUpstream {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let mut replies: VecDeque<_> = replies.into();
        Self::start_handler(move |_| {
            replies.pop_front().unwrap_or_else(|| Reply {
                status: 500,
                ..Reply::json(Value::Null)
            })
        })
        .await
    }

    pub async fn start_handler(
        mut handler: impl FnMut(&CapturedRequest) -> Reply + Send + 'static,
    ) -> Self {
        let generated = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        let certificate = generated.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
        let server = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], key.into())
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server));
        let client = crate::upstream::client_builder()
            .add_root_certificate(reqwest::Certificate::from_der(certificate.as_ref()).unwrap())
            .build()
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let observed = Arc::new(Notify::new());
        let signal = observed.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let Ok(Ok(stream)) =
                    tokio::time::timeout(Duration::from_secs(5), acceptor.accept(stream)).await
                else {
                    continue;
                };
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        break;
                    }
                    head.push_str(&line);
                    if line == "\r\n" || head.len() > 64 * 1024 {
                        break;
                    }
                }
                let mut lines = head.lines();
                let parts: Vec<_> = lines
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .collect();
                if parts.len() != 3 {
                    continue;
                }
                let headers: BTreeMap<_, _> = lines
                    .filter_map(|line| {
                        line.split_once(':').map(|(name, value)| {
                            (name.to_ascii_lowercase(), value.trim().to_owned())
                        })
                    })
                    .collect();
                let length: usize = headers
                    .get("content-length")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                assert!(length <= 64 * 1024 * 1024);
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).await.is_err() {
                    continue;
                }
                let request = CapturedRequest {
                    method: parts[0].to_owned(),
                    target: parts[1].to_owned(),
                    headers,
                    body,
                };
                let reply = handler(&request);
                captured.lock().unwrap().push(request);
                signal.notify_waiters();
                if !reply.delay.is_zero() {
                    tokio::time::sleep(reply.delay).await;
                }
                if reply.disconnect {
                    continue;
                }
                let mut head = format!(
                    "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nConnection: close\r\n",
                    reply.status
                );
                if !reply.omit_length {
                    head.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
                }
                if let Some(location) = reply.location {
                    head.push_str(&format!("Location: {location}\r\n"));
                }
                for (name, value) in reply.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = reader.get_mut().write_all(head.as_bytes()).await;
                let _ = reader.get_mut().write_all(&reply.body).await;
                let _ = reader.get_mut().shutdown().await;
            }
        });
        Self {
            port,
            client,
            requests,
            observed,
            task,
        }
    }

    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub async fn wait_for_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let notified = self.observed.notified();
                if self.requests.lock().unwrap().len() >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .expect("fake upstream did not receive the expected request");
    }
}

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub struct Fixture {
    pub gateway: Arc<Gateway>,
    pub vault: Vault,
    pub store: ConfigStore,
    pub capability: Zeroizing<String>,
    pub upstream: FakeUpstream,
    pub broker_listener: Option<TcpListener>,
    pub _directory: tempfile::TempDir,
}

impl Fixture {
    pub async fn new(provider: Provider, operations: &[Operation], replies: Vec<Reply>) -> Self {
        let upstream = FakeUpstream::start(replies).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, mdbx_core::tiga::TigaMode::Multi).unwrap();
        let suffix = if provider == Provider::Gitlab {
            "/api/v4/"
        } else {
            "/"
        };
        let base = format!("https://127.0.0.1:{}{suffix}", upstream.port);
        let (collection, connection) = vault
            .store_credential(
                None,
                "work",
                provider,
                &base,
                "用于项目 Issue 跟踪",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        let broker_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Config::new(path);
        config
            .listen
            .set_port(broker_listener.local_addr().unwrap().port());
        config.collection_id = Some(collection);
        let capability = new_capability();
        let now = chrono::Utc::now().timestamp();
        config.grants.push(Grant {
            name: "test-agent".to_owned(),
            capability_hash: capability_hash(&capability),
            connection: "work".to_owned(),
            connection_fingerprint: connection_fingerprint(&connection),
            repositories: [if operations.iter().any(|op| op.is_api()) {
                "*"
            } else {
                REPOSITORY
            }
            .to_owned()]
            .into(),
            operations: operations.iter().copied().collect(),
            issued_at: now,
            expires_at: now + 3600,
            requests_per_minute: 60,
            max_calls: 0,
            client_file: None,
        });
        config.connections.insert("work".to_owned(), connection);
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        store.update(|_| Ok((config, ()))).unwrap();
        let gateway = Arc::new(
            Gateway::with_client(store.clone(), vault.clone(), upstream.client.clone()).unwrap(),
        );
        Self {
            gateway,
            vault,
            store,
            capability,
            upstream,
            broker_listener: Some(broker_listener),
            _directory: directory,
        }
    }

    /// A fresh Gateway over the same store, like restarting the broker after a lock.
    pub fn rebuild(&self) -> Arc<Gateway> {
        Arc::new(
            Gateway::with_client(
                self.store.clone(),
                self.vault.clone(),
                self.upstream.client.clone(),
            )
            .unwrap(),
        )
    }

    pub fn call(&self, operation: Operation, arguments: Value) -> ToolCall {
        let provider = self.store.load().unwrap().connections["work"].provider;
        ToolCall {
            tool: operation.tool_name(provider),
            arguments,
        }
    }
}

pub fn issue(provider: Provider, number: u64) -> Value {
    match provider {
        Provider::Github => {
            json!({"number":number, "title":"Fixture issue", "state":"open", "body":"Private fixture body", "html_url":"https://github.com/example/project/issues/42", "ignored":"not forwarded"})
        }
        Provider::Gitlab => {
            json!({"iid":number, "title":"Fixture issue", "state":"opened", "description":"Private fixture body", "web_url":"https://gitlab.com/example/project/-/issues/42", "ignored":"not forwarded"})
        }
    }
}
