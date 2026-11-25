//! Management handlers and the single maintenance worker share the same persisted queue.
use crate::{
    domain::{self, Decision, Mode, Outcome, Reason, Settings, State, Target, Task, Verdict},
    fingerprint::Fingerprint,
    host::{Host, safe_reason},
};
use gateway_plugin_sdk::{
    PluginFault,
    call::management::{
        ManagementPage, ManagementRegistration, ManagementRequest, ManagementResource,
        ManagementResponse, ManagementRoute,
    },
    client::{Empty, TypedCall, TypedReply},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const JSON: &str = "application/json";
const MAX_REQUEST: usize = 8 * 1024;

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
            description: Some("按指定 Codex 账号比较模型参考指纹".into()),
            entry: "web/index.html".into(),
            icon: None,
        }],
        callbacks: vec![],
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Submit {
    run_id: String,
    target: Target,
    #[serde(default)]
    mode: Mode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditSchedule {
    settings: Settings,
    previous: Settings,
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
                Reason::Budget | Reason::RateLimited => 429,
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
}

impl Service {
    pub fn new(version: String, bank: Option<Fingerprint>) -> Self {
        Self {
            version,
            bank,
            maintenance: Mutex::new(()),
        }
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

    async fn route(
        &self,
        host: &Host<'_>,
        request: &ManagementRequest,
        payload: &[u8],
    ) -> Result<(u16, Value), HttpError> {
        if !request.query.is_empty() || payload.len() > MAX_REQUEST {
            return Err(Reason::InvalidInput.into());
        }
        let path = request.path.as_str();
        match (request.method.as_str(), path) {
            ("GET", "catalog") => {
                let (accounts, keys) = host.catalog().await?;
                Ok((
                    200,
                    json!({"accounts":accounts,"keys":keys,"detectorReady":self.bank.is_some(),
                    "detectorRevision":self.bank.as_ref().map(|b| &b.revision), "pluginVersion":self.version,
                    "profiles":self.bank.as_ref().map(Fingerprint::options).unwrap_or_default(), "maxBatch":domain::MAX_BATCH}),
                ))
            }
            ("GET", "runs") => {
                let (mut state, _) = host.load().await.map_err(|e| safe_reason(&e))?;
                state.prune(now());
                let queue: Vec<_> = state.queue.iter().map(|q| json!({"runId":q.run_id,"accountId":q.account_id,
                    "profileId":q.profile_id,"mode":q.mode,"scheduled":q.scheduled,"queuedAt":q.queued_at,
                    "running":q.claimed,"round": if q.first_round.is_some() {2} else {1}})).collect();
                Ok((
                    200,
                    json!({"queue":queue,"history":state.history,"remaining":state.remaining(),"budget":state.settings.probe_budget}),
                ))
            }
            ("GET", "schedule") => {
                let (state, _) = host.load().await.map_err(|e| safe_reason(&e))?;
                Ok((200, schedule_value(&state)))
            }
            ("POST", "runs") => {
                let mut submit: Submit = parse(payload)?;
                submit.target.normalize()?;
                let model = &self
                    .bank
                    .as_ref()
                    .ok_or(Reason::DetectorUnavailable)?
                    .profile(&submit.target.profile_id, submit.mode)?
                    .model;
                host.access(
                    &submit.target.client_key_id,
                    &submit.target.account_ids,
                    model,
                )
                .await?;
                let at = now();
                let result = host
                    .change(|state, _| {
                        state.prune(at);
                        state.enqueue_manual(
                            submit.run_id.clone(),
                            submit.target.clone(),
                            submit.mode,
                            at,
                        )
                    })
                    .await
                    .map_err(|e| safe_reason(&e))?;
                result?;
                let base = submit.target.account_ids.len() * domain::PROBES_PER_ROUND as usize;
                Ok((
                    202,
                    json!({"runId":submit.run_id,"probesMin":base,"probesMax":if submit.mode == Mode::Detect {2*base} else {base}}),
                ))
            }
            ("PUT", "schedule") => {
                let mut edit: EditSchedule = parse(payload)?;
                edit.settings.normalize()?;
                edit.previous.normalize()?;
                if edit.settings.enabled {
                    let target = edit.settings.target.as_ref().ok_or(Reason::InvalidInput)?;
                    let model = &self
                        .bank
                        .as_ref()
                        .ok_or(Reason::DetectorUnavailable)?
                        .profile(&target.profile_id, Mode::Detect)?
                        .model;
                    host.access(&target.client_key_id, &target.account_ids, model)
                        .await?;
                }
                let at = now();
                let value = host
                    .change(|state, _| {
                        if state.settings != edit.previous && state.settings != edit.settings {
                            return Err(Reason::Conflict);
                        }
                        state.prune(at);
                        state.schedule(edit.settings.clone(), at, &self.version);
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
        let task = host
            .change(|state, _| {
                state.prune(at);
                // An earlier committed claim still present after its maintenance scope ended is never replayed.
                state.recover(at, &self.version);
                state.schedule_due(at, &self.version);
                state.claim(at, &self.version)
            })
            .await?;
        if let Some(task) = task {
            let result = tokio::select! {
                biased;
                () = call.cancellation.cancelled() => Decision::Finish(failure(Reason::Cancelled)),
                result = self.run_round(&host, &task) => result,
            };
            // CAS may retry this *settlement*, but never rerun a model request.
            let at = now();
            host.change(|state, _| state.complete(&task, result.clone(), at, &self.version))
                .await?;
        }
        Ok(TypedReply::new(Empty {}))
    }

    async fn run_round(&self, host: &Host<'_>, task: &Task) -> Decision {
        match self.try_round(host, task).await {
            Ok(result) => result,
            Err(error) => Decision::Finish(failure(error)),
        }
    }

    async fn try_round(&self, host: &Host<'_>, task: &Task) -> Result<Decision, Reason> {
        let bank = self.bank.as_ref().ok_or(Reason::DetectorUnavailable)?;
        let p = bank.profile(&task.profile_id, task.mode)?;
        let revision = host.account_revision(&task.account_id).await?;
        let stamp = p.round_stamp(revision);
        if task.first_round.as_ref().is_some_and(|r| r.stamp != stamp) {
            return Err(Reason::VersionChanged);
        }
        host.access(
            &task.client_key_id,
            std::slice::from_ref(&task.account_id),
            &p.model,
        )
        .await?;
        let mut outputs = Vec::with_capacity(domain::PROBES_PER_ROUND as usize);
        for c in bank.challenges()? {
            let text = host
                .sample(
                    &task.client_key_id,
                    &task.account_id,
                    &p.model,
                    &p.effort,
                    &c.prompt,
                )
                .await?;
            outputs.push(lmfpd::Output {
                text,
                expected_count: c.expected_count,
            });
        }
        if host.account_revision(&task.account_id).await? != revision {
            return Err(Reason::AccountChanged);
        }
        let round = bank.rank(&outputs, stamp)?;
        if task.mode == Mode::Detect
            && p.minimum_margin()
                .is_none_or(|threshold| round.top_margin_bps < threshold)
        {
            return Ok(Decision::Finish(Outcome {
                verdict: Verdict::Insufficient,
                reason: Some(Reason::Nearby),
                last_round: Some(round),
            }));
        }
        Ok(domain::decide(
            task.mode,
            &p.label_id,
            task.first_round.as_ref(),
            round,
        ))
    }
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Reason> {
    serde_json::from_slice(bytes).map_err(|_| Reason::InvalidInput)
}

fn schedule_value(state: &State) -> Value {
    json!({"settings":state.settings,"nextDueAt":state.next_due_at,"lastSkip":state.last_schedule_skip})
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
}
