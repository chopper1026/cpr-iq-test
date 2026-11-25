//! LM Detector is called only through its offline API. The bus never sends credentials to it.
use crate::domain::{Candidate, Mode, PROBES_PER_ROUND, Ranking, Reason, Round};
use lmfpd::{Analysis, Challenge, Decision, Detector, LoadOptions, Output, ProbabilityStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

const LOCK_BYTES: &[u8] = include_bytes!("../data/detector.lock.json");
const PROFILES_BYTES: &[u8] = include_bytes!("../data/profiles.json");
const POLICY: &str = "responses-openai-strict-rank-v1";

#[derive(Debug, Deserialize)]
struct LockedAsset {
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct BankLock {
    revision: String,
    files: BTreeMap<String, LockedAsset>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub model: String,
    pub effort: String,
    pub label_id: String,
    pub validation: Option<Validation>,
}

/// Presence alone is insufficient. Validation is bound to the *exact* bank and request profile.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Validation {
    profile_stamp: String,
    host_tag: String,
    evidence: String,
    positive_rounds: u16,
    minimum_margin_bps: u16,
}

impl Profile {
    fn stamp(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(LOCK_BYTES);
        for field in [POLICY, &self.id, &self.model, &self.effort, &self.label_id] {
            hasher.update([0]);
            hasher.update(field.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    pub fn minimum_margin(&self) -> Option<u16> {
        self.validation
            .as_ref()
            .filter(|e| {
                e.profile_stamp == self.stamp()
                    && e.host_tag.starts_with('v')
                    && !e.evidence.trim().is_empty()
                    && e.positive_rounds > 0
                    && (1..=10_000).contains(&e.minimum_margin_bps)
            })
            .map(|e| e.minimum_margin_bps)
    }

    pub fn round_stamp(&self, credential_revision: u64) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!(
                "{}:{:?}:{credential_revision}",
                self.stamp(),
                self.minimum_margin()
            ))
        )
    }
}

pub struct Fingerprint {
    detector: Detector,
    profiles: Vec<Profile>,
    labels: BTreeMap<String, String>,
    pub revision: String,
}

impl Fingerprint {
    pub fn load(directory: &Path) -> Result<Self, Reason> {
        let fail = Reason::DetectorUnavailable;
        let local = directory.canonicalize().map_err(|_| fail)?;
        let absolute = local.to_str().filter(|s| !s.is_empty()).ok_or(fail)?;
        let lock: BankLock = serde_json::from_slice(LOCK_BYTES).map_err(|_| fail)?;
        for name in ["unified_bank.json", "shared_detector.bin"] {
            verify(&local.join(name), lock.files.get(name).ok_or(fail)?)?;
        }
        #[derive(Deserialize)]
        struct Labels {
            models: Vec<Label>,
        }
        #[derive(Deserialize)]
        struct Label {
            id: String,
            display_name: String,
        }
        let labels: BTreeMap<_, _> = serde_json::from_reader::<_, Labels>(std::io::BufReader::new(
            File::open(local.join("unified_bank.json")).map_err(|_| fail)?,
        ))
        .map_err(|_| fail)?
        .models
        .into_iter()
        .map(|entry| (entry.id, entry.display_name))
        .collect();
        let profiles: Vec<Profile> = serde_json::from_slice(PROFILES_BYTES).map_err(|_| fail)?;
        let mut ids = BTreeSet::new();
        if profiles.len() > 16
            || profiles.iter().any(|p| {
                !ids.insert(&p.id)
                    || !labels.contains_key(&p.label_id)
                    || p.id.is_empty()
                    || p.id.len() > 64
                    || p.model.is_empty()
                    || p.model.len() > 128
                    || !matches!(
                        p.effort.as_str(),
                        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
                    )
            })
        {
            return Err(fail);
        }
        let detector = Detector::load(&LoadOptions {
            sources: vec![absolute.into()],
            cache: local.join("unused-cache"),
            ttl: 0.0,
        })
        .map_err(|_| fail)?;
        Ok(Self {
            detector,
            profiles,
            labels,
            revision: lock.revision,
        })
    }

    pub fn options(&self) -> Vec<ProfileChoice<'_>> {
        self.profiles
            .iter()
            .map(|profile| ProfileChoice {
                id: &profile.id,
                model: &profile.model,
                effort: &profile.effort,
                label_id: &profile.label_id,
                label: &self.labels[&profile.label_id],
                validated: profile.minimum_margin().is_some(),
                profile_stamp: profile.stamp(),
            })
            .collect()
    }

    pub fn profile(&self, id: &str, mode: Mode) -> Result<&Profile, Reason> {
        self.profiles
            .iter()
            .find(|p| p.id == id && (mode == Mode::Calibration || p.minimum_margin().is_some()))
            .ok_or(Reason::UnsupportedProfile)
    }

    pub fn challenges(&self) -> Result<Vec<Challenge>, Reason> {
        let result = lmfpd::generate_challenges(PROBES_PER_ROUND as usize)
            .map_err(|_| Reason::DetectorUnavailable)?;
        if result.len() != PROBES_PER_ROUND as usize
            || result.iter().map(|c| &c.id).collect::<BTreeSet<_>>().len() != result.len()
        {
            return Err(Reason::DetectorUnavailable);
        }
        Ok(result)
    }

    pub fn rank(&self, outputs: &[Output], stamp: String) -> Result<Round, Reason> {
        let analysis = self
            .detector
            .analyze(outputs, true)
            .map_err(|_| Reason::NotScorable)?;
        summarize(analysis, stamp, &self.revision)
    }
}

