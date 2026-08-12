//! Versioned Markdown corpora and runtime-provided corpus snapshots.
//!
//! Static and dynamic data implement the same source contract. The ASR
//! context layer therefore does not need to know whether terms came from a
//! release asset, a VRChat room API, or another future backend process.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
pub const CORPUS_SCHEMA: &str = "xrtranslate-corpus/v1";
pub const CORPUS_LANGUAGE_ORDER: &[&str] = &[
    "zh", "en", "fr", "pt", "es", "ja", "ru", "ko", "th", "it", "de", "vi", "id", "pl", "cs", "nl",
];
const MAX_CORPUS_FILES: usize = 1_024;
const MAX_CORPUS_BYTES: u64 = 256 * 1024;
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
    #[serde(default = "default_corpora_directory")]
    pub corpora_directory: PathBuf,
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
            corpora_directory: default_corpora_directory(),
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
fn default_corpora_directory() -> PathBuf {
    PathBuf::from("corpora/v1")
}

fn default_corpus_activation() -> CorpusActivation {
    CorpusActivation::OnEvidence
}

/// Controls how a corpus enters the prompt candidate set.
///
/// Static Markdown corpora use [`Self::OnEvidence`]. Runtime providers may
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

    fn validate(&self, label: &str) -> Result<(), String> {
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

/// Canonical corpus data shared by Markdown and future API sources.
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
    fn validate(&self) -> Result<(), String> {
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
            if !valid_qualified_id(value, label == "id") {
                return Err(format!(
                    "corpus {} has invalid {label} {:?}; use lowercase ASCII, digits and hyphens",
                    self.id, value
                ));
            }
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
            || (self.activation == CorpusActivation::OnEvidence && self.triggers.is_empty())
        {
            return Err(format!(
                "corpus {} must contain at least one term, and evidence-activated corpora need at least one trigger",
                self.id
            ));
        }
        Ok(())
    }

    /// Renders the canonical Markdown representation used by static corpus
    /// writers and generation tools. The taxonomy ID remains represented by
    /// the destination path, so it is not duplicated inside the document.
    pub fn to_markdown(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!(
            "# {}\n\n> 本文件遵循 `corpora/v1/SCHEMA.md`。每行一个概念，语言字段严格按固定顺序排列；缺失译名保留空列。\n\n## Metadata\n\nschema: {}\npriority: {}\nactivation: {}\n\n## Language Order\n\n{}\n\n## Triggers\n\n{}\n\n## Trigger Aliases\n\n{}\n\n## Activation Context\n\n{}\n\n## Terms\n\n{}\n",
            self.title.trim(),
            self.schema,
            self.priority,
            self.activation.as_metadata_value(),
            CORPUS_LANGUAGE_ORDER.join(","),
            render_terms(&self.triggers),
            render_terms(&self.trigger_aliases),
            render_terms(&self.activation_context),
            render_terms(&self.terms),
        ))
    }
}

impl CorpusActivation {
    fn as_metadata_value(self) -> &'static str {
        match self {
            Self::OnEvidence => "on-evidence",
            Self::Always => "always",
            Self::RuntimeOnly => "runtime-only",
        }
    }
}

