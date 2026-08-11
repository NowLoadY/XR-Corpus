// SPDX-FileCopyrightText: 2026 febilly
// SPDX-FileCopyrightText: 2026 NowLoadY
// SPDX-License-Identifier: AGPL-3.0-only

//! Automatic, read-only VRCX runtime-corpus provider.
//!
//! The bounded snapshot and stale-context safety model was informed by
//! febilly's MIT-licensed Yakutan VRCX bridge. XRTranslate does not inject code
//! into VRCX: it detects VRCX/VRChat, opens VRCX's documented local game-log
//! SQLite database read-only, and rebuilds the current room snapshot from
//! `gamelog_location` and `gamelog_join_leave`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::State,
};
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use tracing::{info, warn};
use xr_corpus_protocol::VrcxStatusResponse;
use xr_corpus_core::{
    CORPUS_LANGUAGE_ORDER, CORPUS_SCHEMA, CorpusActivation, CorpusDefinition, CorpusTerm,
    DynamicCorpusSource,
};

use crate::AppState;

const PROVIDER_ID: &str = "vrcx";
const RUNTIME_CORPUS_ID: &str = "virtual-worlds.vrchat.runtime-room";
const DATABASE_FILE: &str = "VRCX.sqlite3";
const VRCX_CONFIG_FILE: &str = "VRCX.json";

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct VrcxIntegrationConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub database_path: Option<PathBuf>,
    #[serde(default = "default_snapshot_ttl_seconds")]
    pub snapshot_ttl_seconds: u64,
    #[serde(default = "default_max_players")]
    pub max_players: usize,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
}

impl Default for VrcxIntegrationConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            database_path: None,
            snapshot_ttl_seconds: default_snapshot_ttl_seconds(),
            max_players: default_max_players(),
            poll_interval_ms: default_poll_interval_ms(),
        }
    }
}

const fn default_enabled() -> bool { true }
const fn default_snapshot_ttl_seconds() -> u64 { 30 }
const fn default_max_players() -> usize { 80 }
const fn default_poll_interval_ms() -> u64 { 2_000 }

#[derive(Clone)]
pub(crate) struct VrcxRuntimeSource {
    config: VrcxIntegrationConfig,
    dynamic_source: DynamicCorpusSource,
    project_root: PathBuf,
    status: Arc<RwLock<RuntimeStatus>>,
}

#[derive(Clone, Debug, Default)]
struct RuntimeStatus {
    vrcx_running: bool,
    vrchat_running: bool,
    connected: bool,
    database_path: PathBuf,
    world_name: String,
    player_count: usize,
    term_count: usize,
    updated_at: Option<Instant>,
    last_error: String,
}

#[derive(Debug)]
struct RoomSnapshot {
    world_name: String,
    group_name: String,
    region: String,
    players: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcessState {
    vrcx: bool,
    vrchat: bool,
}

#[derive(Debug)]
enum RefreshOutcome {
    Unavailable {
        vrcx_running: bool,
        vrchat_running: bool,
        database_path: PathBuf,
    },
    Available {
        database_path: PathBuf,
        room: RoomSnapshot,
    },
}

impl VrcxRuntimeSource {
    pub(crate) fn new(
        config: VrcxIntegrationConfig,
        dynamic_source: DynamicCorpusSource,
        project_root: &Path,
    ) -> Result<Self, String> {
        if config.snapshot_ttl_seconds < 5 || config.snapshot_ttl_seconds > 3_600 {
            return Err("integrations.vrcx.snapshot_ttl_seconds must be between 5 and 3600".into());
        }
        if config.max_players == 0 || config.max_players > 256 {
            return Err("integrations.vrcx.max_players must be between 1 and 256".into());
        }
        if config.poll_interval_ms < 500 || config.poll_interval_ms > 60_000 {
            return Err("integrations.vrcx.poll_interval_ms must be between 500 and 60000".into());
        }
        Ok(Self {
            config,
            dynamic_source,
            project_root: project_root.to_owned(),
            status: Arc::new(RwLock::new(RuntimeStatus::default())),
        })
    }

