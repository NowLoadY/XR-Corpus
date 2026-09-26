//! Persistent terminology graph and transient runtime corpus snapshots.

mod graph;

pub use graph::{
    GraphDomain, GraphEdge, GraphEdgeKind, GraphNode, GraphNodeStatePatch, GraphSnapshot,
    GraphStore, validate_seed_database,
};

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
pub const CORPUS_SCHEMA: &str = "xrtranslate-corpus/v1";
pub const VRCX_DOMAIN_ID: &str = "vrcx";
pub const CORPUS_LANGUAGE_ORDER: &[&str] = &[
    "zh", "en", "fr", "pt", "es", "ja", "ru", "ko", "th", "it", "de", "vi", "id", "pl", "cs", "nl",
];
const MAX_TRIGGERS: usize = 128;
const MAX_TERMS: usize = 512;
const MAX_ITEM_CHARS: usize = 512;

/// Runtime-independent corpus selection and prompt limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_asr_max_chars")]
    pub asr_max_chars: usize,
    #[serde(default = "default_translation_max_chars")]
    pub translation_max_chars: usize,
    #[serde(default = "default_asr_history_entries")]
    pub asr_history_entries: usize,
    #[serde(default = "default_translation_history_entries")]
    pub translation_history_entries: usize,
    #[serde(default = "default_database_path")]
    pub database_path: PathBuf,
    #[serde(default = "default_seed_database_path")]
    pub seed_database_path: PathBuf,
}

impl Default for CorpusConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: default_max_entries(),
            asr_max_chars: default_asr_max_chars(),
            translation_max_chars: default_translation_max_chars(),
            asr_history_entries: default_asr_history_entries(),
            translation_history_entries: default_translation_history_entries(),
            database_path: default_database_path(),
            seed_database_path: default_seed_database_path(),
        }
    }
}

const fn default_true() -> bool {
    true
}
const fn default_max_entries() -> usize {
    6
}
const fn default_asr_max_chars() -> usize {
    800
}
const fn default_translation_max_chars() -> usize {
    1_200
}
const fn default_asr_history_entries() -> usize {
    1
}
const fn default_translation_history_entries() -> usize {
    6
}
fn default_database_path() -> PathBuf {
    PathBuf::from("runtime/xr-corpus.sqlite")
}
fn default_seed_database_path() -> PathBuf {
    PathBuf::from("corpora/default.sqlite")
}

fn default_corpus_activation() -> CorpusActivation {
    CorpusActivation::OnEvidence
}

/// Controls how a corpus enters the prompt candidate set.
///
/// Persisted graph nodes usually use [`Self::OnEvidence`]. Runtime providers may
/// publish [`Self::Always`] snapshots for short-lived facts such as the
/// current VRChat world and player names. Always-active terms are also used as
/// activation evidence for regular corpora, allowing a player called
/// "Overwatch" or "Mercy" to activate an Overwatch terminology corpus without
/// coupling the provider to that game's taxonomy. Runtime-only corpora still
/// enter ASR/translation prompts, but their terms do not activate static
/// corpora.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CorpusActivation {
    #[default]
    OnEvidence,
    Always,
    RuntimeOnly,
}

/// One conceptual term with values stored in [`CORPUS_LANGUAGE_ORDER`].
/// Empty values represent a language for which the concept has no established
/// equivalent; positions are never collapsed or reordered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusTerm {
    pub ordered_values: Vec<String>,
}

