use crate::account::AccountService;
use crate::cli::SecretString;
use crate::error::{LpcError, Result};
use crate::model::AccountRecord;
use crate::scope::ScopeBatch;
use crate::scope_policy::MAX_SINGLE_AUTH_SCOPES;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

const AUTH_POLL_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthFlowStart {
    pub flow_id: Uuid,
    pub verification_url: String,
    pub expires_in: u64,
    pub qr_code_png: Option<Vec<u8>>,
    pub batch: ScopeBatch,
    pub remaining_scope_count: usize,
    pub expected_user_open_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProgress {
    pub complete: bool,
    pub account: Option<AccountRecord>,
    pub next: Option<AuthFlowStart>,
    pub effective_scopes: BTreeSet<String>,
    pub missing_scopes: BTreeSet<String>,
}

#[derive(Debug, Clone)]
enum FlowKind {
    NewAccount { app_ref: Uuid },
    Reauthorize { account_id: Uuid },
}

struct PendingFlow {
    kind: FlowKind,
    canonical_dir: PathBuf,
    batch_dir: PathBuf,
    target: BTreeSet<String>,
    effective: BTreeSet<String>,
    protected_effective: BTreeSet<String>,
    expected_open_id: Option<String>,
    verification_url: String,
    expires_in: u64,
    device_code: SecretString,
    created_at: DateTime<Utc>,
}

impl PendingFlow {
    fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        let expires_in = i64::try_from(self.expires_in).unwrap_or(i64::MAX);
        now >= self.created_at + ChronoDuration::seconds(expires_in)
    }
}

/// In-memory, single-process OAuth coordinator. Device codes are intentionally
/// absent from serialized state and UI DTOs.
pub struct AuthCoordinator {
    service: AccountService,
    flows: HashMap<Uuid, PendingFlow>,
}

impl AuthCoordinator {
    pub fn new(service: AccountService) -> Result<Self> {
        Ok(Self {
            service,
            flows: HashMap::new(),
        })
    }

    pub fn begin_new_account(&mut self, app_ref: Uuid) -> Result<AuthFlowStart> {
        let app = self.service.refresh_app_boundary(app_ref)?;
        let effective = BTreeSet::new();
        self.begin(
            FlowKind::NewAccount { app_ref },
            app_ref,
            app.available_scopes,
            app.policy_scopes,
            effective,
            None,
        )
    }

    pub fn begin_reauthorization(&mut self, account_id: Uuid) -> Result<AuthFlowStart> {
        let (account, app_before_refresh) = self.service.account_and_app(account_id)?;
        let app = self.service.refresh_app_boundary(app_before_refresh.id)?;
        let status = self.service.cli().status(&account.config_dir, false)?.value;
        let effective = status.identities.user.scope;
        self.begin(
            FlowKind::Reauthorize { account_id },
            app.id,
            app.available_scopes,
            app.policy_scopes,
            effective,
            Some(account.user_open_id),
        )
    }

    fn begin(
        &mut self,
        kind: FlowKind,
        app_ref: Uuid,
        boundary: BTreeSet<String>,
        target: BTreeSet<String>,
        effective: BTreeSet<String>,
        expected_open_id: Option<String>,
    ) -> Result<AuthFlowStart> {
        if target.is_empty() {
            return Err(LpcError::ScopeOutOfBoundary(Vec::new()));
        }
        let initial_batch = authorization_batch(
            &boundary,
            &target,
            &effective,
            matches!(kind, FlowKind::Reauthorize { .. }),
        )?;
        let id = Uuid::new_v4();
        let canonical_dir = self
            .service
            .store()
            .paths()
            .staging_dir()
            .join(format!("auth-{id}-canonical"));
        let batch_dir = self
            .service
            .store()
            .paths()
            .staging_dir()
            .join(format!("auth-{id}-batch-0"));
        self.service
            .prepare_account_config(app_ref, &canonical_dir)?;
        self.service.prepare_account_config(app_ref, &batch_dir)?;
        let device = self
            .service
            .cli()
            .begin_login(&batch_dir, &initial_batch.scopes)?
            .value;
        let created_at = Utc::now();
        let qr_code_png = self
            .service
            .cli()
            .render_qrcode_png(&batch_dir, &device.verification_url)
            .ok();
        let protected_effective = effective.intersection(&target).cloned().collect();
        let start = AuthFlowStart {
            flow_id: id,
            verification_url: device.verification_url.clone(),
            expires_in: device.expires_in,
            qr_code_png,
            batch: initial_batch,
            remaining_scope_count: target.difference(&effective).count(),
            expected_user_open_id: expected_open_id.clone(),
        };
        self.flows.insert(
            id,
            PendingFlow {
                kind,
                canonical_dir,
                batch_dir,
                target,
                effective,
                protected_effective,
                expected_open_id,
                verification_url: device.verification_url,
                expires_in: device.expires_in,
                device_code: SecretString::new(device.device_code),
                created_at,
            },
        );
        Ok(start)
    }

