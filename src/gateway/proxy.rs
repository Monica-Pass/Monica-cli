use super::*;
use crate::ai_proxy::{Delivery, ProxyRequest, REQUEST_TIMEOUT};
use mdbx_storage::tiga::TigaService;
use mdbx_storage::tiga_policy::CredentialUseLease;

pub(super) struct ProxyAuthorization {
    grant: Grant,
    pub(super) lease: CredentialUseLease,
}

impl Gateway {
    /// Called once by trusted management after password authentication, never by a request.
    pub(crate) fn authorize_proxy_grants(
        &mut self,
        names: &[String],
        seconds: u32,
        now: i64,
    ) -> Result<()> {
        if !self.proxy_leases.is_empty() || names.len() > 64 {
            return Err(GatewayError::InvalidRequest);
        }
        let config = self.store.load()?;
        let usage = load_call_usage(&self.store)?;
        let conn = self
            .vault
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        let mut leases = BTreeMap::new();
        for name in names {
            let grant = config
                .grants
                .iter()
                .find(|g| &g.name == name)
                .ok_or(GatewayError::NotFound)?;
            if crate::config::grant_state(grant, &usage, now).refresh_required() {
                return Err(GatewayError::ReauthorizationRequired);
            }
            if now < grant.issued_at {
                return Err(GatewayError::Unauthorized);
            }
            if grant.expires_at <= now
                || grant.operations.is_empty()
                || !grant
                    .operations
                    .iter()
                    .all(|op| matches!(op, Operation::ModelList | Operation::ModelInvoke))
                || grant.repositories != ["*".to_owned()].into()
            {
                return Err(GatewayError::PermissionDenied);
            }
            let binding = config
                .connections
                .get(&grant.connection)
                .ok_or(GatewayError::NotFound)?;
            let source = binding
                .api_key
                .as_ref()
                .ok_or(GatewayError::PermissionDenied)?;
            if grant.connection_fingerprint != connection_fingerprint(binding) {
                return Err(GatewayError::Unauthorized);
            }
            let summary = mdbx_storage::repo::ObjectSummaryRepo::get(&conn, &binding.credential_id)
                .map_err(|_| GatewayError::StateUnavailable)?
                .ok_or(GatewayError::ObjectChanged)?;
            if !source.matches(&summary) {
                return Err(GatewayError::ObjectChanged);
            }
            let requested = i64::from(seconds).min(grant.expires_at - now) as u32;
            let lease = TigaService::authorize_credential_use(
                &conn,
                &binding.credential_id,
                &grant.capability_hash,
                requested,
                &crate::api_keys::broker_device(),
                now,
            )
            .map_err(|_| GatewayError::UnlockRequired)?;
            if leases
                .insert(
                    grant.capability_hash.clone(),
                    ProxyAuthorization {
                        grant: grant.clone(),
                        lease,
                    },
                )
                .is_some()
            {
                return Err(GatewayError::InvalidRequest);
            }
        }
        self.proxy_leases = leases;
        Ok(())
    }

    pub(crate) fn proxy_session_info(&self) -> Value {
        json!(
            self.proxy_leases
                .values()
                .map(|a| json!({
                    "grant": a.grant.name, "expires_at_unix": a.lease.expires_at_unix_secs(),
                }))
                .collect::<Vec<_>>()
        )
    }

    fn proxy_lease(&self, grant: &Grant) -> Result<Option<&CredentialUseLease>> {
        let Some(auth) = self.proxy_leases.get(&grant.capability_hash) else {
            return Ok(None);
        };
        if &auth.grant != grant {
            auth.lease.revoke();
            return Err(GatewayError::PermissionDenied);
        }
        Ok(Some(&auth.lease))
    }

