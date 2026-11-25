//! CPR RPC has a 64 KiB metadata limit. One bounded compressed document preserves CAS across
//! schedule, budget, history and queue, without trimming history to fit an RPC frame.
use crate::domain::State;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{Read, Write};

const MAX_ENCODED: usize = 58 * 1024;
const MAX_JSON: usize = 256 * 1024;

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
    serde_json::from_slice(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_document_round_trips_and_rejects_garbage() {
        let state = State::default();
        assert_eq!(decode(&encode(&state).unwrap()), Some(state));
        assert_eq!(decode("garbage!"), None);
        assert_eq!(decode(&"A".repeat(MAX_ENCODED + 1)), None);
    }
}