impl CorpusTerm {
    pub fn from_ordered(
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, String> {
        let term = Self {
            ordered_values: values.into_iter().map(Into::into).collect(),
        };
        term.validate("dynamic corpus term")?;
        Ok(term)
    }

    #[must_use]
    pub fn value(&self, language: &str) -> Option<&str> {
        language_index(language)
            .and_then(|index| self.ordered_values.get(index))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn validate(&self, label: &str) -> Result<(), String> {
        if self.ordered_values.len() != CORPUS_LANGUAGE_ORDER.len() {
            return Err(format!(
                "{label} has {} language columns; expected {} in order {}",
                self.ordered_values.len(),
                CORPUS_LANGUAGE_ORDER.len(),
                CORPUS_LANGUAGE_ORDER.join(",")
            ));
        }
        if self
            .ordered_values
            .iter()
            .all(|value| value.trim().is_empty())
        {
            return Err(format!("{label} has no value in any language"));
        }
        if self.ordered_values.iter().any(|value| {
            value.contains(',')
                || value.contains('\r')
                || value.contains('\n')
                || value.chars().count() > MAX_ITEM_CHARS
        }) {
            return Err(format!(
                "{label} contains a comma, newline, or value longer than {MAX_ITEM_CHARS} characters"
            ));
        }
        Ok(())
    }
}

#[must_use]
pub fn language_index(language: &str) -> Option<usize> {
    let normalized = language.trim().to_ascii_lowercase().replace('_', "-");
    let base = normalized.split('-').next().unwrap_or_default();
    CORPUS_LANGUAGE_ORDER.iter().position(|code| *code == base)
}

/// Inference view of one graph node or transient provider corpus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusDefinition {
    pub schema: String,
    pub id: String,
    pub domain: String,
    pub subdomain: String,
    pub title: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_corpus_activation")]
    pub activation: CorpusActivation,
    #[serde(default)]
    pub triggers: Vec<CorpusTerm>,
    #[serde(default)]
    pub trigger_aliases: Vec<CorpusTerm>,
    #[serde(default)]
    pub activation_context: Vec<CorpusTerm>,
    #[serde(default)]
    pub terms: Vec<CorpusTerm>,
}

impl CorpusDefinition {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema != CORPUS_SCHEMA {
            return Err(format!(
                "corpus {} uses unsupported schema {:?}; expected {CORPUS_SCHEMA}",
                self.id, self.schema
            ));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("domain", self.domain.as_str()),
            ("subdomain", self.subdomain.as_str()),
        ] {
            graph::check_id(value, label)?;
        }
        if self.title.trim().is_empty() || self.title.contains('\r') || self.title.contains('\n') {
            return Err(format!(
                "corpus {} has an empty or multi-line title",
                self.id
            ));
        }
        validate_terms(&self.id, "triggers", &self.triggers, MAX_TRIGGERS)?;
        validate_terms(
            &self.id,
            "trigger aliases",
            &self.trigger_aliases,
            MAX_TRIGGERS,
        )?;
        validate_terms(
            &self.id,
            "activation context",
            &self.activation_context,
            MAX_TRIGGERS,
        )?;
        validate_terms(&self.id, "terms", &self.terms, MAX_TERMS)?;
        if self.terms.is_empty()
            || (self.activation == CorpusActivation::OnEvidence
                && self.triggers.is_empty()
                && self.trigger_aliases.is_empty())
        {
            return Err(format!(
                "corpus {} must contain at least one term, and evidence-activated corpora need at least one trigger",
                self.id
            ));
        }
        Ok(())
    }
}

/// Snapshot contract implemented by every corpus provider.
pub trait CorpusSource: Send + Sync {
    fn source_id(&self) -> &str;
    fn snapshot(&self) -> Result<Vec<CorpusDefinition>, String>;
}

#[derive(Clone)]
struct StaticCorpusSource {
    source_id: String,
    corpora: Arc<[CorpusDefinition]>,
}

impl CorpusSource for StaticCorpusSource {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn snapshot(&self) -> Result<Vec<CorpusDefinition>, String> {
        Ok(self.corpora.to_vec())
    }
}

#[derive(Clone, Default)]
pub struct DynamicCorpusSource {
    snapshots: Arc<RwLock<BTreeMap<String, DynamicSnapshot>>>,
}

#[derive(Clone)]
struct DynamicSnapshot {
    expires_at: Option<Instant>,
    corpora: Vec<CorpusDefinition>,
}

impl DynamicCorpusSource {
    /// Atomically replaces one provider's full snapshot. This is suitable for
    /// room name/player lists: each API poll publishes one coherent view and
    /// the optional TTL prevents stale world state from living indefinitely.
    pub fn replace_snapshot(
        &self,
        provider_id: &str,
        corpora: Vec<CorpusDefinition>,
        ttl: Option<Duration>,
    ) -> Result<(), String> {
        if !valid_provider_id(provider_id) {
            return Err(format!(
                "invalid dynamic corpus provider ID {provider_id:?}"
            ));
        }
        validate_unique(&corpora)?;
        for corpus in &corpora {
            corpus.validate()?;
        }
        let expires_at = ttl.and_then(|duration| Instant::now().checked_add(duration));
        self.snapshots
            .write()
            .map_err(|_| "dynamic corpus registry lock is poisoned".to_owned())?
            .insert(
                provider_id.to_owned(),
                DynamicSnapshot {
                    expires_at,
                    corpora,
                },
            );
        Ok(())
    }