    pub(crate) fn proxy_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.proxy_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| GatewayError::BrokerBusy)
    }
    pub(crate) fn proxy_recheck(
        &self,
        capability: &str,
        binding: &Connection,
        operation: Operation,
    ) -> Result<()> {
        if self.stopped.is_cancelled() {
            return Err(GatewayError::UnlockRequired);
        }
        self.recheck_access(capability, binding, operation, "*")?;
        let (grant, _) = self.access(capability)?;
        let lease = self.proxy_lease(&grant)?;
        let source = binding
            .api_key
            .as_ref()
            .ok_or(GatewayError::PermissionDenied)?;
        let conn = self
            .vault
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        let current = mdbx_storage::repo::ObjectSummaryRepo::get(&conn, &binding.credential_id)
            .map_err(|_| GatewayError::StateUnavailable)?;
        if !current.as_ref().is_some_and(|s| source.matches(s)) {
            return Err(GatewayError::ObjectChanged);
        }
        // Re-evaluate policy while streaming without revealing the payload again
        // or extending the engine session's idle/authentication timestamps.
        use mdbx_core::tiga::{
            AuthorizationOutcome, DeviceAssurance, DeviceContext, TigaOperation, TigaScope,
        };
        let decision = if let Some(lease) = lease {
            TigaService::evaluate_credential_use(
                &conn,
                lease,
                &grant.capability_hash,
                &crate::api_keys::broker_device(),
                chrono::Utc::now().timestamp(),
            )
        } else {
            mdbx_storage::tiga::TigaService::evaluate_operation(
                &conn,
                &TigaScope::Entry {
                    entry_id: binding.credential_id.clone(),
                },
                TigaOperation::RevealSecret,
                mdbx_storage::tiga_policy::TigaAuthorizationContext {
                    session: conn.active_session(),
                    device: &DeviceContext {
                        assurance: DeviceAssurance::Standard,
                        ..Default::default()
                    },
                    now_unix_secs: chrono::Utc::now().timestamp(),
                },
            )
        }
        .map_err(|_| GatewayError::UnlockRequired)?;
        if conn.keyring().is_none()
            || !matches!(
                decision.outcome,
                AuthorizationOutcome::Allow | AuthorizationOutcome::AllowWithConstraints
            )
        {
            return Err(GatewayError::UnlockRequired);
        }
        Ok(())
    }

    pub(crate) async fn proxy_request(
        self: Arc<Self>,
        capability: zeroize::Zeroizing<String>,
        call: ProxyRequest,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<axum::response::Response> {
        let operation = call.operation();
        let (grant, binding) = self.access(&capability)?;
        let source = binding
            .api_key
            .as_ref()
            .ok_or(GatewayError::PermissionDenied)?;
        if source.protocol != call.protocol
            || !grant.operations.contains(&operation)
            || !grant.repositories.contains("*")
        {
            return Err(GatewayError::PermissionDenied);
        }
        self.proxy_recheck(&capability, &binding, operation)?;
        {
            let mut state = self.turn(QUEUE_WAIT).await?;
            Self::charge_rate(&mut state, &grant)?;
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        if grant.approval.requires(operation.is_write()) {
            self.await_approval(
                format!("proxy:{request_id}"),
                Request {
                    id: 0,
                    grant: grant.name.clone(),
                    tool: format!("{}_proxy", call.protocol.name()),
                    repository: "*".to_owned(),
                    is_write: operation.is_write(),
                    preview: format!("{} /v1/{}", call.method, call.path),
                    asked_at: Instant::now(),
                },
                Instant::now() + crate::approval::WAIT,
            )
            .await?;
        }
        let mut state = self.turn(QUEUE_WAIT).await?;
        self.proxy_recheck(&capability, &binding, operation)?;
        let credential = self.vault.bound_api_key_with_lease(
            &binding,
            chrono::Utc::now().timestamp(),
            self.proxy_lease(&grant)?
                .map(|lease| (lease, grant.capability_hash.as_str())),
        )?;
        let local_key = zeroize::Zeroizing::new(format!("monica-{}", capability.as_str()));
        if let Some(body) = &call.parsed {
            upstream::reject_secret_value(body, &credential.token)?;
            upstream::reject_secret_value(body, &capability)?;
        }
        for (_, value) in &call.headers {
            let value = json!(value.to_str().map_err(|_| GatewayError::InvalidRequest)?);
            upstream::reject_secret_value(&value, &credential.token)?;
            upstream::reject_secret_value(&value, &capability)?;
        }
        self.charge_calls(&mut state, &grant)?;
        self.audit(&AuditEvent {
            timestamp: chrono::Utc::now().timestamp(),
            grant: Some(grant.name.clone()),
            operation: Some(operation),
            repository: Some("*".to_owned()),
            request_id: Some(request_id),
            stage: "proxy_authorized",
            error: None,
        })?;
        drop(state);
        let mut base =
            url::Url::parse(&binding.api_base).map_err(|_| GatewayError::InvalidConfig)?;
        if base.path() == "/" {
            base.set_path("/v1/");
        }
        let mut url = base
            .join(call.path)
            .map_err(|_| GatewayError::InvalidConfig)?;
        if url.origin() != base.origin() || !url.path().starts_with(base.path()) {
            return Err(GatewayError::PermissionDenied);
        }
        url.set_query(call.query);
        let mut headers = upstream::authentication_headers(&binding, &credential)?;
        headers.extend(call.headers);
        headers.insert(
            "content-type",
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "accept-encoding",
            reqwest::header::HeaderValue::from_static("identity"),
        );
        if source.protocol == crate::api_keys::ApiProtocol::Anthropic
            && !headers.contains_key("anthropic-version")
        {
            headers.insert(
                "anthropic-version",
                reqwest::header::HeaderValue::from_static("2023-06-01"),
            );
        }
        let mut request = self
            .http
            .request(call.method, url)
            .headers(headers)
            .timeout(REQUEST_TIMEOUT);
        // Send the exact JSON value that validation and secret checks inspected.
        // Forwarding the original bytes would reintroduce shadowed duplicate keys.
        if let Some(body) = &call.parsed {
            request = request.json(body);
        }
        self.proxy_recheck(&capability, &binding, operation)?;
        // Model POST requests do not supply Monica's MCP write UUID. Each HTTP
        // request is one attempt; never retry or switch upstream after dispatch.
        let response = request.send();
        tokio::pin!(response);
        let response = loop {
            self.proxy_recheck(&capability, &binding, operation)?;
            tokio::select! {
                result = &mut response => break result.map_err(|_| if operation.is_write() { GatewayError::WriteOutcomeUnknown } else { GatewayError::UpstreamUnavailable })?,
                _ = self.stopped.cancelled() => return Err(GatewayError::UnlockRequired),
                _ = tokio::time::sleep(Duration::from_millis(200)) => {},
            }
        };
        let secrets = zeroize::Zeroizing::new(vec![
            credential.token.to_string(),
            capability.to_string(),
            local_key.to_string(),
        ]);
        crate::ai_proxy::response(
            response,
            Delivery {
                gateway: self,
                capability,
                binding,
                operation,
                secrets,
                _permit: permit,
            },
        )
        .await
    }
}
