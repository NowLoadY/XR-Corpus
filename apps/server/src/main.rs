//! XR Corpus local HTTP service.
//!
//! The service, rather than each inference caller, owns activation state,
//! bounded bilingual history and immutable per-request context snapshots.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use clap::Parser;
use serde::Deserialize;
use tracing::{info, warn};
use xr_corpus_protocol::{
    API_VERSION, ContextBudgets, CreateSessionRequest, CreateSessionResponse, ErrorResponse,
    HealthResponse, PrepareAsrRequest, PrepareAsrResponse, PrepareTranslationRequest,
    PrepareTranslationResponse, RecordTranslationRequest, RecordTranslationResponse,
    SegmentContext,
};
use xr_corpus_session::{PromptContextManager, PromptContextSnapshot};
use xr_corpus_core::{CorpusCatalog, CorpusConfig};

mod vrcx;

const MAX_SNAPSHOTS_PER_SESSION: usize = 12;

#[derive(Debug, Parser)]
#[command(name = "xr-corpus-server", version, about = "XR Corpus local service")]
struct Arguments {
    #[arg(long, default_value = "config.json")]
    config: PathBuf,
    #[arg(long, default_value = "127.0.0.1:7766")]
    listen: SocketAddr,
    #[arg(long, default_value_t = 900)]
    session_idle_seconds: u64,
}

#[derive(Clone)]
struct AppState {
    sessions: Arc<Mutex<HashMap<String, CorpusSession>>>,
    catalog: CorpusCatalog,
    config: CorpusConfig,
    next_session_id: Arc<AtomicU64>,
    session_ttl: Duration,
    corpus_count: usize,
    vrcx: vrcx::VrcxRuntimeSource,
}

struct CorpusSession {
    manager: PromptContextManager,
    snapshots: HashMap<u64, PromptContextSnapshot>,
    next_context_id: u64,
    last_used: Instant,
}

#[derive(Debug, Deserialize)]
struct ServiceConfig {
    #[serde(default)]
    prompt_context: CorpusConfig,
    #[serde(default)]
    integrations: IntegrationsConfig,
}