    pub fn complete_current_batch(&mut self, flow_id: Uuid) -> Result<AuthProgress> {
        let flow = self
            .flows
            .get(&flow_id)
            .ok_or_else(|| LpcError::AuthFlowNotFound(flow_id.to_string()))?;
        if flow.is_expired_at(Utc::now()) {
            let flow = self.flows.remove(&flow_id).expect("flow checked above");
            self.cleanup_flow(&flow);
            return Err(LpcError::AuthFlowExpired);
        }
        match self.service.cli().complete_login_with_timeout(
            &flow.batch_dir,
            &flow.device_code,
            AUTH_POLL_TIMEOUT,
        ) {
            Ok(_) => {
                let mut flow = self.flows.remove(&flow_id).expect("flow checked above");
                let result = self.complete_inner(&mut flow);
                if result.is_err() {
                    self.cleanup_flow(&flow);
                }
                result
            }
            Err(LpcError::CliTimeout(_)) => Ok(pending_progress(flow)),
            Err(error) => {
                let flow = self.flows.remove(&flow_id).expect("flow checked above");
                self.cleanup_flow(&flow);
                Err(error)
            }
        }
    }

    fn complete_inner(&mut self, flow: &mut PendingFlow) -> Result<AuthProgress> {
        let whoami = self.service.cli().whoami(&flow.batch_dir)?.value;
        let delegated = whoami.on_behalf_of.as_ref().ok_or_else(|| {
            LpcError::InvalidCliOutput("login completed without onBehalfOf".into())
        })?;
        if delegated.open_id.is_empty() {
            return Err(LpcError::InvalidCliOutput(
                "login returned empty openId".into(),
            ));
        }
        if let Some(expected) = &flow.expected_open_id {
            if expected != &delegated.open_id {
                return Err(LpcError::AuthIdentityMismatch {
                    expected: expected.clone(),
                    actual: delegated.open_id.clone(),
                });
            }
        } else {
            flow.expected_open_id = Some(delegated.open_id.clone());
        }

        let status = self.service.cli().status(&flow.batch_dir, true)?.value;
        if !status.identities.user.available {
            return Err(LpcError::CliFailed {
                code: 1,
                message: status.identities.user.message,
            });
        }
        let after = status.identities.user.scope;
        let regression: Vec<String> = flow
            .protected_effective
            .difference(&after)
            .cloned()
            .collect();
        if !regression.is_empty() {
            return Err(LpcError::ScopeRegression(regression));
        }
        let gained: BTreeSet<String> = after.difference(&flow.effective).cloned().collect();
        let missing: BTreeSet<String> = flow.target.difference(&after).cloned().collect();
        if gained.is_empty() && !missing.is_empty() {
            return Err(LpcError::ScopeNoProgress);
        }

        if !missing.is_empty() {
            return Err(LpcError::ScopeIncomplete(missing.into_iter().collect()));
        }

        fs::create_dir_all(&flow.canonical_dir)?;
        let config = fs::read(flow.batch_dir.join("config.json"))?;
        crate::atomic::write_bytes_atomic(&flow.canonical_dir.join("config.json"), &config)?;
        let _ = fs::remove_dir_all(&flow.batch_dir);

        let account = match &flow.kind {
            FlowKind::NewAccount { app_ref } => self
                .service
                .register_new_account_from_config(*app_ref, &flow.canonical_dir)?,
            FlowKind::Reauthorize { account_id } => {
                self.service.commit_reauthorization_from_config(
                    *account_id,
                    &flow.canonical_dir,
                    flow.expected_open_id.as_deref().unwrap_or_default(),
                )?
            }
        };
        self.cleanup_flow(flow);
        Ok(AuthProgress {
            complete: true,
            account: Some(account),
            next: None,
            effective_scopes: after,
            missing_scopes: BTreeSet::new(),
        })
    }

    pub fn active_flow_count(&self) -> usize {
        self.flows.len()
    }

    pub fn render_qr(&self, flow_id: Uuid) -> Result<Vec<u8>> {
        let flow = self
            .flows
            .get(&flow_id)
            .ok_or_else(|| LpcError::AuthFlowNotFound(flow_id.to_string()))?;
        self.service
            .cli()
            .render_qrcode_png(&flow.batch_dir, &flow.verification_url)
    }