    pub fn remove_provider(&self, provider_id: &str) -> Result<(), String> {
        self.snapshots
            .write()
            .map_err(|_| "dynamic corpus registry lock is poisoned".to_owned())?
            .remove(provider_id);
        Ok(())
    }
    pub fn snapshot_with_providers(&self) -> Result<Vec<(String, Vec<CorpusDefinition>)>, String> {
        let now = Instant::now();
        let mut snapshots = self
            .snapshots
            .write()
            .map_err(|_| "dynamic corpus registry lock is poisoned".to_owned())?;
        snapshots.retain(|_, snapshot| snapshot.expires_at.is_none_or(|expiry| expiry > now));
        Ok(snapshots
            .iter()
            .map(|(provider_id, snapshot)| (provider_id.clone(), snapshot.corpora.clone()))
            .collect())
    }
}

impl CorpusSource for DynamicCorpusSource {
    fn source_id(&self) -> &str {
        "runtime-dynamic"
    }

    fn snapshot(&self) -> Result<Vec<CorpusDefinition>, String> {
        let now = Instant::now();
        let mut snapshots = self
            .snapshots
            .write()
            .map_err(|_| "dynamic corpus registry lock is poisoned".to_owned())?;
        snapshots.retain(|_, snapshot| snapshot.expires_at.is_none_or(|expiry| expiry > now));
        Ok(snapshots
            .values()
            .flat_map(|snapshot| snapshot.corpora.iter().cloned())
            .collect())
    }
}

fn is_corpus_disabled(
    disabled: &HashSet<String>,
    source_id: &str,
    provider_id: Option<&str>,
    corpus: &CorpusDefinition,
) -> bool {
    if disabled.is_empty() {
        return false;
    }
    if disabled.contains(source_id) {
        return true;
    }
    if let Some(provider) = provider_id {
        if disabled.contains(provider) {
            return true;
        }
    }
    if disabled.contains(&corpus.domain) {
        return true;
    }
    if !corpus.subdomain.is_empty() {
        if disabled.contains(&corpus.subdomain) {
            return true;
        }
        let hierarchical = format!("{}.{}", corpus.domain, corpus.subdomain);
        if disabled.contains(&hierarchical) {
            return true;
        }
    }
    for disabled_domain in disabled {
        if corpus.id == *disabled_domain
            || corpus.id.starts_with(&format!("{disabled_domain}."))
        {
            return true;
        }
    }
    false
}

/// Read-optimized aggregation of immutable static sources and a shared dynamic
/// registry. Additional backend programs can be adapted by implementing
/// [`CorpusSource`] or publishing snapshots to [`DynamicCorpusSource`].
#[derive(Clone)]
pub struct CorpusCatalog {
    sources: Arc<[Arc<dyn CorpusSource>]>,
    dynamic: DynamicCorpusSource,
    graph: Option<GraphStore>,
}

impl CorpusCatalog {
    pub fn load(config: &CorpusConfig, project_root: &Path) -> Result<Self, String> {
        let dynamic = DynamicCorpusSource::default();
        let database = resolve_from_project_root(project_root, &config.database_path);
        let seed = resolve_from_project_root(project_root, &config.seed_database_path);
        let graph = GraphStore::open(&database, &seed)?;
        Self::from_sources_with_dynamic(Vec::new(), dynamic, Some(graph))
    }

    /// Builds a catalog from custom sources and adds the standard dynamic
    /// registry. API adapters can either implement [`CorpusSource`] directly
    /// or publish atomic snapshots through [`Self::dynamic_source`].
    pub fn from_sources(sources: Vec<Arc<dyn CorpusSource>>) -> Result<Self, String> {
        Self::from_sources_with_dynamic(sources, DynamicCorpusSource::default(), None)
    }

    pub fn dynamic_source(&self) -> DynamicCorpusSource {
        self.dynamic.clone()
    }

    pub fn graph_store(&self) -> Option<GraphStore> {
        self.graph.clone()
    }

