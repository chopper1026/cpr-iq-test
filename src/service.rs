//! Management handlers and the single maintenance worker share the same persisted queue.
use crate::{
    domain::{
        self, AccountMeta, Decision, Outcome, ProbeDiagnostic, Reason, Round, RunDiagnostic,
        Settings, State, Target, Task, Verdict,
    },
    fingerprint::Fingerprint,
    host::{AccountChoice, DiscoveredModel, Host, safe_reason},
};
use gateway_plugin_sdk::{
    PluginFault,
    call::management::{
        ManagementPage, ManagementRegistration, ManagementRequest, ManagementResource,
        ManagementResponse, ManagementRoute,
    },
    client::{Empty, TypedCall, TypedReply},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, RwLock},
    time::Instant,
};

const JSON: &str = "application/json";
const MAX_REQUEST: usize = 32 * 1024;
const MODEL_CACHE_TTL_MS: u64 = 10 * 60 * 1000;
const MAX_RUNTIME_LOGS: usize = 300;

type Reply = TypedReply<ManagementResponse>;

pub fn registration() -> ManagementRegistration {
    let route = |method: &str, path: &str| ManagementRoute {
        method: method.into(),
        path: path.into(),
        request_content_types: if method == "GET" {
            vec![]
        } else {
            vec![JSON.into()]
        },
        response_content_types: vec![JSON.into()],
    };
    ManagementRegistration {
        routes: vec![
            route("GET", "catalog"),
            route("GET", "runs"),
            route("POST", "runs"),
            route("POST", "runs/cancel"),
            route("GET", "logs"),
            route("GET", "schedule"),
            route("PUT", "schedule"),
        ],
        resources: ["web/index.html", "web/style.css", "web/app.js"]
            .into_iter()
            .map(|path| ManagementResource {
                path: path.into(),
                public: false,
            })
            .collect(),
        pages: vec![ManagementPage {
            id: "fingerprint".into(),
            title: "账号指纹巡检".into(),
            description: Some("对指定 Codex 账号执行手动与定时模型指纹检测".into()),
            entry: "web/index.html".into(),
            icon: None,
        }],
        callbacks: vec![],
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetInput {
    client_key_id: String,
    model: String,
    effort: String,
    account_ids: Vec<String>,
}

impl TargetInput {
    fn into_target(self) -> Result<Target, Reason> {
        let mut target = Target::new(
            self.client_key_id,
            self.model,
            self.effort,
            self.account_ids,
        );
        target.normalize()?;
        Ok(target)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Submit {
    run_id: String,
    target: TargetInput,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelRequest {
    #[serde(default)]
    run_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsInput {
    enabled: bool,
    hours: u16,
    target: Option<TargetInput>,
}

impl SettingsInput {
    fn into_settings(self) -> Result<Settings, Reason> {
        let target = self.target.map(TargetInput::into_target).transpose()?;
        let mut settings = Settings {
            enabled: self.enabled,
            hours: self.hours,
            target,
        };
        settings.normalize()?;
        Ok(settings)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditSchedule {
    settings: SettingsInput,
    previous: SettingsInput,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelView {
    id: String,
    display_name: String,
    efforts: Vec<String>,
    default_effort: String,
    in_reference_bank: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reference_label: Option<String>,
}

#[derive(Clone)]
struct CachedModels {
    fetched_at: u64,
    key_scope: Option<String>,
    models: Vec<DiscoveredModel>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntry {
    seq: u64,
    at: u64,
    level: &'static str,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<Value>,
}

#[derive(Default)]
struct LogRing {
    next_seq: u64,
    entries: VecDeque<LogEntry>,
}

impl LogRing {
    fn push(
        &mut self,
        level: &'static str,
        category: &'static str,
        run_id: Option<String>,
        account_id: Option<String>,
        message: impl Into<String>,
        detail: Option<Value>,
    ) {
        self.next_seq = self.next_seq.saturating_add(1);
        if self.entries.len() >= MAX_RUNTIME_LOGS {
            self.entries.pop_front();
        }
        self.entries.push_back(LogEntry {
            seq: self.next_seq,
            at: now(),
            level,
            category,
            run_id,
            account_id,
            message: message.into(),
            detail,
        });
    }

    fn snapshot(&self, run_id: Option<&str>) -> Vec<LogEntry> {
        self.entries
            .iter()
            .filter(|entry| run_id.is_none_or(|target| entry.run_id.as_deref() == Some(target)))
            .cloned()
            .collect()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunRecord {
    kind: &'static str,
    run_id: String,
    account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_name: Option<String>,
    model: String,
    effort: String,
    scheduled: bool,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin_version: Option<String>,
    first_round: Option<Round>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<RunDiagnostic>,
}

#[derive(Debug)]
struct HttpError(u16, Reason);

impl From<Reason> for HttpError {
    fn from(reason: Reason) -> Self {
        Self(
            match reason {
                Reason::InvalidInput => 400,
                Reason::AccessDenied => 403,
                Reason::Conflict | Reason::QueueFull => 409,
                Reason::RateLimited => 429,
                Reason::UnsupportedProfile | Reason::AccountUnavailable => 422,
                Reason::Timeout => 504,
                _ => 503,
            },
            reason,
        )
    }
}

pub struct Service {
    version: String,
    bank: Option<Fingerprint>,
    maintenance: Mutex<()>,
    model_cache: RwLock<Option<CachedModels>>,
    logs: RwLock<LogRing>,
}

impl Service {
    pub fn new(version: String, bank: Option<Fingerprint>) -> Self {
        let mut ring = LogRing::default();
        ring.push(
            "info",
            "system",
            None,
            None,
            "Plugin service initialized",
            Some(json!({
                "pluginVersion": version,
                "detectorReady": bank.is_some(),
                "detectorRevision": bank.as_ref().map(|b| &b.revision),
            })),
        );
        Self {
            version,
            bank,
            maintenance: Mutex::new(()),
            model_cache: RwLock::new(None),
            logs: RwLock::new(ring),
        }
    }

    async fn log(
        &self,
        level: &'static str,
        category: &'static str,
        run_id: Option<&str>,
        account_id: Option<&str>,
        message: impl Into<String>,
        detail: Option<Value>,
    ) {
        self.logs.write().await.push(
            level,
            category,
            run_id.map(str::to_owned),
            account_id.map(str::to_owned),
            message,
            detail,
        );
    }

    pub async fn management(
        &self,
        call: TypedCall<ManagementRequest>,
    ) -> Result<Reply, PluginFault> {
        let result = self
            .route(&Host(&call.host), &call.request, &call.payload)
            .await;
        let (status, data) = match result {
            Ok(response) => response,
            Err(HttpError(status, code)) => (status, json!({"error": code})),
        };
        Ok(TypedReply::new(ManagementResponse {
            status,
            content_type: JSON.into(),
            headers: vec![],
        })
        .with_payload(data.to_string().into_bytes()))
    }

    async fn resolve_models(
        &self,
        host: &Host<'_>,
        accounts: &[AccountChoice],
        keys: &[crate::host::Choice],
        preferred_key: Option<&str>,
        force_refresh: bool,
    ) -> (Vec<ModelView>, Option<u64>, Option<String>) {
        let key_scope = preferred_key
            .filter(|id| keys.iter().any(|k| k.id == *id))
            .or_else(|| keys.first().map(|k| k.id.as_str()))
            .map(str::to_owned);
        let at = now();
        if !force_refresh {
            let cached = self.model_cache.read().await;
            if let Some(entry) = cached.as_ref()
                && at.saturating_sub(entry.fetched_at) < MODEL_CACHE_TTL_MS
                && entry.key_scope == key_scope
                && (!entry.models.is_empty() || accounts.is_empty())
            {
                return (
                    self.annotate_models(&entry.models),
                    Some(entry.fetched_at),
                    entry.error.clone(),
                );
            }
        }
        let (models, error) = host
            .discover_models(accounts, keys, key_scope.as_deref())
            .await;
        let fetched_at = now();
        let views = self.annotate_models(&models);
        self.log(
            if error.is_some() { "warn" } else { "info" },
            "catalog",
            None,
            None,
            "Model catalog discovered",
            Some(json!({
                "accountCount": accounts.len(),
                "keyCount": keys.len(),
                "modelCount": models.len(),
                "forceRefresh": force_refresh,
                "error": error,
            })),
        )
        .await;
        *self.model_cache.write().await = Some(CachedModels {
            fetched_at,
            key_scope,
            models,
            error: error.clone(),
        });
        (views, Some(fetched_at), error)
    }

    fn annotate_models(&self, models: &[DiscoveredModel]) -> Vec<ModelView> {
        models
            .iter()
            .map(|m| {
                let reference = self.bank.as_ref().and_then(|b| b.expected_label(&m.id));
                ModelView {
                    id: m.id.clone(),
                    display_name: m.display_name.clone(),
                    efforts: m.efforts.clone(),
                    default_effort: m.default_effort.clone(),
                    in_reference_bank: reference.is_some(),
                    reference_label: reference.map(|(_, display)| display.to_owned()),
                }
            })
            .collect()
    }

    async fn route(
        &self,
        host: &Host<'_>,
        request: &ManagementRequest,
        payload: &[u8],
    ) -> Result<(u16, Value), HttpError> {
        if payload.len() > MAX_REQUEST || request.query.len() > 2048 {
            return Err(Reason::InvalidInput.into());
        }
        let query = parse_query(&request.query);
        let path = request.path.as_str();
        match (request.method.as_str(), path) {
            ("GET", "catalog") => {
                let force_refresh = query
                    .get("refresh")
                    .is_some_and(|v| matches!(v.as_str(), "1" | "true"));
                let preferred_key = query
                    .get("clientKeyId")
                    .map(String::as_str)
                    .filter(|s| !s.is_empty());
                let (accounts, keys) = host.catalog().await?;
                let (models, model_catalog_at, model_catalog_error) = self
                    .resolve_models(host, &accounts, &keys, preferred_key, force_refresh)
                    .await;
                Ok((
                    200,
                    json!({
                        "accounts": accounts,
                        "keys": keys,
                        "models": models,
                        "modelCatalogAt": model_catalog_at,
                        "modelCatalogError": model_catalog_error,
                        "detectorReady": self.bank.is_some(),
                        "detectorRevision": self.bank.as_ref().map(|b| &b.revision),
                        "pluginVersion": self.version,
                        "maxBatch": domain::MAX_BATCH,
                    }),
                ))
            }
            ("GET", "runs") => {
                let (mut state, _) = host.load().await.map_err(|e| safe_reason(&e))?;
                state.prune(now());
                let params = RunQuery::from_map(&query)?;
                let needs_labels = state.queue.iter().any(|q| q.account_email.is_none())
                    || state.history.iter().any(|h| h.account_email.is_none());
                let live_labels = if needs_labels {
                    account_labels(host).await
                } else {
                    BTreeMap::new()
                };
                Ok((200, build_runs_response(&state, &params, &live_labels)))
            }
            ("GET", "logs") => {
                let (mut state, _) = host.load().await.map_err(|e| safe_reason(&e))?;
                state.prune(now());
                let run_id = query
                    .get("runId")
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty());
                if let Some(id) = &run_id
                    && !domain::valid_id(id, 64)
                {
                    return Err(Reason::InvalidInput.into());
                }
                let scope_all = query.get("scope").is_some_and(|v| v == "all");
                let mut params = if scope_all {
                    RunQuery::unfiltered()
                } else {
                    RunQuery::from_map(&query)?
                };
                params.run_id = run_id.clone();
                let accounts = host.accounts().await.unwrap_or_default();
                let live_labels: BTreeMap<String, AccountMeta> = accounts
                    .iter()
                    .map(|account| (account.id.clone(), account.meta()))
                    .collect();
                let cached_models = self.model_cache.read().await.clone();
                let runtime_logs = self.logs.read().await.snapshot(run_id.as_deref());
                Ok((
                    200,
                    self.build_logs_response(
                        &state,
                        &params,
                        &live_labels,
                        &accounts,
                        cached_models.as_ref(),
                        runtime_logs,
                    ),
                ))
            }
            ("GET", "schedule") => {
                if !request.query.is_empty() {
                    return Err(Reason::InvalidInput.into());
                }
                let (state, _) = host.load().await.map_err(|e| safe_reason(&e))?;
                Ok((200, schedule_value(&state)))
            }
            ("POST", "runs") => {
                if !request.query.is_empty() {
                    return Err(Reason::InvalidInput.into());
                }
                if self.bank.is_none() {
                    return Err(Reason::DetectorUnavailable.into());
                }
                let submit: Submit = parse(payload)?;
                let target = submit.target.into_target()?;
                host.access(&target.client_key_id, &target.account_ids, &target.model)
                    .await?;
                let labels = account_labels(host).await;
                let expected_in_bank = self
                    .bank
                    .as_ref()
                    .and_then(|b| b.expected_label(&target.model))
                    .is_some();
                let count = target.account_ids.len();
                let at = now();
                let result = host
                    .change(|state, _| {
                        state.prune(at);
                        state.enqueue_manual(submit.run_id.clone(), target.clone(), at, &labels)
                    })
                    .await
                    .map_err(|e| safe_reason(&e))?;
                result?;
                self.log(
                    "info",
                    "queue",
                    Some(&submit.run_id),
                    None,
                    "Manual detection batch enqueued",
                    Some(json!({
                        "model": target.model,
                        "effort": target.effort,
                        "accountCount": count,
                        "inReferenceBank": expected_in_bank,
                    })),
                )
                .await;
                let base = count * domain::PROBES_PER_ROUND as usize;
                Ok((
                    202,
                    json!({
                        "runId": submit.run_id,
                        "queuedCount": count,
                        "probesMin": base,
                        "probesMax": if expected_in_bank { 2 * base } else { base },
                    }),
                ))
            }
            ("POST", "runs/cancel") => {
                if !request.query.is_empty() {
                    return Err(Reason::InvalidInput.into());
                }
                let cancel: CancelRequest = if payload.is_empty() {
                    CancelRequest { run_id: None }
                } else {
                    parse(payload)?
                };
                if let Some(id) = &cancel.run_id
                    && !domain::valid_id(id, 64)
                {
                    return Err(Reason::InvalidInput.into());
                }
                let cancelled = host
                    .change(|state, _| state.cancel_pending(cancel.run_id.as_deref()))
                    .await
                    .map_err(|e| safe_reason(&e))?;
                Ok((200, json!({"cancelled": cancelled})))
            }
            ("PUT", "schedule") => {
                if !request.query.is_empty() {
                    return Err(Reason::InvalidInput.into());
                }
                let edit: EditSchedule = parse(payload)?;
                let settings = edit.settings.into_settings()?;
                let previous = edit.previous.into_settings()?;
                if settings.enabled {
                    if self.bank.is_none() {
                        return Err(Reason::DetectorUnavailable.into());
                    }
                    let target = settings.target.as_ref().ok_or(Reason::InvalidInput)?;
                    host.access(&target.client_key_id, &target.account_ids, &target.model)
                        .await?;
                }
                let at = now();
                let value = host
                    .change(|state, _| {
                        if state.settings != previous && state.settings != settings {
                            return Err(Reason::Conflict);
                        }
                        state.prune(at);
                        state.schedule(settings.clone(), at, &self.version);
                        Ok(schedule_value(state))
                    })
                    .await
                    .map_err(|e| safe_reason(&e))??;
                Ok((200, value))
            }
            _ => Err(HttpError(404, Reason::InvalidInput)),
        }
    }

    pub async fn reconcile(
        &self,
        call: TypedCall<Empty>,
    ) -> Result<TypedReply<Empty>, PluginFault> {
        let Ok(_guard) = self.maintenance.try_lock() else {
            return Ok(TypedReply::new(Empty {}));
        };
        let host = Host(&call.host);
        let at = now();
        let (preview, _) = host.load().await?;
        let labels = if preview.is_schedule_due(at) {
            account_labels(&host).await
        } else {
            BTreeMap::new()
        };
        let task = host
            .change(|state, _| {
                state.prune(at);
                // An earlier committed claim still present after its maintenance scope ended is never replayed.
                state.recover(at, &self.version);
                state.schedule_due(at, &self.version, &labels);
                state.claim()
            })
            .await?;
        if let Some(task) = task {
            let (result, refreshed_meta, diagnostics) = tokio::select! {
                biased;
                () = call.cancellation.cancelled() => (
                    Decision::Finish(failure(Reason::Cancelled)),
                    None,
                    Some(RunDiagnostic {
                        round: if task.first_round.is_some() { 2 } else { 1 },
                        stage: "cancelled".into(),
                        error_detail: Some("maintenance call cancelled by host".into()),
                        ..RunDiagnostic::default()
                    }),
                ),
                output = self.run_round(&host, &task) => output,
            };
            // CAS may retry this *settlement*, but never rerun a model request.
            let at = now();
            host.change(|state, _| {
                state.complete(
                    &task,
                    result.clone(),
                    at,
                    &self.version,
                    refreshed_meta.clone(),
                    diagnostics.clone(),
                );
            })
            .await?;
        }
        Ok(TypedReply::new(Empty {}))
    }

    async fn run_round(
        &self,
        host: &Host<'_>,
        task: &Task,
    ) -> (Decision, Option<AccountMeta>, Option<RunDiagnostic>) {
        let round_num = if task.first_round.is_some() { 2 } else { 1 };
        let started = Instant::now();
        let mut diag = RunDiagnostic {
            round: round_num,
            stage: "starting".into(),
            ..RunDiagnostic::default()
        };
        let result = self.try_round(host, task, &mut diag).await;
        diag.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        match result {
            Ok((decision, meta)) => {
                diag.stage = "completed".into();
                let outcome_label = match &decision {
                    Decision::Confirm(_) => "confirm_round2",
                    Decision::Finish(outcome) => outcome.verdict.as_str(),
                };
                self.log(
                    "info",
                    "round",
                    Some(&task.run_id),
                    Some(&task.account_id),
                    format!("Round {round_num} finished: {outcome_label}"),
                    Some(json!({
                        "model": task.model,
                        "effort": task.effort,
                        "durationMs": diag.duration_ms,
                        "credentialRevisionBefore": diag.credential_revision_before,
                        "credentialRevisionAfter": diag.credential_revision_after,
                        "expectedLabel": diag.expected_label,
                        "detectorDecision": diag.detector_decision,
                        "usedOutputs": diag.used_outputs,
                    })),
                )
                .await;
                (decision, meta, Some(diag))
            }
            Err((error, meta, detail)) => {
                if diag.error_detail.is_none() {
                    diag.error_detail = detail;
                }
                self.log(
                    "warn",
                    "round",
                    Some(&task.run_id),
                    Some(&task.account_id),
                    format!(
                        "Round {round_num} ended with {error:?} at stage {}",
                        diag.stage
                    ),
                    Some(json!({
                        "model": task.model,
                        "effort": task.effort,
                        "reason": error,
                        "stage": diag.stage,
                        "durationMs": diag.duration_ms,
                        "credentialRevisionBefore": diag.credential_revision_before,
                        "credentialRevisionAfter": diag.credential_revision_after,
                        "identityChanged": diag.identity_changed,
                        "errorDetail": diag.error_detail,
                        "probesExecuted": diag.probes.len(),
                    })),
                )
                .await;
                (Decision::Finish(failure(error)), meta, Some(diag))
            }
        }
    }

    async fn try_round(
        &self,
        host: &Host<'_>,
        task: &Task,
        diag: &mut RunDiagnostic,
    ) -> Result<(Decision, Option<AccountMeta>), (Reason, Option<AccountMeta>, Option<String>)>
    {
        diag.stage = "detector_check".into();
        let bank = self.bank.as_ref().ok_or((
            Reason::DetectorUnavailable,
            None,
            Some("reference bank not loaded".into()),
        ))?;

        diag.stage = "account_info_before".into();
        let info = host
            .account_info(&task.account_id)
            .await
            .map_err(|(reason, detail)| (reason, None, detail))?;
        let meta = Some(info.meta.clone());
        diag.credential_revision_before = Some(info.credential_revision);
        diag.plan_type = info.meta.plan_type.clone();
        let expected = bank.expected_label(&task.model);
        diag.expected_label = expected.map(|(id, _)| id.to_owned());

        let identity_fp = info.identity.fingerprint();
        let stamp = bank.round_stamp(&task.model, &task.effort, &identity_fp);
        if task.first_round.as_ref().is_some_and(|r| r.stamp != stamp) {
            diag.stage = "round2_stamp_check".into();
            return Err((
                Reason::VersionChanged,
                meta,
                Some("round 1 stamp differs from current model/effort/reference-bank/account-identity".into()),
            ));
        }

        diag.stage = "access_check".into();
        host.access_with_detail(
            &task.client_key_id,
            std::slice::from_ref(&task.account_id),
            &task.model,
        )
        .await
        .map_err(|(reason, detail)| (reason, meta.clone(), detail))?;

        diag.stage = "challenges".into();
        let challenges = bank.challenges().map_err(|e| {
            (
                e,
                meta.clone(),
                Some("failed to generate lmfpd challenges".into()),
            )
        })?;

        self.log(
            "info",
            "round",
            Some(&task.run_id),
            Some(&task.account_id),
            format!("Starting round {}", diag.round),
            Some(json!({
                "email": info.meta.email,
                "planType": info.meta.plan_type,
                "model": task.model,
                "effort": task.effort,
                "credentialRevision": info.credential_revision,
                "expectedLabel": diag.expected_label,
            })),
        )
        .await;

        let mut outputs = Vec::with_capacity(domain::PROBES_PER_ROUND as usize);
        for (idx, c) in challenges.into_iter().enumerate() {
            let probe_index = (idx + 1) as u8;
            diag.stage = format!("probe_{probe_index}");
            let (sample_result, trace) = host
                .sample_with_trace(
                    &task.client_key_id,
                    &task.account_id,
                    &task.model,
                    &task.effort,
                    &c.prompt,
                )
                .await;
            let ok = sample_result.is_ok();
            let err_reason = sample_result.as_ref().err().copied();
            let probe_diag = ProbeDiagnostic {
                index: probe_index,
                challenge_id: c.id.clone(),
                expected_count: c.expected_count,
                duration_ms: trace.duration_ms,
                ok,
                batches: trace.batches,
                events: trace.events,
                text_parts: trace.text_parts,
                text_chars: trace.text_chars,
                reasoning_chars: trace.reasoning_chars,
                finish_reason: trace.finish_reason.clone(),
                wire_terminal: trace.wire_terminal.clone(),
                error_reason: err_reason,
                error_detail: trace.error_detail.clone(),
            };
            self.log(
                if ok { "info" } else { "warn" },
                "probe",
                Some(&task.run_id),
                Some(&task.account_id),
                format!(
                    "Probe #{probe_index}/{} {} in {}ms",
                    domain::PROBES_PER_ROUND,
                    if ok { "succeeded" } else { "failed" },
                    trace.duration_ms
                ),
                Some(json!({
                    "challengeId": c.id,
                    "expectedCount": c.expected_count,
                    "batches": trace.batches,
                    "events": trace.events,
                    "textParts": trace.text_parts,
                    "textChars": trace.text_chars,
                    "reasoningChars": trace.reasoning_chars,
                    "finishReason": trace.finish_reason,
                    "wireTerminal": trace.wire_terminal,
                    "errorReason": err_reason,
                    "errorDetail": trace.error_detail,
                })),
            )
            .await;
            diag.probes.push(probe_diag);
            match sample_result {
                Ok(text) => outputs.push(lmfpd::Output {
                    text,
                    expected_count: c.expected_count,
                }),
                Err(reason) => {
                    return Err((reason, meta.clone(), trace.error_detail));
                }
            }
        }

        diag.stage = "account_info_after".into();
        let after = host
            .account_info(&task.account_id)
            .await
            .map_err(|(reason, detail)| (reason, meta.clone(), detail))?;
        diag.credential_revision_after = Some(after.credential_revision);
        if after.identity != info.identity {
            diag.stage = "identity_check".into();
            diag.identity_changed = true;
            return Err((
                Reason::AccountChanged,
                Some(after.meta),
                Some(format!(
                    "upstream account identity changed during round (credential_revision {} -> {})",
                    info.credential_revision, after.credential_revision
                )),
            ));
        }
        if after.credential_revision != info.credential_revision {
            self.log(
                "info",
                "round",
                Some(&task.run_id),
                Some(&task.account_id),
                format!(
                    "Account credential_revision updated {} -> {} during round with unchanged upstream identity; continuing",
                    info.credential_revision, after.credential_revision
                ),
                None,
            )
            .await;
        }

        diag.stage = "scoring".into();
        let (rank_result, score_trace) = bank.rank_with_trace(&outputs, stamp);
        diag.detector_decision = score_trace.decision;
        diag.used_outputs = score_trace.used_outputs;
        diag.probability_status = score_trace.probability_status;
        let round = match rank_result {
            Ok(round) => round,
            Err(reason) => {
                return Err((reason, Some(after.meta), score_trace.detail));
            }
        };
        Ok((
            domain::decide(expected.map(|(id, _)| id), task.first_round.as_ref(), round),
            Some(after.meta),
        ))
    }

    fn build_logs_response(
        &self,
        state: &State,
        query: &RunQuery,
        live_labels: &BTreeMap<String, AccountMeta>,
        accounts: &[AccountChoice],
        cached_models: Option<&CachedModels>,
        runtime_logs: Vec<LogEntry>,
    ) -> Value {
        let (records, summary, _) = collect_run_records(state, live_labels);
        let matched: Vec<RunRecord> = records
            .into_iter()
            .filter(|record| matches_query(record, query))
            .collect();
        let account_snapshot: Vec<Value> = accounts
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "email": a.email,
                    "name": a.name,
                    "planType": a.plan_type,
                    "credentialRevision": a.credential_revision,
                })
            })
            .collect();
        json!({
            "exportedAt": now(),
            "pluginVersion": self.version,
            "detectorReady": self.bank.is_some(),
            "detectorRevision": self.bank.as_ref().map(|b| &b.revision),
            "environment": {
                "accountCount": accounts.len(),
                "accounts": account_snapshot,
                "modelCount": cached_models.map(|c| c.models.len()).unwrap_or(0),
                "modelCatalogAt": cached_models.map(|c| c.fetched_at),
                "modelCatalogError": cached_models.and_then(|c| c.error.as_deref()),
            },
            "schedule": schedule_value(state),
            "summary": summary,
            "recordCount": matched.len(),
            "records": matched,
            "runtimeLogCount": runtime_logs.len(),
            "runtimeLogs": runtime_logs,
        })
    }
}

async fn account_labels(host: &Host<'_>) -> BTreeMap<String, AccountMeta> {
    host.accounts()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|account| {
            let meta = account.meta();
            (account.id, meta)
        })
        .collect()
}

struct RunQuery {
    page: usize,
    page_size: usize,
    search: Option<String>,
    source: Option<bool>,
    verdict: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    account_id: Option<String>,
    run_id: Option<String>,
}

impl RunQuery {
    fn unfiltered() -> Self {
        Self {
            page: 1,
            page_size: 20,
            search: None,
            source: None,
            verdict: None,
            model: None,
            effort: None,
            account_id: None,
            run_id: None,
        }
    }

    fn from_map(map: &BTreeMap<String, String>) -> Result<Self, Reason> {
        let page = match map.get("page").filter(|s| !s.is_empty()) {
            Some(raw) => raw.parse::<usize>().map_err(|_| Reason::InvalidInput)?,
            None => 1,
        };
        let page_size = match map.get("pageSize").filter(|s| !s.is_empty()) {
            Some(raw) => raw.parse::<usize>().map_err(|_| Reason::InvalidInput)?,
            None => 20,
        };
        if page == 0 || !(1..=100).contains(&page_size) {
            return Err(Reason::InvalidInput);
        }
        let source = match map.get("source").map(String::as_str) {
            None | Some("" | "all") => None,
            Some("manual") => Some(false),
            Some("scheduled") => Some(true),
            Some(_) => return Err(Reason::InvalidInput),
        };
        let clean = |key: &str| {
            map.get(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty() && v != "all")
        };
        Ok(Self {
            page,
            page_size,
            search: clean("search").map(|s| s.to_ascii_lowercase()),
            source,
            verdict: clean("verdict").map(|s| s.to_ascii_lowercase()),
            model: clean("model"),
            effort: clean("effort"),
            account_id: clean("accountId"),
            run_id: clean("runId"),
        })
    }
}

fn collect_run_records(
    state: &State,
    live_labels: &BTreeMap<String, AccountMeta>,
) -> (Vec<RunRecord>, Value, Value) {
    let mut all_models = BTreeSet::new();
    let mut all_efforts = BTreeSet::new();
    let mut running_count = 0_usize;
    let mut queued_count = 0_usize;
    let mut match_count = 0_usize;
    let mut ranked_count = 0_usize;
    let mut suspect_count = 0_usize;
    let mut insufficient_count = 0_usize;
    let mut failed_count = 0_usize;
    let mut interrupted_count = 0_usize;
    let mut skipped_count = 0_usize;

    let mut records = Vec::with_capacity(state.queue.len() + state.history.len());
    for q in &state.queue {
        if q.claimed {
            running_count += 1;
        } else {
            queued_count += 1;
        }
        if !q.model.is_empty() {
            all_models.insert(q.model.clone());
        }
        if !q.effort.is_empty() {
            all_efforts.insert(q.effort.clone());
        }
        let fallback = live_labels.get(&q.account_id);
        records.push(RunRecord {
            kind: "queue",
            run_id: q.run_id.clone(),
            account_id: q.account_id.clone(),
            account_email: q
                .account_email
                .clone()
                .or_else(|| fallback.and_then(|m| m.email.clone())),
            account_name: q
                .account_name
                .clone()
                .or_else(|| fallback.and_then(|m| m.name.clone())),
            model: q.model.clone(),
            effort: q.effort.clone(),
            scheduled: q.scheduled,
            status: if q.claimed { "running" } else { "queued" }.into(),
            round: Some(if q.first_round.is_some() { 2 } else { 1 }),
            queued_at: Some(q.queued_at),
            ended_at: None,
            plugin_version: None,
            first_round: q.first_round.clone(),
            outcome: None,
            diagnostics: None,
        });
    }
    for h in &state.history {
        match h.outcome.verdict {
            Verdict::Match => match_count += 1,
            Verdict::Ranked => ranked_count += 1,
            Verdict::Suspect => suspect_count += 1,
            Verdict::Insufficient => insufficient_count += 1,
            Verdict::Failed => failed_count += 1,
            Verdict::Interrupted => interrupted_count += 1,
            Verdict::Skipped => skipped_count += 1,
        }
        if !h.model.is_empty() {
            all_models.insert(h.model.clone());
        }
        if !h.effort.is_empty() {
            all_efforts.insert(h.effort.clone());
        }
        let fallback = live_labels.get(&h.account_id);
        records.push(RunRecord {
            kind: "history",
            run_id: h.run_id.clone(),
            account_id: h.account_id.clone(),
            account_email: h
                .account_email
                .clone()
                .or_else(|| fallback.and_then(|m| m.email.clone())),
            account_name: h
                .account_name
                .clone()
                .or_else(|| fallback.and_then(|m| m.name.clone())),
            model: h.model.clone(),
            effort: h.effort.clone(),
            scheduled: h.scheduled,
            status: h.outcome.verdict.as_str().into(),
            round: None,
            queued_at: None,
            ended_at: Some(h.ended_at),
            plugin_version: Some(h.plugin_version.clone()),
            first_round: h.first_round.clone(),
            outcome: Some(h.outcome.clone()),
            diagnostics: h.diagnostics.clone(),
        });
    }
    let summary = json!({
        "total": state.queue.len() + state.history.len(),
        "running": running_count,
        "queued": queued_count,
        "historyTotal": state.history.len(),
        "match": match_count,
        "ranked": ranked_count,
        "suspect": suspect_count,
        "insufficient": insufficient_count,
        "failed": failed_count,
        "interrupted": interrupted_count,
        "skipped": skipped_count,
    });
    let filters = json!({
        "models": all_models.into_iter().collect::<Vec<_>>(),
        "efforts": all_efforts.into_iter().collect::<Vec<_>>(),
    });
    (records, summary, filters)
}

fn build_runs_response(
    state: &State,
    query: &RunQuery,
    live_labels: &BTreeMap<String, AccountMeta>,
) -> Value {
    let (records, summary, filters) = collect_run_records(state, live_labels);
    let filtered: Vec<RunRecord> = records
        .into_iter()
        .filter(|record| matches_query(record, query))
        .collect();
    let total = filtered.len();
    let total_pages = if total == 0 {
        1
    } else {
        total.div_ceil(query.page_size)
    };
    let page = query.page.min(total_pages);
    let start = (page - 1) * query.page_size;
    let end = (start + query.page_size).min(total);
    let items = if start < end {
        &filtered[start..end]
    } else {
        &[]
    };

    json!({
        "items": items,
        "pagination": {
            "page": page,
            "pageSize": query.page_size,
            "total": total,
            "totalPages": total_pages,
        },
        "summary": summary,
        "filters": filters,
    })
}

fn matches_query(record: &RunRecord, query: &RunQuery) -> bool {
    if let Some(run_id) = &query.run_id
        && record.run_id != *run_id
    {
        return false;
    }
    if let Some(scheduled) = query.source
        && record.scheduled != scheduled
    {
        return false;
    }
    if let Some(verdict) = &query.verdict
        && record.status != *verdict
    {
        return false;
    }
    if let Some(model) = &query.model
        && record.model != *model
    {
        return false;
    }
    if let Some(effort) = &query.effort
        && record.effort != *effort
    {
        return false;
    }
    if let Some(account_id) = &query.account_id
        && record.account_id != *account_id
    {
        return false;
    }
    if let Some(needle) = &query.search {
        let mut matched = record.account_id.to_ascii_lowercase().contains(needle)
            || record.run_id.to_ascii_lowercase().contains(needle)
            || record.model.to_ascii_lowercase().contains(needle)
            || record.effort.to_ascii_lowercase().contains(needle)
            || record
                .account_email
                .as_deref()
                .is_some_and(|e| e.to_ascii_lowercase().contains(needle))
            || record
                .account_name
                .as_deref()
                .is_some_and(|n| n.to_ascii_lowercase().contains(needle));
        if !matched && let Some(round) = record.outcome.as_ref().and_then(|o| o.last_round.as_ref())
        {
            matched = round.ranking.candidates.iter().any(|c| {
                c.id.to_ascii_lowercase().contains(needle)
                    || c.label.to_ascii_lowercase().contains(needle)
            });
        }
        if !matched {
            return false;
        }
    }
    true
}

fn parse_query(raw: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if let (Some(key), Some(val)) = (percent_decode(k), percent_decode(v))
            && !key.is_empty()
        {
            map.insert(key, val);
        }
    }
    map
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1])?;
                let lo = hex_val(bytes[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'%' => return None,
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Reason> {
    serde_json::from_slice(bytes).map_err(|_| Reason::InvalidInput)
}

fn schedule_value(state: &State) -> Value {
    json!({
        "settings": state.settings,
        "nextDueAt": state.next_due_at,
        "lastSkip": state.last_schedule_skip,
    })
}

fn failure(reason: Reason) -> Outcome {
    let verdict = match reason {
        Reason::Cancelled => Verdict::Interrupted,
        Reason::Incomplete
        | Reason::Transformed
        | Reason::NotScorable
        | Reason::Nearby
        | Reason::Inconsistent
        | Reason::UnsupportedProfile
        | Reason::VersionChanged
        | Reason::AccountChanged => Verdict::Insufficient,
        _ => Verdict::Failed,
    };
    Outcome::no_rank(verdict, reason)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::HistoryItem;

    #[test]
    fn operational_failures_and_incomplete_replies_never_create_a_suspect() {
        for reason in [
            Reason::AccountUnavailable,
            Reason::AccessDenied,
            Reason::RateLimited,
            Reason::Timeout,
        ] {
            assert_eq!(failure(reason).verdict, Verdict::Failed);
        }
        assert_eq!(failure(Reason::Cancelled).verdict, Verdict::Interrupted);
        assert_eq!(failure(Reason::Incomplete).verdict, Verdict::Insufficient);
        assert_eq!(
            failure(Reason::UnsupportedProfile).verdict,
            Verdict::Insufficient
        );
    }

    #[test]
    fn runs_query_filters_and_paginates_queue_and_history() {
        let mut state = State::default();
        let labels = BTreeMap::from([(
            "acct_1".into(),
            AccountMeta {
                email: Some("alice@example.com".into()),
                name: Some("Alice".into()),
                plan_type: Some("pro".into()),
            },
        )]);
        state
            .enqueue_manual(
                "m-run1234567".into(),
                Target::new(
                    "key_1".into(),
                    "gpt-6-sol".into(),
                    "high".into(),
                    vec!["acct_1".into()],
                ),
                1000,
                &labels,
            )
            .unwrap();
        for idx in 0..25_u64 {
            state.history.push(HistoryItem {
                run_id: format!("m-hist{idx:04}"),
                account_id: format!("acct_{}", idx % 3),
                account_email: Some(if idx % 2 == 0 {
                    "alice@example.com".into()
                } else {
                    "bob@example.com".into()
                }),
                account_name: None,
                model: if idx % 2 == 0 {
                    "gpt-6-sol".into()
                } else {
                    "gpt-6.2-sol".into()
                },
                effort: if idx % 3 == 0 {
                    "high".into()
                } else {
                    "low".into()
                },
                legacy_profile_id: None,
                scheduled: idx % 4 == 0,
                ended_at: 2000 + idx,
                plugin_version: "0.2.0".into(),
                first_round: None,
                outcome: Outcome::no_rank(
                    if idx % 5 == 0 {
                        Verdict::Failed
                    } else {
                        Verdict::Match
                    },
                    Reason::Timeout,
                ),
                diagnostics: Some(RunDiagnostic {
                    round: 1,
                    stage: "completed".into(),
                    duration_ms: 1200 + idx,
                    ..RunDiagnostic::default()
                }),
            });
        }
        let q = parse_query("page=2&pageSize=5&search=alice%40example.com&model=gpt-6-sol");
        let params = RunQuery::from_map(&q).unwrap();
        let res = build_runs_response(&state, &params, &BTreeMap::new());
        assert_eq!(res["pagination"]["page"], 2);
        assert_eq!(res["pagination"]["pageSize"], 5);
        assert_eq!(res["pagination"]["total"], 14); // 1 queued + 13 history
        assert_eq!(res["pagination"]["totalPages"], 3);
        assert_eq!(res["items"].as_array().unwrap().len(), 5);
        assert_eq!(res["summary"]["queued"], 1);
        assert_eq!(res["summary"]["historyTotal"], 25);

        let q2 = parse_query("source=scheduled&verdict=failed");
        let params2 = RunQuery::from_map(&q2).unwrap();
        let res2 = build_runs_response(&state, &params2, &BTreeMap::new());
        assert_eq!(res2["pagination"]["total"], 2); // idx 0 and 20

        let svc = Service::new("0.1.0-dev.3".into(), None);
        let mut ring = LogRing::default();
        ring.push(
            "info",
            "round",
            Some("m-hist0000".into()),
            Some("acct_0".into()),
            "ok",
            None,
        );
        ring.push(
            "warn",
            "probe",
            Some("m-hist0001".into()),
            Some("acct_1".into()),
            "err",
            None,
        );
        let mut q3 = RunQuery::unfiltered();
        q3.run_id = Some("m-hist0000".into());
        let bundle = svc.build_logs_response(
            &state,
            &q3,
            &BTreeMap::new(),
            &[],
            None,
            ring.snapshot(Some("m-hist0000")),
        );
        assert_eq!(bundle["recordCount"], 1);
        assert_eq!(bundle["records"][0]["runId"], "m-hist0000");
        assert_eq!(bundle["records"][0]["diagnostics"]["stage"], "completed");
        assert_eq!(bundle["runtimeLogCount"], 1);
        assert_eq!(bundle["runtimeLogs"][0]["runId"], "m-hist0000");
    }
}
