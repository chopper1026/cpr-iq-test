//! State transitions are pure; a host CAS commits a claim before any probe is sent.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MAX_BATCH: usize = 100;
pub const MAX_QUEUED: usize = 200;
pub const MAX_HISTORY: usize = 500;
pub const MAX_DAILY_TIMES: usize = 12;
pub const SUSPECT_NOTE_PREFIX: &str = "[指纹异常]";
pub const MAX_ACCOUNT_NOTES_CHARS: usize = 500;
pub const PROBES_PER_ROUND: u16 = 3;
pub const MINUTE_MS: u64 = 60_000;
pub const HOUR_MS: u64 = 3_600_000;
pub const DAY_MS: u64 = 86_400_000;
pub const ROUND_BUDGET_MS: u64 = 600_000;
pub const DEFAULT_EFFORT: &str = "default";
const RETENTION_MS: u64 = 30 * DAY_MS;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccountMeta {
    pub email: Option<String>,
    pub name: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeDiagnostic {
    pub index: u8,
    pub challenge_id: String,
    pub expected_count: u64,
    pub duration_ms: u64,
    pub ok: bool,
    pub batches: u32,
    pub events: u32,
    pub text_parts: u16,
    pub text_chars: usize,
    pub reasoning_chars: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<Reason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDiagnostic {
    pub round: u8,
    pub stage: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_revision_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_revision_after: Option<u64>,
    #[serde(default)]
    pub identity_changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detector_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_outputs: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_disable_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_disable_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<ProbeDiagnostic>,
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

pub fn default_hours() -> u16 {
    24
}

pub fn default_daily_times() -> Vec<String> {
    vec!["09:00".to_owned()]
}

pub fn parse_daily_time(value: &str) -> Option<u16> {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    if !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || !bytes[3].is_ascii_digit()
        || !bytes[4].is_ascii_digit()
    {
        return None;
    }
    let hour = u16::from(bytes[0] - b'0') * 10 + u16::from(bytes[1] - b'0');
    let minute = u16::from(bytes[3] - b'0') * 10 + u16::from(bytes[4] - b'0');
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

pub fn valid_utc_offset(minutes: i16) -> bool {
    (-720..=840).contains(&minutes) && minutes % 15 == 0
}

pub fn format_timestamp_with_offset(epoch_ms: u64, utc_offset_minutes: i16) -> String {
    let clamped_offset = utc_offset_minutes.clamp(-720, 840);
    let total_secs =
        i64::try_from(epoch_ms / 1000).unwrap_or(i64::MAX / 2) + i64::from(clamped_offset) * 60;
    let days = total_secs.div_euclid(86_400);
    let sod = total_secs.rem_euclid(86_400);
    let hour = sod / 3_600;
    let minute = (sod % 3_600) / 60;
    let (year, month, day) = civil_from_days(days);
    if clamped_offset == 0 {
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
    } else {
        let sign = if clamped_offset >= 0 { '+' } else { '-' };
        let abs = clamped_offset.unsigned_abs();
        format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC{sign}{:02}:{:02}",
            abs / 60,
            abs % 60
        )
    }
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year, m as u32, d as u32)
}

pub fn format_suspect_account_note(
    model: &str,
    effort: &str,
    outcome: &Outcome,
    ended_at: u64,
    utc_offset_minutes: i16,
) -> String {
    let clean = |s: &str, max: usize| -> String {
        s.chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(max)
            .collect()
    };
    let model_clean = clean(model, 64);
    let effort_clean = clean(effort, 24);
    let target = if effort_clean.is_empty() || effort_clean == DEFAULT_EFFORT {
        model_clean
    } else {
        format!("{model_clean}({effort_clean})")
    };
    let top_desc = outcome
        .last_round
        .as_ref()
        .and_then(|r| r.ranking.candidates.first())
        .map(|c| {
            let label = clean(&c.label, 64);
            match c.probability_bps {
                Some(bps) => format!("判定 #1 {label}({:.1}%)", f64::from(bps) / 100.0),
                None => format!("判定 #1 {label}"),
            }
        })
        .unwrap_or_else(|| "两轮复测均未进入前三".to_owned());
    let ts = format_timestamp_with_offset(ended_at, utc_offset_minutes);
    format!("{SUSPECT_NOTE_PREFIX} {ts} | {target} 两轮复测疑似异常，{top_desc}，已自动停用")
        .chars()
        .take(240)
        .collect()
}

/// Merges a new suspect detection note line at the top of existing account notes,
/// replacing any previous `[指纹异常]` entry while preserving manual notes within CPR's 500-char limit.
pub fn merge_account_notes(existing: Option<&str>, entry: &str) -> String {
    let sanitize = |s: &str| -> String {
        s.chars()
            .filter(|&ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
            .collect()
    };
    let entry_clean: String = sanitize(entry)
        .replace(['\r', '\n'], " ")
        .trim()
        .chars()
        .take(240)
        .collect();
    let preserved: Vec<String> = existing
        .unwrap_or("")
        .lines()
        .map(|line| sanitize(line).trim_end().to_owned())
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with(SUSPECT_NOTE_PREFIX)
        })
        .collect();
    if preserved.is_empty() {
        return entry_clean;
    }
    let rest = preserved.join("\n");
    let entry_chars = entry_clean.chars().count();
    let budget = MAX_ACCOUNT_NOTES_CHARS.saturating_sub(entry_chars + 1);
    if budget == 0 {
        return entry_clean;
    }
    if rest.chars().count() <= budget {
        format!("{entry_clean}\n{rest}")
    } else if budget <= 1 {
        entry_clean
    } else {
        let kept: String = rest.chars().take(budget - 1).collect();
        format!("{entry_clean}\n{}…", kept.trim_end())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleMode {
    #[default]
    Interval,
    Daily,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    #[serde(default)]
    pub mode: ScheduleMode,
    #[serde(default = "default_hours")]
    pub hours: u16,
    #[serde(default = "default_daily_times")]
    pub daily_times: Vec<String>,
    #[serde(default)]
    pub utc_offset_minutes: i16,
    #[serde(default)]
    pub auto_disable_on_suspect: bool,
    pub target: Option<Target>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: ScheduleMode::Interval,
            hours: default_hours(),
            daily_times: default_daily_times(),
            utc_offset_minutes: 0,
            auto_disable_on_suspect: false,
            target: None,
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) -> Result<(), Reason> {
        if !matches!(self.hours, 1 | 6 | 12 | 24 | 48 | 168)
            || !valid_utc_offset(self.utc_offset_minutes)
            || self.enabled && self.target.is_none()
        {
            return Err(Reason::InvalidInput);
        }
        if self.daily_times.is_empty() && self.mode == ScheduleMode::Interval {
            self.daily_times = default_daily_times();
        }
        if !(1..=MAX_DAILY_TIMES).contains(&self.daily_times.len()) {
            return Err(Reason::InvalidInput);
        }
        let mut parsed = Vec::with_capacity(self.daily_times.len());
        for raw in &self.daily_times {
            let trimmed = raw.trim();
            let mins = parse_daily_time(trimmed).ok_or(Reason::InvalidInput)?;
            parsed.push((mins, format!("{:02}:{:02}", mins / 60, mins % 60)));
        }
        parsed.sort_unstable_by_key(|(mins, _)| *mins);
        if parsed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Reason::InvalidInput);
        }
        self.daily_times = parsed.into_iter().map(|(_, formatted)| formatted).collect();
        if let Some(target) = &mut self.target {
            target.normalize()?;
        }
        Ok(())
    }

    fn period_ms(&self) -> u64 {
        u64::from(self.hours) * HOUR_MS
    }

    fn daily_minutes(&self) -> Vec<u16> {
        let mut mins: Vec<u16> = self
            .daily_times
            .iter()
            .filter_map(|t| parse_daily_time(t.trim()))
            .collect();
        mins.sort_unstable();
        mins.dedup();
        if mins.is_empty() {
            mins.push(9 * 60);
        }
        mins
    }

    pub fn next_due_after(&self, now: u64) -> u64 {
        match self.mode {
            ScheduleMode::Interval => now.saturating_add(self.period_ms()),
            ScheduleMode::Daily => {
                let minutes = self.daily_minutes();
                let offset_ms = i64::from(self.utc_offset_minutes) * (MINUTE_MS as i64);
                let day_ms = DAY_MS as i64;
                let local_now = i64::try_from(now)
                    .unwrap_or(i64::MAX / 2)
                    .saturating_add(offset_ms);
                let local_day_start = local_now.div_euclid(day_ms) * day_ms;
                let next_local = minutes
                    .iter()
                    .map(|&m| local_day_start + i64::from(m) * (MINUTE_MS as i64))
                    .find(|&candidate| candidate > local_now)
                    .unwrap_or_else(|| {
                        local_day_start + day_ms + i64::from(minutes[0]) * (MINUTE_MS as i64)
                    });
                u64::try_from(next_local - offset_ms).unwrap_or(0)
            }
        }
    }

    fn latest_daily_slot_at_or_before(&self, now: u64) -> u64 {
        let minutes = self.daily_minutes();
        let offset_ms = i64::from(self.utc_offset_minutes) * (MINUTE_MS as i64);
        let day_ms = DAY_MS as i64;
        let local_now = i64::try_from(now)
            .unwrap_or(i64::MAX / 2)
            .saturating_add(offset_ms);
        let local_day_start = local_now.div_euclid(day_ms) * day_ms;
        let slot_local = minutes
            .iter()
            .rev()
            .map(|&m| local_day_start + i64::from(m) * (MINUTE_MS as i64))
            .find(|&candidate| candidate <= local_now)
            .unwrap_or_else(|| {
                local_day_start - day_ms + i64::from(*minutes.last().unwrap()) * (MINUTE_MS as i64)
            });
        u64::try_from(slot_local - offset_ms).unwrap_or(0)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<RunDiagnostic>,
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
        if self.settings.daily_times.is_empty() {
            self.settings.daily_times = default_daily_times();
        }
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
        let schedule_changed = self.settings.enabled != new.enabled
            || self.settings.mode != new.mode
            || self.settings.hours != new.hours
            || self.settings.daily_times != new.daily_times
            || self.settings.utc_offset_minutes != new.utc_offset_minutes
            || self.settings.target != new.target;
        self.settings = new;
        if schedule_changed {
            self.schedule_revision = self.schedule_revision.saturating_add(1);
            self.last_schedule_skip = None;
            self.next_due_at = self
                .settings
                .enabled
                .then(|| self.settings.next_due_after(now));
            self.finish_pending(now, version, |q| q.scheduled, Reason::ScheduleChanged);
        }
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
        // Skip all older slots; this tick schedules at most one batch.
        let slot = match self.settings.mode {
            ScheduleMode::Interval => {
                let period = self.settings.period_ms();
                let slot = now - (now - due) % period;
                self.next_due_at = Some(slot.saturating_add(period));
                slot
            }
            ScheduleMode::Daily => {
                let slot = self.settings.latest_daily_slot_at_or_before(now).max(due);
                self.next_due_at = Some(self.settings.next_due_after(slot));
                slot
            }
        };
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
        diagnostics: Option<RunDiagnostic>,
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
                self.finish_at(index, outcome, now, version, refreshed_meta, diagnostics)
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
                        diagnostics,
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
        diagnostics: Option<RunDiagnostic>,
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
                diagnostics,
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
                plan_type: Some("pro".into()),
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
            ..Settings::default()
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
    fn daily_fixed_time_schedule_respects_timezone_multi_slots_and_no_catchup() {
        let mut s = State::default();
        let labels = BTreeMap::new();
        // Base UTC midnight: day 10
        let utc_day10 = 10 * DAY_MS;
        // UTC+08:00 (+480 min): 09:00 local = 01:00 UTC, 21:30 local = 13:30 UTC
        let mut settings = Settings {
            enabled: true,
            mode: ScheduleMode::Daily,
            hours: 24,
            daily_times: vec!["21:30".into(), "09:00".into()],
            utc_offset_minutes: 480,
            auto_disable_on_suspect: false,
            target: Some(target(&["A"])),
        };
        settings.normalize().unwrap();
        assert_eq!(settings.daily_times, vec!["09:00", "21:30"]);

        // Enable at 08:00 UTC+8 (00:00 UTC on day 10) -> next due is 09:00 UTC+8 (01:00 UTC)
        s.schedule(settings.clone(), utc_day10, "v");
        let slot_morning_d10 = utc_day10 + HOUR_MS;
        let slot_evening_d10 = utc_day10 + 13 * HOUR_MS + 30 * MINUTE_MS;
        let slot_morning_d11 = utc_day10 + DAY_MS + HOUR_MS;
        assert_eq!(s.next_due_at, Some(slot_morning_d10));

        // Tick 1 minute before due -> nothing scheduled
        s.schedule_due(slot_morning_d10 - MINUTE_MS, "v", &labels);
        assert!(s.queue.is_empty());

        // Tick 5 seconds after 09:00 UTC+8 -> schedules morning slot, advances to 21:30 UTC+8
        s.schedule_due(slot_morning_d10 + 5_000, "v", &labels);
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.next_due_at, Some(slot_evening_d10));
        // Duplicate tick before 21:30 does nothing
        s.schedule_due(slot_morning_d10 + 60_000, "v", &labels);
        assert_eq!(s.queue.len(), 1);

        // Simulate 2-day outage: wake up at day 12 10:00 UTC+8 (02:00 UTC)
        // Must skip missed evening d10, morning d11, evening d11, and only enqueue morning d12!
        let slot_morning_d12 = utc_day10 + 2 * DAY_MS + HOUR_MS;
        let slot_evening_d12 = utc_day10 + 2 * DAY_MS + 13 * HOUR_MS + 30 * MINUTE_MS;
        s.schedule_due(slot_morning_d12 + HOUR_MS, "v", &labels);
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.queue[0].run_id, format!("s-1-{slot_morning_d12}"));
        assert_eq!(s.next_due_at, Some(slot_evening_d12));
        assert_eq!(s.history[0].outcome.reason, Some(Reason::StaleSlot));

        // Negative timezone offset: UTC-05:00 (-300 min), daily 23:00 local = next UTC day 04:00
        let mut ny = Settings {
            enabled: true,
            mode: ScheduleMode::Daily,
            hours: 24,
            daily_times: vec!["23:00".into()],
            utc_offset_minutes: -300,
            auto_disable_on_suspect: false,
            target: Some(target(&["A"])),
        };
        ny.normalize().unwrap();
        // At day 10 04:00 UTC (day 9 23:00 local), strictly next due is day 10 23:00 local = day 11 04:00 UTC
        assert_eq!(
            ny.next_due_after(utc_day10 + 4 * HOUR_MS),
            utc_day10 + DAY_MS + 4 * HOUR_MS
        );
        // At day 10 03:59 UTC (day 9 22:59 local), next due is day 9 23:00 local = day 10 04:00 UTC
        assert_eq!(
            ny.next_due_after(utc_day10 + 4 * HOUR_MS - MINUTE_MS),
            utc_day10 + 4 * HOUR_MS
        );

        // Validation rejects invalid times, duplicates, empty daily_times in Daily mode, bad offset
        let mut bad = settings.clone();
        bad.daily_times = vec!["24:00".into()];
        assert_eq!(bad.normalize(), Err(Reason::InvalidInput));
        bad.daily_times = vec!["09:00".into(), "09:00".into()];
        assert_eq!(bad.normalize(), Err(Reason::InvalidInput));
        bad.daily_times = vec![];
        assert_eq!(bad.normalize(), Err(Reason::InvalidInput));
        bad.daily_times = vec!["09:00".into()];
        bad.utc_offset_minutes = 7;
        assert_eq!(bad.normalize(), Err(Reason::InvalidInput));

        // Also test slot boundary when now == slot_morning_d11 exact
        let mut s2 = State::default();
        s2.schedule(settings, slot_morning_d10, "v");
        assert_eq!(s2.next_due_at, Some(slot_evening_d10));
        s2.schedule_due(slot_morning_d11, "v", &labels);
        assert_eq!(s2.queue[0].run_id, format!("s-1-{slot_morning_d11}"));
        assert_eq!(
            s2.next_due_at,
            Some(utc_day10 + DAY_MS + 13 * HOUR_MS + 30 * MINUTE_MS)
        );
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
                ..Settings::default()
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
            ..Settings::default()
        };
        s.schedule(replacement, 2 * DAY_MS + 2, "v");
        s.complete(
            &active,
            Decision::Confirm(round(&["B", "C", "D"], 1200)),
            2 * DAY_MS + 3,
            "v",
            None,
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
        assert!(!state.settings.auto_disable_on_suspect);
    }

    #[test]
    fn toggling_auto_disable_preserves_active_schedule_and_pending_tasks() {
        let mut s = State::default();
        let labels = BTreeMap::new();
        let base = Settings {
            enabled: true,
            mode: ScheduleMode::Daily,
            hours: 24,
            daily_times: vec!["09:00".into()],
            utc_offset_minutes: 480,
            auto_disable_on_suspect: false,
            target: Some(target(&["A"])),
        };
        let utc_day10 = 10 * DAY_MS;
        s.schedule(base.clone(), utc_day10, "0.1.0");
        let expected_due = s.next_due_at;
        s.schedule_due(utc_day10 + HOUR_MS + 1000, "0.1.0", &labels);
        assert_eq!(s.queue.len(), 1);
        let due_after_tick = s.next_due_at;
        let run_id = s.queue[0].run_id.clone();

        let toggled = Settings {
            auto_disable_on_suspect: true,
            ..base
        };
        s.schedule(toggled, utc_day10 + HOUR_MS + 2000, "0.1.0");
        assert!(s.settings.auto_disable_on_suspect);
        assert_eq!(s.next_due_at, due_after_tick);
        assert_ne!(expected_due, due_after_tick);
        assert_eq!(s.queue.len(), 1);
        assert_eq!(s.queue[0].run_id, run_id);
        assert!(s.history.is_empty());
    }

    #[test]
    fn suspect_note_format_and_merge_preserves_existing_and_respects_500_char_limit() {
        // 2024-02-29 16:30:00 UTC => 2024-03-01 00:30 UTC+08:00 (leap day check)
        let leap_ms = 1_709_224_200_000_u64;
        assert_eq!(
            format_timestamp_with_offset(leap_ms, 0),
            "2024-02-29 16:30 UTC"
        );
        assert_eq!(
            format_timestamp_with_offset(leap_ms, 480),
            "2024-03-01 00:30 UTC+08:00"
        );
        assert_eq!(
            format_timestamp_with_offset(leap_ms, -300),
            "2024-02-29 11:30 UTC-05:00"
        );

        let outcome = Outcome {
            verdict: Verdict::Suspect,
            reason: None,
            last_round: Some(Round {
                stamp: "st".into(),
                detector_revision: "rev".into(),
                top_margin_bps: 7500,
                ranking: Ranking {
                    candidates: vec![Candidate {
                        id: "gpt-5.3-codex".into(),
                        label: "gpt-5.3-codex".into(),
                        probability_bps: Some(9420),
                    }],
                },
            }),
        };
        let note = format_suspect_account_note("gpt-6.2-sol", "high", &outcome, leap_ms, 480);
        assert!(note.starts_with("[指纹异常] 2024-03-01 00:30 UTC+08:00 | "));
        assert!(note.contains("gpt-6.2-sol(high)"));
        assert!(note.contains("#1 gpt-5.3-codex(94.2%)"));
        assert!(note.contains("已自动停用"));

        // Empty existing notes
        assert_eq!(merge_account_notes(None, &note), note);

        // Existing notes + stale previous suspect line -> replace stale line, keep manual notes
        let existing =
            "[指纹异常] 2024-01-01 00:00 UTC | old entry\n团队主力号\n联系 ops@example.com";
        let merged = merge_account_notes(Some(existing), &note);
        assert_eq!(merged, format!("{note}\n团队主力号\n联系 ops@example.com"));
        assert_eq!(merged.matches(SUSPECT_NOTE_PREFIX).count(), 1);

        // Very long existing notes with stray control char -> capped <= 500 chars and clean
        let huge = format!("{}\u{0007}尾部", "备".repeat(490));
        let capped = merge_account_notes(Some(&huge), &note);
        assert!(capped.chars().count() <= MAX_ACCOUNT_NOTES_CHARS);
        assert!(capped.starts_with(&note));
        assert!(capped.ends_with('…'));
        assert!(
            !capped
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        );
    }
}