    /// Live provider words that may activate persisted specialist nodes.
    pub fn runtime_activation_terms(&self) -> Result<Vec<CorpusTerm>, String> {
        let disabled = self
            .graph
            .as_ref()
            .map(GraphStore::projection)
            .transpose()?
            .map_or_else(HashSet::new, |view| view.disabled_domains.clone());
        let mut terms = Vec::new();
        for (provider_id, corpora) in self.dynamic.snapshot_with_providers()? {
            if disabled.contains(&provider_id) {
                continue;
            }
            for corpus in corpora {
                if corpus.activation == CorpusActivation::Always
                    && !is_corpus_disabled(
                        &disabled,
                        "runtime-dynamic",
                        Some(&provider_id),
                        &corpus,
                    )
                {
                    terms.extend(corpus.terms);
                }
            }
        }
        Ok(terms)
    }

    pub fn snapshot(&self) -> Result<Vec<CorpusDefinition>, String> {
        self.snapshot_with_revision().map(|(corpora, _)| corpora)
    }

    /// Returns one coherent persisted graph view and its semantic revision.
    /// Transient provider data may change independently between calls.
    pub fn snapshot_with_revision(&self) -> Result<(Vec<CorpusDefinition>, u64), String> {
        let mut all = Vec::new();
        let mut owners = BTreeMap::new();
        let mut revision = 0;
        let disabled = if let Some(graph) = &self.graph {
            let view = graph.projection()?;
            revision = view.revision;
            for corpus in &view.corpora {
                owners.insert(corpus.id.clone(), "persisted graph".to_owned());
                all.push(corpus.clone());
            }
            view.disabled_domains.clone()
        } else {
            HashSet::new()
        };
        for source in self.sources.iter() {
            if source.source_id() == "runtime-dynamic" {
                for (provider_id, corpora) in self.dynamic.snapshot_with_providers()? {
                    if disabled.contains(&provider_id) {
                        continue;
                    }
                    for corpus in corpora {
                        if is_corpus_disabled(
                            &disabled,
                            "runtime-dynamic",
                            Some(&provider_id),
                            &corpus,
                        ) {
                            continue;
                        }
                        corpus.validate()?;
                        if let Some(previous) =
                            owners.insert(corpus.id.clone(), format!("dynamic:{provider_id}"))
                        {
                            return Err(format!(
                                "duplicate corpus ID {} from sources {previous:?} and dynamic:{provider_id:?}",
                                corpus.id
                            ));
                        }
                        all.push(corpus);
                    }
                }
            } else {
                if disabled.contains(source.source_id()) {
                    continue;
                }
                for corpus in source.snapshot()? {
                    if is_corpus_disabled(&disabled, source.source_id(), None, &corpus) {
                        continue;
                    }
                    corpus.validate()?;
                    if let Some(previous) =
                        owners.insert(corpus.id.clone(), source.source_id().to_owned())
                    {
                        return Err(format!(
                            "duplicate corpus ID {} from sources {previous:?} and {:?}",
                            corpus.id,
                            source.source_id()
                        ));
                    }
                    all.push(corpus);
                }
            }
        }
        all.sort_by(|left, right| left.id.cmp(&right.id));
        Ok((all, revision))
    }

    /// Builds a catalog from programmatic static data plus an empty dynamic
    /// registry. Useful for generated configurations and adapter tests.
    pub fn from_definitions(corpora: Vec<CorpusDefinition>) -> Result<Self, String> {
        validate_unique(&corpora)?;
        for corpus in &corpora {
            corpus.validate()?;
        }
        Self::from_sources(vec![Arc::new(StaticCorpusSource {
            source_id: "programmatic-static".into(),
            corpora: corpora.into(),
        })])
    }

    fn from_sources_with_dynamic(
        mut sources: Vec<Arc<dyn CorpusSource>>,
        dynamic: DynamicCorpusSource,
        graph: Option<GraphStore>,
    ) -> Result<Self, String> {
        sources.push(Arc::new(dynamic.clone()));
        let catalog = Self {
            sources: sources.into(),
            dynamic,
            graph,
        };
        let _ = catalog.snapshot()?;
        Ok(catalog)
    }
}

fn resolve_from_project_root(project_root: &Path, configured: &Path) -> PathBuf {
    if configured.is_absolute() {
        configured.to_owned()
    } else {
        project_root.join(configured)
    }
}

fn validate_unique(corpora: &[CorpusDefinition]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for corpus in corpora {
        if !seen.insert(corpus.id.as_str()) {
            return Err(format!("duplicate corpus ID {}", corpus.id));
        }
    }
    Ok(())
}