    pub fn cancel(&mut self, flow_id: Uuid) -> Result<()> {
        let flow = self
            .flows
            .remove(&flow_id)
            .ok_or_else(|| LpcError::AuthFlowNotFound(flow_id.to_string()))?;
        self.cleanup_flow(&flow);
        Ok(())
    }

    pub fn purge_expired(&mut self) {
        let now = Utc::now();
        let expired: Vec<Uuid> = self
            .flows
            .iter()
            .filter(|(_, flow)| flow.is_expired_at(now))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(flow) = self.flows.remove(&id) {
                self.cleanup_flow(&flow);
            }
        }
    }

    fn cleanup_flow(&self, flow: &PendingFlow) {
        let _ = fs::remove_dir_all(&flow.batch_dir);
        let _ = fs::remove_dir_all(&flow.canonical_dir);
    }
}

impl Drop for AuthCoordinator {
    fn drop(&mut self) {
        let flows = std::mem::take(&mut self.flows);
        for flow in flows.into_values() {
            self.cleanup_flow(&flow);
        }
    }
}

fn authorization_batch(
    boundary: &BTreeSet<String>,
    target: &BTreeSet<String>,
    effective: &BTreeSet<String>,
    force_reauthorization: bool,
) -> Result<ScopeBatch> {
    let outside: Vec<String> = target.difference(boundary).cloned().collect();
    if !outside.is_empty() {
        return Err(LpcError::ScopeOutOfBoundary(outside));
    }
    let requested: BTreeSet<String> = if force_reauthorization {
        target.clone()
    } else {
        target.difference(effective).cloned().collect()
    };
    if requested.is_empty() {
        return Err(LpcError::ScopeNoProgress);
    }
    if requested.len() > MAX_SINGLE_AUTH_SCOPES {
        return Err(LpcError::ScopeLimitExceeded {
            requested: requested.len(),
            limit: MAX_SINGLE_AUTH_SCOPES,
        });
    }
    Ok(ScopeBatch::from_scopes(0, requested))
}