fn render_terms(terms: &[CorpusTerm]) -> String {
    terms
        .iter()
        .map(|term| term.ordered_values.join(","))
        .collect::<Vec<_>>()
        .join("\n")
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
        if !valid_qualified_id(provider_id, false) {
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

/// Read-optimized aggregation of immutable static sources and a shared dynamic
/// registry. Additional backend programs can be adapted by implementing
/// [`CorpusSource`] or publishing snapshots to [`DynamicCorpusSource`].
#[derive(Clone)]
pub struct CorpusCatalog {
    sources: Arc<[Arc<dyn CorpusSource>]>,
    dynamic: DynamicCorpusSource,
}

impl CorpusCatalog {
    pub fn load(config: &CorpusConfig, project_root: &Path) -> Result<Self, String> {
        let dynamic = DynamicCorpusSource::default();
        let mut sources: Vec<Arc<dyn CorpusSource>> = Vec::new();
        if config.enabled {
            let root = resolve_from_project_root(project_root, &config.corpora_directory);
            let corpora = load_markdown_directory(&root)?;
            sources.push(Arc::new(StaticCorpusSource {
                source_id: format!("markdown:{}", root.display()),
                corpora: corpora.into(),
            }));
        }
        Self::from_sources_with_dynamic(sources, dynamic)
    }

    /// Builds a catalog from custom sources and adds the standard dynamic
    /// registry. API adapters can either implement [`CorpusSource`] directly
    /// or publish atomic snapshots through [`Self::dynamic_source`].
    pub fn from_sources(sources: Vec<Arc<dyn CorpusSource>>) -> Result<Self, String> {
        Self::from_sources_with_dynamic(sources, DynamicCorpusSource::default())
    }

    pub fn dynamic_source(&self) -> DynamicCorpusSource {
        self.dynamic.clone()
    }

    pub fn snapshot(&self) -> Result<Vec<CorpusDefinition>, String> {
        let mut all = Vec::new();
        let mut owners = BTreeMap::new();
        for source in self.sources.iter() {
            for corpus in source.snapshot()? {
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
        all.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(all)
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
    ) -> Result<Self, String> {
        sources.push(Arc::new(dynamic.clone()));
        let catalog = Self {
            sources: sources.into(),
            dynamic,
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

/// Loads and validates one version directory such as `corpora/v1`.
pub fn load_markdown_directory(root: &Path) -> Result<Vec<CorpusDefinition>, String> {
    require_directory(root, "Markdown corpus root")?;
    let domains = root.join("domains");
    require_directory(&domains, "corpus domains directory")?;
    let mut paths = Vec::new();
    for domain in child_directories(&domains)? {
        let domain_id = directory_id(&domain)?;
        require_file(&domain.join("domain.md"), "domain descriptor")?;
        let subdomains = domain.join("subdomains");
        require_directory(&subdomains, "subdomains directory")?;
        for subdomain in child_directories(&subdomains)? {
            let subdomain_id = directory_id(&subdomain)?;
            require_file(&subdomain.join("subdomain.md"), "subdomain descriptor")?;
            let corpus_directory = subdomain.join("corpora");
            require_directory(&corpus_directory, "corpus leaf directory")?;
            for path in corpus_files(&corpus_directory)? {
                let file_id = corpus_file_id(&corpus_directory, &path)?;
                paths.push((domain_id.clone(), subdomain_id.clone(), file_id, path));
            }
        }
    }
    paths.sort_by(|left, right| left.3.cmp(&right.3));
    if paths.len() > MAX_CORPUS_FILES {
        return Err(format!(
            "corpus root contains {} files; maximum is {MAX_CORPUS_FILES}",
            paths.len()
        ));
    }

    let mut corpora = Vec::with_capacity(paths.len());
    for (domain_id, subdomain_id, file_id, path) in paths {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect corpus {}: {error}", path.display()))?;
        if metadata.len() > MAX_CORPUS_BYTES {
            return Err(format!(
                "corpus {} has {} bytes; maximum is {MAX_CORPUS_BYTES}",
                path.display(),
                metadata.len()
            ));
        }
        let markdown = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read corpus {}: {error}", path.display()))?;
        let expected_id = format!("{domain_id}.{subdomain_id}.{file_id}");
        let mut corpus =
            parse_corpus_markdown(&markdown, &path, expected_id, domain_id, subdomain_id)?;
        corpus.triggers = normalized_terms(corpus.triggers);
        corpus.terms = normalized_terms(corpus.terms);
        corpus.validate()?;
        corpora.push(corpus);
    }
    validate_unique(&corpora)?;
    Ok(corpora)
}

fn parse_corpus_markdown(
    markdown: &str,
    path: &Path,
    id: String,
    domain: String,
    subdomain: String,
) -> Result<CorpusDefinition, String> {
    let lines = markdown.lines().collect::<Vec<_>>();
    let title = lines
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .and_then(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| {
            format!(
                "corpus {} must begin with a '# Title' heading",
                path.display()
            )
        })?;

    let metadata = section_lines(&lines, "Metadata", path)?;
    let schema = metadata_value(metadata, "schema", path)?;
    let priority = metadata_value(metadata, "priority", path)?
        .parse::<i32>()
        .map_err(|error| format!("invalid corpus priority in {}: {error}", path.display()))?;
    let languages = section_data_lines(&lines, "Language Order", path)?;
    if languages.len() != 1 || languages[0] != CORPUS_LANGUAGE_ORDER.join(",") {
        return Err(format!(
            "corpus {} must declare exactly this language order: {}",
            path.display(),
            CORPUS_LANGUAGE_ORDER.join(",")
        ));
    }

    let activation = optional_metadata_value(metadata, "activation")
        .map(parse_activation)
        .transpose()
        .map_err(|error| format!("invalid corpus activation in {}: {error}", path.display()))?
        .unwrap_or_default();

    Ok(CorpusDefinition {
        schema: schema.to_owned(),
        id,
        domain,
        subdomain,
        title: title.to_owned(),
        priority,
        activation,
        triggers: parse_term_section(&lines, "Triggers", path)?,
        trigger_aliases: parse_optional_term_section(&lines, "Trigger Aliases", path)?,
        activation_context: parse_optional_term_section(&lines, "Activation Context", path)?,
        terms: parse_term_section(&lines, "Terms", path)?,
    })
}

fn section_lines<'a>(
    lines: &'a [&'a str],
    heading: &str,
    path: &Path,
) -> Result<&'a [&'a str], String> {
    let marker = format!("## {heading}");
    let matches = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.trim() == marker).then_some(index))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "corpus {} must contain exactly one {marker:?} heading",
            path.display()
        ));
    }
    let start = matches[0] + 1;
    let end = lines[start..]
        .iter()
        .position(|line| line.trim().starts_with("## "))
        .map_or(lines.len(), |offset| start + offset);
    Ok(&lines[start..end])
}

