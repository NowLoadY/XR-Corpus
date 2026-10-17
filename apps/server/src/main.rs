//! XR Corpus local HTTP service.
//!
//! The service owns vocabulary activation, bounded recognition evidence and
//! immutable vocabulary snapshots. Translation context belongs to callers.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
};
use clap::Parser;
use serde::Deserialize;
use tracing::{info, warn};
use xr_corpus_core::{
    CorpusCatalog, CorpusConfig, DynamicCorpusSource, GraphDomainEdit, GraphEdge, GraphNode,
    GraphNodeStatePatch, GraphSnapshot, GraphStore, GraphTermEdit,
};
use xr_corpus_protocol::{
    API_VERSION, ContextBudgets, CreateSessionRequest, CreateSessionResponse, ErrorResponse,
    HealthResponse, PrepareAsrRequest, PrepareAsrResponse, PrepareTranslationRequest,
    PrepareTranslationResponse, ProviderSnapshotResponse, PublishProviderRequest, SegmentContext,
    SessionStateResponse,
};
use xr_corpus_session::PromptContextManager;

mod vrcx;

const MAX_SNAPSHOTS_PER_SESSION: usize = 12;
const MAX_OBSERVED_TURNS_PER_SESSION: usize = 64;

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
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<CorpusSession>>>>>,
    catalog: CorpusCatalog,
    graph: GraphStore,
    config: CorpusConfig,
    next_session_id: Arc<AtomicU64>,
    session_ttl: Duration,
    dynamic_source: DynamicCorpusSource,
    vrcx: vrcx::VrcxRuntimeSource,
}

struct CorpusSession {
    manager: PromptContextManager,
    snapshots: VecDeque<u64>,
    observed_turn_ids: HashSet<String>,
    observed_turn_order: VecDeque<String>,
    next_context_id: u64,
    last_used: Instant,
}

