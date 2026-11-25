//! State transitions are pure; a host CAS commits a claim before any probe is sent.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MAX_BATCH: usize = 100;
pub const MAX_QUEUED: usize = 200;
pub const MAX_HISTORY: usize = 500;
pub const PROBES_PER_ROUND: u16 = 3;
pub const HOUR_MS: u64 = 3_600_000;
pub const DAY_MS: u64 = 86_400_000;
pub const ROUND_BUDGET_MS: u64 = 600_000;
pub const DEFAULT_EFFORT: &str = "default";
const RETENTION_MS: u64 = 30 * DAY_MS;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountMeta {
    pub email: Option<String>,
    pub name: Option<String>,
}

fn default_effort() -> String {
    DEFAULT_EFFORT.to_owned()
}

pub fn valid_id(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

pub fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':' | b'/'))
}

pub fn valid_effort(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

fn legacy_profile_parts(profile_id: &str) -> (String, String) {
    match profile_id {
        "gpt6-sol-low" => ("gpt-6-sol".into(), "low".into()),
        other => (other.to_owned(), DEFAULT_EFFORT.to_owned()),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub client_key_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    pub account_ids: Vec<String>,
    #[serde(default, skip_serializing, rename = "profileId")]
    pub(crate) legacy_profile_id: Option<String>,
}

impl Target {
    pub fn new(
        client_key_id: String,
        model: String,
        effort: String,
        account_ids: Vec<String>,
    ) -> Self {
        Self {
            client_key_id,
            model,
            effort,
            account_ids,
            legacy_profile_id: None,
        }
    }

    pub fn normalize(&mut self) -> Result<(), Reason> {
        self.migrate_legacy();
        self.model = self.model.trim().to_owned();
        self.effort = self.effort.trim().to_owned();
        if self.effort.is_empty() {
            self.effort = DEFAULT_EFFORT.to_owned();
        }
        if !valid_id(&self.client_key_id, 128)
            || !valid_model_id(&self.model)
            || !valid_effort(&self.effort)
            || !(1..=MAX_BATCH).contains(&self.account_ids.len())
            || self.account_ids.iter().any(|id| !valid_id(id, 128))
        {
            return Err(Reason::InvalidInput);
        }
        self.account_ids.sort_unstable();
        if self.account_ids.windows(2).any(|x| x[0] == x[1]) {
            return Err(Reason::InvalidInput);
        }
        Ok(())
    }

    fn migrate_legacy(&mut self) {
        if let Some(profile_id) = self.legacy_profile_id.take()
            && self.model.is_empty()
        {
            let (model, effort) = legacy_profile_parts(&profile_id);
            self.model = model;
            self.effort = effort;
        }
        if self.effort.is_empty() {
            self.effort = DEFAULT_EFFORT.to_owned();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    pub hours: u16,
    pub target: Option<Target>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            hours: 24,
            target: None,
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) -> Result<(), Reason> {
        if !matches!(self.hours, 1 | 6 | 12 | 24 | 48 | 168)
            || self.enabled && self.target.is_none()
        {
            return Err(Reason::InvalidInput);
        }
        if let Some(target) = &mut self.target {
            target.normalize()?;
        }
        Ok(())
    }

    fn period_ms(&self) -> u64 {
        u64::from(self.hours) * HOUR_MS
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Match,
    Suspect,
    #[serde(alias = "calibration")]
    Ranked,
    Insufficient,
    Failed,
    Interrupted,
    Skipped,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Suspect => "suspect",
            Self::Ranked => "ranked",
            Self::Insufficient => "insufficient",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    InvalidInput,
    Conflict,
    QueueFull,
    // Legacy reason variants kept solely so existing v0.1.0-dev.1 history records deserialize cleanly.
    Budget,
    UnsupportedProfile,
    DetectorUnavailable,
    AccountUnavailable,
    AccountChanged,
    AccessDenied,
    RateLimited,
    Timeout,
    HostFailure,
    Cancelled,
    Incomplete,
    Transformed,
    NotScorable,
    Nearby,
    Inconsistent,
    VersionChanged,
    Restarted,
    ScheduleChanged,
    StaleSlot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability_bps: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ranking {
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub ranking: Ranking,
    pub stamp: String,
    #[serde(default)]
    pub detector_revision: String,
    pub top_margin_bps: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub verdict: Verdict,
    pub reason: Option<Reason>,
    pub last_round: Option<Round>,
}

impl Outcome {
    pub fn no_rank(verdict: Verdict, reason: Reason) -> Self {
        Self {
            verdict,
            reason: Some(reason),
            last_round: None,
        }
    }
}

#[derive(Clone)]
pub enum Decision {
    Finish(Outcome),
    Confirm(Round),
}

/// Evaluates a scored round.
///
/// - When `expected` is `Some(label_id)` (tested model exists in the reference bank):
///   - Top-1 match with positive margin -> `Match`.
///   - Exact tie for #1 or target in #2/#3 -> `Insufficient(Nearby)`.
///   - Target outside Top-3 -> `Confirm` (one independent retest), then `Suspect` only if both
///     rounds agree on the same non-target #1 and omit the expected label from Top-3.
/// - When `expected` is `None` (tested model is not in the reference bank):
///   - Returns `Ranked` with the Top-3 closest reference models and their probabilities.
pub fn decide(expected: Option<&str>, prior: Option<&Round>, round: Round) -> Decision {
    let labels = &round.ranking.candidates;
    let top = labels.first().map(|c| c.id.as_str());
    let Some(expected) = expected else {
        let (verdict, reason) = if top.is_some() {
            (Verdict::Ranked, None)
        } else {
            (Verdict::Insufficient, Some(Reason::NotScorable))
        };
        return Decision::Finish(Outcome {
            verdict,
            reason,
            last_round: Some(round),
        });
    };
    let target_rank = labels.iter().position(|c| c.id == expected);
    let (verdict, reason) = match (prior, top, target_rank) {
        (_, None, _) => (Verdict::Insufficient, Some(Reason::NotScorable)),
        (None, _, Some(0)) if round.top_margin_bps > 0 => (Verdict::Match, None),
        (None, _, Some(_)) => (Verdict::Insufficient, Some(Reason::Nearby)),
        (None, _, None) if labels.len() >= 3 => return Decision::Confirm(round),
        (None, _, None) => (Verdict::Insufficient, Some(Reason::NotScorable)),
        (Some(first), Some(top), None)
            if first.stamp == round.stamp
                && labels.len() >= 3
                && first.ranking.candidates.len() >= 3
                && !first
                    .ranking
                    .candidates
                    .iter()
                    .any(|label| label.id == expected)
                && first
                    .ranking
                    .candidates
                    .first()
                    .is_some_and(|c| c.id == top) =>
        {
            (Verdict::Suspect, None)
        }
        _ => (Verdict::Insufficient, Some(Reason::Inconsistent)),
    };
    Decision::Finish(Outcome {
        verdict,
        reason,
        last_round: Some(round),
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub run_id: String,
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    pub client_key_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default, skip_serializing, rename = "profileId")]
    pub(crate) legacy_profile_id: Option<String>,
    pub scheduled: bool,
    pub queued_at: u64,
    pub claimed: bool,
    pub first_round: Option<Round>,
}

impl Task {
    fn migrate_legacy(&mut self) {
        if let Some(profile_id) = self.legacy_profile_id.take()
            && self.model.is_empty()
        {
            let (model, effort) = legacy_profile_parts(&profile_id);
            self.model = model;
            self.effort = effort;
        }
        if self.effort.is_empty() {
            self.effort = DEFAULT_EFFORT.to_owned();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub run_id: String,
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default, skip_serializing, rename = "profileId")]
    pub(crate) legacy_profile_id: Option<String>,
    pub scheduled: bool,
    pub ended_at: u64,
    pub plugin_version: String,
    pub first_round: Option<Round>,
    pub outcome: Outcome,
}

impl HistoryItem {
    fn migrate_legacy(&mut self) {
        if let Some(profile_id) = self.legacy_profile_id.take()
            && self.model.is_empty()
        {
            let (model, effort) = legacy_profile_parts(&profile_id);
            self.model = model;
            self.effort = effort;
        }
        if self.effort.is_empty() {
            self.effort = DEFAULT_EFFORT.to_owned();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Accepted {
    id: String,
    digest: String,
    at: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub settings: Settings,
    pub next_due_at: Option<u64>,
    pub last_schedule_skip: Option<Reason>,
    pub queue: Vec<Task>,
    pub history: Vec<HistoryItem>,
    #[serde(default)]
    accepted: Vec<Accepted>,
    #[serde(default)]
    schedule_revision: u64,
}

impl State {
    pub fn migrate_legacy(&mut self) {
        if let Some(target) = &mut self.settings.target {
            target.migrate_legacy();
        }
        for task in &mut self.queue {
            task.migrate_legacy();
        }
        for item in &mut self.history {
            item.migrate_legacy();
        }
    }

    pub fn prune(&mut self, now: u64) {
        self.history
            .retain(|h| now.saturating_sub(h.ended_at) < RETENTION_MS);
        self.accepted.retain(|a| {
            now.saturating_sub(a.at) < DAY_MS || self.queue.iter().any(|q| q.run_id == a.id)
        });
    }

    pub fn schedule(&mut self, new: Settings, now: u64, version: &str) {
        if self.settings == new {
            return;
        }
        self.settings = new;
        self.schedule_revision = self.schedule_revision.saturating_add(1);
        self.last_schedule_skip = None;
        self.next_due_at = self
            .settings
            .enabled
            .then(|| now.saturating_add(self.settings.period_ms()));
        self.finish_pending(now, version, |q| q.scheduled, Reason::ScheduleChanged);
    }

    pub fn is_schedule_due(&self, now: u64) -> bool {
        self.settings.enabled
            && self.settings.target.is_some()
            && self.next_due_at.is_some_and(|due| now >= due)
    }

    pub fn schedule_due(
        &mut self,
        now: u64,
        version: &str,
        labels: &BTreeMap<String, AccountMeta>,
    ) {
        let (Some(due), Some(target)) = (self.next_due_at, self.settings.target.clone()) else {
            return;
        };
        if !self.settings.enabled || now < due {
            return;
        }
        // Skip all older periods; this tick schedules at most one batch.
        let period = self.settings.period_ms();
        let slot = now - (now - due) % period;
        self.next_due_at = Some(slot.saturating_add(period));
        let id = format!("s-{}-{slot}", self.schedule_revision);
        self.finish_pending(
            now,
            version,
            |q| q.scheduled && q.run_id != id,
            Reason::StaleSlot,
        );
        self.last_schedule_skip = self.enqueue(id, target, true, now, labels).err();
    }

    pub fn enqueue_manual(
        &mut self,
        id: String,
        target: Target,
        now: u64,
        labels: &BTreeMap<String, AccountMeta>,
    ) -> Result<(), Reason> {
        if !id.starts_with("m-") || id.len() < 10 || !valid_id(&id, 64) {
            return Err(Reason::InvalidInput);
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&target).map_err(|_| Reason::InvalidInput)?)
        );
        if let Some(old) = self.accepted.iter().find(|a| a.id == id) {
            return if old.digest == digest {
                Ok(())
            } else {
                Err(Reason::Conflict)
            };
        }
        if self.accepted.len() >= MAX_HISTORY {
            self.accepted.remove(0);
        }
        self.enqueue(id.clone(), target, false, now, labels)?;
        self.accepted.push(Accepted {
            id,
            digest,
            at: now,
        });
        Ok(())
    }

    fn enqueue(
        &mut self,
        id: String,
        target: Target,
        scheduled: bool,
        now: u64,
        labels: &BTreeMap<String, AccountMeta>,
    ) -> Result<(), Reason> {
        if self.queue.len() + target.account_ids.len() > MAX_QUEUED {
            return Err(Reason::QueueFull);
        }
        if self.queue.iter().any(|q| {
            q.model == target.model
                && q.effort == target.effort
                && target.account_ids.contains(&q.account_id)
        }) {
            return Err(Reason::Conflict);
        }
        for account_id in target.account_ids {
            let meta = labels.get(&account_id);
            self.queue.push(Task {
                run_id: id.clone(),
                account_id,
                account_email: meta.and_then(|m| m.email.clone()),
                account_name: meta.and_then(|m| m.name.clone()),
                client_key_id: target.client_key_id.clone(),
                model: target.model.clone(),
                effort: target.effort.clone(),
                legacy_profile_id: None,
                scheduled,
                queued_at: now,
                claimed: false,
                first_round: None,
            });
        }
        Ok(())
    }

    pub fn cancel_pending(&mut self, run_id: Option<&str>) -> usize {
        let before = self.queue.len();
        self.queue
            .retain(|q| q.claimed || run_id.is_some_and(|target| q.run_id != target));
        before - self.queue.len()
    }

    /// The host serializes reconciles within one instance. A surviving claim is therefore an
    /// interrupted call, not a lease to reclaim. Never replay a request that may have been sent.
    pub fn recover(&mut self, now: u64, version: &str) {
        while let Some(index) = self.queue.iter().position(|q| q.claimed) {
            self.finish_at(
                index,
                Outcome::no_rank(Verdict::Interrupted, Reason::Restarted),
                now,
                version,
                None,
            );
        }
    }

    pub fn claim(&mut self) -> Option<Task> {
        if self.queue.iter().any(|q| q.claimed) {
            return None;
        }
        let next = self.queue.first_mut()?;
        next.claimed = true;
        Some(next.clone())
    }

    pub fn complete(
        &mut self,
        task: &Task,
        decision: Decision,
        now: u64,
        version: &str,
        refreshed_meta: Option<AccountMeta>,
    ) {
        let Some(index) = self.queue.iter().position(|q| {
            q.claimed
                && q.run_id == task.run_id
                && q.account_id == task.account_id
                && q.model == task.model
                && q.effort == task.effort
        }) else {
            return;
        };
        if let Some(meta) = &refreshed_meta {
            if meta.email.is_some() {
                self.queue[index].account_email = meta.email.clone();
            }
            if meta.name.is_some() {
                self.queue[index].account_name = meta.name.clone();
            }
        }
        match decision {
            Decision::Finish(outcome) => {
                self.finish_at(index, outcome, now, version, refreshed_meta)
            }
            Decision::Confirm(round) => {
                self.queue[index].first_round = Some(round);
                if task.scheduled
                    && (!self.settings.enabled
                        || !task
                            .run_id
                            .starts_with(&format!("s-{}-", self.schedule_revision)))
                {
                    self.finish_at(
                        index,
                        Outcome::no_rank(Verdict::Skipped, Reason::ScheduleChanged),
                        now,
                        version,
                        refreshed_meta,
                    );
                } else {
                    self.queue[index].claimed = false;
                }
            }
        }
    }

    fn finish_pending(
        &mut self,
        at: u64,
        version: &str,
        matches: impl Fn(&Task) -> bool,
        reason: Reason,
    ) {
        while let Some(index) = self.queue.iter().position(|q| !q.claimed && matches(q)) {
            self.finish_at(
                index,
                Outcome::no_rank(Verdict::Skipped, reason),
                at,
                version,
                None,
            );
        }
    }

    fn finish_at(
        &mut self,
        index: usize,
        outcome: Outcome,
        now: u64,
        version: &str,
        refreshed_meta: Option<AccountMeta>,
    ) {
        let task = self.queue.remove(index);
        let account_email = refreshed_meta
            .as_ref()
            .and_then(|m| m.email.clone())
            .or(task.account_email);
        let account_name = refreshed_meta
            .as_ref()
            .and_then(|m| m.name.clone())
            .or(task.account_name);
        self.history.insert(
            0,
            HistoryItem {
                run_id: task.run_id,
                account_id: task.account_id,
                account_email,
                account_name,
                model: task.model,
                effort: task.effort,
                legacy_profile_id: None,
                scheduled: task.scheduled,
                ended_at: now,
                plugin_version: version.to_owned(),
                first_round: task.first_round,
                outcome,
            },
        );
        self.history.truncate(MAX_HISTORY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(ids: &[&str], margin: u16) -> Round {
        Round {
            stamp: "locked-bank".into(),
            detector_revision: "fixture".into(),
            top_margin_bps: margin,
            ranking: Ranking {
                candidates: ids
                    .iter()
                    .map(|id| Candidate {
                        id: (*id).into(),
                        label: (*id).into(),
                        probability_bps: Some(5000),
                    })
                    .collect(),
            },
        }
    }

    fn target(accounts: &[&str]) -> Target {
        Target::new(
            "key".into(),
            "gpt-6-sol".into(),
            "low".into(),
            accounts.iter().map(|a| (*a).into()).collect(),
        )
    }

    fn outcome(decision: Decision) -> Verdict {
        match decision {
            Decision::Finish(outcome) => outcome.verdict,
            _ => panic!("unexpected confirmation"),
        }
    }

    #[test]
    fn concordant_miss_match_nearby_and_unlisted_ranked() {
        let Decision::Confirm(first) =
            decide(Some("expected"), None, round(&["A", "B", "C"], 1500))
        else {
            panic!()
        };
        assert_eq!(
            outcome(decide(
                Some("expected"),
                Some(&first),
                round(&["A", "C", "B"], 1500)
            )),
            Verdict::Suspect
        );
        assert_eq!(
            outcome(decide(
                Some("expected"),
                Some(&first),
                round(&["B", "C", "A"], 1500)
            )),
            Verdict::Insufficient
        );
        assert_eq!(
            outcome(decide(
                Some("expected"),
                Some(&first),
                round(&["A", "expected", "C"], 1500)
            )),
            Verdict::Insufficient
        );
        assert_eq!(
            outcome(decide(
                Some("expected"),
                None,
                round(&["expected", "B", "C"], 1500)
            )),
            Verdict::Match
        );
        // Zero margin tie for #1 stays Insufficient(Nearby) while preserving ranking.
        assert_eq!(
            outcome(decide(
                Some("expected"),
                None,
                round(&["expected", "B", "C"], 0)
            )),
            Verdict::Insufficient
        );
        // Models outside the reference bank return Ranked without confirmation.
        assert_eq!(
            outcome(decide(None, None, round(&["A", "B", "C"], 1500))),
            Verdict::Ranked
        );
    }

    #[test]
    fn restart_never_recharges_unknown_attempt() {
        let mut s = State::default();
        let labels = BTreeMap::from([(
            "A".into(),
            AccountMeta {
                email: Some("alice@example.com".into()),
                name: Some("Alice".into()),
            },
        )]);
        s.enqueue_manual("m-batch001".into(), target(&["A", "B"]), 0, &labels)
            .unwrap();
        assert_eq!(s.claim().unwrap().account_id, "A");
        let mut s: State = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        s.recover(20, "0.1.1");
        assert_eq!(s.history[0].outcome.verdict, Verdict::Interrupted);
        assert_eq!(
            s.history[0].account_email.as_deref(),
            Some("alice@example.com")
        );
        assert_eq!(s.claim().unwrap().account_id, "B");
    }

    #[test]
    fn schedule_is_once_per_slot_no_catchup_and_can_disable() {
        let mut s = State::default();
        let settings = Settings {
            enabled: true,
            hours: 24,
            target: Some(target(&["A"])),
        };
        s.schedule(settings, 0, "v");
        let labels = BTreeMap::new();
        s.schedule_due(3 * DAY_MS + 27, "v", &labels);
        s.schedule_due(3 * DAY_MS + 28, "v", &labels);
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.next_due_at, Some(4 * DAY_MS));
        s.schedule(Settings::default(), 3 * DAY_MS + 29, "v");
        assert!(s.queue.is_empty());
        assert_eq!(s.history[0].outcome.reason, Some(Reason::ScheduleChanged));
        s.schedule_due(4 * DAY_MS, "v", &labels);
        assert!(s.queue.is_empty());
    }

    #[test]
    fn new_slot_drops_old_queued_work_and_cancels_retest_on_change() {
        let mut s = State::default();
        let labels = BTreeMap::new();
        s.schedule(
            Settings {
                enabled: true,
                hours: 24,
                target: Some(target(&["A"])),
            },
            0,
            "v",
        );
        s.schedule_due(DAY_MS, "v", &labels);
        let run_id = s.queue[0].run_id.clone();
        s.schedule_due(2 * DAY_MS, "v", &labels);
        assert_eq!(s.queue.len(), 1);
        assert_ne!(s.queue[0].run_id, run_id);
        assert_eq!(s.history[0].outcome.reason, Some(Reason::StaleSlot));
        let active = s.claim().unwrap();
        let replacement = Settings {
            enabled: true,
            hours: 48,
            target: Some(target(&["A"])),
        };
        s.schedule(replacement, 2 * DAY_MS + 2, "v");
        s.complete(
            &active,
            Decision::Confirm(round(&["B", "C", "D"], 1200)),
            2 * DAY_MS + 3,
            "v",
            None,
        );
        assert!(s.queue.is_empty());
        assert_eq!(s.history[0].outcome.reason, Some(Reason::ScheduleChanged));
    }

    #[test]
    fn manual_nonce_idempotent_conflict_and_cancel_pending() {
        let mut s = State::default();
        let labels = BTreeMap::new();
        for _ in 0..2 {
            s.enqueue_manual("m-idempotent".into(), target(&["A", "B"]), 0, &labels)
                .unwrap();
        }
        assert_eq!(s.queue.len(), 2);
        assert_eq!(
            s.enqueue_manual("m-idempotent".into(), target(&["C"]), 0, &labels),
            Err(Reason::Conflict)
        );
        let _active = s.claim().unwrap();
        assert_eq!(s.cancel_pending(None), 1);
        assert_eq!(s.queue.len(), 1);
        assert!(s.queue[0].claimed);
    }

    #[test]
    fn legacy_v010_dev1_state_deserializes_and_migrates_cleanly() {
        let legacy = serde_json::json!({
            "settings": {
                "enabled": false,
                "hours": 24,
                "probeBudget": 60,
                "target": {
                    "clientKeyId": "key_1",
                    "profileId": "gpt6-sol-low",
                    "accountIds": ["acct_1"]
                }
            },
            "nextDueAt": null,
            "lastScheduleSkip": "budget",
            "queue": [],
            "history": [{
                "runId": "m-legacy001",
                "accountId": "acct_1",
                "profileId": "gpt6-sol-low",
                "mode": "calibration",
                "scheduled": false,
                "endedAt": 1795619000000_u64,
                "pluginVersion": "0.1.0-dev.1",
                "firstRound": null,
                "outcome": {
                    "verdict": "calibration",
                    "reason": null,
                    "lastRound": {
                        "ranking": {
                            "candidates": [
                                {"id": "gpt-6-sol", "label": "gpt-6-sol"}
                            ]
                        },
                        "stamp": "old-stamp",
                        "detectorRevision": "rev",
                        "topMarginBps": 1200
                    }
                }
            }],
            "accepted": [],
            "reservations": [1795619300000_u64],
            "scheduleRevision": 2
        });
        let mut state: State = serde_json::from_value(legacy).unwrap();
        state.migrate_legacy();
        let target = state.settings.target.as_ref().unwrap();
        assert_eq!(target.model, "gpt-6-sol");
        assert_eq!(target.effort, "low");
        assert_eq!(state.history[0].model, "gpt-6-sol");
        assert_eq!(state.history[0].effort, "low");
        assert_eq!(state.history[0].outcome.verdict, Verdict::Ranked);
        let reencoded = serde_json::to_value(&state).unwrap();
        assert!(reencoded["settings"].get("probeBudget").is_none());
        assert!(reencoded.get("reservations").is_none());
        assert!(reencoded["history"][0].get("profileId").is_none());
    }
}
