//! State transitions are pure; a host CAS commits a claim before any probe is sent.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_BATCH: usize = 10;
pub const MAX_QUEUED: usize = 30;
pub const MAX_HISTORY: usize = 100;
pub const DEFAULT_BUDGET: u16 = 60;
pub const PROBES_PER_ROUND: u16 = 3;
pub const DAY_MS: u64 = 86_400_000;
pub const ROUND_BUDGET_MS: u64 = 300_000;
const RETENTION_MS: u64 = 30 * DAY_MS;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Detect,
    Calibration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub client_key_id: String,
    pub profile_id: String,
    pub account_ids: Vec<String>,
}

fn valid_id(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

impl Target {
    pub fn normalize(&mut self) -> Result<(), Reason> {
        if !valid_id(&self.client_key_id, 128)
            || !valid_id(&self.profile_id, 64)
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub hours: u16,
    pub probe_budget: u16,
    pub target: Option<Target>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            hours: 24,
            probe_budget: DEFAULT_BUDGET,
            target: None,
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) -> Result<(), Reason> {
        if !matches!(self.hours, 24 | 48 | 168)
            || !(PROBES_PER_ROUND..=DEFAULT_BUDGET).contains(&self.probe_budget)
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
        u64::from(self.hours) * 3_600_000
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Match,
    Suspect,
    Insufficient,
    Failed,
    Interrupted,
    Skipped,
    Calibration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    InvalidInput,
    Conflict,
    QueueFull,
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

/// Requires two independent, concordant misses. Closed-set scores are not confidence in identity.
pub fn decide(mode: Mode, expected: &str, prior: Option<&Round>, round: Round) -> Decision {
    let labels = &round.ranking.candidates;
    let top = labels.first().map(|c| c.id.as_str());
    let target_rank = labels.iter().position(|c| c.id == expected);
    let (verdict, reason) = match (mode, prior, top, target_rank) {
        (Mode::Calibration, _, _, _) => (Verdict::Calibration, None),
        (_, _, None, _) => (Verdict::Insufficient, Some(Reason::NotScorable)),
        (_, None, _, Some(0)) => (Verdict::Match, None),
        (_, None, _, Some(_)) => (Verdict::Insufficient, Some(Reason::Nearby)),
        (_, None, _, None) if labels.len() >= 3 => return Decision::Confirm(round),
        (_, None, _, None) => (Verdict::Insufficient, Some(Reason::NotScorable)),
        (_, Some(first), Some(top), None)
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
    pub client_key_id: String,
    pub profile_id: String,
    pub mode: Mode,
    pub scheduled: bool,
    pub queued_at: u64,
    pub claimed: bool,
    pub first_round: Option<Round>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub run_id: String,
    pub account_id: String,
    pub profile_id: String,
    pub mode: Mode,
    pub scheduled: bool,
    pub ended_at: u64,
    pub plugin_version: String,
    pub first_round: Option<Round>,
    pub outcome: Outcome,
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
    accepted: Vec<Accepted>,
    reservations: Vec<u64>,
    schedule_revision: u64,
}

impl State {
    pub fn prune(&mut self, now: u64) {
        self.reservations
            .retain(|until| now.saturating_sub(*until) < DAY_MS);
        self.history
            .retain(|h| now.saturating_sub(h.ended_at) < RETENTION_MS);
        self.accepted.retain(|a| {
            now.saturating_sub(a.at) < DAY_MS || self.queue.iter().any(|q| q.run_id == a.id)
        });
    }

    pub fn remaining(&self) -> u16 {
        self.settings
            .probe_budget
            .saturating_sub((self.reservations.len() as u16) * PROBES_PER_ROUND)
    }

    pub fn schedule(&mut self, new: Settings, now: u64, version: &str) {
        if self.settings == new {
            return;
        }
        let plan_changed = (
            self.settings.enabled,
            self.settings.hours,
            &self.settings.target,
        ) != (new.enabled, new.hours, &new.target);
        self.settings = new;
        if !plan_changed {
            return;
        }
        self.schedule_revision = self.schedule_revision.saturating_add(1);
        self.last_schedule_skip = None;
        self.next_due_at = self
            .settings
            .enabled
            .then(|| now.saturating_add(self.settings.period_ms()));
        self.finish_pending(now, version, |q| q.scheduled, Reason::ScheduleChanged);
    }

    pub fn schedule_due(&mut self, now: u64, version: &str) {
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
        self.last_schedule_skip = self.enqueue(id, target, Mode::Detect, true, now).err();
    }

    pub fn enqueue_manual(
        &mut self,
        id: String,
        target: Target,
        mode: Mode,
        now: u64,
    ) -> Result<(), Reason> {
        if !id.starts_with("m-") || id.len() < 10 || !valid_id(&id, 64) {
            return Err(Reason::InvalidInput);
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&target, mode)).map_err(|_| Reason::InvalidInput)?)
        );
        if let Some(old) = self.accepted.iter().find(|a| a.id == id) {
            return if old.digest == digest {
                Ok(())
            } else {
                Err(Reason::Conflict)
            };
        }
        if self.accepted.len() >= MAX_HISTORY {
            return Err(Reason::QueueFull);
        }
        self.enqueue(id.clone(), target, mode, false, now)?;
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
        mode: Mode,
        scheduled: bool,
        now: u64,
    ) -> Result<(), Reason> {
        if self.queue.len() + target.account_ids.len() > MAX_QUEUED {
            return Err(Reason::QueueFull);
        }
        if self.remaining() < PROBES_PER_ROUND {
            return Err(Reason::Budget);
        }
        if self.queue.iter().any(|q| {
            q.profile_id == target.profile_id && target.account_ids.contains(&q.account_id)
        }) {
            return Err(Reason::Conflict);
        }
        for account_id in target.account_ids {
            self.queue.push(Task {
                run_id: id.clone(),
                account_id,
                client_key_id: target.client_key_id.clone(),
                profile_id: target.profile_id.clone(),
                mode,
                scheduled,
                queued_at: now,
                claimed: false,
                first_round: None,
            });
        }
        Ok(())
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
            );
        }
    }

    pub fn claim(&mut self, now: u64, version: &str) -> Option<Task> {
        if self.queue.iter().any(|q| q.claimed) {
            return None;
        }
        if self.remaining() < PROBES_PER_ROUND {
            self.finish_pending(now, version, |_| true, Reason::Budget);
            return None;
        }
        let next = self.queue.first_mut()?;
        next.claimed = true;
        // Count all three calls from the latest possible send time (including a crash).
        self.reservations.push(now.saturating_add(ROUND_BUDGET_MS));
        Some(next.clone())
    }

    pub fn complete(&mut self, task: &Task, decision: Decision, now: u64, version: &str) {
        let Some(index) = self
            .queue
            .iter()
            .position(|q| q.claimed && q.run_id == task.run_id && q.account_id == task.account_id)
        else {
            return;
        };
        match decision {
            Decision::Finish(outcome) => self.finish_at(index, outcome, now, version),
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
            );
        }
    }

    fn finish_at(&mut self, index: usize, outcome: Outcome, now: u64, version: &str) {
        let task = self.queue.remove(index);
        self.history.insert(
            0,
            HistoryItem {
                run_id: task.run_id,
                account_id: task.account_id,
                profile_id: task.profile_id,
                mode: task.mode,
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

    fn round(ids: &[&str]) -> Round {
        Round {
            stamp: "locked-bank".into(),
            detector_revision: "fixture".into(),
            top_margin_bps: 1500,
            ranking: Ranking {
                candidates: ids
                    .iter()
                    .map(|id| Candidate {
                        id: (*id).into(),
                        label: (*id).into(),
                    })
                    .collect(),
            },
        }
    }
    fn target(accounts: &[&str]) -> Target {
        Target {
            client_key_id: "key".into(),
            profile_id: "profile".into(),
            account_ids: accounts.iter().map(|a| (*a).into()).collect(),
        }
    }
    fn outcome(decision: Decision) -> Verdict {
        match decision {
            Decision::Finish(outcome) => outcome.verdict,
            _ => panic!("unexpected confirmation"),
        }
    }

    #[test]
    fn concordant_miss_only() {
        let Decision::Confirm(first) =
            decide(Mode::Detect, "expected", None, round(&["A", "B", "C"]))
        else {
            panic!()
        };
        assert_eq!(
            outcome(decide(
                Mode::Detect,
                "expected",
                Some(&first),
                round(&["A", "C", "B"])
            )),
            Verdict::Suspect
        );
        assert_eq!(
            outcome(decide(
                Mode::Detect,
                "expected",
                Some(&first),
                round(&["B", "C", "A"])
            )),
            Verdict::Insufficient
        );
        assert_eq!(
            outcome(decide(
                Mode::Detect,
                "expected",
                Some(&first),
                round(&["A", "expected", "C"])
            )),
            Verdict::Insufficient
        );
        assert_eq!(
            outcome(decide(
                Mode::Detect,
                "expected",
                None,
                round(&["expected", "B", "C"])
            )),
            Verdict::Match
        );
        assert_eq!(
            outcome(decide(
                Mode::Calibration,
                "expected",
                None,
                round(&["A", "B", "C"])
            )),
            Verdict::Calibration
        );
    }

    #[test]
    fn restart_never_recharges_unknown_attempt() {
        let mut s = State::default();
        s.enqueue_manual("m-batch001".into(), target(&["A", "B"]), Mode::Detect, 0)
            .unwrap();
        assert_eq!(s.claim(10, "0.1.0").unwrap().account_id, "A");
        let mut s: State = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        s.recover(20, "0.1.1");
        assert_eq!(s.history[0].outcome.verdict, Verdict::Interrupted);
        assert_eq!(s.remaining(), 57);
        assert_eq!(s.claim(30, "0.1.1").unwrap().account_id, "B");
        assert_eq!(s.remaining(), 54);
    }

    #[test]
    fn schedule_is_once_per_slot_no_catchup_and_can_disable() {
        let mut s = State::default();
        let settings = Settings {
            enabled: true,
            target: Some(target(&["A"])),
            ..Settings::default()
        };
        s.schedule(settings, 0, "v");
        s.schedule_due(3 * DAY_MS + 27, "v");
        s.schedule_due(3 * DAY_MS + 28, "v");
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.next_due_at, Some(4 * DAY_MS));
        s.schedule(Settings::default(), 3 * DAY_MS + 29, "v");
        assert!(s.queue.is_empty());
        assert_eq!(s.history[0].outcome.reason, Some(Reason::ScheduleChanged));
        s.schedule_due(4 * DAY_MS, "v");
        assert!(s.queue.is_empty());
    }

    #[test]
    fn new_slot_drops_old_queued_work_and_does_not_replay_a_skipped_slot() {
        let mut s = State::default();
        s.schedule(
            Settings {
                enabled: true,
                target: Some(target(&["A"])),
                ..Settings::default()
            },
            0,
            "v",
        );
        s.schedule_due(DAY_MS, "v");
        let run_id = s.queue[0].run_id.clone();
        s.schedule_due(2 * DAY_MS, "v");
        assert_eq!(s.queue.len(), 1);
        assert_ne!(s.queue[0].run_id, run_id);
        assert_eq!(s.history[0].outcome.reason, Some(Reason::StaleSlot));
        let active = s.claim(2 * DAY_MS + 1, "v").unwrap();
        let replacement = Settings {
            enabled: true,
            hours: 48,
            target: Some(target(&["A"])),
            ..Settings::default()
        };
        s.schedule(replacement, 2 * DAY_MS + 2, "v");
        s.complete(
            &active,
            Decision::Confirm(round(&["B", "C", "D"])),
            2 * DAY_MS + 3,
            "v",
        );
        assert!(s.queue.is_empty());
        assert_eq!(s.history[0].outcome.reason, Some(Reason::ScheduleChanged));
    }

    #[test]
    fn rolling_cap_is_shared_and_skips_retest() {
        let mut s = State::default();
        s.settings.probe_budget = 3;
        s.enqueue_manual("m-budget001".into(), target(&["A", "B"]), Mode::Detect, 0)
            .unwrap();
        let a = s.claim(5, "v").unwrap();
        s.complete(&a, Decision::Confirm(round(&["B", "C", "D"])), 6, "v");
        assert!(s.claim(7, "v").is_none());
        assert!(s.queue.is_empty());
        assert_eq!(s.history.len(), 2);
        assert!(
            s.history
                .iter()
                .all(|h| h.outcome.reason == Some(Reason::Budget))
        );
        s.prune(5 + DAY_MS + ROUND_BUDGET_MS);
        assert_eq!(s.remaining(), 3);
    }

    #[test]
    fn manual_nonce_is_idempotent_and_payload_bound() {
        let mut s = State::default();
        for _ in 0..2 {
            s.enqueue_manual("m-idempotent".into(), target(&["A"]), Mode::Detect, 0)
                .unwrap();
        }
        assert_eq!(s.queue.len(), 1);
        assert_eq!(
            s.enqueue_manual("m-idempotent".into(), target(&["B"]), Mode::Detect, 0),
            Err(Reason::Conflict)
        );
    }
}
