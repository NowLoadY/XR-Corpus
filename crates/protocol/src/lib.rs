//! Versioned wire types for the XR Corpus local service.

use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub api_version: u16,
    pub corpus_count: usize,
    pub session_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CreateSessionRequest {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
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
    pub activation_matches: Vec<CorpusTermMatch>,
    pub context_matches: Vec<CorpusTermMatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrepareTranslationResponse {
    pub context_id: u64,
    pub corrected_text: String,
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
    pub error: String,
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
