//! Versioned wire types for the XR Corpus local service.

use serde::{Deserialize, Serialize};
pub use xr_corpus_core::{
    CORPUS_LANGUAGE_ORDER, CORPUS_SCHEMA, CorpusActivation, CorpusDefinition, CorpusTerm,
};
pub const API_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusTermMatch {
    pub start_byte: u32,
    pub end_byte: u32,
    pub text: String,
    pub sources: Vec<CorpusTermSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusTermSource {
    pub corpus_id: String,
    pub domain: String,
    pub subdomain: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusPromptTerm {
    pub values: Vec<(String, String)>,
    pub sources: Vec<CorpusTermSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusRecognitionCorrection {
    pub start_byte: u32,
    pub end_byte: u32,
    pub original_text: String,
    pub corrected_text: String,
    pub sources: Vec<CorpusTermSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub api_version: u16,
    #[serde(default)]
    pub server_version: String,
    pub corpus_count: usize,
    pub session_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CreateSessionRequest {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionStateResponse {
    pub session_id: String,
    pub active_corpus_ids: Vec<String>,
    pub snapshot_count: usize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ContextBudgets {
    pub asr_tokens: usize,
    pub translation_tokens: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareAsrRequest {
    pub source_language: String,
    pub target_language: String,
    pub budgets: ContextBudgets,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareAsrResponse {
    pub context_id: u64,
    pub prompt: Option<String>,
    pub echo_guard: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareTranslationRequest {
    pub asr_context_id: u64,
    /// Stable identifier for one user utterance. Streaming revisions of the
    /// same utterance reuse this value so topic decay advances only once.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// Neutral speaker identity assigned by recognition infrastructure.
    #[serde(default)]
    pub speaker_id: String,
    pub source_language: String,
    pub target_language: String,
    pub recognized_text: String,
    pub segments: Vec<String>,
    pub budgets: ContextBudgets,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentContext {
    pub corrected_text: String,
    #[serde(default)]
    pub prompt_terms: Vec<CorpusPromptTerm>,
    /// Neutral, already-bounded conversation facts. Callers may use these to
    /// compose a model prompt without depending on XR Corpus's default text
    /// rendering or internal session types.
    #[serde(default)]
    pub context_data: TranslationContextData,
    #[serde(default)]
    pub source_corrections: Vec<CorpusRecognitionCorrection>,
    pub activation_matches: Vec<CorpusTermMatch>,
    pub context_matches: Vec<CorpusTermMatch>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationContextData {
    /// Completed earlier logical turns, oldest first and bounded by the
    /// session's configured history and model budget.
    #[serde(default)]
    pub recent_turns: Vec<BilingualContextTurn>,
    /// The preceding overlapping window for the same logical speech turn.
    #[serde(default)]
    pub previous_revision: Option<BilingualContextTurn>,
    /// Source text surrounding this exact translation segment. The segment
    /// itself is intentionally absent so it cannot be translated twice.
    #[serde(default)]
    pub surrounding_source: Option<SurroundingSourceContext>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BilingualContextTurn {
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub speaker_id: String,
    pub source_language: String,
    pub target_language: String,
    pub source_text: String,
    pub translated_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurroundingSourceContext {
    #[serde(default)]
    pub speaker_id: String,
    pub source_language: String,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareTranslationResponse {
    pub context_id: u64,
    pub corrected_text: String,
    #[serde(default)]
    pub source_corrections: Vec<CorpusRecognitionCorrection>,
    pub segments: Vec<SegmentContext>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordTranslationRequest {
    pub context_id: u64,
    /// Stable logical speech-turn identity. Repeated IDs update the latest
    /// continuous recognition revision instead of appending overlap as a new
    /// dialogue turn.
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub speaker_id: String,
    pub source_language: String,
    pub target_language: String,
    pub source_text: String,
    pub translated_text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordTranslationResponse {
    pub term_matches: Vec<CorpusTermMatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    #[serde(default = "default_error_code")]
    pub code: String,
    pub error: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishProviderRequest {
    /// Full atomic snapshot. Publishing again replaces the previous snapshot.
    pub corpora: Vec<CorpusDefinition>,
    /// Required expiry for runtime data. Accepted range: 5–3600 seconds.
    #[serde(default = "default_provider_ttl_seconds")]
    pub ttl_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderSnapshotResponse {
    pub provider_id: String,
    pub corpus_count: usize,
    pub ttl_seconds: u64,
}

const fn default_provider_ttl_seconds() -> u64 {
    60
}

fn default_error_code() -> String {
    "unknown_error".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VrcxStatusResponse {
    pub enabled: bool,
    pub vrcx_running: bool,
    pub vrchat_running: bool,
    pub connected: bool,
    pub database_path: String,
    pub world_name: String,
    pub player_count: usize,
    pub term_count: usize,
    pub age_ms: Option<u64>,
    pub last_error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_turn_metadata_is_backward_compatible() {
        let request: RecordTranslationRequest = serde_json::from_str(
            r#"{"context_id":1,"source_language":"en","target_language":"zh","source_text":"hello","translated_text":"你好"}"#,
        )
        .unwrap();
        assert_eq!(request.turn_id, None);
        assert!(request.speaker_id.is_empty());

        let request: PrepareTranslationRequest = serde_json::from_str(
            r#"{"asr_context_id":1,"source_language":"en","target_language":"zh","recognized_text":"hello","segments":["hello"],"budgets":{"asr_tokens":64,"translation_tokens":128}}"#,
        )
        .unwrap();
        assert_eq!(request.turn_id, None);
        assert!(request.speaker_id.is_empty());
    }

    #[test]
    fn structured_translation_context_is_serialized_for_translation() {
        let segment: SegmentContext = serde_json::from_str(
            r#"{"corrected_text":"hello","activation_matches":[],"context_matches":[]}"#,
        )
        .unwrap();
        assert_eq!(segment.context_data, TranslationContextData::default());

        let encoded = serde_json::to_value(SegmentContext {
            corrected_text: "Tell the team.".into(),
            prompt_terms: Vec::new(),
            context_data: TranslationContextData {
                recent_turns: vec![BilingualContextTurn {
                    turn_id: Some("previous".into()),
                    speaker_id: "speaker-01".into(),
                    source_language: "en".into(),
                    target_language: "zh".into(),
                    source_text: "The plan changed.".into(),
                    translated_text: "计划变了。".into(),
                }],
                previous_revision: None,
                surrounding_source: Some(SurroundingSourceContext {
                    speaker_id: "speaker-01".into(),
                    source_language: "en".into(),
                    before: "If it slips, move it to Friday.".into(),
                    after: String::new(),
                }),
            },
            source_corrections: Vec::new(),
            activation_matches: Vec::new(),
            context_matches: Vec::new(),
        })
        .unwrap();
        assert_eq!(
            encoded["context_data"]["recent_turns"][0]["turn_id"],
            "previous"
        );
        assert_eq!(
            encoded["context_data"]["surrounding_source"]["before"],
            "If it slips, move it to Friday."
        );
    }
}
