//! Versioned wire types for the XR Corpus local service.

use serde::{Deserialize, Serialize};
pub use xr_corpus_core::{
    CORPUS_LANGUAGE_ORDER, CORPUS_SCHEMA, CorpusActivation, CorpusDefinition, CorpusTerm,
    GraphDomain, GraphDomainEdit, GraphEdge, GraphEdgeKind, GraphNode, GraphNodeStatePatch,
    GraphSnapshot, GraphTermEdit, RecognitionAlias, RecognitionAliases, VRCX_DOMAIN_ID,
};
pub const API_VERSION: u16 = 3;

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
    /// Structured recognition vocabulary selected for this turn. Consumers
    /// decide whether to render it as prompt context or deliver it through a
    /// provider's weighted vocabulary contract.
    #[serde(default)]
    pub vocabulary: Vec<String>,
    /// Compact display of the selected recognition vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareTranslationRequest {
    pub asr_context_id: u64,
    /// Stable identifier for one user utterance. Streaming revisions of the
    /// same utterance reuse this value so topic decay advances only once.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// Relevant caller-owned context for vocabulary ranking and topic continuity.
    /// It does not count as a new topic trigger or become retained dialogue.
    #[serde(default)]
    pub context_evidence: Vec<String>,
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
    #[serde(default)]
    pub source_corrections: Vec<CorpusRecognitionCorrection>,
    pub activation_matches: Vec<CorpusTermMatch>,
    pub context_matches: Vec<CorpusTermMatch>,
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
    fn optional_context_evidence_defaults_to_empty() {
        let request: PrepareTranslationRequest = serde_json::from_str(
            r#"{"asr_context_id":1,"source_language":"en","target_language":"zh","recognized_text":"hello","segments":["hello"],"budgets":{"asr_tokens":64,"translation_tokens":128}}"#,
        ).unwrap();
        assert_eq!(request.turn_id, None);
        assert!(request.context_evidence.is_empty());
        let segment: SegmentContext = serde_json::from_str(
            r#"{"corrected_text":"hello","activation_matches":[],"context_matches":[]}"#,
        )
        .unwrap();
        assert!(segment.prompt_terms.is_empty());
        assert!(segment.source_corrections.is_empty());
    }
}