fn pending_progress(flow: &PendingFlow) -> AuthProgress {
    AuthProgress {
        complete: false,
        account: None,
        next: None,
        effective_scopes: flow.effective.clone(),
        missing_scopes: flow.target.difference(&flow.effective).cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OfficialCli;
    use crate::paths::AppPaths;
    use crate::store::StateStore;
    use chrono::Duration as ChronoDuration;

    fn test_coordinator(temp: &tempfile::TempDir) -> AuthCoordinator {
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let service = AccountService::new(
            store,
            OfficialCli::new(temp.path().join("missing-lark-cli.exe")),
        );
        AuthCoordinator::new(service).unwrap()
    }

    fn numbered_scopes(count: usize) -> BTreeSet<String> {
        (0..count)
            .map(|index| format!("scope:{index:03}"))
            .collect()
    }

    #[test]
    fn single_authorization_batch_contains_all_250_missing_scopes() {
        let boundary = numbered_scopes(250);
        let batch = authorization_batch(&boundary, &boundary, &BTreeSet::new(), false).unwrap();

        assert_eq!(batch.index, 0);
        assert_eq!(batch.scopes.len(), 250);
        assert_eq!(batch.scopes, boundary);
    }

    #[test]
    fn single_authorization_rejects_251_scopes_before_device_flow() {
        let boundary = numbered_scopes(251);
        let error = authorization_batch(&boundary, &boundary, &BTreeSet::new(), false).unwrap_err();

        assert_eq!(error.stable_code(), "LPC_SCOPE_LIMIT_EXCEEDED");
        assert!(matches!(
            error,
            LpcError::ScopeLimitExceeded {
                requested: 251,
                limit: 250
            }
        ));
    }

    #[test]
    fn reauthorization_requests_policy_scopes_even_when_already_effective() {
        let target = numbered_scopes(3);

        let batch = authorization_batch(&target, &target, &target, true).unwrap();

        assert_eq!(batch.scopes, target);
    }

    #[test]
    fn new_account_still_rejects_when_no_scope_is_missing() {
        let target = numbered_scopes(3);

        let error = authorization_batch(&target, &target, &target, false).unwrap_err();

        assert!(matches!(error, LpcError::ScopeNoProgress));
    }

    #[test]
    fn pending_poll_never_creates_another_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let flow = pending_flow(temp.path(), Uuid::new_v4(), Utc::now(), 600);
        let progress = pending_progress(&flow);

        assert!(!progress.complete);
        assert!(progress.next.is_none());
        assert!(progress.account.is_none());
    }

    fn pending_flow(
        root: &std::path::Path,
        id: Uuid,
        created_at: DateTime<Utc>,
        expires_in: u64,
    ) -> PendingFlow {
        let canonical_dir = root.join(format!("auth-{id}-canonical"));
        let batch_dir = root.join(format!("auth-{id}-batch-0"));
        fs::create_dir_all(&canonical_dir).unwrap();
        fs::create_dir_all(&batch_dir).unwrap();
        PendingFlow {
            kind: FlowKind::NewAccount {
                app_ref: Uuid::nil(),
            },
            canonical_dir,
            batch_dir,
            target: BTreeSet::new(),
            effective: BTreeSet::new(),
            protected_effective: BTreeSet::new(),
            expected_open_id: None,
            verification_url: "https://example.com/device".into(),
            expires_in,
            device_code: SecretString::new("fixture-device-secret"),
            created_at,
        }
    }

    #[test]
    fn auth_flow_start_serialization_keeps_safe_fields_only() {
        let start = AuthFlowStart {
            flow_id: Uuid::nil(),
            verification_url: "https://example.com/device".into(),
            expires_in: 600,
            batch: ScopeBatch::from_scopes(0, ["docs:read".to_owned()].into_iter().collect()),
            remaining_scope_count: 1,
            expected_user_open_id: None,
            qr_code_png: Some(b"\x89PNG\r\n\x1a\nfixture".to_vec()),
        };

        let value = serde_json::to_value(&start).unwrap();
        assert_eq!(value["flowId"], Uuid::nil().to_string());
        assert_eq!(value["expiresIn"], 600);
        assert!(value["qrCodePng"].is_array());
        let serialized = serde_json::to_string(&start).unwrap().to_ascii_lowercase();
        assert!(!serialized.contains("devicecode"));
        assert!(!serialized.contains("fixture-device-secret"));
    }

    #[test]
    fn expired_flow_is_rejected_before_invoking_cli() {
        let temp = tempfile::tempdir().unwrap();
        let mut coordinator = test_coordinator(&temp);
        let flow_id = Uuid::new_v4();
        let flow = pending_flow(
            &coordinator.service.store().paths().staging_dir(),
            flow_id,
            Utc::now() - ChronoDuration::seconds(2),
            1,
        );
        let batch_dir = flow.batch_dir.clone();
        let canonical_dir = flow.canonical_dir.clone();
        coordinator.flows.insert(flow_id, flow);

        let error = coordinator.complete_current_batch(flow_id).unwrap_err();

        assert_eq!(error.stable_code(), "LPC_AUTH_FLOW_EXPIRED");
        assert_eq!(coordinator.active_flow_count(), 0);
        assert!(!batch_dir.exists());
        assert!(!canonical_dir.exists());
    }

    #[test]
    fn purge_expired_uses_each_flows_own_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let mut coordinator = test_coordinator(&temp);
        let staging = coordinator.service.store().paths().staging_dir();
        let expired_id = Uuid::new_v4();
        let active_id = Uuid::new_v4();
        let expired = pending_flow(
            &staging,
            expired_id,
            Utc::now() - ChronoDuration::seconds(30),
            5,
        );
        let expired_batch_dir = expired.batch_dir.clone();
        let active = pending_flow(
            &staging,
            active_id,
            Utc::now() - ChronoDuration::seconds(30),
            300,
        );
        let active_batch_dir = active.batch_dir.clone();
        coordinator.flows.insert(expired_id, expired);
        coordinator.flows.insert(active_id, active);

        coordinator.purge_expired();

        assert_eq!(coordinator.active_flow_count(), 1);
        assert!(!expired_batch_dir.exists());
        assert!(active_batch_dir.exists());
    }

    #[test]
    fn dropping_coordinator_removes_pending_staging_directories() {
        let temp = tempfile::tempdir().unwrap();
        let mut coordinator = test_coordinator(&temp);
        let flow_id = Uuid::new_v4();
        let flow = pending_flow(
            &coordinator.service.store().paths().staging_dir(),
            flow_id,
            Utc::now(),
            600,
        );
        let batch_dir = flow.batch_dir.clone();
        let canonical_dir = flow.canonical_dir.clone();
        coordinator.flows.insert(flow_id, flow);

        drop(coordinator);

        assert!(!batch_dir.exists());
        assert!(!canonical_dir.exists());
    }
}