fn validate_terms(
    corpus_id: &str,
    label: &str,
    terms: &[CorpusTerm],
    maximum: usize,
) -> Result<(), String> {
    if terms.len() > maximum {
        return Err(format!(
            "corpus {corpus_id} contains {} {label}; maximum is {maximum}",
            terms.len()
        ));
    }
    for (index, term) in terms.iter().enumerate() {
        term.validate(&format!("corpus {corpus_id} {label} entry {}", index + 1))?;
    }
    Ok(())
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_term(val: &str) -> CorpusTerm {
        CorpusTerm::from_ordered(vec![val.to_owned(); CORPUS_LANGUAGE_ORDER.len()]).unwrap()
    }

    fn sample_corpus(id: &str, domain: &str, subdomain: &str) -> CorpusDefinition {
        CorpusDefinition {
            schema: CORPUS_SCHEMA.into(),
            id: id.into(),
            domain: domain.into(),
            subdomain: subdomain.into(),
            title: id.into(),
            priority: 100,
            activation: CorpusActivation::Always,
            triggers: Vec::new(),
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![sample_term("VRChat"), sample_term("VRC")],
        }
    }

    #[test]
    fn test_is_corpus_disabled_by_provider() {
        let mut disabled = HashSet::new();
        disabled.insert("vrcx".to_string());

        let corpus = sample_corpus(
            "virtual-worlds.vrchat.runtime-game-mode",
            "virtual-worlds",
            "vrchat",
        );

        // When provider is vrcx and vrcx is disabled:
        assert!(is_corpus_disabled(
            &disabled,
            "runtime-dynamic",
            Some("vrcx"),
            &corpus
        ));

        // When provider is something else:
        assert!(!is_corpus_disabled(
            &disabled,
            "runtime-dynamic",
            Some("other"),
            &corpus
        ));
    }

    #[test]
    fn test_is_corpus_disabled_by_subdomain_and_hierarchical() {
        let mut disabled = HashSet::new();
        disabled.insert("virtual-worlds.vrchat".to_string());

        let corpus = sample_corpus(
            "virtual-worlds.vrchat.runtime-game-mode",
            "virtual-worlds",
            "vrchat",
        );

        // Disabling child domain "virtual-worlds.vrchat" must disable this corpus:
        assert!(is_corpus_disabled(
            &disabled,
            "runtime-dynamic",
            Some("other"),
            &corpus
        ));
    }

    #[test]
    fn test_is_corpus_disabled_by_root_domain() {
        let mut disabled = HashSet::new();
        disabled.insert("virtual-worlds".to_string());

        let corpus = sample_corpus(
            "virtual-worlds.vrchat.runtime-game-mode",
            "virtual-worlds",
            "vrchat",
        );

        assert!(is_corpus_disabled(
            &disabled,
            "runtime-dynamic",
            Some("other"),
            &corpus
        ));
    }

    #[test]
    fn test_catalog_filters_dynamic_vrcx_when_disabled() {
        let temp_dir = std::env::temp_dir().join(format!(
            "xr_corpus_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("test.sqlite");
        let seed_path = temp_dir.join("seed.sqlite");

        {
            let conn = rusqlite::Connection::open(&seed_path).unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
            conn.execute_batch(
                "CREATE TABLE domains (id TEXT PRIMARY KEY, title TEXT NOT NULL, enabled INTEGER NOT NULL, parent_id TEXT);
                 CREATE TABLE nodes (id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, title TEXT NOT NULL, enabled INTEGER NOT NULL, promptable INTEGER NOT NULL, activation TEXT NOT NULL, priority INTEGER NOT NULL, values_json TEXT NOT NULL);
                 CREATE TABLE edges (source_id TEXT NOT NULL, target_id TEXT NOT NULL, kind TEXT NOT NULL, enabled INTEGER NOT NULL, PRIMARY KEY(source_id, target_id, kind));"
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('vrcx', 'VRCX', 0, NULL)",
                [],
            ).unwrap();
        }

        let graph = GraphStore::open(&db_path, &seed_path).unwrap();
        let dynamic = DynamicCorpusSource::default();
        let catalog = CorpusCatalog::from_sources_with_dynamic(
            Vec::new(),
            dynamic.clone(),
            Some(graph),
        ).unwrap();

        let game_mode = sample_corpus(
            "virtual-worlds.vrchat.runtime-game-mode",
            "virtual-worlds",
            "vrchat",
        );
        dynamic.replace_snapshot("vrcx", vec![game_mode.clone()], None).unwrap();

        let (corpora, _) = catalog.snapshot_with_revision().unwrap();
        assert!(
            corpora.is_empty(),
            "expected dynamic vrcx corpora to be filtered out when vrcx domain is disabled"
        );

        let terms = catalog.runtime_activation_terms().unwrap();
        assert!(
            terms.is_empty(),
            "expected runtime activation terms to be filtered out when vrcx domain is disabled"
        );

        // Now test disabling virtual-worlds.vrchat child domain while vrcx is enabled
        let db_path2 = temp_dir.join("test2.sqlite");
        let seed_path2 = temp_dir.join("seed2.sqlite");
        {
            let conn = rusqlite::Connection::open(&seed_path2).unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
            conn.execute_batch(
                "CREATE TABLE domains (id TEXT PRIMARY KEY, title TEXT NOT NULL, enabled INTEGER NOT NULL, parent_id TEXT);
                 CREATE TABLE nodes (id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, title TEXT NOT NULL, enabled INTEGER NOT NULL, promptable INTEGER NOT NULL, activation TEXT NOT NULL, priority INTEGER NOT NULL, values_json TEXT NOT NULL);
                 CREATE TABLE edges (source_id TEXT NOT NULL, target_id TEXT NOT NULL, kind TEXT NOT NULL, enabled INTEGER NOT NULL, PRIMARY KEY(source_id, target_id, kind));"
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('virtual-worlds', 'Virtual Worlds', 1, NULL)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('virtual-worlds.vrchat', 'VRChat', 0, 'virtual-worlds')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('vrcx', 'VRCX', 1, NULL)",
                [],
            ).unwrap();
        }

        let graph2 = GraphStore::open(&db_path2, &seed_path2).unwrap();
        let dynamic2 = DynamicCorpusSource::default();
        let catalog2 = CorpusCatalog::from_sources_with_dynamic(
            Vec::new(),
            dynamic2.clone(),
            Some(graph2),
        ).unwrap();
        dynamic2.replace_snapshot("vrcx", vec![game_mode.clone()], None).unwrap();

        let (corpora2, _) = catalog2.snapshot_with_revision().unwrap();
        assert!(
            corpora2.is_empty(),
            "expected game_mode to be filtered out when virtual-worlds.vrchat is disabled"
        );

        // When everything is enabled, game_mode should be present
        let db_path3 = temp_dir.join("test3.sqlite");
        let seed_path3 = temp_dir.join("seed3.sqlite");
        {
            let conn = rusqlite::Connection::open(&seed_path3).unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
            conn.execute_batch(
                "CREATE TABLE domains (id TEXT PRIMARY KEY, title TEXT NOT NULL, enabled INTEGER NOT NULL, parent_id TEXT);
                 CREATE TABLE nodes (id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, title TEXT NOT NULL, enabled INTEGER NOT NULL, promptable INTEGER NOT NULL, activation TEXT NOT NULL, priority INTEGER NOT NULL, values_json TEXT NOT NULL);
                 CREATE TABLE edges (source_id TEXT NOT NULL, target_id TEXT NOT NULL, kind TEXT NOT NULL, enabled INTEGER NOT NULL, PRIMARY KEY(source_id, target_id, kind));"
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('virtual-worlds', 'Virtual Worlds', 1, NULL)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('virtual-worlds.vrchat', 'VRChat', 1, 'virtual-worlds')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO domains (id, title, enabled, parent_id) VALUES ('vrcx', 'VRCX', 1, NULL)",
                [],
            ).unwrap();
        }

        let graph3 = GraphStore::open(&db_path3, &seed_path3).unwrap();
        let dynamic3 = DynamicCorpusSource::default();
        let catalog3 = CorpusCatalog::from_sources_with_dynamic(
            Vec::new(),
            dynamic3.clone(),
            Some(graph3),
        ).unwrap();
        dynamic3.replace_snapshot("vrcx", vec![game_mode], None).unwrap();

        let (corpora3, _) = catalog3.snapshot_with_revision().unwrap();
        assert_eq!(
            corpora3.len(),
            1,
            "expected game_mode to be included when both vrcx and virtual-worlds.vrchat are enabled"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
