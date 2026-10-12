//! Recognition spellings belong to a term, independently of activation rules.
use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{CORPUS_LANGUAGE_ORDER, CorpusTerm, GraphEdgeKind, GraphSnapshot, language_index};

pub type RecognitionAliases = BTreeMap<String, Vec<String>>;

/// A language-specific correction. Only `canonical` may enter model vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionAlias {
    pub language: String,
    pub canonical: String,
    pub variants: Vec<String>,
}

impl RecognitionAlias {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if language_index(&self.language).is_none() || self.canonical.trim().is_empty() {
            return Err(
                "recognition aliases need a supported language and its canonical word".into(),
            );
        }
        for value in std::iter::once(&self.canonical).chain(&self.variants) {
            if value.trim().is_empty()
                || value.contains([',', '\r', '\n'])
                || value.chars().count() > 512
            {
                return Err("recognition aliases must be single phrases of at most 512 characters, without commas".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn term_aliases(
    values: &[String],
    aliases: &RecognitionAliases,
) -> Result<Vec<RecognitionAlias>, String> {
    aliases
        .iter()
        .filter(|(_, variants)| !variants.is_empty())
        .map(|(language, variants)| {
            let canonical = language_index(language)
                .and_then(|i| values.get(i))
                .map(|s| s.trim())
                .unwrap_or_default();
            let mut seen = HashSet::new();
            let rule = RecognitionAlias {
                language: language.clone(),
                canonical: canonical.to_owned(),
                variants: variants
                    .iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(canonical))
                    .filter(|s| seen.insert(s.to_lowercase()))
                    .map(str::to_owned)
                    .collect(),
            };
            rule.validate()?;
            Ok(rule)
        })
        .collect()
}

/// Compatibility for old provider payloads and database seeds. Resolve each
/// spelling against an unambiguous term, never against an arbitrary first match.
pub(crate) fn legacy_aliases<'a>(
    aliases: impl Iterator<Item = &'a CorpusTerm>,
    terms: &[CorpusTerm],
) -> Vec<RecognitionAlias> {
    let mut result = Vec::new();
    for alias in aliases {
        let shared: HashSet<_> = alias
            .ordered_values
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let candidates: Vec<_> = terms
            .iter()
            .filter(|term| {
                term.ordered_values
                    .iter()
                    .any(|s| shared.contains(&s.trim().to_lowercase()))
            })
            .collect();
        if candidates.len() != 1 {
            continue;
        }
        let canonical = candidates[0];
        for language in CORPUS_LANGUAGE_ORDER {
            if let (Some(text), Some(word)) = (alias.value(language), canonical.value(language)) {
                if !text.eq_ignore_ascii_case(word) {
                    result.push(RecognitionAlias {
                        language: (*language).into(),
                        canonical: word.into(),
                        variants: vec![text.into()],
                    });
                }
            }
        }
    }
    result
}

/// One-time upgrade of the old alias-node representation. Activation edges
/// remain intact; subsequent edits only need the term's own alias fields.
pub(crate) fn import_legacy_aliases(snapshot: &mut GraphSnapshot) {
    let nodes: HashMap<_, _> = snapshot
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut incoming: HashMap<_, Vec<CorpusTerm>> = HashMap::new();
    for edge in &snapshot.edges {
        if edge.enabled && edge.kind == GraphEdgeKind::Trigger {
            if let Some(source) = nodes
                .get(edge.source_id.as_str())
                .filter(|node| node.enabled && !node.promptable)
            {
                incoming
                    .entry(edge.target_id.as_str())
                    .or_default()
                    .push(CorpusTerm {
                        ordered_values: source.values.clone(),
                    });
            }
        }
    }
    let updates: Vec<_> = snapshot
        .nodes
        .iter()
        .map(|node| {
            let terms = [CorpusTerm {
                ordered_values: node.values.clone(),
            }];
            let mut aliases = RecognitionAliases::new();
            for rule in legacy_aliases(incoming.get(node.id.as_str()).into_iter().flatten(), &terms)
            {
                aliases
                    .entry(rule.language)
                    .or_default()
                    .extend(rule.variants);
            }
            aliases
        })
        .collect();
    for (node, aliases) in snapshot.nodes.iter_mut().zip(updates) {
        node.recognition_aliases = aliases;
    }
}
