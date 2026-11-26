//! CPR RPC has a 64 KiB metadata limit. One bounded compressed document preserves CAS across
//! schedule, history and queue, trimming oldest history only if compression nears the frame cap.
use crate::domain::State;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{Read, Write};

const MAX_ENCODED: usize = 58 * 1024;
const MAX_JSON: usize = 512 * 1024;

pub fn encode(state: &State) -> Option<String> {
    let raw = serde_json::to_vec(state).ok()?;
    if raw.len() > MAX_JSON {
        return None;
    }
    let mut writer = ZlibEncoder::new(Vec::new(), Compression::best());
    writer.write_all(&raw).ok()?;
    let result = STANDARD.encode(writer.finish().ok()?);
    (result.len() <= MAX_ENCODED).then_some(result)
}

pub fn encode_bounded(state: &mut State) -> Option<String> {
    if let Some(encoded) = encode(state) {
        return Some(encoded);
    }
    // Strip oldest persisted per-run diagnostics first so all history summaries stay intact.
    for idx in (0..state.history.len()).rev() {
        if state.history[idx].diagnostics.take().is_some()
            && let Some(encoded) = encode(state)
        {
            return Some(encoded);
        }
    }
    while !state.history.is_empty() {
        let next_len = (state.history.len() * 3) / 4;
        state.history.truncate(next_len);
        if let Some(encoded) = encode(state) {
            return Some(encoded);
        }
    }
    None
}

pub fn decode(document: &str) -> Option<State> {
    if document.len() > MAX_ENCODED {
        return None;
    }
    let compressed = STANDARD.decode(document).ok()?;
    let mut raw = Vec::new();
    ZlibDecoder::new(compressed.as_slice())
        .take((MAX_JSON + 1) as u64)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() > MAX_JSON {
        return None;
    }
    let mut state: State = serde_json::from_slice(&raw).ok()?;
    state.migrate_legacy();
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Candidate, HistoryItem, Outcome, ProbeDiagnostic, Ranking, Round, RunDiagnostic, Verdict,
    };

    #[test]
    fn compressed_document_round_trips_and_rejects_garbage() {
        let state = State::default();
        assert_eq!(decode(&encode(&state).unwrap()), Some(state));
        assert_eq!(decode("garbage!"), None);
        assert_eq!(decode(&"A".repeat(MAX_ENCODED + 1)), None);
    }

    #[test]
    fn five_hundred_history_entries_fit_comfortably_within_rpc_limit() {
        let mut state = State::default();
        for idx in 0..500_u64 {
            state.history.push(HistoryItem {
                run_id: format!("m-batch{idx:08x}"),
                account_id: format!("acct_01a17bf1b2ec7659bfd910025398{:04x}", idx % 50),
                account_email: Some(format!("user{}@example.com", idx % 50)),
                account_name: Some(format!("Account {}", idx % 50)),
                model: "gpt-6-sol".into(),
                effort: "medium".into(),
                legacy_profile_id: None,
                scheduled: idx % 3 == 0,
                ended_at: 1_795_600_000_000 + idx * 1000,
                plugin_version: "0.2.0".into(),
                first_round: None,
                outcome: Outcome {
                    verdict: Verdict::Match,
                    reason: None,
                    last_round: Some(Round {
                        ranking: Ranking {
                            candidates: vec![
                                Candidate {
                                    id: "gpt-6-sol".into(),
                                    label: "gpt-6-sol".into(),
                                    probability_bps: Some(9210),
                                },
                                Candidate {
                                    id: "gpt-6-luna".into(),
                                    label: "gpt-6-luna".into(),
                                    probability_bps: Some(450),
                                },
                                Candidate {
                                    id: "gpt-5.5".into(),
                                    label: "gpt-5.5".into(),
                                    probability_bps: Some(180),
                                },
                            ],
                        },
                        stamp: "a".repeat(64),
                        detector_revision: "e0472890bea827d15ca883636a77d4c10bbe843b".into(),
                        top_margin_bps: 8760,
                    }),
                },
                diagnostics: Some(RunDiagnostic {
                    round: 1,
                    stage: "completed".into(),
                    duration_ms: 14_200 + idx,
                    credential_revision_before: Some(3),
                    credential_revision_after: Some(4),
                    identity_changed: false,
                    plan_type: Some("pro".into()),
                    expected_label: Some("gpt-6-sol".into()),
                    detector_decision: Some("Scored".into()),
                    used_outputs: Some(3),
                    probability_status: Some("ReferenceCalibrated".into()),
                    error_detail: None,
                    auto_disabled: None,
                    auto_disable_note: None,
                    auto_disable_error: None,
                    probes: (1..=3)
                        .map(|p| ProbeDiagnostic {
                            index: p,
                            challenge_id: format!("c-{p}"),
                            expected_count: 20,
                            duration_ms: 4_500,
                            ok: true,
                            batches: 40,
                            events: 120,
                            text_parts: 1,
                            text_chars: 180,
                            reasoning_chars: 900,
                            finish_reason: Some("Stop".into()),
                            wire_terminal: None,
                            error_reason: None,
                            error_detail: None,
                        })
                        .collect(),
                }),
            });
        }
        let encoded = encode_bounded(&mut state).unwrap();
        assert!(encoded.len() <= MAX_ENCODED);
        assert_eq!(state.history.len(), 500);
        assert_eq!(decode(&encoded).unwrap().history.len(), 500);
    }
}