#[derive(Debug, Default, Deserialize)]
struct IntegrationsConfig {
    #[serde(default)]
    vrcx: vrcx::VrcxIntegrationConfig,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
    let args = Arguments::parse();
    let config: ServiceConfig = serde_json::from_str(&std::fs::read_to_string(&args.config)?)?;
    PromptContextManager::validate(&config.prompt_context)?;
    let project_root = args
        .config
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let catalog = CorpusCatalog::load(&config.prompt_context, &project_root)?;
    let corpus_count = catalog.snapshot()?.len();
    let vrcx = vrcx::VrcxRuntimeSource::new(
        config.integrations.vrcx.clone(),
        catalog.dynamic_source(),
        &project_root,
    )?;
    let _vrcx_monitor = vrcx.start();
    let state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        catalog,
        config: config.prompt_context,
        next_session_id: Arc::new(AtomicU64::new(1)),
        session_ttl: Duration::from_secs(args.session_idle_seconds.max(30)),
        corpus_count,
        vrcx,
    };
    spawn_session_reaper(state.clone());
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/integrations/vrcx/status", get(vrcx::get_status))
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions/{session_id}", delete(delete_session))
        .route("/v1/sessions/{session_id}/asr", post(prepare_asr))
        .route(
            "/v1/sessions/{session_id}/translation",
            post(prepare_translation),
        )
        .route(
            "/v1/sessions/{session_id}/results",
            post(record_translation),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    info!(address = %args.listen, corpus_count, "XR Corpus is ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let session_count = state.sessions.lock().map(|value| value.len()).unwrap_or(0);
    Json(HealthResponse {
        status: "ok".into(),
        api_version: API_VERSION,
        corpus_count: state.corpus_count,
        session_count,
    })
}

async fn create_session(
    State(state): State<AppState>,
    Json(_request): Json<CreateSessionRequest>,
) -> ApiResult<CreateSessionResponse> {
    let id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
    let session_id = format!("session-{id}");
    let manager = PromptContextManager::new(state.config.clone(), state.catalog.clone())
        .map_err(internal_error)?;
    state.sessions.lock().map_err(lock_error)?.insert(
        session_id.clone(),
        CorpusSession {
            manager,
            snapshots: HashMap::new(),
            next_context_id: 1,
            last_used: Instant::now(),
        },
    );
    Ok(Json(CreateSessionResponse { session_id }))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> StatusCode {
    match state.sessions.lock() {
        Ok(mut sessions) => {
            if sessions.remove(&session_id).is_some() {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::NOT_FOUND
            }
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn prepare_asr(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PrepareAsrRequest>,
) -> ApiResult<PrepareAsrResponse> {
    let mut sessions = state.sessions.lock().map_err(lock_error)?;
    let session = sessions.get_mut(&session_id).ok_or_else(session_not_found)?;
    session.last_used = Instant::now();
    let snapshot = session
        .manager
        .select(
            &request.source_language,
            &request.target_language,
            &[],
            budgets(request.budgets),
        )
        .map_err(internal_error)?;
    let context_id = insert_snapshot(session, snapshot.clone());
    Ok(Json(PrepareAsrResponse {
        context_id,
        prompt: snapshot.asr_prompt(),
        echo_guard: snapshot.asr_echo_guard().to_vec(),
    }))
}

async fn prepare_translation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PrepareTranslationRequest>,
) -> ApiResult<PrepareTranslationResponse> {
    let mut sessions = state.sessions.lock().map_err(lock_error)?;
    let session = sessions.get_mut(&session_id).ok_or_else(session_not_found)?;
    session.last_used = Instant::now();
    let asr_snapshot = session
        .snapshots
        .get(&request.asr_context_id)
        .cloned()
        .ok_or_else(|| bad_request("ASR context snapshot has expired"))?;
    let corrected_text = asr_snapshot.correct_recognition_proper_names(&request.recognized_text);
    let corrected_segments = request
        .segments
        .iter()
        .map(|text| asr_snapshot.correct_recognition_proper_names(text))
        .collect::<Vec<_>>();
    let snapshot = session
        .manager
        .select(
            &request.source_language,
            &request.target_language,
            &[corrected_text.as_str()],
            budgets(request.budgets),
        )
        .map_err(internal_error)?;
    session.manager.record_transcript(&corrected_text);
    let segments = corrected_segments
        .into_iter()
        .map(|text| SegmentContext {
            prompt: snapshot.translation_prompt_for(&text),
            activation_matches: snapshot.activation_matches(&text),
            context_matches: snapshot.recognition_context_matches(&text),
            corrected_text: text,
        })
        .collect();
    let context_id = insert_snapshot(session, snapshot);
    Ok(Json(PrepareTranslationResponse {
        context_id,
        corrected_text,
        segments,
    }))
}

async fn record_translation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<RecordTranslationRequest>,
) -> ApiResult<RecordTranslationResponse> {
    let mut sessions = state.sessions.lock().map_err(lock_error)?;
    let session = sessions.get_mut(&session_id).ok_or_else(session_not_found)?;
    session.last_used = Instant::now();
    let snapshot = session
        .snapshots
        .get(&request.context_id)
        .cloned()
        .ok_or_else(|| bad_request("translation context snapshot has expired"))?;
    let term_matches = snapshot.translation_term_matches(
        &request.source_text,
        &request.translated_text,
        &request.target_language,
    );
    session.manager.record_translation(
        &request.source_language,
        &request.target_language,
        &request.source_text,
        &request.translated_text,
    );
    Ok(Json(RecordTranslationResponse { term_matches }))
}

fn insert_snapshot(session: &mut CorpusSession, snapshot: PromptContextSnapshot) -> u64 {
    let context_id = session.next_context_id;
    session.next_context_id = session.next_context_id.saturating_add(1);
    session.snapshots.insert(context_id, snapshot);
    if session.snapshots.len() > MAX_SNAPSHOTS_PER_SESSION
        && let Some(oldest) = session.snapshots.keys().min().copied()
    {
        session.snapshots.remove(&oldest);
    }
    context_id
}

fn budgets(value: ContextBudgets) -> (usize, usize) {
    (value.asr_tokens, value.translation_tokens)
}

fn spawn_session_reaper(state: AppState) {
    tokio::spawn(async move {
        let interval = (state.session_ttl / 2).clamp(Duration::from_secs(15), Duration::from_secs(60));
        loop {
            tokio::time::sleep(interval).await;
            let Ok(mut sessions) = state.sessions.lock() else {
                warn!("XR Corpus session store lock was poisoned");
                continue;
            };
            let before = sessions.len();
            sessions.retain(|_, session| session.last_used.elapsed() < state.session_ttl);
            let removed = before.saturating_sub(sessions.len());
            if removed > 0 {
                info!(removed, "expired idle XR Corpus sessions");
            }
        }
    });
}

fn session_not_found() -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "corpus session does not exist".into() }))
}

fn bad_request(message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: message.into() }))
}

fn internal_error(message: impl ToString) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: message.to_string() }))
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> (StatusCode, Json<ErrorResponse>) {
    internal_error(format!("corpus session state is unavailable: {error}"))
}

async fn shutdown_signal() {
    let ctrl_c = async { let _ = tokio::signal::ctrl_c().await; };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
