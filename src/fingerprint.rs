//! LM Detector is called only through its offline API. The bus never sends credentials to it.
use crate::domain::{Candidate, PROBES_PER_ROUND, Ranking, Reason, Round};
use lmfpd::{Analysis, Challenge, Decision, Detector, LoadOptions, Output, ProbabilityStatus};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

const LOCK_BYTES: &[u8] = include_bytes!("../data/detector.lock.json");
const POLICY: &str = "responses-openai-strict-rank-v3";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScoreTrace {
    pub decision: Option<String>,
    pub used_outputs: Option<usize>,
    pub probability_status: Option<String>,
    pub candidate_count: usize,
    pub detail: Option<String>,
}

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

pub struct Fingerprint {
    detector: Detector,
    labels: BTreeMap<String, String>,
    aliases: BTreeMap<String, String>,
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
        let entries = serde_json::from_reader::<_, Labels>(std::io::BufReader::new(
            File::open(local.join("unified_bank.json")).map_err(|_| fail)?,
        ))
        .map_err(|_| fail)?
        .models;
        let mut labels = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        for entry in entries {
            let canonical = entry.id.clone();
            aliases.insert(canonical.to_ascii_lowercase(), canonical.clone());
            aliases.insert(entry.display_name.to_ascii_lowercase(), canonical.clone());
            // Support merged reference names such as "gpt-6-astra/6.1-sol" -> "gpt-6.1-sol".
            if let Some((left, right)) = entry.display_name.split_once('/') {
                aliases.insert(left.trim().to_ascii_lowercase(), canonical.clone());
                let right = right.trim();
                if !right.is_empty() {
                    aliases.insert(right.to_ascii_lowercase(), canonical.clone());
                    if let Some((prefix, _)) = left.split_once('-')
                        && !right.starts_with(&format!("{prefix}-"))
                    {
                        aliases.insert(
                            format!("{prefix}-{right}").to_ascii_lowercase(),
                            canonical.clone(),
                        );
                    }
                }
            }
            labels.insert(entry.id, entry.display_name);
        }
        let detector = Detector::load(&LoadOptions {
            sources: vec![absolute.into()],
            cache: local.join("unused-cache"),
            ttl: 0.0,
        })
        .map_err(|_| fail)?;
        Ok(Self {
            detector,
            labels,
            aliases,
            revision: lock.revision,
        })
    }

    /// Maps a requested upstream model ID to its reference bank `(label_id, display_name)`, if present.
    pub fn expected_label(&self, model: &str) -> Option<(&str, &str)> {
        let key = model.trim().to_ascii_lowercase();
        let id = self.aliases.get(&key)?;
        let display = self.labels.get(id)?;
        Some((id.as_str(), display.as_str()))
    }

    /// Binds a round and optional confirmation retest to the exact bank lock, request pair, and stable upstream account identity.
    pub fn round_stamp(&self, model: &str, effort: &str, identity_fingerprint: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(LOCK_BYTES);
        for field in [POLICY, model, effort, identity_fingerprint] {
            hasher.update([0]);
            hasher.update(field.as_bytes());
        }
        format!("{:x}", hasher.finalize())
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

    pub fn rank_with_trace(
        &self,
        outputs: &[Output],
        stamp: String,
    ) -> (Result<Round, Reason>, ScoreTrace) {
        let analysis = match self.detector.analyze(outputs, true) {
            Ok(analysis) => analysis,
            Err(error) => {
                return (
                    Err(Reason::NotScorable),
                    ScoreTrace {
                        decision: Some("Error".into()),
                        detail: Some(format!("lmfpd analyze error: {error}")),
                        ..ScoreTrace::default()
                    },
                );
            }
        };
        let mut trace = ScoreTrace {
            decision: Some(format!("{:?}", analysis.decision)),
            used_outputs: Some(analysis.used_outputs),
            probability_status: Some(format!("{:?}", analysis.probability_status)),
            candidate_count: analysis.results.len(),
            detail: None,
        };
        let result = summarize(analysis, stamp, &self.revision);
        if let Err(reason) = &result {
            trace.detail = Some(format!(
                "reason={reason:?} decision={} used_outputs={}/{} prob_status={} candidates={}",
                trace.decision.as_deref().unwrap_or("?"),
                trace.used_outputs.unwrap_or(0),
                PROBES_PER_ROUND,
                trace.probability_status.as_deref().unwrap_or("?"),
                trace.candidate_count,
            ));
        }
        (result, trace)
    }

    #[cfg(test)]
    pub fn rank(&self, outputs: &[Output], stamp: String) -> Result<Round, Reason> {
        self.rank_with_trace(outputs, stamp).0
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
    if a < b {
        return Err(Reason::NotScorable);
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
        top_margin_bps: ((a - b).max(0.0) * 10_000.0).floor() as u16,
        ranking: Ranking {
            candidates: candidates
                .iter()
                .map(|entry| Candidate {
                    id: entry.model.clone(),
                    label: entry.display_name.clone(),
                    probability_bps: prob(entry.probability)
                        .map(|p| (p * 10_000.0).round().clamp(0.0, 10_000.0) as u16),
                })
                .collect(),
        },
    })
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
    fn bank_loads_offline_checks_assets_matches_aliases_and_generates_independent_challenges() {
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
            assert_eq!(
                bank.expected_label("gpt-6-sol"),
                Some(("gpt-6-sol", "gpt-6-sol"))
            );
            assert_eq!(
                bank.expected_label("gpt-6.1-sol"),
                Some(("gpt-6-astra", "gpt-6-astra/6.1-sol"))
            );
            assert_eq!(bank.expected_label("not-in-reference-bank"), None);
            assert_ne!(
                bank.round_stamp("gpt-6-sol", "low", "openai|oauth|u:u1|a:a1"),
                bank.round_stamp("gpt-6-sol", "high", "openai|oauth|u:u1|a:a1")
            );
            assert_ne!(
                bank.round_stamp("gpt-6-sol", "low", "openai|oauth|u:u1|a:a1"),
                bank.round_stamp("gpt-6-sol", "low", "openai|oauth|u:u2|a:a2")
            );
        }
        assert!(Fingerprint::load(Path::new("does-not-exist")).is_err());
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
    assert!(first.ranking.candidates[0].probability_bps.is_some());
    // Frozen upstream reference fixture verifying 2-round Suspect decision pipeline.
    let expected = Some("claude-opus-4.8");
    let VerdictDecision::Confirm(first) = domain::decide(expected, None, first) else {
        panic!()
    };
    let VerdictDecision::Finish(result) = domain::decide(expected, Some(&first), second) else {
        panic!()
    };
    assert_eq!(result.verdict, Verdict::Suspect);
}