    pub(crate) fn start(&self) -> Option<tokio::task::JoinHandle<()>> {
        if !self.config.enabled {
            return None;
        }
        let source = self.clone();
        Some(tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_millis(source.config.poll_interval_ms));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_logged_error = String::new();
            let mut was_connected = false;
            loop {
                interval.tick().await;
                let worker = source.clone();
                let result =
                    tokio::task::spawn_blocking(move || worker.inspect_local_state()).await;
                let result = match result {
                    Ok(result) => result,
                    Err(error) => Err(format!("VRCX monitor worker failed: {error}")),
                };
                match result {
                    Ok(outcome) => {
                        last_logged_error.clear();
                        let connected = matches!(outcome, RefreshOutcome::Available { .. });
                        if let Err(error) = source.apply_outcome(outcome) {
                            if error != last_logged_error {
                                warn!(%error, "cannot publish VRCX runtime corpus");
                                last_logged_error = error;
                            }
                        } else if connected && !was_connected {
                            info!("VRCX runtime corpus connected automatically");
                        } else if !connected && was_connected {
                            info!("VRCX runtime corpus disconnected and was removed");
                        }
                        was_connected = connected;
                    }
                    Err(error) => {
                        source.set_error(&error);
                        if error != last_logged_error {
                            warn!(%error, "VRCX runtime corpus refresh failed");
                            last_logged_error = error;
                        }
                        was_connected = false;
                    }
                }
            }
        }))
    }

    fn inspect_local_state(&self) -> Result<RefreshOutcome, String> {
        let processes = relevant_processes()?;
        let vrcx_running = processes.vrcx;
        let vrchat_running = processes.vrchat;
        if !vrcx_running || !vrchat_running {
            return Ok(RefreshOutcome::Unavailable {
                vrcx_running,
                vrchat_running,
                database_path: resolve_database_path(
                    self.config.database_path.as_deref(),
                    &self.project_root,
                )
                .unwrap_or_default(),
            });
        }
        let database_path =
            resolve_database_path(self.config.database_path.as_deref(), &self.project_root)?;
        if !database_path.is_file() {
            return Err(format!(
                "VRCX database does not exist: {}",
                database_path.display()
            ));
        }
        let room = read_room_snapshot(&database_path, self.config.max_players)?;
        Ok(RefreshOutcome::Available {
            database_path,
            room,
        })
    }

    fn apply_outcome(&self, outcome: RefreshOutcome) -> Result<(), String> {
        match outcome {
            RefreshOutcome::Unavailable {
                vrcx_running,
                vrchat_running,
                database_path,
            } => {
                self.dynamic_source.remove_provider(PROVIDER_ID)?;
                *self
                    .status
                    .write()
                    .map_err(|_| "VRCX status lock is poisoned".to_owned())? = RuntimeStatus {
                    vrcx_running,
                    vrchat_running,
                    database_path,
                    ..RuntimeStatus::default()
                };
            }
            RefreshOutcome::Available {
                database_path,
                room,
            } => {
                let corpus = room_corpus(&room)?;
                let term_count = corpus.terms.len();
                let player_count = room.players.len();
                let world_name = room.world_name.clone();
                self.dynamic_source.replace_snapshot(
                    PROVIDER_ID,
                    vec![corpus],
                    Some(Duration::from_secs(self.config.snapshot_ttl_seconds)),
                )?;
                *self
                    .status
                    .write()
                    .map_err(|_| "VRCX status lock is poisoned".to_owned())? = RuntimeStatus {
                    vrcx_running: true,
                    vrchat_running: true,
                    connected: true,
                    database_path,
                    world_name,
                    player_count,
                    term_count,
                    updated_at: Some(Instant::now()),
                    last_error: String::new(),
                };
            }
        }
        Ok(())
    }

    fn set_error(&self, error: &str) {
        if let Ok(mut status) = self.status.write() {
            status.connected = status.updated_at.is_some_and(|updated| {
                updated.elapsed() < Duration::from_secs(self.config.snapshot_ttl_seconds)
            });
            status.last_error = error.to_owned();
        }
    }

    fn status_response(&self) -> VrcxStatusResponse {
        let status = self.status.read().map_or_else(
            |_| RuntimeStatus {
                last_error: "VRCX status lock is poisoned".into(),
                ..RuntimeStatus::default()
            },
            |status| status.clone(),
        );
        VrcxStatusResponse {
            enabled: self.config.enabled,
            vrcx_running: status.vrcx_running,
            vrchat_running: status.vrchat_running,
            connected: status.connected,
            database_path: status.database_path.display().to_string(),
            world_name: status.world_name,
            player_count: status.player_count,
            term_count: status.term_count,
            age_ms: status
                .updated_at
                .map(|updated| updated.elapsed().as_millis().min(u128::from(u64::MAX)) as u64),
            last_error: status.last_error,
        }
    }
}