impl CorpusSession {
    fn should_advance_topic(&mut self, turn_id: Option<&str>, has_text: bool) -> bool {
        if !has_text {
            return false;
        }
        let Some(turn_id) = turn_id.map(str::trim).filter(|value| !value.is_empty()) else {
            // Backward compatibility for clients predating turn IDs.
            return true;
        };
        if !self.observed_turn_ids.insert(turn_id.to_owned()) {
            return false;
        }
        self.observed_turn_order.push_back(turn_id.to_owned());
        while self.observed_turn_order.len() > MAX_OBSERVED_TURNS_PER_SESSION {
            if let Some(expired) = self.observed_turn_order.pop_front() {
                self.observed_turn_ids.remove(&expired);
            }
        }
        true
    }
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
    if !args.listen.ip().is_loopback() {
        return Err("XR Corpus currently accepts loopback listen addresses only".into());
    }
    let config: ServiceConfig = serde_json::from_str(&std::fs::read_to_string(&args.config)?)?;
    PromptContextManager::validate(&config.prompt_context)?;
    let project_root = args
        .config
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let catalog = CorpusCatalog::load(&config.prompt_context, &project_root)?;
    let graph = catalog.graph_store().ok_or("graph store unavailable")?;
    let corpus_count = catalog.snapshot()?.len();
    let dynamic_source = catalog.dynamic_source();
    let vrcx = vrcx::VrcxRuntimeSource::new(
        config.integrations.vrcx.clone(),
        dynamic_source.clone(),
        &project_root,
    )?;
    let _vrcx_monitor = vrcx.start();
    let state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
        catalog,
        graph,
        config: config.prompt_context,
        next_session_id: Arc::new(AtomicU64::new(1)),
        session_ttl: Duration::from_secs(args.session_idle_seconds.max(30)),
        dynamic_source,
        vrcx,
    };
    spawn_session_reaper(state.clone());
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v3/graph", get(graph_snapshot))
        .route(
            "/v3/graph/domains/{id}",
            put(upsert_domain).delete(delete_domain),
        )
        .route("/v3/graph/nodes", put(patch_node_state))
        .route("/v3/graph/terms/{id}", put(save_term))
        .route(
            "/v3/graph/terms",
            post(import_terms).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/v3/graph/nodes/{id}", put(upsert_node).delete(delete_node))
        .route("/v3/graph/edges", put(upsert_edge).delete(delete_edge))
        .route("/v3/integrations/vrcx/status", get(vrcx::get_status))
        .route(
            "/v3/providers/{provider_id}",
            put(publish_provider).delete(delete_provider),
        )
        .route("/v3/sessions", post(create_session))
        .route(
            "/v3/sessions/{session_id}",
            get(session_state).delete(delete_session),
        )
        .route("/v3/sessions/{session_id}/asr", post(prepare_asr))
        .route(
            "/v3/sessions/{session_id}/translation",
            post(prepare_translation),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    info!(address = %args.listen, corpus_count, "XR Corpus is ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> ApiResult<HealthResponse> {
    let session_count = state.sessions.lock().map(|value| value.len()).unwrap_or(0);
    let corpus_count = state.catalog.snapshot().map_err(internal_error)?.len();
    Ok(Json(HealthResponse {
        status: "ok".into(),
        api_version: API_VERSION,
        server_version: env!("CARGO_PKG_VERSION").into(),
        corpus_count,
        session_count,
    }))
}

async fn graph_snapshot(State(state): State<AppState>) -> ApiResult<GraphSnapshot> {
    Ok(Json(state.graph.snapshot().map_err(internal_error)?))
}

async fn upsert_domain(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(edit): Json<GraphDomainEdit>,
) -> ApiResult<GraphSnapshot> {
    if id != edit.domain.id {
        return Err(bad_request("id_mismatch", "domain ID differs from URL"));
    }
    state
        .graph
        .save_domain_rules(&edit)
        .map_err(|message| bad_request("invalid_domain", message))?;
    graph_snapshot(State(state)).await
}

async fn delete_domain(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<GraphSnapshot> {
    state
        .graph
        .delete_domain(&id)
        .map_err(|message| bad_request("invalid_domain", message))?;
    graph_snapshot(State(state)).await
}

async fn import_terms(
    State(state): State<AppState>,
    Json(nodes): Json<Vec<GraphNode>>,
) -> ApiResult<GraphSnapshot> {
    state
        .graph
        .import_terms(&nodes)
        .map_err(|message| bad_request("invalid_terms", message))?;
    graph_snapshot(State(state)).await
}

async fn save_term(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(edit): Json<GraphTermEdit>,
) -> ApiResult<GraphSnapshot> {
    if id != edit.node.id {
        return Err(bad_request("id_mismatch", "term ID differs from URL"));
    }
    state
        .graph
        .save_term(&edit)
        .map_err(|message| bad_request("invalid_term", message))?;
    graph_snapshot(State(state)).await
}

async fn upsert_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(node): Json<GraphNode>,
) -> ApiResult<GraphSnapshot> {
    if id != node.id {
        return Err(bad_request("id_mismatch", "node ID differs from URL"));
    }
    state
        .graph
        .upsert_node(&node)
        .map_err(|message| bad_request("invalid_node", message))?;
    graph_snapshot(State(state)).await
}

async fn delete_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<GraphSnapshot> {
    state
        .graph
        .delete_node(&id)
        .map_err(|message| bad_request("invalid_node", message))?;
    graph_snapshot(State(state)).await
}

async fn patch_node_state(
    State(state): State<AppState>,
    Json(patch): Json<GraphNodeStatePatch>,
) -> ApiResult<GraphSnapshot> {
    state
        .graph
        .patch_node_state(&patch)
        .map_err(|message| bad_request("invalid_node_state", message))?;
    graph_snapshot(State(state)).await
}

async fn upsert_edge(
    State(state): State<AppState>,
    Json(edge): Json<GraphEdge>,
) -> ApiResult<GraphSnapshot> {
    state
        .graph
        .upsert_edge(&edge)
        .map_err(|message| bad_request("invalid_edge", message))?;
    graph_snapshot(State(state)).await
}

async fn delete_edge(
    State(state): State<AppState>,
    Json(edge): Json<GraphEdge>,
) -> ApiResult<GraphSnapshot> {
    state
        .graph
        .delete_edge(&edge.source_id, &edge.target_id, edge.kind)
        .map_err(|message| bad_request("invalid_edge", message))?;
    graph_snapshot(State(state)).await
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
        Arc::new(Mutex::new(CorpusSession {
            manager,
            snapshots: VecDeque::new(),
            observed_turn_ids: HashSet::new(),
            observed_turn_order: VecDeque::new(),
            next_context_id: 1,
            last_used: Instant::now(),
        })),
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

async fn session_state(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<SessionStateResponse> {
    let session = find_session(&state, &session_id)?;
    let session = session.lock().map_err(lock_error)?;
    Ok(Json(SessionStateResponse {
        session_id,
        active_corpus_ids: session.manager.active_corpus_ids(),
        snapshot_count: session.snapshots.len(),
    }))
}

async fn publish_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(request): Json<PublishProviderRequest>,
) -> ApiResult<ProviderSnapshotResponse> {
    if !(5..=3_600).contains(&request.ttl_seconds) {
        return Err(bad_request(
            "provider_ttl_out_of_range",
            "ttl_seconds must be between 5 and 3600",
        ));
    }
    let corpus_count = request.corpora.len();
    state
        .dynamic_source
        .replace_snapshot(
            &provider_id,
            request.corpora,
            Some(Duration::from_secs(request.ttl_seconds)),
        )
        .map_err(|message| bad_request("invalid_provider_snapshot", message))?;
    Ok(Json(ProviderSnapshotResponse {
        provider_id,
        corpus_count,
        ttl_seconds: request.ttl_seconds,
    }))
}

async fn delete_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    state
        .dynamic_source
        .remove_provider(&provider_id)
        .map_err(internal_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn prepare_asr(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PrepareAsrRequest>,
) -> ApiResult<PrepareAsrResponse> {
    let session = find_session(&state, &session_id)?;
    let mut session = session.lock().map_err(lock_error)?;
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
    let context_id = insert_snapshot(&mut session);
    Ok(Json(PrepareAsrResponse {
        context_id,
        vocabulary: snapshot.asr_vocabulary().to_vec(),
        prompt: snapshot.asr_prompt(),
    }))
}

async fn prepare_translation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PrepareTranslationRequest>,
) -> ApiResult<PrepareTranslationResponse> {
    let session = find_session(&state, &session_id)?;
    let mut session = session.lock().map_err(lock_error)?;
    session.last_used = Instant::now();
    if !session.snapshots.contains(&request.asr_context_id) {
        return Err(bad_request(
            "context_expired",
            "ASR context snapshot has expired",
        ));
    }
    let advance_topic = session.should_advance_topic(
        request.turn_id.as_deref(),
        !request.recognized_text.trim().is_empty(),
    );
    let snapshot = session
        .manager
        .select_turn_observation(
            &request.source_language,
            &request.target_language,
            &[request.recognized_text.as_str()],
            budgets(request.budgets),
            advance_topic,
            &request
                .context_evidence
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        )
        .map_err(internal_error)?;
    let source_corrections = snapshot.recognition_corrections(&request.recognized_text);
    session
        .manager
        .record_transcript_turn(request.turn_id.as_deref(), &request.recognized_text);
    let segments = request
        .segments
        .iter()
        .map(|text| {
            let source_corrections = snapshot.recognition_corrections(text);
            SegmentContext {
                prompt_terms: snapshot.translation_prompt_terms_for(&text),
                source_corrections,
                activation_matches: snapshot.activation_matches(&text),
                context_matches: snapshot.recognition_context_matches(&text),
                corrected_text: text.clone(),
            }
        })
        .collect();
    let context_id = insert_snapshot(&mut session);
    Ok(Json(PrepareTranslationResponse {
        context_id,
        corrected_text: request.recognized_text,
        source_corrections,
        segments,
    }))
}

fn insert_snapshot(session: &mut CorpusSession) -> u64 {
    let context_id = session.next_context_id;
    session.next_context_id = session.next_context_id.saturating_add(1);
    session.snapshots.push_back(context_id);
    if session.snapshots.len() > MAX_SNAPSHOTS_PER_SESSION {
        session.snapshots.pop_front();
    }
    context_id
}

fn budgets(value: ContextBudgets) -> (usize, usize) {
    (value.asr_tokens, value.translation_tokens)
}

fn find_session(
    state: &AppState,
    session_id: &str,
) -> Result<Arc<Mutex<CorpusSession>>, (StatusCode, Json<ErrorResponse>)> {
    state
        .sessions
        .lock()
        .map_err(lock_error)?
        .get(session_id)
        .cloned()
        .ok_or_else(session_not_found)
}

fn spawn_session_reaper(state: AppState) {
    tokio::spawn(async move {
        let interval =
            (state.session_ttl / 2).clamp(Duration::from_secs(15), Duration::from_secs(60));
        loop {
            tokio::time::sleep(interval).await;
            let Ok(mut sessions) = state.sessions.lock() else {
                warn!("XR Corpus session store lock was poisoned");
                continue;
            };
            let before = sessions.len();
            sessions.retain(|_, session| {
                session
                    .lock()
                    .is_ok_and(|session| session.last_used.elapsed() < state.session_ttl)
            });
            let removed = before.saturating_sub(sessions.len());
            if removed > 0 {
                info!(removed, "expired idle XR Corpus sessions");
            }
        }
    });
}

fn session_not_found() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            code: "session_not_found".into(),
            error: "corpus session does not exist".into(),
        }),
    )
}

fn bad_request(
    code: impl Into<String>,
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            code: code.into(),
            error: message.into(),
        }),
    )
}

fn internal_error(message: impl ToString) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            code: "internal_error".into(),
            error: message.to_string(),
        }),
    )
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> (StatusCode, Json<ErrorResponse>) {
    internal_error(format!("corpus session state is unavailable: {error}"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => info!("shutdown signal received"),
            Err(error) => {
                warn!(%error, "Ctrl+C listener is unavailable; keeping XR Corpus running");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => match signal.recv().await {
                Some(()) => info!("termination signal received"),
                None => {
                    warn!("termination signal stream closed; keeping XR Corpus running");
                    std::future::pending::<()>().await;
                }
            },
            Err(error) => {
                warn!(%error, "termination signal listener is unavailable; keeping XR Corpus running");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
