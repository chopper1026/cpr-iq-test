//! CPR wire adapter. No raw credential, generic HTTP, or data-plane mutations are used.
use crate::{
    domain::{AccountMeta, DEFAULT_EFFORT, Reason, State, valid_effort, valid_id, valid_model_id},
    state_codec,
};
use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::{
        data::AccessQuery,
        host::{self as wire, *},
        middleware::http as http_wire,
        model::{CanonicalEvent, ExecutionEvent, FinishReason, WirePayload},
    },
    client::{HostClient, HostReply, HttpBody, HttpFrame, HttpRequest, SessionError},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    time::Duration,
};

const PROVIDER: &str = "openai";
const CODEX_PROTOCOL: &str = "codex";
const CODEX_CLIENT_VERSION: &str = "0.154.0";
const NAMESPACE: &str = "iq-test";
const STATE_KEY: &str = "state";
const MODEL_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_TEXT: usize = 64 * 1024;
const MAX_PARTS: usize = 32;
const READ_BATCHES: usize = 65_536;
const MAX_CATALOG_BODY: usize = 4 * 1024 * 1024;
const MAX_CATALOG_PROBE_ACCOUNTS: usize = 3;

const EFFORT_ORDER: &[&str] = &[
    "default", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
const FALLBACK_REASONING_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

fn fault() -> PluginFault {
    PluginFault::new(ErrorCode::Fault, "iq-test: invalid host result")
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub fn sanitize_detail(value: &str, max_chars: usize) -> String {
    let mut compact = String::with_capacity(value.len().min(max_chars));
    let mut prev_space = false;
    for ch in value.chars() {
        let ch = if ch.is_control() || ch.is_whitespace() {
            ' '
        } else {
            ch
        };
        if ch == ' ' {
            if !prev_space && !compact.is_empty() {
                compact.push(' ');
            }
            prev_space = true;
        } else {
            compact.push(ch);
            prev_space = false;
        }
    }
    let lowered = compact.to_ascii_lowercase();
    for secret_marker in ["bearer ", "cookie:", "set-cookie:", "sk-"] {
        if let Some(pos) = lowered.find(secret_marker) {
            compact.truncate(pos);
            compact.push_str("[redacted]");
            break;
        }
    }
    truncate(compact.trim(), max_chars)
}

pub fn fault_detail(error: &PluginFault) -> String {
    let kind = error
        .details
        .as_ref()
        .and_then(|d| d.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let status = error
        .http_status
        .map(|s| s.to_string())
        .unwrap_or_else(|| "-".into());
    let msg = sanitize_detail(&error.message, 140);
    format!("code={:?} http={status} kind={kind} msg={msg}", error.code)
}

pub fn safe_reason(error: &PluginFault) -> Reason {
    match (error.code, error.http_status) {
        (ErrorCode::Cancelled, _) => Reason::Cancelled,
        (ErrorCode::Timeout, _) | (_, Some(408 | 504)) => Reason::Timeout,
        (_, Some(429)) => Reason::RateLimited,
        (ErrorCode::PermissionDenied, _) | (_, Some(401 | 403)) => Reason::AccessDenied,
        (_, Some(404)) => Reason::AccountUnavailable,
        (ErrorCode::Conflict, _) => Reason::Conflict,
        _ => match error
            .details
            .as_ref()
            .and_then(|d| d.get("kind"))
            .and_then(Value::as_str)
        {
            Some("rate_limited" | "budget_exhausted") => Reason::RateLimited,
            Some("policy_denied" | "unauthorized") => Reason::AccessDenied,
            Some("no_available_provider" | "account_capacity_unavailable") => {
                Reason::AccountUnavailable
            }
            Some("unsupported" | "model_not_found") => Reason::UnsupportedProfile,
            _ => Reason::HostFailure,
        },
    }
}

trait Rpc: Sync {
    fn send(
        &self,
        method: &str,
        params: Value,
        payload: Vec<u8>,
    ) -> impl Future<Output = Result<HostReply, PluginFault>> + Send;
}

impl Rpc for HostClient {
    async fn send(
        &self,
        method: &str,
        params: Value,
        payload: Vec<u8>,
    ) -> Result<HostReply, PluginFault> {
        self.call(method, params, payload)
            .await
            .map_err(SessionError::into_plugin_fault)
    }
}

async fn control<R: DeserializeOwned>(
    rpc: &impl Rpc,
    method: &str,
    params: impl Serialize + Send,
) -> Result<R, PluginFault> {
    let reply = rpc
        .send(
            method,
            serde_json::to_value(params).map_err(|_| fault())?,
            vec![],
        )
        .await?;
    if !reply.payload.is_empty() {
        return Err(fault());
    }
    serde_json::from_value(reply.result).map_err(|_| fault())
}

async fn payload<R: DeserializeOwned>(
    rpc: &impl Rpc,
    method: &str,
    input: impl Serialize + Send,
) -> Result<R, PluginFault> {
    let reply = rpc
        .send(
            method,
            json!({}),
            serde_json::to_vec(&input).map_err(|_| fault())?,
        )
        .await?;
    if reply.result != json!({}) {
        return Err(fault());
    }
    serde_json::from_slice(&reply.payload).map_err(|_| fault())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountChoice {
    pub id: String,
    pub email: Option<String>,
    pub name: String,
    pub plan_type: Option<String>,
    pub credential_revision: u64,
    pub display: String,
}

impl AccountChoice {
    fn from_runtime(account: AuthRuntimeAccount) -> Self {
        let email = account
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate(value, 160));
        let name = truncate(account.name.trim(), 120);
        let plan_type = account
            .plan_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate(value, 64));
        let display = if let Some(email) = &email {
            email.clone()
        } else if !name.is_empty() && name != account.account_id {
            name.clone()
        } else {
            account.account_id.clone()
        };
        Self {
            id: account.account_id,
            email,
            name,
            plan_type,
            credential_revision: account.credential_revision,
            display,
        }
    }

    pub fn meta(&self) -> AccountMeta {
        let name = (!self.name.is_empty() && self.name != self.id).then(|| self.name.clone());
        AccountMeta {
            email: self.email.clone(),
            name,
            plan_type: self.plan_type.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountIdentity {
    pub provider_id: String,
    pub authentication_kind: String,
    pub upstream_user_id: Option<String>,
    pub upstream_account_id: Option<String>,
    pub email: Option<String>,
}

impl AccountIdentity {
    fn from_runtime(account: &AuthRuntimeAccount) -> Self {
        let clean = |opt: &Option<String>, max: usize| {
            opt.as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| truncate(v, max))
        };
        Self {
            provider_id: account.provider_id.clone(),
            authentication_kind: account.authentication_kind.clone(),
            upstream_user_id: clean(&account.upstream_user_id, 160),
            upstream_account_id: clean(&account.upstream_account_id, 160),
            email: clean(&account.email, 160).map(|e| e.to_ascii_lowercase()),
        }
    }

    /// Stable upstream identity used across probes and confirmation rounds.
    /// Cookie captures (`Set-Cookie`) and OAuth access-token refreshes increment
    /// `credential_revision` in CPR without changing the upstream account identity.
    pub fn fingerprint(&self) -> String {
        if self.upstream_user_id.is_some() || self.upstream_account_id.is_some() {
            format!(
                "{}|{}|u:{}|a:{}",
                self.provider_id,
                self.authentication_kind,
                self.upstream_user_id.as_deref().unwrap_or(""),
                self.upstream_account_id.as_deref().unwrap_or(""),
            )
        } else if let Some(email) = self.email.as_deref() {
            format!(
                "{}|{}|e:{email}",
                self.provider_id, self.authentication_kind
            )
        } else {
            format!("{}|{}|local", self.provider_id, self.authentication_kind)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRuntimeInfo {
    pub credential_revision: u64,
    pub identity: AccountIdentity,
    pub meta: AccountMeta,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SampleTrace {
    pub duration_ms: u64,
    pub batches: u32,
    pub events: u32,
    pub text_parts: u16,
    pub text_chars: usize,
    pub reasoning_chars: usize,
    pub finish_reason: Option<String>,
    pub wire_terminal: Option<String>,
    pub error_detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: String,
    pub efforts: Vec<String>,
    pub default_effort: String,
    pub priority: i32,
    pub hidden: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    document: String,
}

pub struct Host<'a>(pub &'a HostClient);

impl Host<'_> {
    pub async fn accounts(&self) -> Result<Vec<AccountChoice>, Reason> {
        let mut cursor = None;
        let mut accounts = vec![];
        loop {
            let page: AuthListResult = payload(
                self.0,
                wire::AUTH_LIST,
                AuthListRequest {
                    provider_id: Some(PROVIDER.into()),
                    cursor: cursor.clone(),
                    limit: 100,
                },
            )
            .await
            .map_err(|e| safe_reason(&e))?;
            accounts.extend(
                page.accounts
                    .into_iter()
                    .filter(eligible)
                    .map(AccountChoice::from_runtime),
            );
            match page.next_cursor {
                None => break,
                Some(next) if cursor.as_ref().is_none_or(|old| old < &next) => cursor = Some(next),
                _ => return Err(Reason::HostFailure),
            }
        }
        accounts.sort_by(|a, b| {
            a.display
                .to_ascii_lowercase()
                .cmp(&b.display.to_ascii_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(accounts)
    }

    pub async fn catalog(&self) -> Result<(Vec<AccountChoice>, Vec<Choice>), Reason> {
        let accounts = self.accounts().await?;
        let mut cursor = None;
        let mut keys = vec![];
        loop {
            let page = self
                .0
                .list_keys(KeyListRequest {
                    cursor: cursor.clone(),
                    limit: 100,
                })
                .await
                .map_err(|e| safe_reason(&e))?;
            keys.extend(
                page.keys
                    .into_iter()
                    .filter(|key| key.enabled)
                    .map(|key| Choice {
                        id: key.id,
                        name: truncate(key.name.trim(), 120),
                    }),
            );
            match page.next_cursor {
                None => break,
                Some(next) if cursor.as_ref().is_none_or(|old| old < &next) => cursor = Some(next),
                _ => return Err(Reason::HostFailure),
            }
        }
        Ok((accounts, keys))
    }

    /// Automatically fetches all models and reasoning levels from the host:
    /// 1. Queries native Codex model catalogs via internal admin HTTP dispatch across up to 3
    ///    representative OAuth accounts covering distinct plan types (returns exact `supported_reasoning_levels`).
    /// 2. Enriches with `host.models.list` for the selected or first enabled Client Key.
    /// 3. Falls back to `/api/admin/accounts/models` (cached IDs) if native catalogs are unavailable.
    pub async fn discover_models(
        &self,
        accounts: &[AccountChoice],
        keys: &[Choice],
        preferred_key: Option<&str>,
    ) -> (Vec<DiscoveredModel>, Option<String>) {
        let mut merged: Vec<DiscoveredModel> = Vec::new();
        let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
        let mut last_error: Option<String> = None;

        for account in representative_accounts(accounts) {
            match self.fetch_account_codex_catalog(&account.id).await {
                Ok(models) => merge_discovered(&mut merged, &mut by_id, models),
                Err(message) => last_error = Some(message),
            }
        }

        let key_id = preferred_key
            .filter(|id| keys.iter().any(|k| k.id == *id))
            .or_else(|| keys.first().map(|k| k.id.as_str()));
        if let Some(client_key_id) = key_id
            && let Ok(extra_ids) = self.list_models(client_key_id).await
        {
            let fallback: Vec<_> = extra_ids
                .into_iter()
                .filter(|id| !by_id.contains_key(id))
                .map(|id| fallback_model(&id))
                .collect();
            merge_discovered(&mut merged, &mut by_id, fallback);
        }

        if merged.is_empty()
            && let Some(first) = accounts.first()
            && let Ok(ids) = self.fetch_account_model_ids(&first.id).await
        {
            let fallback: Vec<_> = ids.into_iter().map(|id| fallback_model(&id)).collect();
            merge_discovered(&mut merged, &mut by_id, fallback);
        }

        merged.sort_by(|a, b| {
            a.hidden
                .cmp(&b.hidden)
                .then_with(|| a.priority.cmp(&b.priority))
                .then_with(|| a.id.cmp(&b.id))
        });
        let error = if merged.is_empty() { last_error } else { None };
        (merged, error)
    }

    async fn fetch_account_codex_catalog(
        &self,
        account_id: &str,
    ) -> Result<Vec<DiscoveredModel>, String> {
        if !valid_id(account_id, 128) {
            return Err("invalid account id".into());
        }
        let body = self
            .dispatch_get_json(&format!(
                "/api/admin/accounts/models/catalog?accountId={account_id}"
            ))
            .await?;
        parse_codex_catalog_envelope(&body)
    }

    async fn fetch_account_model_ids(&self, account_id: &str) -> Result<Vec<String>, String> {
        if !valid_id(account_id, 128) {
            return Err("invalid account id".into());
        }
        let body = self
            .dispatch_get_json(&format!(
                "/api/admin/accounts/models?accountId={account_id}"
            ))
            .await?;
        Ok(parse_account_models_envelope(&body))
    }

    async fn dispatch_get_json(&self, uri: &str) -> Result<Value, String> {
        let mut response = self
            .0
            .dispatch_http(HttpRequest {
                settings: Value::Null,
                method: "GET".into(),
                uri: uri.into(),
                version: http_wire::Version::Http11,
                headers: vec![],
                timeout_ms: Some(15_000),
                body: HttpBody::empty(),
            })
            .await
            .map_err(|_| "dispatch_http failed".to_owned())?;
        let mut bytes = Vec::new();
        while let Some(frame) = response
            .body
            .read()
            .await
            .map_err(|_| "response read failed".to_owned())?
        {
            if let HttpFrame::Data(chunk) = frame {
                if bytes.len().saturating_add(chunk.len()) > MAX_CATALOG_BODY {
                    let _ = response.body.close().await;
                    return Err("catalog response too large".into());
                }
                bytes.extend_from_slice(&chunk);
            }
        }
        if !(200..300).contains(&response.status) {
            return Err(format!("HTTP {}", response.status));
        }
        serde_json::from_slice(&bytes).map_err(|_| "invalid catalog json".to_owned())
    }

    async fn list_models(&self, client_key_id: &str) -> Result<Vec<String>, Reason> {
        let result: ModelListResult = control(
            self.0,
            wire::MODELS_LIST,
            ModelListRequest {
                client_key_id: client_key_id.into(),
                protocol: CODEX_PROTOCOL.into(),
                client_version: CODEX_CLIENT_VERSION.into(),
            },
        )
        .await
        .map_err(|e| safe_reason(&e))?;
        Ok(result
            .models
            .into_iter()
            .filter(|id| valid_model_id(id))
            .collect())
    }

    pub async fn account_info(
        &self,
        id: &str,
    ) -> Result<AccountRuntimeInfo, (Reason, Option<String>)> {
        account_info(self.0, id).await
    }

    pub async fn access(&self, key: &str, ids: &[String], model: &str) -> Result<(), Reason> {
        self.access_with_detail(key, ids, model)
            .await
            .map_err(|(reason, _)| reason)
    }

    pub async fn access_with_detail(
        &self,
        key: &str,
        ids: &[String],
        model: &str,
    ) -> Result<(), (Reason, Option<String>)> {
        let access = self
            .0
            .check_access(AccessQuery {
                client_key_id: key.into(),
                account_ids: ids.to_vec(),
                upstream_model: Some(model.into()),
            })
            .await
            .map_err(|e| (safe_reason(&e), Some(fault_detail(&e))))?;
        if access.client_key_id != key
            || !ids.iter().all(|id| {
                access
                    .accounts
                    .iter()
                    .any(|a| a.account_id == *id && a.allowed)
            })
        {
            return Err((
                Reason::AccessDenied,
                Some("host.data.access.check denied target key/account/model".into()),
            ));
        }
        Ok(())
    }

    pub async fn sample_with_trace(
        &self,
        key: &str,
        account: &str,
        model: &str,
        effort: &str,
        prompt: &str,
    ) -> (Result<String, Reason>, SampleTrace) {
        execute_with_trace(self.0, key, account, model, effort, prompt).await
    }

    pub async fn load(&self) -> Result<(State, Option<u64>), PluginFault> {
        let result: StateGetResult = control(
            self.0,
            wire::STATE_GET,
            StateGetRequest {
                namespace: NAMESPACE.into(),
                key: STATE_KEY.into(),
            },
        )
        .await?;
        match result.record {
            None => Ok((State::default(), None)),
            Some(record) if record.schema_version == 1 => {
                let doc: Document = serde_json::from_value(record.value).map_err(|_| fault())?;
                Ok((
                    state_codec::decode(&doc.document).ok_or_else(fault)?,
                    Some(record.version),
                ))
            }
            _ => Err(fault()),
        }
    }

    /// The same pure transition may be retried only after a definitive CAS conflict.
    pub async fn change<T: Send>(
        &self,
        f: impl Fn(&mut State, Option<u64>) -> T + Sync,
    ) -> Result<T, PluginFault> {
        for _ in 0..5 {
            let (mut state, version) = self.load().await?;
            let before = state.clone();
            let output = f(&mut state, version);
            if state == before {
                return Ok(output);
            }
            let encoded = state_codec::encode_bounded(&mut state).ok_or_else(fault)?;
            let result: Result<StatePutResult, _> = control(
                self.0,
                wire::STATE_PUT,
                StatePutRequest {
                    namespace: NAMESPACE.into(),
                    key: STATE_KEY.into(),
                    value: json!({"document": encoded}),
                    expected_version: version,
                },
            )
            .await;
            match result {
                Ok(_) => return Ok(output),
                Err(error) if error.code == ErrorCode::Conflict => {}
                Err(error) => return Err(error),
            }
        }
        Err(PluginFault::new(
            ErrorCode::Conflict,
            "iq-test: state changed; refresh and retry",
        ))
    }
}

fn representative_accounts(accounts: &[AccountChoice]) -> Vec<&AccountChoice> {
    let mut picked = Vec::new();
    let mut seen_plans = BTreeSet::new();
    for account in accounts {
        let plan = account
            .plan_type
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        if seen_plans.insert(plan) {
            picked.push(account);
            if picked.len() >= MAX_CATALOG_PROBE_ACCOUNTS {
                break;
            }
        }
    }
    picked
}

fn effort_rank(effort: &str) -> usize {
    EFFORT_ORDER
        .iter()
        .position(|&item| item == effort)
        .unwrap_or(EFFORT_ORDER.len())
}

fn merge_efforts(existing: &mut Vec<String>, incoming: &[String]) {
    let mut seen: BTreeSet<String> = existing.iter().cloned().collect();
    for effort in incoming {
        if valid_effort(effort) && seen.insert(effort.clone()) {
            existing.push(effort.clone());
        }
    }
    if existing.len() > 1 {
        existing.retain(|effort| effort != DEFAULT_EFFORT);
    }
    existing.sort_by_key(|a| effort_rank(a));
}

fn merge_discovered(
    target: &mut Vec<DiscoveredModel>,
    by_id: &mut BTreeMap<String, usize>,
    incoming: Vec<DiscoveredModel>,
) {
    for model in incoming {
        if let Some(&idx) = by_id.get(&model.id) {
            let entry = &mut target[idx];
            merge_efforts(&mut entry.efforts, &model.efforts);
            entry.hidden &= model.hidden;
            entry.priority = entry.priority.min(model.priority);
            if !entry.efforts.contains(&entry.default_effort)
                && let Some(first) = entry.efforts.first()
            {
                entry.default_effort = first.clone();
            }
        } else {
            let idx = target.len();
            by_id.insert(model.id.clone(), idx);
            target.push(model);
        }
    }
}

fn fallback_model(id: &str) -> DiscoveredModel {
    let lower = id.to_ascii_lowercase();
    let reasoning_likely = lower.starts_with("gpt-5")
        || lower.starts_with("gpt-6")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.contains("codex");
    let efforts = if reasoning_likely {
        FALLBACK_REASONING_EFFORTS
            .iter()
            .map(|e| (*e).to_owned())
            .collect()
    } else {
        vec![DEFAULT_EFFORT.to_owned()]
    };
    let default_effort = if reasoning_likely {
        "low".into()
    } else {
        DEFAULT_EFFORT.into()
    };
    DiscoveredModel {
        id: id.to_owned(),
        display_name: id.to_owned(),
        efforts,
        default_effort,
        priority: 10_000,
        hidden: false,
    }
}

fn parse_codex_catalog_envelope(body: &Value) -> Result<Vec<DiscoveredModel>, String> {
    let Some(models) = body
        .pointer("/data/catalog/models")
        .and_then(Value::as_array)
    else {
        return Err("missing data.catalog.models".into());
    };
    let mut parsed = Vec::with_capacity(models.len());
    for (idx, item) in models.iter().enumerate() {
        if item.get("supported_in_api").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let visibility = item
            .get("visibility")
            .and_then(Value::as_str)
            .unwrap_or("list");
        if visibility.eq_ignore_ascii_case("none") {
            continue;
        }
        let Some(slug) = item
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|slug| valid_model_id(slug))
        else {
            continue;
        };
        let display_name = item
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| truncate(name, 128))
            .unwrap_or_else(|| slug.to_owned());
        let mut efforts = Vec::new();
        let mut seen = BTreeSet::new();
        if let Some(levels) = item
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
        {
            for level in levels {
                let effort = level
                    .get("effort")
                    .and_then(Value::as_str)
                    .or_else(|| level.as_str())
                    .map(str::trim);
                if let Some(effort) = effort
                    && valid_effort(effort)
                    && seen.insert(effort.to_owned())
                {
                    efforts.push(effort.to_owned());
                }
            }
        }
        if efforts.is_empty() {
            efforts.push(DEFAULT_EFFORT.to_owned());
        } else {
            efforts.sort_by_key(|a| effort_rank(a));
        }
        let default_effort = item
            .get("default_reasoning_level")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|level| efforts.iter().any(|e| e == level))
            .map(str::to_owned)
            .or_else(|| efforts.iter().find(|e| e.as_str() == "low").cloned())
            .unwrap_or_else(|| efforts[0].clone());
        let priority = item
            .get("priority")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(1_000 + idx as i32);
        parsed.push(DiscoveredModel {
            id: slug.to_owned(),
            display_name,
            efforts,
            default_effort,
            priority,
            hidden: visibility.eq_ignore_ascii_case("hide"),
        });
    }
    Ok(parsed)
}

fn parse_account_models_envelope(body: &Value) -> Vec<String> {
    body.pointer("/data/models")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .filter(|id| valid_model_id(id))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn eligible(account: &AuthRuntimeAccount) -> bool {
    account.provider_id == PROVIDER && account.authentication_kind == "oauth" && account.enabled
}

async fn account_info(
    rpc: &impl Rpc,
    id: &str,
) -> Result<AccountRuntimeInfo, (Reason, Option<String>)> {
    let account: AuthRuntimeAccount = payload(
        rpc,
        wire::AUTH_GET_RUNTIME,
        AuthGetRequest {
            account_id: id.into(),
        },
    )
    .await
    .map_err(|e| (safe_reason(&e), Some(fault_detail(&e))))?;
    if account.account_id != id || !eligible(&account) {
        return Err((
            Reason::AccountUnavailable,
            Some(format!(
                "account ineligible: enabled={} provider={} auth_kind={} state={}",
                account.enabled,
                account.provider_id,
                account.authentication_kind,
                account.credential_state
            )),
        ));
    }
    let identity = AccountIdentity::from_runtime(&account);
    let choice = AccountChoice::from_runtime(account.clone());
    Ok(AccountRuntimeInfo {
        credential_revision: account.credential_revision,
        identity,
        meta: choice.meta(),
    })
}

/// The only model submission site. Both the provider and the required account are always present.
async fn execute_with_trace(
    rpc: &impl Rpc,
    key: &str,
    account: &str,
    model: &str,
    effort: &str,
    prompt: &str,
) -> (Result<String, Reason>, SampleTrace) {
    let mut trace = SampleTrace::default();
    let submit_at = tokio::time::Instant::now();
    let params = match serde_json::to_value(ModelExecuteRequest {
        client_key_id: Some(key.into()),
        model: model.into(),
        protocol: PROVIDER.into(),
        operation: ModelOperation::Generate,
        provider: Some(PROVIDER.into()),
        account_id: Some(account.into()),
        previous_response_id: None,
    }) {
        Ok(params) => params,
        Err(_) => {
            trace.error_detail = Some("failed to serialize ModelExecuteRequest".into());
            return (Err(Reason::HostFailure), trace);
        }
    };
    let mut request_body = json!({
        "model": model,
        "input": [{"role": "user", "content": prompt}],
        "stream": true,
        "store": false,
    });
    if !effort.is_empty() && effort != DEFAULT_EFFORT {
        request_body["reasoning"] = json!({"effort": effort});
    }
    let body = match serde_json::to_vec(&request_body) {
        Ok(body) => body,
        Err(_) => {
            trace.error_detail = Some("failed to serialize request body".into());
            return (Err(Reason::HostFailure), trace);
        }
    };
    let deadline = submit_at + MODEL_TIMEOUT;
    let reply =
        match tokio::time::timeout_at(deadline, rpc.send(wire::MODEL_EXECUTE_STREAM, params, body))
            .await
        {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => {
                trace.duration_ms = elapsed_ms(submit_at);
                trace.error_detail = Some(format!("execute_stream {}", fault_detail(&error)));
                return (Err(safe_reason(&error)), trace);
            }
            Err(_) => {
                trace.duration_ms = elapsed_ms(submit_at);
                trace.error_detail = Some(format!(
                    "execute_stream timed out after {}ms",
                    trace.duration_ms
                ));
                return (Err(Reason::Timeout), trace);
            }
        };
    let handle: ModelStreamResult = match serde_json::from_value(reply.result) {
        Ok(handle) => handle,
        Err(_) => {
            trace.duration_ms = elapsed_ms(submit_at);
            trace.error_detail = Some("invalid ModelStreamResult payload from host".into());
            return (Err(Reason::HostFailure), trace);
        }
    };
    let result = if reply.payload.is_empty() {
        match tokio::time::timeout_at(deadline, read_stream(rpc, &handle.stream, &mut trace)).await
        {
            Ok(outcome) => outcome,
            Err(_) => {
                trace.error_detail = Some(format!(
                    "stream read timed out after {}ms (batches={} events={} text_chars={} reasoning_chars={})",
                    elapsed_ms(submit_at),
                    trace.batches,
                    trace.events,
                    trace.text_chars,
                    trace.reasoning_chars
                ));
                Err(Reason::Timeout)
            }
        }
    } else {
        trace.error_detail = Some("unexpected binary payload on ModelStreamResult".into());
        Err(Reason::HostFailure)
    };
    trace.duration_ms = elapsed_ms(submit_at);
    if result.is_err() {
        let params = json!({"stream":handle.stream});
        // On parent cancellation, the host also tears down every child stream it owns.
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            rpc.send(wire::MODEL_STREAM_CLOSE, params, vec![]),
        )
        .await;
    }
    (result, trace)
}

fn elapsed_ms(start: tokio::time::Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
async fn execute(
    rpc: &impl Rpc,
    key: &str,
    account: &str,
    model: &str,
    effort: &str,
    prompt: &str,
) -> Result<String, Reason> {
    execute_with_trace(rpc, key, account, model, effort, prompt)
        .await
        .0
}

async fn read_stream(
    rpc: &impl Rpc,
    stream: &str,
    trace: &mut SampleTrace,
) -> Result<String, Reason> {
    let mut text = VisibleText::default();
    let params = json!({"stream":stream,"maximum_bytes":1048576});
    for _ in 0..READ_BATCHES {
        let reply = match rpc
            .send(wire::MODEL_STREAM_READ, params.clone(), vec![])
            .await
        {
            Ok(reply) => reply,
            Err(error) => {
                text.snapshot_into(trace);
                trace.error_detail = Some(format!("stream_read {}", fault_detail(&error)));
                return Err(safe_reason(&error));
            }
        };
        trace.batches = trace.batches.saturating_add(1);
        let read: ModelStreamReadResult = match serde_json::from_value(reply.result) {
            Ok(read) => read,
            Err(_) => {
                text.snapshot_into(trace);
                trace.error_detail = Some("invalid ModelStreamReadResult metadata".into());
                return Err(Reason::Incomplete);
            }
        };
        if read.events != 0 || !reply.payload.is_empty() {
            let batch = match ModelEventBatch::decode(&reply.payload) {
                Ok(batch) => batch,
                Err(_) => {
                    text.snapshot_into(trace);
                    trace.error_detail = Some("failed to decode ModelEventBatch".into());
                    return Err(Reason::Incomplete);
                }
            };
            if batch.events.len() != read.events as usize {
                text.snapshot_into(trace);
                trace.error_detail = Some(format!(
                    "event batch length mismatch: decoded={} declared={}",
                    batch.events.len(),
                    read.events
                ));
                return Err(Reason::Incomplete);
            }
            for event in batch.events {
                trace.events = trace.events.saturating_add(1);
                if let Err(reason) = text.push(event, trace) {
                    text.snapshot_into(trace);
                    return Err(reason);
                }
            }
        } else if !read.end {
            text.snapshot_into(trace);
            trace.error_detail = Some("empty stream batch received before end=true".into());
            return Err(Reason::Incomplete);
        }
        if read.end {
            text.snapshot_into(trace);
            return text.finish(trace);
        }
    }
    text.snapshot_into(trace);
    trace.error_detail = Some(format!("exceeded max READ_BATCHES={READ_BATCHES}"));
    Err(Reason::Incomplete)
}

#[derive(Default)]
struct VisibleText {
    parts: BTreeMap<u32, String>,
    bytes: usize,
    reasoning_chars: usize,
    completed: bool,
    finish_reason: Option<String>,
}

impl VisibleText {
    fn snapshot_into(&self, trace: &mut SampleTrace) {
        trace.text_parts = self.parts.len().min(u16::MAX as usize) as u16;
        trace.text_chars = self.parts.values().map(|s| s.chars().count()).sum();
        trace.reasoning_chars = self.reasoning_chars;
        if self.finish_reason.is_some() {
            trace.finish_reason = self.finish_reason.clone();
        }
    }

    fn push(&mut self, event: ExecutionEvent, trace: &mut SampleTrace) -> Result<(), Reason> {
        if event.host.is_some_and(|host| host.middleware_transformed) {
            trace.error_detail = Some("host flagged middleware_transformed=true".into());
            return Err(Reason::Transformed);
        }
        // Native terminal errors may be delivered on the stream, rather than in its RPC status.
        if let Some(wire) = event.wire {
            let (native, data) = match wire.payload {
                WirePayload::Json { event, data, .. } => (event, Some(data)),
                WirePayload::WebSocket { event, data, .. } => (event, data),
                _ => (None, None),
            };
            let datatype = data
                .as_ref()
                .and_then(|v| v.get("type"))
                .and_then(Value::as_str);
            for kind in [native.as_deref(), datatype].into_iter().flatten() {
                match kind {
                    "error" | "response.failed" => {
                        let code = data
                            .as_ref()
                            .and_then(|v| {
                                v.pointer("/response/error/code").or_else(|| v.get("code"))
                            })
                            .and_then(Value::as_str);
                        let message = data
                            .as_ref()
                            .and_then(|v| {
                                v.pointer("/response/error/message")
                                    .or_else(|| v.get("message"))
                            })
                            .and_then(Value::as_str);
                        trace.wire_terminal = Some(match code {
                            Some(code) => format!("{kind}:{code}"),
                            None => kind.to_owned(),
                        });
                        trace.error_detail = Some(sanitize_detail(
                            &format!(
                                "wire {kind} code={} msg={}",
                                code.unwrap_or("-"),
                                message.unwrap_or("-")
                            ),
                            200,
                        ));
                        return Err(match code {
                            Some("rate_limit_exceeded" | "rate_limited" | "insufficient_quota") => {
                                Reason::RateLimited
                            }
                            Some("timeout" | "request_timeout") => Reason::Timeout,
                            _ => Reason::HostFailure,
                        });
                    }
                    "response.incomplete" | "response.refusal.delta" | "response.refusal.done" => {
                        let incomplete_reason = data
                            .as_ref()
                            .and_then(|v| v.pointer("/response/incomplete_details/reason"))
                            .and_then(Value::as_str);
                        trace.wire_terminal = Some(match incomplete_reason {
                            Some(reason) => format!("{kind}:{reason}"),
                            None => kind.to_owned(),
                        });
                        trace.error_detail = Some(format!(
                            "wire terminal {kind} detail={}",
                            incomplete_reason.unwrap_or("-")
                        ));
                        return Err(Reason::Incomplete);
                    }
                    _ => {}
                }
            }
        }
        for fact in event.facts {
            match fact {
                CanonicalEvent::ReasoningDelta { text, .. } => {
                    self.reasoning_chars =
                        self.reasoning_chars.saturating_add(text.chars().count());
                }
                // CPR encodes Responses content_index as `output_index * 1024 + part_index`,
                // so a message following a reasoning item starts at index 1024. Bound distinct
                // parts count rather than the raw index value.
                CanonicalEvent::TextDelta { index, text }
                    if !self.completed
                        && (self.parts.contains_key(&index) || self.parts.len() < MAX_PARTS) =>
                {
                    self.bytes = self.bytes.saturating_add(text.len());
                    if self.bytes > MAX_TEXT {
                        trace.error_detail =
                            Some(format!("visible text exceeded MAX_TEXT={MAX_TEXT} bytes"));
                        return Err(Reason::Incomplete);
                    }
                    self.parts.entry(index).or_default().push_str(&text);
                }
                CanonicalEvent::Completed {
                    reason: FinishReason::Stop,
                    ..
                } if !self.completed => {
                    self.completed = true;
                    self.finish_reason = Some("Stop".into());
                }
                CanonicalEvent::Completed { reason, .. } => {
                    let already = self.completed;
                    self.finish_reason = Some(format!("{reason:?}"));
                    trace.error_detail = Some(format!(
                        "unexpected Completed(reason={reason:?}, duplicate={already})"
                    ));
                    return Err(Reason::Incomplete);
                }
                CanonicalEvent::TextDelta { index, .. } => {
                    trace.error_detail = Some(format!(
                        "rejected TextDelta(index={index}, completed={}, parts={})",
                        self.completed,
                        self.parts.len()
                    ));
                    return Err(Reason::Incomplete);
                }
                CanonicalEvent::ToolCallDelta { .. } => {
                    trace.error_detail =
                        Some("unexpected ToolCallDelta in fingerprint probe".into());
                    return Err(Reason::Incomplete);
                }
                _ => {} // usage and metadata are not fingerprint evidence
            }
        }
        Ok(())
    }

    fn finish(self, trace: &mut SampleTrace) -> Result<String, Reason> {
        let visible: String = self.parts.into_values().collect();
        if !self.completed || visible.trim().is_empty() {
            trace.error_detail = Some(format!(
                "stream ended incomplete: completed={} visible_chars={} reasoning_chars={} finish={:?}",
                self.completed,
                visible.trim().chars().count(),
                self.reasoning_chars,
                self.finish_reason
            ));
            return Err(Reason::Incomplete);
        }
        Ok(visible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Stub {
        calls: Mutex<Vec<String>>,
        status: Option<u16>,
        finish: FinishReason,
        expect_effort: Option<&'static str>,
    }
    impl Rpc for Stub {
        async fn send(
            &self,
            method: &str,
            params: Value,
            body: Vec<u8>,
        ) -> Result<HostReply, PluginFault> {
            self.calls.lock().unwrap().push(method.into());
            match method {
                wire::MODEL_EXECUTE_STREAM => {
                    assert_eq!(params["account_id"], "account-A"); // B is healthy, but never used.
                    assert_eq!(params["provider"], "openai");
                    assert_eq!(params["client_key_id"], "key-id");
                    assert_eq!(params["operation"], "generate");
                    let value: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(value["store"], false);
                    match self.expect_effort {
                        Some(effort) => assert_eq!(value["reasoning"]["effort"], effort),
                        None => assert!(value.get("reasoning").is_none()),
                    }
                    if let Some(status) = self.status {
                        let mut error = PluginFault::new(ErrorCode::Rejected, "stub error body");
                        error.http_status = Some(status);
                        return Err(error);
                    }
                    Ok(HostReply {
                        result: json!({"request_id":"r1","stream":"s1"}),
                        payload: vec![],
                    })
                }
                wire::MODEL_STREAM_READ => Ok(HostReply {
                    result: json!({"events":1,"end":true}),
                    payload: ModelEventBatch {
                        events: vec![ExecutionEvent {
                            facts: vec![
                                CanonicalEvent::ReasoningDelta {
                                    index: 0,
                                    text: "not visible".into(),
                                },
                                // Matches CPR CodexCanonicalDecoder: output_index=1 => 1*1024 + 0.
                                CanonicalEvent::TextDelta {
                                    index: 1025,
                                    text: "world".into(),
                                },
                                CanonicalEvent::TextDelta {
                                    index: 1024,
                                    text: "hello ".into(),
                                },
                                CanonicalEvent::Completed {
                                    id: "r".into(),
                                    model: None,
                                    reason: self.finish,
                                },
                            ],
                            ..ExecutionEvent::default()
                        }],
                    }
                    .encode()
                    .unwrap(),
                }),
                wire::MODEL_STREAM_CLOSE => Ok(HostReply {
                    result: json!({}),
                    payload: vec![],
                }),
                _ => panic!("unexpected callback"),
            }
        }
    }

    #[tokio::test]
    async fn fixed_account_and_failure_no_fallback() {
        for (status, reason) in [
            (404, Reason::AccountUnavailable),
            (403, Reason::AccessDenied),
            (429, Reason::RateLimited),
        ] {
            let stub = Stub {
                calls: Mutex::default(),
                status: Some(status),
                finish: FinishReason::Stop,
                expect_effort: Some("low"),
            };
            assert_eq!(
                execute(&stub, "key-id", "account-A", "gpt-6-sol", "low", "probe").await,
                Err(reason)
            );
            assert_eq!(*stub.calls.lock().unwrap(), [wire::MODEL_EXECUTE_STREAM]);
        }
    }

    #[tokio::test]
    async fn handles_sparse_responses_content_index_and_default_effort() {
        let mut stub = Stub {
            calls: Mutex::default(),
            status: None,
            finish: FinishReason::Stop,
            expect_effort: Some("low"),
        };
        let (reply, trace) =
            execute_with_trace(&stub, "key-id", "account-A", "gpt-6-sol", "low", "probe").await;
        assert_eq!(reply.unwrap(), "hello world");
        assert_eq!(trace.batches, 1);
        assert_eq!(trace.events, 1);
        assert_eq!(trace.text_parts, 2);
        assert_eq!(trace.text_chars, 11);
        assert_eq!(trace.reasoning_chars, 11);
        assert_eq!(trace.finish_reason.as_deref(), Some("Stop"));
        stub.expect_effort = None;
        assert_eq!(
            execute(&stub, "key-id", "account-A", "gpt-4o", "default", "probe")
                .await
                .unwrap(),
            "hello world"
        );
        stub.finish = FinishReason::Length;
        let (err, err_trace) =
            execute_with_trace(&stub, "key-id", "account-A", "gpt-4o", "default", "probe").await;
        assert_eq!(err, Err(Reason::Incomplete));
        assert_eq!(err_trace.finish_reason.as_deref(), Some("Length"));
        assert!(err_trace.error_detail.unwrap().contains("Length"));
        assert_eq!(
            stub.calls.lock().unwrap().last().unwrap(),
            wire::MODEL_STREAM_CLOSE
        );
        let mut empty_trace = SampleTrace::default();
        assert_eq!(
            VisibleText::default().finish(&mut empty_trace),
            Err(Reason::Incomplete)
        );
        assert!(
            empty_trace
                .error_detail
                .unwrap()
                .contains("completed=false")
        );
    }

    #[tokio::test]
    async fn disabled_target_never_reaches_model_execution() {
        struct Disabled;
        impl Rpc for Disabled {
            async fn send(
                &self,
                method: &str,
                params: Value,
                body: Vec<u8>,
            ) -> Result<HostReply, PluginFault> {
                assert_eq!(method, wire::AUTH_GET_RUNTIME);
                assert_eq!(params, json!({}));
                let request: AuthGetRequest = serde_json::from_slice(&body).unwrap();
                assert_eq!(request.account_id, "account-A");
                Ok(HostReply {
                    result: json!({}),
                    payload: serde_json::to_vec(&AuthRuntimeAccount {
                        account_id: "account-A".into(),
                        provider_id: "openai".into(),
                        credential_revision: 3,
                        name: "acct_01".into(),
                        email: Some("alice@example.com".into()),
                        upstream_user_id: None,
                        upstream_account_id: None,
                        plan_type: Some("pro".into()),
                        authentication_kind: "oauth".into(),
                        enabled: false,
                        credential_state: "ready".into(),
                        has_refresh_token: false,
                        access_token_expires_at_ms: None,
                        next_refresh_at_ms: None,
                    })
                    .unwrap(),
                })
            }
        }
        assert_eq!(
            account_info(&Disabled, "account-A").await.unwrap_err().0,
            Reason::AccountUnavailable
        );
    }

    #[test]
    fn account_identity_ignores_cookie_or_token_revision_bumps_but_detects_replacement() {
        let base = AuthRuntimeAccount {
            account_id: "acct_01".into(),
            provider_id: "openai".into(),
            credential_revision: 5,
            name: "acct_01".into(),
            email: Some("alice@example.com".into()),
            upstream_user_id: Some("user-aaa".into()),
            upstream_account_id: Some("org-111".into()),
            plan_type: Some("team".into()),
            authentication_kind: "oauth".into(),
            enabled: true,
            credential_state: "ready".into(),
            has_refresh_token: true,
            access_token_expires_at_ms: None,
            next_refresh_at_ms: None,
        };
        let mut after_cookie = base.clone();
        after_cookie.credential_revision = 6;
        assert_eq!(
            AccountIdentity::from_runtime(&base).fingerprint(),
            AccountIdentity::from_runtime(&after_cookie).fingerprint()
        );

        let mut replaced = base.clone();
        replaced.credential_revision = 6;
        replaced.upstream_user_id = Some("user-bbb".into());
        assert_ne!(
            AccountIdentity::from_runtime(&base).fingerprint(),
            AccountIdentity::from_runtime(&replaced).fingerprint()
        );
    }

    #[test]
    fn account_choice_prefers_real_email_over_raw_account_id_name() {
        let runtime = AuthRuntimeAccount {
            account_id: "acct_01a17bf1b2ec7659bfd910025398ee7f".into(),
            provider_id: "openai".into(),
            credential_revision: 1,
            name: "acct_01a17bf1b2ec7659bfd910025398ee7f".into(),
            email: Some("alice@example.com".into()),
            upstream_user_id: None,
            upstream_account_id: None,
            plan_type: Some("plus".into()),
            authentication_kind: "oauth".into(),
            enabled: true,
            credential_state: "ready".into(),
            has_refresh_token: true,
            access_token_expires_at_ms: None,
            next_refresh_at_ms: None,
        };
        let choice = AccountChoice::from_runtime(runtime);
        assert_eq!(choice.display, "alice@example.com");
        assert_eq!(choice.email.as_deref(), Some("alice@example.com"));
        assert_eq!(choice.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn codex_catalog_parses_all_models_and_merges_reasoning_efforts() {
        let plus_body = json!({
            "code": 200,
            "message": "OK",
            "data": {
                "modelCount": 3,
                "observedAt": "2026-11-25T00:00:00Z",
                "catalog": {
                    "models": [
                        {
                            "slug": "gpt-6-sol",
                            "display_name": "GPT-6 Sol",
                            "priority": 1,
                            "visibility": "list",
                            "supported_in_api": true,
                            "default_reasoning_level": "medium",
                            "supported_reasoning_levels": [
                                {"effort": "low"},
                                {"effort": "medium"},
                                {"effort": "high"}
                            ]
                        },
                        {
                            "slug": "gpt-4o",
                            "display_name": "GPT-4o",
                            "priority": 5,
                            "visibility": "list",
                            "supported_in_api": true,
                            "supported_reasoning_levels": []
                        },
                        {
                            "slug": "web-only",
                            "display_name": "Web Only",
                            "supported_in_api": false
                        }
                    ]
                }
            }
        });
        let pro_body = json!({
            "code": 200,
            "message": "OK",
            "data": {
                "catalog": {
                    "models": [
                        {
                            "slug": "gpt-6-sol",
                            "display_name": "GPT-6 Sol",
                            "priority": 1,
                            "visibility": "list",
                            "supported_in_api": true,
                            "default_reasoning_level": "high",
                            "supported_reasoning_levels": [
                                {"effort": "low"},
                                {"effort": "medium"},
                                {"effort": "high"},
                                {"effort": "xhigh"}
                            ]
                        },
                        {
                            "slug": "gpt-6.2-sol",
                            "display_name": "GPT-6.2 Sol",
                            "priority": 0,
                            "visibility": "list",
                            "supported_in_api": true,
                            "supported_reasoning_levels": [
                                {"effort": "high"},
                                {"effort": "low"}
                            ]
                        }
                    ]
                }
            }
        });
        let mut merged = Vec::new();
        let mut by_id = BTreeMap::new();
        merge_discovered(
            &mut merged,
            &mut by_id,
            parse_codex_catalog_envelope(&plus_body).unwrap(),
        );
        merge_discovered(
            &mut merged,
            &mut by_id,
            parse_codex_catalog_envelope(&pro_body).unwrap(),
        );
        assert_eq!(merged.len(), 3);
        let sol = merged.iter().find(|m| m.id == "gpt-6-sol").unwrap();
        assert_eq!(sol.efforts, vec!["low", "medium", "high", "xhigh"]);
        assert_eq!(sol.default_effort, "medium");
        let gpt4o = merged.iter().find(|m| m.id == "gpt-4o").unwrap();
        assert_eq!(gpt4o.efforts, vec!["default"]);
        let sol62 = merged.iter().find(|m| m.id == "gpt-6.2-sol").unwrap();
        assert_eq!(sol62.efforts, vec!["low", "high"]);
    }

    #[test]
    fn native_terminal_error_and_rate_limit_are_not_scored() {
        use gateway_plugin_sdk::call::model::WireEvent;
        let mut text = VisibleText::default();
        let mut trace = SampleTrace::default();
        let event = ExecutionEvent::wire(WireEvent {
            protocol: "openai".into(),
            payload: WirePayload::Json {
                event: None,
                data: json!({"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded"}}}),
                id: None,
                retry: None,
                raw_sse: None,
            },
        });
        assert_eq!(text.push(event, &mut trace), Err(Reason::RateLimited));
        assert_eq!(
            trace.wire_terminal.as_deref(),
            Some("response.failed:rate_limit_exceeded")
        );
        let mut e = PluginFault::new(ErrorCode::Capacity, "not evidence");
        e.details = Some(json!({"kind":"rate_limited"}));
        assert_eq!(safe_reason(&e), Reason::RateLimited);
    }

    #[test]
    fn cancellation_and_edited_replies_are_not_evidence() {
        let mut txt = VisibleText::default();
        let mut trace = SampleTrace::default();
        let event = ExecutionEvent {
            host: Some(Box::new(
                gateway_plugin_sdk::call::model::facts::ExecutionFacts {
                    middleware_transformed: true,
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        assert_eq!(txt.push(event, &mut trace), Err(Reason::Transformed));
        assert!(
            trace
                .error_detail
                .unwrap()
                .contains("middleware_transformed")
        );
        assert_eq!(
            safe_reason(&PluginFault::new(ErrorCode::Cancelled, "don't surface me")),
            Reason::Cancelled
        );
    }
}