fn section_data_lines<'a>(
    lines: &'a [&'a str],
    heading: &str,
    path: &Path,
) -> Result<Vec<&'a str>, String> {
    Ok(section_lines(lines, heading, path)?
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('>'))
        .collect())
}

fn metadata_value<'a>(lines: &'a [&str], key: &str, path: &Path) -> Result<&'a str, String> {
    let prefix = format!("{key}:");
    lines
        .iter()
        .map(|line| line.trim())
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("corpus {} metadata is missing {key:?}", path.display()))
}

fn optional_metadata_value<'a>(lines: &'a [&str], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    lines
        .iter()
        .map(|line| line.trim())
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_activation(value: &str) -> Result<CorpusActivation, String> {
    match value {
        "on-evidence" => Ok(CorpusActivation::OnEvidence),
        "always" => Ok(CorpusActivation::Always),
        "runtime-only" => Ok(CorpusActivation::RuntimeOnly),
        _ => Err(format!(
            "expected one of on-evidence, always, runtime-only; got {value:?}"
        )),
    }
}

fn parse_term_section(
    lines: &[&str],
    heading: &str,
    path: &Path,
) -> Result<Vec<CorpusTerm>, String> {
    section_data_lines(lines, heading, path)?
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let term = CorpusTerm {
                ordered_values: line
                    .split(',')
                    .map(|value| value.trim().to_owned())
                    .collect(),
            };
            term.validate(&format!(
                "corpus {} {heading} line {}",
                path.display(),
                index + 1
            ))?;
            Ok(term)
        })
        .collect()
}

fn parse_optional_term_section(
    lines: &[&str],
    heading: &str,
    path: &Path,
) -> Result<Vec<CorpusTerm>, String> {
    let marker = format!("## {heading}");
    if !lines.iter().any(|line| line.trim() == marker) {
        return Ok(Vec::new());
    }
    parse_term_section(lines, heading, path)
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
        term.validate(&format!("corpus {corpus_id} {label} line {}", index + 1))?;
    }
    Ok(())
}

fn normalized_terms(terms: Vec<CorpusTerm>) -> Vec<CorpusTerm> {
    let mut seen = HashSet::new();
    terms
        .into_iter()
        .map(|term| CorpusTerm {
            ordered_values: term
                .ordered_values
                .into_iter()
                .map(|value| value.trim().to_owned())
                .collect(),
        })
        .filter(|term| {
            seen.insert(
                term.ordered_values
                    .iter()
                    .map(|value| value.to_lowercase())
                    .collect::<Vec<_>>()
                    .join("\u{1f}"),
            )
        })
        .collect()
}

fn child_directories(path: &Path) -> Result<Vec<PathBuf>, String> {
    children(path, true)
}

fn corpus_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_corpus_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_corpus_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(path)
        .map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry_path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "corpus hierarchy must not contain symlinks: {}",
                entry_path.display()
            ));
        }
        if file_type.is_dir() {
            collect_corpus_files(&entry_path, files)?;
        } else if file_type.is_file() {
            if entry_path.extension().and_then(|value| value.to_str()) != Some("md") {
                return Err(format!(
                    "corpus directory contains a non-Markdown file: {}",
                    entry_path.display()
                ));
            }
            files.push(entry_path);
        }
    }
    Ok(())
}

fn corpus_file_id(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("corpus file is outside root: {}", path.display()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let segment = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| format!("corpus path is not valid UTF-8: {}", path.display()))?;
        let id = if segment.ends_with(".md") {
            segment.trim_end_matches(".md")
        } else {
            segment
        };
        if !valid_qualified_id(id, false) {
            return Err(format!(
                "invalid corpus path segment {id:?} in {}",
                path.display()
            ));
        }
        parts.push(id.to_owned());
    }
    if parts.is_empty() {
        return Err(format!(
            "corpus file has no relative ID: {}",
            path.display()
        ));
    }
    Ok(parts.join("."))
}

