//! CPR wire adapter. No raw credential, generic HTTP, or data-plane mutations are used.
use crate::{
    domain::{Reason, State},
    state_codec,
};
use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::{
        data::AccessQuery,
        host::{self as wire, *},
        model::{CanonicalEvent, ExecutionEvent, FinishReason, WirePayload},
    },
    client::{HostClient, HostReply, SessionError},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, time::Duration};

const PROVIDER: &str = "openai";
const NAMESPACE: &str = "iq-test";
const STATE_KEY: &str = "state";
const MODEL_TIMEOUT: Duration = Duration::from_secs(85);
const MAX_TEXT: usize = 64 * 1024;
const MAX_PARTS: usize = 32;
const READ_BATCHES: usize = 4096;

fn fault() -> PluginFault {
    PluginFault::new(ErrorCode::Fault, "iq-test: invalid host result")
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    document: String,
}

pub struct Host<'a>(pub &'a HostClient);

impl Host<'_> {
    pub async fn catalog(&self) -> Result<(Vec<Choice>, Vec<Choice>), Reason> {
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
            accounts.extend(page.accounts.into_iter().filter(eligible).map(|a| Choice {
                id: a.account_id,
                name: a.name.chars().take(120).collect(),
            }));
            match page.next_cursor {
                None => break,
                Some(next) if cursor.as_ref().is_none_or(|old| old < &next) => cursor = Some(next),
                _ => return Err(Reason::HostFailure),
            }
        }
        cursor = None;
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
                        name: key.name.chars().take(120).collect(),
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

    pub async fn account_revision(&self, id: &str) -> Result<u64, Reason> {
        account_revision(self.0, id).await
    }

    pub async fn access(&self, key: &str, ids: &[String], model: &str) -> Result<(), Reason> {
        let access = self
            .0
            .check_access(AccessQuery {
                client_key_id: key.into(),
                account_ids: ids.to_vec(),
                upstream_model: Some(model.into()),
            })
            .await
            .map_err(|e| safe_reason(&e))?;
        if access.client_key_id != key
            || !ids.iter().all(|id| {
                access
                    .accounts
                    .iter()
                    .any(|a| a.account_id == *id && a.allowed)
            })
        {
            return Err(Reason::AccessDenied);
        }
        Ok(())
    }

    pub async fn sample(
        &self,
        key: &str,
        account: &str,
        model: &str,
        effort: &str,
        prompt: &str,
    ) -> Result<String, Reason> {
        execute(self.0, key, account, model, effort, prompt).await
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
            let result: Result<StatePutResult, _> = control(
                self.0,
                wire::STATE_PUT,
                StatePutRequest {
                    namespace: NAMESPACE.into(),
                    key: STATE_KEY.into(),
                    value: json!({"document":state_codec::encode(&state).ok_or_else(fault)?}),
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

fn eligible(account: &AuthRuntimeAccount) -> bool {
    account.provider_id == PROVIDER && account.authentication_kind == "oauth" && account.enabled
}

async fn account_revision(rpc: &impl Rpc, id: &str) -> Result<u64, Reason> {
    let account: AuthRuntimeAccount = payload(
        rpc,
        wire::AUTH_GET_RUNTIME,
        AuthGetRequest {
            account_id: id.into(),
        },
    )
    .await
    .map_err(|e| safe_reason(&e))?;
    if account.account_id != id || !eligible(&account) {
        return Err(Reason::AccountUnavailable);
    }
    Ok(account.credential_revision)
}

/// The only model submission site. Both the provider and the required account are always present.
async fn execute(
    rpc: &impl Rpc,
    key: &str,
    account: &str,
    model: &str,
    effort: &str,
    prompt: &str,
) -> Result<String, Reason> {
    let params = serde_json::to_value(ModelExecuteRequest {
        client_key_id: Some(key.into()),
        model: model.into(),
        protocol: PROVIDER.into(),
        operation: ModelOperation::Generate,
        provider: Some(PROVIDER.into()),
        account_id: Some(account.into()),
        previous_response_id: None,
    })
    .map_err(|_| Reason::HostFailure)?;
    let body = serde_json::to_vec(
        &json!({"model":model,"input":[{"role":"user","content":prompt}],
        "stream":true,"store":false,"reasoning":{"effort":effort}}),
    )
    .map_err(|_| Reason::HostFailure)?;
    let submit_at = tokio::time::Instant::now();
    let deadline = submit_at + MODEL_TIMEOUT;
    let reply =
        tokio::time::timeout_at(deadline, rpc.send(wire::MODEL_EXECUTE_STREAM, params, body))
            .await
            .map_err(|_| Reason::Timeout)?
            .map_err(|e| safe_reason(&e))?;
    let handle: ModelStreamResult =
        serde_json::from_value(reply.result).map_err(|_| Reason::HostFailure)?;
    let result = if reply.payload.is_empty() {
        tokio::time::timeout_at(deadline, read_stream(rpc, &handle.stream))
            .await
            .unwrap_or(Err(Reason::Timeout))
    } else {
        Err(Reason::HostFailure)
    };
    if result.is_err() {
        let params = json!({"stream":handle.stream});
        // On parent cancellation, the host also tears down every child stream it owns.
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            rpc.send(wire::MODEL_STREAM_CLOSE, params, vec![]),
        )
        .await;
    }
    result
}

async fn read_stream(rpc: &impl Rpc, stream: &str) -> Result<String, Reason> {
    let mut text = VisibleText::default();
    let params = json!({"stream":stream,"maximum_bytes":1048576});
    for _ in 0..READ_BATCHES {
        let reply = rpc
            .send(wire::MODEL_STREAM_READ, params.clone(), vec![])
            .await
            .map_err(|e| safe_reason(&e))?;
        let read: ModelStreamReadResult =
            serde_json::from_value(reply.result).map_err(|_| Reason::Incomplete)?;
        if read.events != 0 || !reply.payload.is_empty() {
            let batch = ModelEventBatch::decode(&reply.payload).map_err(|_| Reason::Incomplete)?;
            if batch.events.len() != read.events as usize {
                return Err(Reason::Incomplete);
            }
            for event in batch.events {
                text.push(event)?;
            }
        } else if !read.end {
            return Err(Reason::Incomplete);
        }
        if read.end {
            return text.finish();
        }
    }
    Err(Reason::Incomplete)
}

#[derive(Default)]
struct VisibleText {
    parts: BTreeMap<u32, String>,
    bytes: usize,
    completed: bool,
}

impl VisibleText {
    fn push(&mut self, event: ExecutionEvent) -> Result<(), Reason> {
        if event.host.is_some_and(|host| host.middleware_transformed) {
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
                        return Err(match code {
                            Some("rate_limit_exceeded" | "rate_limited" | "insufficient_quota") => {
                                Reason::RateLimited
                            }
                            Some("timeout" | "request_timeout") => Reason::Timeout,
                            _ => Reason::HostFailure,
                        });
                    }
                    "response.incomplete" | "response.refusal.delta" | "response.refusal.done" => {
                        return Err(Reason::Incomplete);
                    }
                    _ => {}
                }
            }
        }
        for fact in event.facts {
            match fact {
                CanonicalEvent::TextDelta { index, text }
                    if !self.completed && index < MAX_PARTS as u32 =>
                {
                    self.bytes = self.bytes.saturating_add(text.len());
                    if self.bytes > MAX_TEXT {
                        return Err(Reason::Incomplete);
                    }
                    self.parts.entry(index).or_default().push_str(&text);
                }
                CanonicalEvent::Completed {
                    reason: FinishReason::Stop,
                    ..
                } if !self.completed => self.completed = true,
                CanonicalEvent::Completed { .. }
                | CanonicalEvent::TextDelta { .. }
                | CanonicalEvent::ToolCallDelta { .. } => {
                    return Err(Reason::Incomplete);
                }
                _ => {} // usage, metadata and reasoning are not fingerprint evidence
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<String, Reason> {
        let visible: String = self.parts.into_values().collect();
        if !self.completed || visible.trim().is_empty() {
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
                    assert_eq!(value["reasoning"]["effort"], "low");
                    assert_eq!(value["store"], false);
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
                                CanonicalEvent::TextDelta {
                                    index: 1,
                                    text: "world".into(),
                                },
                                CanonicalEvent::TextDelta {
                                    index: 0,
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
            };
            assert_eq!(
                execute(&stub, "key-id", "account-A", "gpt", "low", "probe").await,
                Err(reason)
            );
            assert_eq!(*stub.calls.lock().unwrap(), [wire::MODEL_EXECUTE_STREAM]);
        }
    }

    #[tokio::test]
    async fn only_complete_visible_text_is_returned() {
        let mut stub = Stub {
            calls: Mutex::default(),
            status: None,
            finish: FinishReason::Stop,
        };
        assert_eq!(
            execute(&stub, "key-id", "account-A", "gpt", "low", "probe")
                .await
                .unwrap(),
            "hello world"
        );
        stub.finish = FinishReason::Length;
        assert_eq!(
            execute(&stub, "key-id", "account-A", "gpt", "low", "probe").await,
            Err(Reason::Incomplete)
        );
        assert_eq!(
            stub.calls.lock().unwrap().last().unwrap(),
            wire::MODEL_STREAM_CLOSE
        );
        assert_eq!(VisibleText::default().finish(), Err(Reason::Incomplete));
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
                        name: "fixture".into(),
                        email: None,
                        upstream_user_id: None,
                        upstream_account_id: None,
                        plan_type: None,
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
            account_revision(&Disabled, "account-A").await,
            Err(Reason::AccountUnavailable)
        );
    }

    #[test]
    fn native_terminal_error_and_rate_limit_are_not_scored() {
        use gateway_plugin_sdk::call::model::WireEvent;
        let mut text = VisibleText::default();
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
        assert_eq!(text.push(event), Err(Reason::RateLimited));
        let mut e = PluginFault::new(ErrorCode::Capacity, "not evidence");
        e.details = Some(json!({"kind":"rate_limited"}));
        assert_eq!(safe_reason(&e), Reason::RateLimited);
    }

    #[test]
    fn cancellation_and_edited_replies_are_not_evidence() {
        let mut txt = VisibleText::default();
        let event = ExecutionEvent {
            host: Some(Box::new(
                gateway_plugin_sdk::call::model::facts::ExecutionFacts {
                    middleware_transformed: true,
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        assert_eq!(txt.push(event), Err(Reason::Transformed));
        assert_eq!(
            safe_reason(&PluginFault::new(ErrorCode::Cancelled, "don't surface me")),
            Reason::Cancelled
        );
    }
}