fn summarize(analysis: Analysis, stamp: String, detector_revision: &str) -> Result<Round, Reason> {
    if analysis.decision != Decision::Scored
        || analysis.used_outputs != PROBES_PER_ROUND as usize
        || analysis.probability_status != ProbabilityStatus::ReferenceCalibrated
        || analysis.results.len() < 3
    {
        return Err(Reason::NotScorable);
    }
    let candidates = &analysis.results[..3];
    let prob = |value: Option<f64>| value.filter(|p| p.is_finite() && (0.0..=1.0).contains(p));
    let a = prob(candidates[0].probability).ok_or(Reason::NotScorable)?;
    let b = prob(candidates[1].probability).ok_or(Reason::NotScorable)?;
    if a < b || (a - b).abs() < f64::EPSILON {
        return Err(Reason::Nearby);
    }
    let mut ids = BTreeSet::new();
    if candidates.iter().any(|c| {
        !c.score.is_finite()
            || c.model.is_empty()
            || c.model.len() > 96
            || c.display_name.is_empty()
            || c.display_name.len() > 128
            || !ids.insert(&c.model)
    }) {
        return Err(Reason::NotScorable);
    }
    Ok(Round {
        stamp,
        detector_revision: detector_revision.into(),
        top_margin_bps: ((a - b) * 10_000.0).floor() as u16,
        ranking: Ranking {
            candidates: candidates
                .iter()
                .map(|entry| Candidate {
                    id: entry.model.clone(),
                    label: entry.display_name.clone(),
                })
                .collect(),
        },
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileChoice<'a> {
    id: &'a str,
    model: &'a str,
    effort: &'a str,
    label_id: &'a str,
    label: &'a str,
    validated: bool,
    profile_stamp: String,
}

fn verify(file: &Path, expected: &LockedAsset) -> Result<(), Reason> {
    let fail = Reason::DetectorUnavailable;
    let mut input = File::open(file).map_err(|_| fail)?;
    if input.metadata().map_err(|_| fail)?.len() != expected.bytes {
        return Err(fail);
    }
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 32 * 1024];
    loop {
        let n = input.read(&mut buf).map_err(|_| fail)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    if format!("{:x}", hasher.finalize()) != expected.sha256 {
        return Err(fail);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_loads_offline_checks_both_assets_and_generates_independent_challenges() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let bank = Fingerprint::load(&directory).ok();
        if std::env::var_os("CPR_REQUIRE_BANK").is_some() {
            assert!(bank.is_some(), "locked pair must work offline");
        }
        if let Some(bank) = bank {
            let a = bank.challenges().unwrap();
            let b = bank.challenges().unwrap();
            assert!(!a.iter().any(|x| b.iter().any(|y| x.id == y.id)));
            let outputs: Vec<_> = a
                .iter()
                .map(|c| Output {
                    text: "drop".into(),
                    expected_count: c.expected_count,
                })
                .collect();
            assert_eq!(bank.rank(&outputs, "test".into()), Err(Reason::NotScorable));
            assert!(bank.profile("gpt6-sol-low", Mode::Calibration).is_ok());
        }
        assert!(Fingerprint::load(Path::new("does-not-exist")).is_err());
    }

    #[test]
    fn evidence_is_tied_to_exact_model_effort_and_data() {
        let mut profiles = serde_json::from_slice::<Vec<Profile>>(PROFILES_BYTES).unwrap();
        let profile = profiles.first_mut().unwrap();
        profile.validation = Some(Validation {
            profile_stamp: profile.stamp(),
            host_tag: "v3.54.5".into(),
            evidence: "fixture".into(),
            positive_rounds: 3,
            minimum_margin_bps: 100,
        });
        assert_eq!(profile.minimum_margin(), Some(100));
        profile.effort = "changed".into();
        assert_eq!(profile.minimum_margin(), None);
    }
}

#[cfg(test)]
#[test]
fn a_known_different_label_repeats_with_independent_upstream_replies() {
    use crate::domain::{self, Decision as VerdictDecision, Verdict};
    let Ok(bank) = Fingerprint::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("data")) else {
        assert!(
            std::env::var_os("CPR_REQUIRE_BANK").is_none(),
            "detector data required"
        );
        return;
    };
    #[derive(Deserialize)]
    struct Fixture {
        label: String,
        rounds: Vec<Vec<Output>>,
    }
    let fixture: Fixture = serde_json::from_str(include_str!("../tests/known_other.json")).unwrap();
    let first = bank.rank(&fixture.rounds[0], "one-bank".into()).unwrap();
    let second = bank.rank(&fixture.rounds[1], "one-bank".into()).unwrap();
    assert_eq!(first.ranking.candidates[0].id, fixture.label);
    assert_eq!(second.ranking.candidates[0].id, fixture.label);
    // This is a frozen upstream reference fixture, not a live OAuth positive or a general accuracy test.
    let expected = "claude-opus-4.8";
    let VerdictDecision::Confirm(first) = domain::decide(Mode::Detect, expected, None, first)
    else {
        panic!()
    };
    let VerdictDecision::Finish(result) =
        domain::decide(Mode::Detect, expected, Some(&first), second)
    else {
        panic!()
    };
    assert_eq!(result.verdict, Verdict::Suspect);
}