fn children(path: &Path, directories: bool) -> Result<Vec<PathBuf>, String> {
    let mut children = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "corpus hierarchy must not contain symlinks: {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir() == directories && (file_type.is_dir() || file_type.is_file()) {
            children.push(entry.path());
        }
    }
    children.sort();
    Ok(children)
}

fn directory_id(path: &Path) -> Result<String, String> {
    let id = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("invalid corpus directory name: {}", path.display()))?;
    if !valid_qualified_id(id, false) {
        return Err(format!("invalid corpus taxonomy ID {id:?}"));
    }
    Ok(id.to_owned())
}

fn valid_qualified_id(value: &str, allow_dots: bool) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            (allow_dots || !value.contains('.'))
                && !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn require_directory(path: &Path, label: &str) -> Result<(), String> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!("{label} is missing: {}", path.display()))
    }
}

fn require_file(path: &Path, label: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{label} is missing: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "xrtranslate-corpus-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_fixture(root: &Path, language_order: &str) -> PathBuf {
        let leaf =
            root.join("corpora/v1/domains/virtual-worlds/subdomains/vrchat/corpora/platform.md");
        fs::create_dir_all(leaf.parent().unwrap()).unwrap();
        fs::write(
            root.join("corpora/v1/domains/virtual-worlds/domain.md"),
            "# Domain",
        )
        .unwrap();
        fs::write(
            root.join("corpora/v1/domains/virtual-worlds/subdomains/vrchat/subdomain.md"),
            "# Subdomain",
        )
        .unwrap();
        fs::write(
            &leaf,
            format!(
                "# Platform\n\n> Fixed-order multilingual corpus.\n\n## Metadata\n\nschema: {CORPUS_SCHEMA}\npriority: 10\n\n## Language Order\n\n{language_order}\n\n## Triggers\n\n{}\n\n## Terms\n\n{}\n",
                term_row(&[("zh", "VRChat"), ("en", "VRChat")]),
                term_row(&[("zh", "实例"), ("en", "instance")]),
            ),
        )
        .unwrap();
        leaf
    }

    fn term_row(values: &[(&str, &str)]) -> String {
        let mut row = vec![String::new(); CORPUS_LANGUAGE_ORDER.len()];
        for (language, value) in values {
            row[language_index(language).unwrap()] = (*value).into();
        }
        row.join(",")
    }

    fn term(values: &[(&str, &str)]) -> CorpusTerm {
        CorpusTerm::from_ordered(term_row(values).split(',')).unwrap()
    }

    #[test]
    fn markdown_loader_uses_the_config_root_not_the_executable_directory() {
        let root = temp_root("release-root");
        write_fixture(&root, &CORPUS_LANGUAGE_ORDER.join(","));
        let catalog = CorpusCatalog::load(&CorpusConfig::default(), &root).unwrap();
        let snapshot = catalog.snapshot().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].terms[0].value("zh"), Some("实例"));
        assert_eq!(snapshot[0].terms[0].value("en"), Some("instance"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn markdown_loader_uses_nested_corpus_paths_as_concept_ids() {
        let root = temp_root("nested-corpus");
        let leaf = root.join(
            "corpora/v1/domains/entertainment/subdomains/anime-and-manga/corpora/works/sora-no-otoshimono/characters.md",
        );
        fs::create_dir_all(leaf.parent().unwrap()).unwrap();
        fs::write(
            root.join("corpora/v1/domains/entertainment/domain.md"),
            "# Domain",
        )
        .unwrap();
        fs::write(
            root.join("corpora/v1/domains/entertainment/subdomains/anime-and-manga/subdomain.md"),
            "# Subdomain",
        )
        .unwrap();
        fs::write(
            &leaf,
            format!(
                "# Characters\n\n> Fixed-order multilingual corpus.\n\n## Metadata\n\nschema: {CORPUS_SCHEMA}\npriority: 58\n\n## Language Order\n\n{}\n\n## Triggers\n\n{}\n\n## Terms\n\n{}\n",
                CORPUS_LANGUAGE_ORDER.join(","),
                term_row(&[("zh", "天降之物"), ("en", "Heaven's Lost Property")]),
                term_row(&[("zh", "伊卡洛斯"), ("en", "Ikaros")]),
            ),
        )
        .unwrap();

        let catalog = CorpusCatalog::load(&CorpusConfig::default(), &root).unwrap();
        let snapshot = catalog.snapshot().unwrap();
        assert_eq!(
            snapshot[0].id,
            "entertainment.anime-and-manga.works.sora-no-otoshimono.characters"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_in_corpora_follow_the_versioned_schema() {
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&CorpusConfig::default(), &project_root).unwrap();
        let snapshot = catalog.snapshot().unwrap();
        assert!(snapshot.len() >= 15);
        assert!(
            snapshot
                .iter()
                .any(|corpus| corpus.id == "virtual-worlds.vrchat.instance-social")
        );
        assert!(
            snapshot
                .iter()
                .any(|corpus| corpus.id == "virtual-worlds.vrchat.community-language")
        );
        assert!(
            snapshot
                .iter()
                .any(|corpus| corpus.id == "technology.software-and-ai.frontier-models")
        );
        assert!(snapshot.iter().any(|corpus| {
            corpus.id == "internet-culture.memes.chinese-casual-teasing"
                && corpus.title == "中文网络热梗调侃"
        }));
    }

    #[test]
    fn language_column_order_mismatch_is_rejected() {
        let root = temp_root("language-order");
        write_fixture(&root, "en,zh");
        let error = CorpusCatalog::load(&CorpusConfig::default(), &root)
            .err()
            .unwrap();
        assert!(error.contains("language order"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dynamic_provider_replaces_snapshots_and_expires_stale_room_data() {
        let source = DynamicCorpusSource::default();
        let corpus = CorpusDefinition {
            schema: CORPUS_SCHEMA.into(),
            id: "virtual-worlds.vrchat.room-snapshot".into(),
            domain: "virtual-worlds".into(),
            subdomain: "vrchat".into(),
            title: "Current room".into(),
            priority: 100,
            activation: CorpusActivation::Always,
            triggers: vec![term(&[("zh", "VRChat"), ("en", "VRChat")])],
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![
                term(&[("zh", "房间 The Great Pug"), ("en", "Room The Great Pug")]),
                term(&[("zh", "玩家 Alice"), ("en", "Player Alice")]),
                term(&[("zh", "玩家 Bob"), ("en", "Player Bob")]),
            ],
        };
        source
            .replace_snapshot("vrchat-api", vec![corpus.clone()], None)
            .unwrap();
        assert_eq!(source.snapshot().unwrap(), [corpus]);
        source
            .replace_snapshot("vrchat-api", Vec::new(), Some(Duration::ZERO))
            .unwrap();
        assert!(source.snapshot().unwrap().is_empty());
    }

    #[test]
    fn always_active_runtime_corpus_does_not_require_fake_triggers() {
        let source = DynamicCorpusSource::default();
        let corpus = CorpusDefinition {
            schema: CORPUS_SCHEMA.into(),
            id: "runtime.example.room".into(),
            domain: "virtual-worlds".into(),
            subdomain: "example".into(),
            title: "Current room".into(),
            priority: 100,
            activation: CorpusActivation::Always,
            triggers: Vec::new(),
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![term(&[("en", "Player One")])],
        };

        source
            .replace_snapshot("example", vec![corpus.clone()], None)
            .unwrap();
        assert_eq!(source.snapshot().unwrap(), [corpus]);
    }

    #[test]
    fn canonical_writer_emits_markdown_with_fixed_language_columns() {
        let corpus = CorpusDefinition {
            schema: CORPUS_SCHEMA.into(),
            id: "virtual-worlds.vrchat.generated".into(),
            domain: "virtual-worlds".into(),
            subdomain: "vrchat".into(),
            title: "Generated".into(),
            priority: 5,
            activation: CorpusActivation::OnEvidence,
            triggers: vec![term(&[("zh", "房间"), ("en", "room")])],
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![term(&[("en", "instance owner")])],
        };
        let markdown = corpus.to_markdown().unwrap();
        assert!(markdown.starts_with("# Generated\n"));
        assert!(markdown.contains("## Language Order"));
        assert!(markdown.contains(&CORPUS_LANGUAGE_ORDER.join(",")));
        assert!(!markdown.contains("```json"));
        let term_line = markdown
            .lines()
            .find(|line| line.contains("instance owner"))
            .unwrap();
        assert_eq!(term_line.split(',').count(), CORPUS_LANGUAGE_ORDER.len());
    }

    #[test]
    fn repository_corpora_conform_to_the_versioned_markdown_schema() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpora/v1");
        let corpora = load_markdown_directory(&root).unwrap();
        assert!(corpora.len() >= 13);
        assert!(
            corpora
                .iter()
                .any(|corpus| corpus.id == "games.overwatch.heroes")
        );
        assert!(
            corpora
                .iter()
                .any(|corpus| corpus.id == "identity-and-community.lgbtq.identity-and-language")
        );
    }
}
