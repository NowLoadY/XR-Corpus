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
    pub source_language: String,
    pub target_language: String,
    pub recognized_text: String,
    pub segments: Vec<String>,
    pub budgets: ContextBudgets,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentContext {
    pub corrected_text: String,
    pub prompt: Option<String>,
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
pub struct RecordTranslationRequest {
    pub context_id: u64,
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