pub(crate) async fn get_status(
    State(state): State<AppState>,
) -> Json<VrcxStatusResponse> {
    Json(state.vrcx.status_response())
}

fn resolve_database_path(
    configured: Option<&Path>,
    project_root: &Path,
) -> Result<PathBuf, String> {
    if let Some(path) = configured.filter(|path| !path.as_os_str().is_empty()) {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            project_root.join(path)
        };
        return Ok(database_file_from_path(&path));
    }
    let app_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "APPDATA is unavailable; configure integrations.vrcx.database_path".to_owned()
        })?;
    let vrcx_directory = app_data.join("VRCX");
    let config_path = vrcx_directory.join(VRCX_CONFIG_FILE);
    let custom = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
        .and_then(|value| {
            value
                .get("VRCX_DatabaseLocation")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|path| {
                    let path = PathBuf::from(path);
                    if path.is_absolute() {
                        path
                    } else {
                        vrcx_directory.join(path)
                    }
                })
        });
    Ok(custom.map_or_else(
        || vrcx_directory.join(DATABASE_FILE),
        |path| database_file_from_path(&path),
    ))
}

fn database_file_from_path(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sqlite3"))
    {
        path.to_owned()
    } else {
        path.join(DATABASE_FILE)
    }
}

fn read_room_snapshot(path: &Path, max_players: usize) -> Result<RoomSnapshot, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("cannot open VRCX database {}: {error}", path.display()))?;
    connection
        .busy_timeout(Duration::from_millis(200))
        .map_err(|error| format!("cannot configure VRCX database timeout: {error}"))?;
    let (created_at, location, world_name, group_name) = connection
        .query_row(
            "SELECT created_at, location, world_name, COALESCE(group_name, '') \
             FROM gamelog_location ORDER BY id DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(|error| format!("cannot read VRCX current location: {error}"))?;

    let scan_limit = max_players.saturating_mul(32).clamp(256, 8_192) as i64;
    let mut statement = connection
        .prepare(
            "SELECT type, display_name, COALESCE(user_id, '') \
             FROM gamelog_join_leave \
             WHERE location = ?1 AND created_at >= ?2 \
             ORDER BY id DESC LIMIT ?3",
        )
        .map_err(|error| format!("cannot prepare VRCX player query: {error}"))?;
    let rows = statement
        .query_map(params![location, created_at, scan_limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("cannot query VRCX players: {error}"))?;
    let mut seen = HashSet::new();
    let mut players = Vec::new();
    for row in rows {
        let (event_type, display_name, user_id) =
            row.map_err(|error| format!("cannot decode VRCX player row: {error}"))?;
        let display_name = clean_term(&display_name, 80);
        if display_name.is_empty() {
            continue;
        }
        let identity = if user_id.is_empty() {
            display_name.to_lowercase()
        } else {
            user_id
        };
        if !seen.insert(identity) {
            continue;
        }
        if event_type == "OnPlayerJoined" {
            players.push(display_name);
            if players.len() >= max_players {
                break;
            }
        }
    }
    players.sort_by_key(|name| name.to_lowercase());
    Ok(RoomSnapshot {
        world_name: clean_term(&world_name, 120),
        group_name: clean_term(&group_name, 120),
        region: location_region(&location),
        players,
    })
}

fn room_corpus(room: &RoomSnapshot) -> Result<CorpusDefinition, String> {
    let mut values = Vec::new();
    push_unique(&mut values, &room.world_name);
    push_unique(&mut values, &room.group_name);
    push_unique(&mut values, &room.region);
    for player in &room.players {
        push_unique(&mut values, player);
    }
    if values.is_empty() {
        values.push("VRChat".into());
    }
    Ok(CorpusDefinition {
        schema: CORPUS_SCHEMA.into(),
        id: RUNTIME_CORPUS_ID.into(),
        domain: "virtual-worlds".into(),
        subdomain: "vrchat".into(),
        title: "Current VRChat room".into(),
        priority: 1_000,
        activation: CorpusActivation::Always,
        triggers: vec![multilingual_proper_name("VRChat")?],
        trigger_aliases: Vec::new(),
        activation_context: Vec::new(),
        terms: values
            .iter()
            .map(|value| multilingual_proper_name(value))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn multilingual_proper_name(value: &str) -> Result<CorpusTerm, String> {
    CorpusTerm::from_ordered(std::iter::repeat_n(value, CORPUS_LANGUAGE_ORDER.len()))
}

fn location_region(location: &str) -> String {
    let Some(start) = location.find("~region(") else {
        return String::new();
    };
    let value = &location[start + "~region(".len()..];
    let end = value.find(')').unwrap_or(value.len());
    clean_term(&value[..end], 24)
}

fn clean_term(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(',', " ")
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !value.is_empty()
        && !values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
    {
        values.push(value.to_owned());
    }
}

#[cfg(windows)]
fn relevant_processes() -> Result<ProcessState, String> {
    use std::{mem, slice};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };

    // SAFETY: ToolHelp owns the snapshot handle. PROCESSENTRY32W is initialized
    // with the documented size, all returned UTF-16 is bounded by szExeFile,
    // and the handle is closed exactly once on every successful snapshot path.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!(
                "cannot enumerate Windows processes: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut state = ProcessState::default();
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let executable = String::from_utf16_lossy(slice::from_raw_parts(
                    entry.szExeFile.as_ptr(),
                    length,
                ));
                if executable.eq_ignore_ascii_case("VRCX.exe")
                    || executable.eq_ignore_ascii_case("VRCX")
                {
                    state.vrcx = true;
                } else if executable.eq_ignore_ascii_case("VRChat.exe")
                    || executable.eq_ignore_ascii_case("VRChat")
                {
                    state.vrchat = true;
                }
                if state.vrcx && state.vrchat {
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        Ok(state)
    }
}

#[cfg(unix)]
fn relevant_processes() -> Result<ProcessState, String> {
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate /proc for VRCX detection: {error}"))?;
    let mut state = ProcessState::default();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Ok(name) = std::fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        match name.trim() {
            "VRCX.exe" | "VRCX" => state.vrcx = true,
            "VRChat.exe" | "VRChat" => state.vrchat = true,
            _ => {}
        }
        if state.vrcx && state.vrchat {
            break;
        }
    }
    Ok(state)
}

#[cfg(not(any(windows, unix)))]
fn relevant_processes() -> Result<ProcessState, String> {
    Ok(ProcessState::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn sqlite_snapshot_reconstructs_only_players_still_in_the_latest_room() {
        let path = std::env::temp_dir().join(format!(
            "xrtranslate-vrcx-{}-{}.sqlite3",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE gamelog_location (\
                    id INTEGER PRIMARY KEY, created_at TEXT, location TEXT, world_id TEXT, \
                    world_name TEXT, time INTEGER, group_name TEXT);\
                 CREATE TABLE gamelog_join_leave (\
                    id INTEGER PRIMARY KEY, created_at TEXT, type TEXT, display_name TEXT, \
                    location TEXT, user_id TEXT, time INTEGER);\
                 INSERT INTO gamelog_location VALUES \
                    (1, '2026-08-11T10:00:00.000Z', 'wrld_old:1~region(us)', 'wrld_old', 'Old', 0, ''),\
                    (2, '2026-08-11T11:00:00.000Z', 'wrld_new:2~region(jp)', 'wrld_new', 'Overwatch Hub', 0, 'Game Night');\
                 INSERT INTO gamelog_join_leave VALUES \
                    (1, '2026-08-11T11:00:01.000Z', 'OnPlayerJoined', 'Alice', 'wrld_new:2~region(jp)', 'usr_a', 0),\
                    (2, '2026-08-11T11:00:02.000Z', 'OnPlayerJoined', 'Bob', 'wrld_new:2~region(jp)', 'usr_b', 0),\
                    (3, '2026-08-11T11:00:03.000Z', 'OnPlayerLeft', 'Alice', 'wrld_new:2~region(jp)', 'usr_a', 0),\
                    (4, '2026-08-11T11:00:04.000Z', 'OnPlayerJoined', 'MercyFan', 'wrld_new:2~region(jp)', 'usr_m', 0);",
            )
            .unwrap();
        drop(connection);

        let room = read_room_snapshot(&path, 80).unwrap();
        assert_eq!(room.world_name, "Overwatch Hub");
        assert_eq!(room.group_name, "Game Night");
        assert_eq!(room.region, "jp");
        assert_eq!(room.players, ["Bob", "MercyFan"]);

        std::fs::remove_file(path).unwrap();
    }
}
