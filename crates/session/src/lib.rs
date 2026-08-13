//! Session-scoped prompt context shared by ASR and translation.
//!
//! Each utterance receives one immutable pre-ASR snapshot containing only a
//! compact lexical bias, then one post-ASR snapshot which may activate new
//! corpora for the current translation. Only successful source/translation
//! pairs become bounded dialogue history, and full sentences never enter the
//! ASR prompt.

use std::collections::{HashMap, HashSet, VecDeque};

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use xr_corpus_core::{
    CORPUS_LANGUAGE_ORDER, CorpusActivation, CorpusCatalog, CorpusConfig as PromptContextConfig,
    CorpusDefinition, CorpusTerm, language_index,
};
use xr_corpus_protocol::{
    CorpusPromptTerm, CorpusRecognitionCorrection, CorpusTermMatch, CorpusTermSource,
};

const MAX_HISTORY_TEXT_CHARS: usize = 240;
const ACTIVE_CORPUS_IDLE_TURN_LIMIT: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
struct BilingualTurn {
    source_language: String,
    target_language: String,
    source_text: String,
    translated_text: String,
}

/// Immutable context for one inference stage. Post-ASR snapshots are cloned
/// across concurrent segment translations so they cannot observe different
/// corpora or history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptContextSnapshot {
    languages: Vec<String>,
    corpora: Vec<SelectedCorpus>,
    asr_prompt: Option<String>,
    asr_echo_guard: Vec<String>,
    translation_prompt: Option<String>,
    translation_terms: Vec<SelectedPromptTerm>,
    translation_history: Vec<BilingualTurn>,
    activation_terms: Vec<SelectedPromptTerm>,
    recognition_context_terms: Vec<SelectedPromptTerm>,
}

impl PromptContextSnapshot {
    pub fn asr_prompt(&self) -> Option<String> {
        self.asr_prompt.clone()
    }

    /// Recent confirmed source/translation sentences are never sent to ASR.
    /// They are retained only as an echo guard so a context-biased result can
    /// be retried without optional context.
    pub fn asr_echo_guard(&self) -> &[String] {
        &self.asr_echo_guard
    }

    pub fn translation_prompt(&self) -> Option<String> {
        self.translation_prompt.clone()
    }

    /// Renders a per-segment prompt containing only terminology that occurs in
    /// the current source. Keeping the rest of an active domain out of the MT
    /// request prevents a large glossary from being copied as the answer.
    pub fn translation_prompt_for(&self, source_text: &str) -> Option<String> {
        let rows = self
            .relevant_translation_terms(source_text)
            .iter()
            .map(|term| term.row.clone())
            .collect::<Vec<_>>();
        let turns = self.translation_history.iter().collect::<Vec<_>>();
        (!rows.is_empty() || !turns.is_empty())
            .then(|| render_translation_prompt(&self.languages, &rows, &turns))
    }

    /// Returns terminology directly relevant to this source window and
    /// admitted to the model prompt.
    pub fn translation_prompt_terms_for(&self, source_text: &str) -> Vec<CorpusPromptTerm> {
        self.relevant_translation_terms(source_text)
            .into_iter()
            .map(|term| CorpusPromptTerm {
                values: term.values.clone(),
                sources: term.sources.clone(),
            })
            .collect()
    }

    /// Matches terms admitted to this source window's model prompt. The
    /// returned spans are authoritative UI metadata, not evidence inferred
    /// from translated text alone.
    pub fn translation_term_matches(
        &self,
        source_text: &str,
        translated_text: &str,
        target_language: &str,
    ) -> Vec<CorpusTermMatch> {
        let relevant = self
            .relevant_translation_terms(source_text)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        match_selected_terms(translated_text, &relevant, Some(target_language))
    }

    /// Trigger spans in the current recognized text which caused a regular
    /// corpus to enter this post-ASR snapshot.
    pub fn activation_matches(&self, source_text: &str) -> Vec<CorpusTermMatch> {
        match_selected_terms(source_text, &self.activation_terms, None)
    }

    /// Source-side terminology belonging to the selected active corpora. UI
    /// clients render these more softly than state-changing activation terms.
    pub fn recognition_context_matches(&self, source_text: &str) -> Vec<CorpusTermMatch> {
        match_selected_terms(source_text, &self.recognition_context_terms, None)
    }

    /// Returns corpus-owned ASR correction candidates as data. Callers decide
    /// whether to apply them and how to render the resulting matches.
    pub fn recognition_corrections(&self, source_text: &str) -> Vec<CorpusRecognitionCorrection> {
        let mut corrections = Vec::new();
        for term in selected_recognition_correction_rows(&self.corpora, &self.languages) {
            for (alias_language, alias) in &term.aliases {
                for (start, end) in term_spans(source_text, alias) {
                    let Some(canonical) = term
                        .canonical
                        .iter()
                        .find(|(language, value)| language == alias_language && value != alias)
                        .map(|(_, value)| value)
                        .or_else(|| {
                            term.canonical
                                .iter()
                                .find(|(_, value)| {
                                    !value.trim().is_empty() && !value.eq_ignore_ascii_case(alias)
                                })
                                .map(|(_, value)| value)
                        })
                    else {
                        continue;
                    };
                    if canonical.trim().is_empty() || canonical.eq_ignore_ascii_case(alias) {
                        continue;
                    }
                    let (Ok(start_byte), Ok(end_byte)) = (u32::try_from(start), u32::try_from(end))
                    else {
                        continue;
                    };
                    corrections.push(CorpusRecognitionCorrection {
                        start_byte,
                        end_byte,
                        original_text: source_text[start..end].to_owned(),
                        corrected_text: canonical.clone(),
                        sources: term.sources.clone(),
                    });
                }
            }
        }
        select_non_overlapping_corrections(corrections)
    }

    /// Builds a compact second-pass context only when an admitted terminology
    /// concept occurs in the source but its target-language value is absent
    /// from the first translation.
    pub fn terminology_retry_prompt(
        &self,
        source_text: &str,
        translated_text: &str,
        source_language: &str,
        target_language: &str,
    ) -> Option<(String, usize)> {
        let missing = self.missing_translation_terms(
            source_text,
            translated_text,
            source_language,
            target_language,
        );
        (!missing.is_empty()).then(|| {
            let rows = missing
                .iter()
                .map(|term| term.row.clone())
                .collect::<Vec<_>>();
            (
                render_translation_prompt(&self.languages, &rows, &[]),
                rows.len(),
            )
        })
    }

    pub fn missing_translation_term_count(
        &self,
        source_text: &str,
        translated_text: &str,
        source_language: &str,
        target_language: &str,
    ) -> usize {
        self.missing_translation_terms(
            source_text,
            translated_text,
            source_language,
            target_language,
        )
        .len()
    }

    fn missing_translation_terms<'a>(
        &'a self,
        source_text: &str,
        translated_text: &str,
        source_language: &str,
        target_language: &str,
    ) -> Vec<&'a SelectedPromptTerm> {
        let source_language = normalized_language_code(source_language);
        let target_language = normalized_language_code(target_language);
        if source_language.is_empty() || target_language.is_empty() {
            return Vec::new();
        }
        self.translation_terms
            .iter()
            .filter(|term| {
                !match_selected_terms(
                    source_text,
                    std::slice::from_ref(*term),
                    Some(&source_language),
                )
                .is_empty()
                    && match_selected_terms(
                        translated_text,
                        std::slice::from_ref(*term),
                        Some(&target_language),
                    )
                    .is_empty()
            })
            .collect()
    }

    fn relevant_translation_terms(&self, source_text: &str) -> Vec<&SelectedPromptTerm> {
        self.translation_terms
            .iter()
            .filter(|term| {
                !match_selected_terms(source_text, std::slice::from_ref(*term), None).is_empty()
                    || self.recognition_alias_matches_term(source_text, term)
            })
            .collect()
    }

    fn recognition_alias_matches_term(
        &self,
        source_text: &str,
        selected: &SelectedPromptTerm,
    ) -> bool {
        selected_recognition_correction_rows(&self.corpora, &self.languages)
            .into_iter()
            .filter(|term| {
                term.canonical.iter().any(|(_, canonical)| {
                    selected
                        .values
                        .iter()
                        .any(|(_, value)| values_equal(canonical, value))
                })
            })
            .any(|term| {
                term.aliases
                    .iter()
                    .any(|(_, alias)| !term_spans(source_text, alias).is_empty())
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectedPromptTerm {
    row: String,
    values: Vec<(String, String)>,
    match_values: Vec<(String, String)>,
    sources: Vec<CorpusTermSource>,
}

struct RecognitionCorrectionTerm {
    canonical: Vec<(String, String)>,
    aliases: Vec<(String, String)>,
    sources: Vec<CorpusTermSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectedCorpus {
    definition: CorpusDefinition,
    matched_canonical_triggers: Vec<CorpusTerm>,
    current_triggers: Vec<CorpusTerm>,
    current_terms: Vec<CorpusTerm>,
    activation_triggers: Vec<CorpusTerm>,
    currently_eligible: bool,
}

/// Per-session context state. Reset it whenever the audio/route generation
/// changes so stale topic and language state cannot leak into a new route.
pub struct PromptContextManager {
    config: PromptContextConfig,
    catalog: CorpusCatalog,
    current_languages: Vec<String>,
    transcript_history: VecDeque<String>,
    dialogue_history: VecDeque<BilingualTurn>,
    active_corpus_ids: HashSet<String>,
    active_corpus_idle_turns: HashMap<String, u8>,
    dormant_corpus_ids: HashSet<String>,
}

impl PromptContextManager {
    /// Sorted IDs currently retained as session activation state.
    pub fn active_corpus_ids(&self) -> Vec<String> {
        let mut ids = self.active_corpus_ids.iter().cloned().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    pub fn validate(config: &PromptContextConfig) -> Result<(), String> {
        if config.enabled && config.asr_max_chars < 256 {
            return Err("prompt_context.asr_max_chars must be at least 256 when enabled".into());
        }
        if config.enabled && config.translation_max_chars < 256 {
            return Err(
                "prompt_context.translation_max_chars must be at least 256 when enabled".into(),
            );
        }
        if config.max_entries > 128 {
            return Err("prompt_context.max_entries must not exceed 128".into());
        }
        if config.asr_history_entries > config.max_entries {
            return Err("prompt_context.asr_history_entries must not exceed max_entries".into());
        }
        if config.translation_history_entries > config.max_entries {
            return Err(
                "prompt_context.translation_history_entries must not exceed max_entries".into(),
            );
        }
        Ok(())
    }

    pub fn new(config: PromptContextConfig, catalog: CorpusCatalog) -> Result<Self, String> {
        Self::validate(&config)?;
        Ok(Self {
            current_languages: Vec::new(),
            transcript_history: VecDeque::with_capacity(config.max_entries),
            dialogue_history: VecDeque::with_capacity(config.max_entries),
            active_corpus_ids: HashSet::new(),
            active_corpus_idle_turns: HashMap::new(),
            dormant_corpus_ids: HashSet::new(),
            config,
            catalog,
        })
    }

    pub fn reset(&mut self) {
        self.current_languages.clear();
        self.transcript_history.clear();
        self.dialogue_history.clear();
        self.active_corpus_ids.clear();
        self.active_corpus_idle_turns.clear();
        self.dormant_corpus_ids.clear();
    }

    fn switch_languages(&mut self, languages: Vec<String>) -> Vec<String> {
        if self.current_languages == languages {
            return languages;
        }
        if self.current_languages.is_empty() {
            self.current_languages = languages.clone();
            return languages;
        }
        // Automatic bidirectional routing reverses source and target on every
        // speaker-language change (for example [en, zh] -> [zh, en]). That is
        // still the same conversation language pair, so retain topic and
        // history state while updating the prompt column order for this turn.
        let same_language_set = self.current_languages.len() == languages.len()
            && self
                .current_languages
                .iter()
                .all(|language| languages.contains(language));
        if same_language_set {
            self.current_languages = languages.clone();
            return languages;
        }
        self.transcript_history.clear();
        self.dialogue_history.clear();
        self.active_corpus_ids.clear();
        self.active_corpus_idle_turns.clear();
        self.dormant_corpus_ids.clear();
        self.current_languages = languages.clone();
        languages
    }

    /// Stores successful, user-visible ASR output only as corpus activation
    /// evidence. It is not treated as confirmed bilingual history until MT
    /// also succeeds.
    pub fn record_transcript(&mut self, transcript: &str) {
        if !self.config.enabled || self.config.max_entries == 0 {
            return;
        }
        let transcript = compact_history_text(transcript);
        if transcript.is_empty() {
            return;
        }
        push_bounded(
            &mut self.transcript_history,
            transcript,
            self.config.max_entries,
        );
    }

    /// Stores a successful source/translation pair for subsequent utterances.
    /// Language labels are actual per-segment route codes, never `auto`.
    pub fn record_translation(
        &mut self,
        source_language: &str,
        target_language: &str,
        source_text: &str,
        translated_text: &str,
    ) {
        if !self.config.enabled || self.config.max_entries == 0 {
            return;
        }
        let source_language = normalized_language_code(source_language);
        let target_language = normalized_language_code(target_language);
        let source_text = compact_history_text(source_text);
        let translated_text = compact_history_text(translated_text);
        if source_language.is_empty()
            || target_language.is_empty()
            || source_language == "auto"
            || target_language == "auto"
            || source_language == target_language
            || source_text.is_empty()
            || translated_text.is_empty()
        {
            return;
        }
        let turn = BilingualTurn {
            source_language,
            target_language,
            source_text,
            translated_text,
        };
        if self.dialogue_history.back() == Some(&turn) {
            return;
        }
        push_bounded(&mut self.dialogue_history, turn, self.config.max_entries);
    }

    /// Selects corpora once and renders stage-specific views under independent
    /// budgets. `hints` remains the extension point for partial-ASR and future
    /// keyword detectors.
    pub fn select(
        &mut self,
        source_language: &str,
        target_language: &str,
        hints: &[&str],
        token_budgets: (usize, usize),
    ) -> Result<PromptContextSnapshot, String> {
        self.select_observation(
            source_language,
            target_language,
            hints,
            token_budgets,
            hints.iter().any(|hint| !hint.trim().is_empty()),
        )
    }

    /// Selects context for one streaming observation. `advance_turn` must be
    /// true only for the first observation of a user utterance; later
    /// revisions may refresh a matching active topic but cannot decay it.
    pub fn select_observation(
        &mut self,
        source_language: &str,
        target_language: &str,
        hints: &[&str],
        token_budgets: (usize, usize),
        advance_turn: bool,
    ) -> Result<PromptContextSnapshot, String> {
        if !self.config.enabled {
            return Ok(PromptContextSnapshot::default());
        }

        let languages = self.switch_languages(task_languages(source_language, target_language));
        if languages.is_empty() {
            return Ok(PromptContextSnapshot::default());
        }
        let mut corpora = self.matching_corpora(&languages, hints)?;
        if self.update_active_corpora(&corpora, advance_turn) {
            corpora = self.matching_corpora(&languages, hints)?;
        }
        let terms = selected_term_rows(&corpora, &languages);
        let mut translation_candidates = terms.clone();
        translation_candidates.sort_by_key(|term| {
            !hints.iter().any(|hint| {
                !match_selected_terms(hint, std::slice::from_ref(term), None).is_empty()
            })
        });
        let activation_terms = selected_activation_rows(&corpora, &languages);
        let recognition_context_terms =
            selected_recognition_context_rows(&corpora, &terms, &languages);
        let turns = self.relevant_turns(&languages);
        let asr_echo_guard = build_asr_echo_guard(
            &self.transcript_history,
            &turns,
            self.config.asr_history_entries,
        );
        let (translation_prompt, translation_terms, translation_history) = build_translation_prompt(
            &languages,
            &translation_candidates,
            &turns,
            self.config.translation_history_entries,
            self.config.translation_max_chars,
            token_budgets.1,
        );
        Ok(PromptContextSnapshot {
            languages,
            corpora,
            asr_prompt: build_asr_prompt(&terms, self.config.asr_max_chars, token_budgets.0),
            asr_echo_guard,
            translation_prompt,
            translation_terms,
            translation_history,
            activation_terms,
            recognition_context_terms,
        })
    }

    fn relevant_turns(&self, languages: &[String]) -> Vec<&BilingualTurn> {
        self.dialogue_history
            .iter()
            .filter(|turn| {
                languages.contains(&turn.source_language)
                    && languages.contains(&turn.target_language)
            })
            .collect()
    }

    fn update_active_corpora(&mut self, corpora: &[SelectedCorpus], advance_turn: bool) -> bool {
        let explicitly_matched = corpora
            .iter()
            .filter(|corpus| {
                corpus.definition.activation == CorpusActivation::OnEvidence
                    && !corpus.current_triggers.is_empty()
            })
            .map(|corpus| corpus.definition.id.clone())
            .collect::<HashSet<_>>();

        if !explicitly_matched.is_empty() {
            self.active_corpus_ids = explicitly_matched.clone();
            self.active_corpus_idle_turns
                .retain(|id, _| explicitly_matched.contains(id));
            for id in explicitly_matched {
                self.active_corpus_idle_turns.insert(id.clone(), 0);
                self.dormant_corpus_ids.remove(&id);
            }
            return false;
        }

        if self.active_corpus_ids.is_empty() {
            return false;
        }

        let selected_ids = corpora
            .iter()
            .map(|corpus| corpus.definition.id.as_str())
            .collect::<HashSet<_>>();
        let mut changed = false;
        for id in self.active_corpus_ids.clone() {
            if !selected_ids.contains(id.as_str()) {
                self.active_corpus_ids.remove(&id);
                self.active_corpus_idle_turns.remove(&id);
                changed = true;
                continue;
            }
            // Once a specialist corpus is active, a matching terminology row
            // is evidence that the conversation is still on that topic. Terms
            // deliberately do not activate a dormant corpus on their own: many
            // of them are ordinary words outside their specialist context.
            let term_matched = corpora
                .iter()
                .any(|corpus| corpus.definition.id == id && !corpus.current_terms.is_empty());
            if term_matched {
                self.active_corpus_idle_turns.insert(id, 0);
                continue;
            }
            if !advance_turn {
                continue;
            }
            let idle_turns = self.active_corpus_idle_turns.entry(id.clone()).or_insert(0);
            *idle_turns = idle_turns.saturating_add(1);
            if *idle_turns >= ACTIVE_CORPUS_IDLE_TURN_LIMIT {
                self.active_corpus_ids.remove(&id);
                self.active_corpus_idle_turns.remove(&id);
                self.dormant_corpus_ids.insert(id);
                changed = true;
            }
        }
        changed
    }

    fn matching_corpora(
        &self,
        languages: &[String],
        hints: &[&str],
    ) -> Result<Vec<SelectedCorpus>, String> {
        let mut evidence =
            fold_for_match(
                &self
                    .transcript_history
                    .iter()
                    .map(String::as_str)
                    .chain(self.dialogue_history.iter().flat_map(|turn| {
                        [turn.source_text.as_str(), turn.translated_text.as_str()]
                    }))
                    .chain(hints.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        let current_evidence = fold_for_match(&hints.join(" "));
        // New activation must be justified by the current turn (plus live
        // runtime facts), never by combining unrelated words from separate
        // history entries. Confirmed active state is retained separately.
        let mut activation_evidence = current_evidence.clone();
        let snapshot = self.catalog.snapshot()?;
        // Runtime `Always` corpora provide mode evidence. Runtime-only corpora
        // remain prompt data, but they do not activate static specialist
        // glossaries by themselves.
        for corpus in snapshot
            .iter()
            .filter(|corpus| corpus.activation == CorpusActivation::Always)
        {
            for term in &corpus.terms {
                for value in languages.iter().filter_map(|language| term.value(language)) {
                    evidence.push(' ');
                    evidence.push_str(&fold_for_match(value));
                    evidence.push(' ');
                    evidence.push_str(&fold_for_match(&split_identifier_words(value)));
                    activation_evidence.push(' ');
                    activation_evidence.push_str(&fold_for_match(value));
                    activation_evidence.push(' ');
                    activation_evidence.push_str(&fold_for_match(&split_identifier_words(value)));
                }
            }
        }

        let mut matches = snapshot
            .into_iter()
            .enumerate()
            .filter_map(|(index, corpus)| {
                let matched_triggers = corpus
                    .triggers
                    .iter()
                    .chain(corpus.trigger_aliases.iter())
                    .filter(|term| term_matches_evidence(term, &evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let matched_canonical_triggers = corpus
                    .triggers
                    .iter()
                    .filter(|term| term_matches_evidence(term, &evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let matched_activation_context = corpus
                    .activation_context
                    .iter()
                    .filter(|term| term_matches_evidence(term, &evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let current_matched_triggers = corpus
                    .triggers
                    .iter()
                    .chain(corpus.trigger_aliases.iter())
                    .filter(|term| term_matches_evidence(term, &activation_evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let current_activation_context = corpus
                    .activation_context
                    .iter()
                    .filter(|term| term_matches_evidence(term, &activation_evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let activation_context_satisfied =
                    corpus.activation_context.is_empty() || !current_activation_context.is_empty();
                let current_speech_triggers = current_matched_triggers
                    .iter()
                    .filter(|term| term_matches_evidence(term, &current_evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let current_terms = corpus
                    .terms
                    .iter()
                    .filter(|term| term_matches_evidence(term, &current_evidence))
                    .cloned()
                    .collect::<Vec<_>>();
                let dormant = self.dormant_corpus_ids.contains(&corpus.id);
                let newly_eligible =
                    !current_matched_triggers.is_empty() && activation_context_satisfied;
                let unconstrained_history_eligible = corpus.activation_context.is_empty()
                    && !matched_triggers.is_empty()
                    && !dormant;
                let eligible = corpus.activation == CorpusActivation::Always
                    || corpus.activation == CorpusActivation::RuntimeOnly
                    || newly_eligible
                    || unconstrained_history_eligible
                    || self.active_corpus_ids.contains(&corpus.id);
                let current_triggers =
                    if corpus.activation == CorpusActivation::OnEvidence && newly_eligible {
                        current_speech_triggers
                    } else {
                        Vec::new()
                    };
                let activation_triggers = if self.active_corpus_ids.contains(&corpus.id) {
                    Vec::new()
                } else {
                    current_triggers.clone()
                };
                let score = current_matched_triggers.len()
                    + current_activation_context.len()
                    + matched_triggers.len()
                    + matched_activation_context.len();
                eligible.then_some((
                    corpus.priority,
                    score,
                    index,
                    SelectedCorpus {
                        definition: corpus,
                        matched_canonical_triggers,
                        current_triggers,
                        current_terms,
                        activation_triggers,
                        currently_eligible: newly_eligible,
                    },
                ))
            })
            .collect::<Vec<_>>();
        let has_current_activation = matches
            .iter()
            .any(|(_, _, _, corpus)| !corpus.current_triggers.is_empty());
        if has_current_activation {
            matches.retain(|(_, _, _, corpus)| {
                corpus.definition.activation == CorpusActivation::Always
                    || corpus.definition.activation == CorpusActivation::RuntimeOnly
                    || corpus.currently_eligible
                    || !corpus.current_triggers.is_empty()
            });
        } else if !self.active_corpus_ids.is_empty() {
            matches.retain(|(_, _, _, corpus)| {
                corpus.definition.activation == CorpusActivation::Always
                    || corpus.definition.activation == CorpusActivation::RuntimeOnly
                    || self.active_corpus_ids.contains(&corpus.definition.id)
            });
        }
        matches.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        Ok(matches
            .into_iter()
            .map(|(_, _, _, corpus)| corpus)
            .collect())
    }
}

fn push_bounded<T>(queue: &mut VecDeque<T>, value: T, capacity: usize) {
    if queue.len() == capacity {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn compact_history_text(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(MAX_HISTORY_TEXT_CHARS).collect()
}

fn split_identifier_words(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let mut expanded = String::with_capacity(value.len());
    for (index, current) in chars.iter().copied().enumerate() {
        let previous = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index + 1).copied();
        let camel_boundary = current.is_uppercase()
            && previous.is_some_and(|value| value.is_lowercase() || value.is_numeric());
        let acronym_boundary = current.is_uppercase()
            && previous.is_some_and(char::is_uppercase)
            && next.is_some_and(char::is_lowercase);
        if (camel_boundary || acronym_boundary) && !expanded.ends_with(' ') {
            expanded.push(' ');
        }
        if current.is_alphanumeric() {
            expanded.push(current);
        } else if !expanded.ends_with(' ') {
            expanded.push(' ');
        }
    }
    expanded
}

fn normalized_language_code(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .split('-')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn task_languages(source_language: &str, target_language: &str) -> Vec<String> {
    let mut languages = Vec::new();
    for language in std::iter::once(source_language)
        .chain(target_language.split(','))
        .map(normalized_language_code)
        .filter(|language| language != "auto")
        .filter(|language| language_index(language).is_some())
    {
        if !languages.contains(&language) {
            languages.push(language);
        }
    }
    languages
}

fn selected_term_rows(corpora: &[SelectedCorpus], languages: &[String]) -> Vec<SelectedPromptTerm> {
    let mut rows: Vec<SelectedPromptTerm> = Vec::new();
    let mut seen = HashSet::new();
    // Canonical triggers may provide a direct title mapping or promote an
    // overlapping authoritative Term. Trigger Aliases are intentionally not
    // considered here: they activate and highlight but never enter prompts.
    for corpus in corpora {
        for trigger in &corpus.matched_canonical_triggers {
            let authoritative_terms = corpus
                .definition
                .terms
                .iter()
                .filter(|term| terms_overlap(trigger, term))
                .collect::<Vec<_>>();
            for term in authoritative_terms {
                insert_selected_term(&mut rows, &mut seen, &corpus.definition, term, languages);
            }
            if !corpus
                .definition
                .terms
                .iter()
                .any(|term| terms_overlap(trigger, term))
                && is_multilingual_mapping(trigger, languages)
            {
                insert_selected_term(&mut rows, &mut seen, &corpus.definition, trigger, languages);
            }
        }
    }
    // Interleave ranked corpora instead of exhausting the largest one first.
    // A busy VRCX room can contain dozens of player names; it must not crowd a
    // triggered specialist corpus (for example Overwatch heroes) out of the
    // prompt before that corpus contributes any terminology.
    let max_terms = corpora
        .iter()
        .map(|corpus| corpus.definition.terms.len())
        .max()
        .unwrap_or(0);
    for term_index in 0..max_terms {
        for (corpus, term) in corpora.iter().filter_map(|corpus| {
            corpus
                .definition
                .terms
                .get(term_index)
                .map(|term| (&corpus.definition, term))
        }) {
            insert_selected_term(&mut rows, &mut seen, corpus, term, languages);
        }
    }
    rows
}

fn terms_overlap(left: &CorpusTerm, right: &CorpusTerm) -> bool {
    let left_values = term_all_values(left)
        .map(fold_for_match)
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    term_all_values(right)
        .map(fold_for_match)
        .any(|value| !value.is_empty() && left_values.contains(&value))
}

fn is_multilingual_mapping(term: &CorpusTerm, languages: &[String]) -> bool {
    languages
        .iter()
        .filter_map(|language| term.value(language))
        .map(fold_for_match)
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>()
        .len()
        >= 2
}

fn selected_activation_rows(
    corpora: &[SelectedCorpus],
    languages: &[String],
) -> Vec<SelectedPromptTerm> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for corpus in corpora {
        for trigger in &corpus.activation_triggers {
            insert_selected_term(&mut rows, &mut seen, &corpus.definition, trigger, languages);
        }
    }
    rows
}

fn selected_recognition_context_rows(
    corpora: &[SelectedCorpus],
    terms: &[SelectedPromptTerm],
    languages: &[String],
) -> Vec<SelectedPromptTerm> {
    let mut rows = terms.to_vec();
    let mut seen = rows
        .iter()
        .map(|term| term.row.to_lowercase())
        .collect::<HashSet<_>>();
    for corpus in corpora {
        for trigger in corpus
            .matched_canonical_triggers
            .iter()
            .chain(corpus.current_triggers.iter())
        {
            insert_selected_term(&mut rows, &mut seen, &corpus.definition, trigger, languages);
        }
    }
    rows
}

fn selected_recognition_correction_rows(
    corpora: &[SelectedCorpus],
    languages: &[String],
) -> Vec<RecognitionCorrectionTerm> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for corpus in corpora {
        for alias in &corpus.definition.trigger_aliases {
            let Some(canonical) = corpus
                .definition
                .terms
                .iter()
                .find(|term| terms_overlap(alias, term))
            else {
                continue;
            };
            let alias_values = task_term_values(alias, languages);
            let canonical_values = task_term_values(canonical, languages);
            let correction_aliases = alias_values
                .iter()
                .filter(|(alias_language, alias_value)| {
                    !canonical_values
                        .iter()
                        .any(|(canonical_language, canonical_value)| {
                            canonical_language == alias_language
                                && values_equal(canonical_value, alias_value)
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            if correction_aliases.is_empty() {
                continue;
            }
            let row_key = canonical_values
                .iter()
                .chain(correction_aliases.iter())
                .map(|(language, value)| format!("{language}:{}", fold_for_match(value)))
                .collect::<Vec<_>>()
                .join("\u{1f}");
            if !seen.insert(format!("{}:{row_key}", corpus.definition.id)) {
                continue;
            }
            rows.push(RecognitionCorrectionTerm {
                canonical: canonical_values,
                aliases: correction_aliases,
                sources: vec![CorpusTermSource {
                    corpus_id: corpus.definition.id.clone(),
                    domain: corpus.definition.domain.clone(),
                    subdomain: corpus.definition.subdomain.clone(),
                    title: corpus.definition.title.clone(),
                }],
            });
        }
    }
    rows
}

fn insert_selected_term(
    rows: &mut Vec<SelectedPromptTerm>,
    seen: &mut HashSet<String>,
    corpus: &CorpusDefinition,
    term: &CorpusTerm,
    languages: &[String],
) {
    let row = selected_term_row(term, languages);
    if row.split(',').all(str::is_empty) {
        return;
    }
    let source = CorpusTermSource {
        corpus_id: corpus.id.clone(),
        domain: corpus.domain.clone(),
        subdomain: corpus.subdomain.clone(),
        title: corpus.title.clone(),
    };
    let normalized_row = row.to_lowercase();
    if !seen.insert(normalized_row.clone()) {
        if let Some(index) = rows
            .iter()
            .position(|existing| existing.row.to_lowercase() == normalized_row)
            && !rows[index].sources.contains(&source)
        {
            rows[index].sources.push(source);
        }
        return;
    }
    rows.push(SelectedPromptTerm {
        values: task_term_values(term, languages),
        match_values: all_term_values(term),
        row,
        sources: vec![source],
    });
}

fn term_matches_evidence(term: &CorpusTerm, evidence: &str) -> bool {
    term_all_values(term)
        .map(fold_for_match)
        .any(|keyword| evidence_contains_term(evidence, &keyword))
}

fn term_all_values(term: &CorpusTerm) -> impl Iterator<Item = &str> {
    term.ordered_values
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn task_term_values(term: &CorpusTerm, languages: &[String]) -> Vec<(String, String)> {
    languages
        .iter()
        .filter_map(|language| {
            term.value(language)
                .map(|value| (language.clone(), value.to_owned()))
        })
        .collect()
}

fn all_term_values(term: &CorpusTerm) -> Vec<(String, String)> {
    CORPUS_LANGUAGE_ORDER
        .iter()
        .filter_map(|language| {
            term.value(language)
                .map(|value| ((*language).to_owned(), value.to_owned()))
        })
        .collect()
}

fn values_equal(left: &str, right: &str) -> bool {
    fold_for_match(left) == fold_for_match(right)
}

fn selected_term_row(term: &CorpusTerm, languages: &[String]) -> String {
    languages
        .iter()
        .map(|language| term.value(language).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",")
}

fn build_asr_prompt(
    terms: &[SelectedPromptTerm],
    budget: usize,
    token_budget: usize,
) -> Option<String> {
    let mut hotwords = Vec::new();
    let mut seen = HashSet::new();
    // Qwen3-ASR officially treats the system prompt as free-form
    // context/hotwords. Full dialogue sentences are intentionally excluded:
    // they can be copied verbatim instead of transcribing the audio.
    for term in terms {
        for (_, value) in &term.values {
            let normalized = fold_for_match(value);
            if normalized.is_empty() || !seen.insert(normalized) {
                continue;
            }
            let mut candidate_hotwords = hotwords.clone();
            candidate_hotwords.push(value.clone());
            let candidate = render_asr_prompt(&candidate_hotwords);
            if within_prompt_budget(&candidate, budget, token_budget) {
                hotwords = candidate_hotwords;
            }
        }
    }
    (!hotwords.is_empty()).then(|| render_asr_prompt(&hotwords))
}

fn build_asr_echo_guard(
    transcripts: &VecDeque<String>,
    turns: &[&BilingualTurn],
    history_limit: usize,
) -> Vec<String> {
    let mut guard = Vec::new();
    let mut seen = HashSet::new();
    let recent_transcripts = transcripts
        .iter()
        .rev()
        .take(history_limit)
        .rev()
        .map(String::as_str);
    let recent_pairs = newest_turns(turns, history_limit)
        .into_iter()
        .flat_map(|turn| [turn.source_text.as_str(), turn.translated_text.as_str()]);
    for value in recent_transcripts.chain(recent_pairs) {
        let normalized = fold_for_match(value);
        if !normalized.is_empty() && seen.insert(normalized) {
            guard.push(value.to_owned());
        }
    }
    guard
}

fn build_translation_prompt(
    languages: &[String],
    terms: &[SelectedPromptTerm],
    turns: &[&BilingualTurn],
    history_limit: usize,
    budget: usize,
    token_budget: usize,
) -> (Option<String>, Vec<SelectedPromptTerm>, Vec<BilingualTurn>) {
    let useful_terms = terms
        .iter()
        .filter(|term| {
            term.row
                .split(',')
                .filter(|value| !value.is_empty())
                .count()
                >= 2
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut selected_terms = Vec::new();
    // Reserve roughly half for aligned history when it exists. Terminology is
    // added in corpus priority order and never split mid-row.
    let term_budget = if turns.is_empty() {
        budget
    } else {
        budget * 3 / 5
    };
    for term in useful_terms {
        let mut candidate_terms = selected_terms.clone();
        candidate_terms.push(term);
        let rows = candidate_terms
            .iter()
            .map(|term| term.row.clone())
            .collect::<Vec<_>>();
        let candidate = render_translation_prompt(languages, &rows, &[]);
        if candidate.chars().count() > term_budget
            || estimated_prompt_tokens(&candidate) > token_budget * 3 / 5
        {
            continue;
        }
        selected_terms = candidate_terms;
    }

    let mut selected_turns = Vec::new();
    for turn in newest_turns(turns, history_limit).into_iter().rev() {
        let mut candidate_turns = vec![turn];
        candidate_turns.extend(selected_turns.iter().copied());
        let rows = selected_terms
            .iter()
            .map(|term| term.row.clone())
            .collect::<Vec<_>>();
        let candidate = render_translation_prompt(languages, &rows, &candidate_turns);
        if !within_prompt_budget(&candidate, budget, token_budget) {
            continue;
        }
        selected_turns = candidate_turns;
    }
    let rows = selected_terms
        .iter()
        .map(|term| term.row.clone())
        .collect::<Vec<_>>();
    let prompt = render_translation_prompt(languages, &rows, &selected_turns);
    let prompt = (!selected_terms.is_empty() || !selected_turns.is_empty())
        .then_some(prompt)
        .filter(|prompt| within_prompt_budget(prompt, budget, token_budget));
    let admitted_terms = if prompt.is_some() {
        selected_terms
    } else {
        Vec::new()
    };
    let admitted_history = if prompt.is_some() {
        selected_turns.into_iter().cloned().collect()
    } else {
        Vec::new()
    };
    (prompt, admitted_terms, admitted_history)
}

fn term_spans(text: &str, term: &str) -> Vec<(usize, usize)> {
    let folded_text = FoldedText::new(text);
    let folded_value = fold_for_match(term);
    if folded_value.is_empty() {
        return Vec::new();
    }
    folded_text
        .text
        .match_indices(&folded_value)
        .filter_map(|(folded_start, _)| {
            let folded_end = folded_start + folded_value.len();
            term_boundary_matches(&folded_text.text, folded_start, folded_end, &folded_value)
                .then(|| folded_text.original_span(folded_start, folded_end))
                .flatten()
        })
        .collect()
}

fn select_non_overlapping_corrections(
    mut corrections: Vec<CorpusRecognitionCorrection>,
) -> Vec<CorpusRecognitionCorrection> {
    corrections.sort_by(|left, right| {
        left.start_byte
            .cmp(&right.start_byte)
            .then_with(|| right.end_byte.cmp(&left.end_byte))
    });
    let mut selected: Vec<CorpusRecognitionCorrection> = Vec::new();
    for correction in corrections {
        if selected.iter().any(|existing| {
            correction.start_byte < existing.end_byte && correction.end_byte > existing.start_byte
        }) {
            continue;
        }
        selected.push(correction);
    }
    selected
}

fn match_selected_terms(
    text: &str,
    terms: &[SelectedPromptTerm],
    language: Option<&str>,
) -> Vec<CorpusTermMatch> {
    let folded_text = FoldedText::new(text);
    let normalized_language = language.map(normalized_language_code);
    let mut candidates = Vec::new();
    for term in terms {
        let mut seen_values = HashSet::new();
        for (value_language, value) in &term.match_values {
            if normalized_language
                .as_ref()
                .is_some_and(|language| language != value_language)
            {
                continue;
            }
            let folded_value = fold_for_match(value);
            if folded_value.is_empty() || !seen_values.insert(folded_value.clone()) {
                continue;
            }
            for (folded_start, _) in folded_text.text.match_indices(&folded_value) {
                let folded_end = folded_start + folded_value.len();
                if !term_boundary_matches(
                    &folded_text.text,
                    folded_start,
                    folded_end,
                    &folded_value,
                ) {
                    continue;
                }
                let Some((start, end)) = folded_text.original_span(folded_start, folded_end) else {
                    continue;
                };
                let (Ok(start_byte), Ok(end_byte)) = (u32::try_from(start), u32::try_from(end))
                else {
                    continue;
                };
                candidates.push(CorpusTermMatch {
                    start_byte,
                    end_byte,
                    text: text[start..end].to_owned(),
                    sources: term.sources.clone(),
                });
            }
        }
    }
    candidates.sort_by(|left, right| {
        left.start_byte
            .cmp(&right.start_byte)
            .then_with(|| right.end_byte.cmp(&left.end_byte))
    });

    let mut matches: Vec<CorpusTermMatch> = Vec::new();
    for candidate in candidates {
        if let Some(existing) = matches.iter_mut().find(|existing| {
            existing.start_byte == candidate.start_byte && existing.end_byte == candidate.end_byte
        }) {
            for source in candidate.sources {
                if !existing.sources.contains(&source) {
                    existing.sources.push(source);
                }
            }
            continue;
        }
        if matches.iter().any(|existing| {
            candidate.start_byte < existing.end_byte && candidate.end_byte > existing.start_byte
        }) {
            continue;
        }
        matches.push(candidate);
    }
    matches.sort_by_key(|term_match| term_match.start_byte);
    matches
}

fn fold_for_match(value: &str) -> String {
    value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

struct FoldedText {
    text: String,
    original_boundaries: Vec<Option<usize>>,
}

impl FoldedText {
    fn new(value: &str) -> Self {
        let mut text = String::new();
        let mut original_boundaries = vec![Some(0)];
        for (original_start, character) in value.char_indices() {
            let original_end = original_start + character.len_utf8();
            let folded = fold_for_match(&character.to_string());
            if folded.is_empty() {
                if let Some(boundary) = original_boundaries.last_mut() {
                    *boundary = Some(original_end);
                }
                continue;
            }
            if let Some(boundary) = original_boundaries.last_mut() {
                *boundary = Some(original_start);
            }
            text.push_str(&folded);
            original_boundaries.resize(text.len() + 1, None);
            original_boundaries[text.len()] = Some(original_end);
        }
        Self {
            text,
            original_boundaries,
        }
    }

    fn original_span(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let original_start = self.original_boundaries.get(start).copied().flatten()?;
        let original_end = self.original_boundaries.get(end).copied().flatten()?;
        Some((original_start, original_end))
    }
}

fn evidence_contains_term(evidence: &str, term: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    evidence
        .match_indices(term)
        .any(|(start, _)| term_boundary_matches(evidence, start, start + term.len(), term))
}

fn term_boundary_matches(text: &str, start: usize, end: usize, value: &str) -> bool {
    if !value.is_ascii() {
        return true;
    }
    let starts_with_word = value.chars().next().is_some_and(char::is_alphanumeric);
    let ends_with_word = value.chars().next_back().is_some_and(char::is_alphanumeric);
    let before_is_word = text[..start]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric);
    let after_is_word = text[end..]
        .chars()
        .next()
        .is_some_and(char::is_alphanumeric);
    (!starts_with_word || !before_is_word) && (!ends_with_word || !after_is_word)
}

fn within_prompt_budget(prompt: &str, char_budget: usize, token_budget: usize) -> bool {
    prompt.chars().count() <= char_budget && estimated_prompt_tokens(prompt) <= token_budget
}

/// Conservative tokenizer-independent estimate used before request assembly.
/// CJK characters generally occupy one token, while ASCII text is budgeted at
/// roughly three bytes per token. Provider-side overflow handling remains the
/// final guard for model-specific tokenizers.
fn estimated_prompt_tokens(prompt: &str) -> usize {
    let weighted = prompt
        .chars()
        .map(|character| if character.is_ascii() { 1 } else { 3 })
        .sum::<usize>();
    weighted.div_ceil(3)
}

fn newest_turns<'a>(turns: &[&'a BilingualTurn], limit: usize) -> Vec<&'a BilingualTurn> {
    turns
        .iter()
        .rev()
        .take(limit)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn render_asr_prompt(hotwords: &[String]) -> String {
    format!("Vocabulary: {}", hotwords.join(", "))
}

fn render_translation_prompt(
    languages: &[String],
    terms: &[String],
    turns: &[&BilingualTurn],
) -> String {
    let mut prompt = format!(
        "# Translation Context\n\n## Language Order\n\n{}",
        languages.join(",")
    );
    append_terms(&mut prompt, terms);
    append_dialogue(&mut prompt, turns);
    prompt
}

fn append_terms(prompt: &mut String, terms: &[String]) {
    if !terms.is_empty() {
        prompt.push_str("\n\n## Terminology\n\n");
        prompt.push_str(&terms.join("\n"));
    }
}

fn append_dialogue(prompt: &mut String, turns: &[&BilingualTurn]) {
    if turns.is_empty() {
        return;
    }
    prompt.push_str("\n\n## Recent Bilingual History");
    for turn in turns {
        prompt.push_str("\n\n");
        prompt.push_str(&turn.source_language);
        prompt.push_str(": ");
        prompt.push_str(&turn.source_text);
        prompt.push('\n');
        prompt.push_str(&turn.target_language);
        prompt.push_str(": ");
        prompt.push_str(&turn.translated_text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PromptContextConfig {
        PromptContextConfig {
            asr_max_chars: 520,
            translation_max_chars: 700,
            max_entries: 3,
            asr_history_entries: 1,
            translation_history_entries: 3,
            ..PromptContextConfig::default()
        }
    }

    fn corpus(
        id: &str,
        priority: i32,
        activation: CorpusActivation,
        triggers: &[(&str, &str)],
        terms: &[(&str, &str)],
    ) -> CorpusDefinition {
        let mut parts = id.split('.');
        CorpusDefinition {
            schema: xr_corpus_core::CORPUS_SCHEMA.into(),
            id: id.into(),
            domain: parts.next().unwrap().into(),
            subdomain: parts.next().unwrap().into(),
            title: id.into(),
            priority,
            activation,
            triggers: triggers
                .iter()
                .map(|(zh, en)| bilingual_term(zh, en))
                .collect(),
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: terms
                .iter()
                .map(|(zh, en)| bilingual_term(zh, en))
                .collect(),
        }
    }

    fn bilingual_term(zh: &str, en: &str) -> CorpusTerm {
        term_from_values(&[("zh", zh), ("en", en)])
    }

    fn term_from_values(values: &[(&str, &str)]) -> CorpusTerm {
        let mut row = vec![String::new(); xr_corpus_core::CORPUS_LANGUAGE_ORDER.len()];
        for (language, value) in values {
            row[language_index(language).unwrap()] = (*value).into();
        }
        CorpusTerm::from_ordered(row).unwrap()
    }

    fn catalog() -> CorpusCatalog {
        CorpusCatalog::from_definitions(vec![
            corpus(
                "virtual-worlds.vrchat.platform",
                1,
                CorpusActivation::OnEvidence,
                &[("虚拟形象", "avatar"), ("VRChat", "VRChat")],
                &[("开放声音控制", "Open Sound Control")],
            ),
            corpus(
                "audio-technology.speech-recognition.models",
                2,
                CorpusActivation::OnEvidence,
                &[("麦克风", "microphone")],
                &[("ERes2NetV2", "ERes2NetV2")],
            ),
        ])
        .unwrap()
    }

    #[test]
    fn one_selection_builds_asr_and_translation_views() {
        let mut context = PromptContextManager::new(config(), catalog()).unwrap();
        context.record_transcript("My VRChat avatar uses this microphone");
        context.record_translation("en", "zh", "Hello Mercy", "你好，天使");
        let snapshot = context
            .select("auto", "zh,en", &[], (1_000, 1_000))
            .unwrap();
        let asr = snapshot.asr_prompt().unwrap();
        let translation = snapshot.translation_prompt().unwrap();
        assert!(asr.starts_with("Vocabulary: "));
        assert!(asr.contains("ERes2NetV2"));
        assert!(!asr.contains("Hello Mercy"));
        assert!(
            snapshot
                .asr_echo_guard()
                .iter()
                .any(|value| value == "Hello Mercy")
        );
        assert!(translation.starts_with("# Translation Context"));
        assert!(translation.contains("开放声音控制,Open Sound Control"));
        assert!(translation.contains("zh: 你好，天使"));
        assert!(asr.chars().count() <= 520);
        assert!(translation.chars().count() <= 700);
    }

    #[test]
    fn disabled_context_returns_an_empty_snapshot() {
        let mut context = PromptContextManager::new(
            PromptContextConfig {
                enabled: false,
                ..PromptContextConfig::default()
            },
            catalog(),
        )
        .unwrap();
        assert_eq!(
            context
                .select("auto", "zh,en", &[], (1_000, 1_000))
                .unwrap(),
            PromptContextSnapshot::default()
        );
    }

    #[test]
    fn history_is_bounded_and_only_successful_pairs_reach_prompts() {
        let mut context = PromptContextManager::new(
            PromptContextConfig {
                max_entries: 2,
                asr_history_entries: 1,
                translation_history_entries: 2,
                ..config()
            },
            catalog(),
        )
        .unwrap();
        context.record_transcript("untranslated evidence");
        context.record_translation("en", "zh", "old", "旧");
        context.record_translation("en", "zh", "middle", "中");
        context.record_translation("en", "zh", "new", "新");
        let snapshot = context
            .select("auto", "zh,en", &[], (1_000, 1_000))
            .unwrap();
        assert!(snapshot.asr_prompt().is_none());
        let translation = snapshot.translation_prompt().unwrap();
        assert!(snapshot.asr_echo_guard().iter().any(|value| value == "new"));
        assert!(snapshot.asr_echo_guard().iter().any(|value| value == "新"));
        assert!(
            !snapshot
                .asr_echo_guard()
                .iter()
                .any(|value| value == "middle")
        );
        assert!(!translation.contains("en: old"));
        assert!(translation.contains("en: middle"));
        assert!(translation.contains("en: new"));
    }

    #[test]
    fn prior_topic_activates_bilingual_terms_for_the_next_translation() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.overwatch.heroes",
            70,
            CorpusActivation::OnEvidence,
            &[("守望先锋", "Overwatch"), ("天使", "Mercy")],
            &[("天使", "Mercy")],
        )])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let mercy_turn = context
            .select("auto", "zh,en", &["Do you play Overwatch?"], (1_000, 1_000))
            .unwrap();
        assert!(
            mercy_turn
                .translation_prompt()
                .unwrap()
                .contains("天使,Mercy")
        );
        let title_matches = mercy_turn.translation_term_matches(
            "Do you play Overwatch?",
            "你玩《守望先锋》吗？",
            "zh",
        );
        assert_eq!(title_matches[0].text, "守望先锋");
        assert_eq!(
            title_matches[0].sources[0].corpus_id,
            "games.overwatch.heroes"
        );
        let matches = mercy_turn.translation_term_matches("I love Mercy.", "我喜欢天使。", "zh");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text, "天使");
        assert_eq!(matches[0].sources[0].corpus_id, "games.overwatch.heroes");
        assert_eq!(matches[0].sources[0].domain, "games");
        assert_eq!(matches[0].sources[0].subdomain, "overwatch");

        context.record_transcript("Do you play Overwatch?");
        context.record_translation("en", "zh", "I love Mercy.", "我喜欢天使。");
        let following_turn = context
            .select("auto", "zh,en", &[], (1_000, 1_000))
            .unwrap();
        let asr = following_turn.asr_prompt().unwrap();
        assert!(!asr.contains("I love Mercy."));
        assert!(!asr.contains("我喜欢天使。"));
        assert!(
            following_turn
                .asr_echo_guard()
                .iter()
                .any(|value| value == "I love Mercy.")
        );
        assert!(
            following_turn
                .asr_echo_guard()
                .iter()
                .any(|value| value == "我喜欢天使。")
        );
        assert!(following_turn.activation_matches("Mercy").is_empty());
    }

    #[test]
    fn current_source_term_survives_general_prompt_budget_truncation() {
        let mut terms = (0..40)
            .map(|index| {
                term_from_values(&[
                    ("zh", &format!("很长的占位术语{index:02}")),
                    ("en", &format!("Long Placeholder Term {index:02}")),
                ])
            })
            .collect::<Vec<_>>();
        terms.push(term_from_values(&[("zh", "莱因哈特"), ("en", "Reinhardt")]));
        let definition = CorpusDefinition {
            schema: xr_corpus_core::CORPUS_SCHEMA.into(),
            id: "games.overwatch.heroes".into(),
            domain: "games".into(),
            subdomain: "overwatch".into(),
            title: "Overwatch Heroes".into(),
            priority: 70,
            activation: CorpusActivation::OnEvidence,
            triggers: vec![term_from_values(&[("zh", "守望先锋"), ("en", "Overwatch")])],
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms,
        };
        let mut context = PromptContextManager::new(
            config(),
            CorpusCatalog::from_definitions(vec![definition]).unwrap(),
        )
        .unwrap();
        let snapshot = context
            .select("auto", "zh,en", &["Overwatch"], (1_000, 256))
            .unwrap();

        let prompt = snapshot
            .translation_prompt_for("I played Reinhardt.")
            .unwrap();
        assert!(prompt.contains("莱因哈特,Reinhardt"));
        let matches =
            snapshot.translation_term_matches("I played Reinhardt.", "我玩莱因哈特。", "zh");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text, "莱因哈特");
        assert!(
            snapshot
                .translation_term_matches("Long Placeholder Term 39", "很长的占位术语39", "zh",)
                .is_empty()
        );
    }

    #[test]
    fn bidirectional_direction_change_preserves_active_topic() {
        let corpus = CorpusDefinition {
            schema: xr_corpus_core::CORPUS_SCHEMA.into(),
            id: "games.overwatch.heroes".into(),
            domain: "games".into(),
            subdomain: "overwatch".into(),
            title: "Overwatch Heroes".into(),
            priority: 70,
            activation: CorpusActivation::OnEvidence,
            triggers: vec![term_from_values(&[("zh", "zh-game"), ("en", "Overwatch")])],
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![term_from_values(&[("zh", "zh-angel"), ("en", "Mercy")])],
        };
        let mut context = PromptContextManager::new(
            config(),
            CorpusCatalog::from_definitions(vec![corpus]).unwrap(),
        )
        .unwrap();

        context
            .select("en", "zh", &["Overwatch"], (1_000, 1_000))
            .unwrap();
        assert_eq!(context.active_corpus_ids(), vec!["games.overwatch.heroes"]);

        let reversed = context
            .select("zh", "en", &["zh-angel"], (1_000, 1_000))
            .unwrap();
        assert_eq!(context.active_corpus_ids(), vec!["games.overwatch.heroes"]);
        let prompt = reversed.translation_prompt_for("zh-angel").unwrap();
        assert!(prompt.contains("## Language Order\n\nzh,en"));
        assert!(prompt.contains("zh-angel,Mercy"));
    }

    #[test]
    fn route_language_change_discards_old_language_context_and_columns() {
        let corpus = CorpusDefinition {
            schema: xr_corpus_core::CORPUS_SCHEMA.into(),
            id: "games.overwatch.heroes".into(),
            domain: "games".into(),
            subdomain: "overwatch".into(),
            title: "Overwatch Heroes".into(),
            priority: 70,
            activation: CorpusActivation::OnEvidence,
            triggers: vec![term_from_values(&[
                ("zh", "zh-game"),
                ("en", "Overwatch"),
                ("ja", "ja-game"),
            ])],
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![term_from_values(&[
                ("zh", "zh-only-hero"),
                ("en", "Mercy"),
                ("ja", "ja-only-hero"),
            ])],
        };
        let mut context = PromptContextManager::new(
            config(),
            CorpusCatalog::from_definitions(vec![corpus]).unwrap(),
        )
        .unwrap();

        let zh_en = context
            .select("auto", "zh,en", &["Overwatch"], (1_000, 1_000))
            .unwrap();
        assert!(zh_en.asr_prompt().unwrap().contains("zh-only-hero"));
        assert!(
            zh_en
                .translation_prompt_for("Mercy")
                .unwrap()
                .contains("zh-only-hero,Mercy")
        );
        context.record_transcript("Overwatch Mercy");
        context.record_translation("en", "zh", "Mercy", "zh-only-hero");

        let ja_en = context
            .select("ja", "en", &["Overwatch"], (1_000, 1_000))
            .unwrap();
        let asr = ja_en.asr_prompt().unwrap();
        assert!(asr.contains("ja-only-hero"));
        assert!(asr.contains("Mercy"));
        assert!(!asr.contains("zh-only-hero"));

        let prompt = ja_en.translation_prompt_for("Mercy").unwrap();
        assert!(prompt.contains("## Language Order\n\nja,en"));
        assert!(prompt.contains("ja-only-hero,Mercy"));
        assert!(!prompt.contains("zh-only-hero"));
        assert!(!prompt.contains("zh:"));
    }

    #[test]
    fn bidirectional_route_uses_only_the_configured_language_pair() {
        let corpus = CorpusDefinition {
            schema: xr_corpus_core::CORPUS_SCHEMA.into(),
            id: "games.overwatch.heroes".into(),
            domain: "games".into(),
            subdomain: "overwatch".into(),
            title: "Overwatch Heroes".into(),
            priority: 70,
            activation: CorpusActivation::OnEvidence,
            triggers: vec![term_from_values(&[
                ("zh", "zh-game"),
                ("en", "Overwatch"),
                ("ja", "ja-game"),
            ])],
            trigger_aliases: Vec::new(),
            activation_context: Vec::new(),
            terms: vec![term_from_values(&[
                ("zh", "zh-only-hero"),
                ("en", "Mercy"),
                ("ja", "ja-only-hero"),
            ])],
        };
        let mut context = PromptContextManager::new(
            config(),
            CorpusCatalog::from_definitions(vec![corpus]).unwrap(),
        )
        .unwrap();

        let snapshot = context
            .select("auto", "ja,en", &["Overwatch"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt_for("Mercy").unwrap();

        assert!(prompt.contains("## Language Order\n\nja,en"));
        assert!(prompt.contains("ja-only-hero,Mercy"));
        assert!(!prompt.contains("zh-only-hero"));
        assert!(!prompt.contains("zh,en"));
        assert!(!prompt.contains("zh:"));
    }

    #[test]
    fn a_new_explicit_domain_replaces_stale_specialist_context() {
        let catalog = CorpusCatalog::from_definitions(vec![
            corpus(
                "games.overwatch.heroes",
                70,
                CorpusActivation::OnEvidence,
                &[("守望先锋", "Overwatch")],
                &[("天使", "Mercy")],
            ),
            corpus(
                "education-and-science.research.common",
                60,
                CorpusActivation::OnEvidence,
                &[("论文", "paper")],
                &[("同行评审", "peer review")],
            ),
        ])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();
        context.record_transcript("Do you play Overwatch?");

        let snapshot = context
            .select("auto", "zh,en", &["论文写没？"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt().unwrap();
        assert!(prompt.contains("论文,paper"));
        assert!(prompt.contains("同行评审,peer review"));
        assert!(!prompt.contains("天使,Mercy"));
        let matches = snapshot.activation_matches("论文写没？");
        assert_eq!(matches[0].text, "论文");
        assert_eq!(
            matches[0].sources[0].corpus_id,
            "education-and-science.research.common"
        );

        context.record_transcript("论文写没？");
        let follow_up = context
            .select("auto", "zh,en", &["写完了吗？"], (1_000, 1_000))
            .unwrap();
        let follow_up_prompt = follow_up.translation_prompt().unwrap();
        assert!(follow_up_prompt.contains("同行评审,peer review"));
        assert!(!follow_up_prompt.contains("天使,Mercy"));
        assert!(follow_up.activation_matches("写完了吗？").is_empty());
    }

    #[test]
    fn always_active_runtime_terms_activate_matching_static_corpora() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.overwatch.heroes",
            70,
            CorpusActivation::OnEvidence,
            &[("天使", "Mercy")],
            &[("安娜", "Ana"), ("源氏", "Genji"), ("天使", "Mercy")],
        )])
        .unwrap();
        let runtime = corpus(
            "virtual-worlds.vrchat.runtime-room",
            1_000,
            CorpusActivation::Always,
            &[("VRChat", "VRChat")],
            &[("MercyFan", "MercyFan")],
        );
        catalog
            .dynamic_source()
            .replace_snapshot("vrcx", vec![runtime], None)
            .unwrap();

        let mut context = PromptContextManager::new(config(), catalog).unwrap();
        let snapshot = context
            .select("auto", "zh,en", &[], (1_000, 1_000))
            .unwrap();
        let translation = snapshot.translation_prompt().unwrap();
        assert!(translation.contains("MercyFan,MercyFan"));
        assert!(translation.contains("安娜,Ana"));
        assert!(translation.contains("天使,Mercy"));
    }

    #[test]
    fn default_platform_terms_do_not_depend_on_runtime_integrations() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.multiplayer.platforms",
            10,
            CorpusActivation::Always,
            &[],
            &[("VRChat", "VRChat")],
        )])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select("auto", "zh,en", &[], (1_000, 1_000))
            .unwrap();

        let asr = snapshot.asr_prompt().unwrap();
        assert!(asr.contains("VRChat"));
    }

    #[test]
    fn vrchat_runtime_game_mode_selects_daily_chat_without_specialist_topics() {
        let daily = corpus(
            "virtual-worlds.vrchat.community-language",
            65,
            CorpusActivation::OnEvidence,
            &[("VRChat", "VRChat")],
            &[("男娘", "femboy"), ("摸摸", "headpat")],
        );
        let mut avatar = corpus(
            "virtual-worlds.vrchat.avatar-osc",
            55,
            CorpusActivation::OnEvidence,
            &[("Avatar", "Avatar"), ("OSC", "OSC")],
            &[("面部追踪", "face tracking")],
        );
        avatar.activation_context = vec![bilingual_term("VRChat", "VRChat")];
        let catalog = CorpusCatalog::from_definitions(vec![daily, avatar]).unwrap();
        catalog
            .dynamic_source()
            .replace_snapshot(
                "vrcx",
                vec![corpus(
                    "virtual-worlds.vrchat.runtime-game-mode",
                    1_100,
                    CorpusActivation::Always,
                    &[],
                    &[("VRChat", "VRChat")],
                )],
                None,
            )
            .unwrap();

        let mut context = PromptContextManager::new(config(), catalog).unwrap();
        let snapshot = context
            .select("auto", "zh,en", &["hi everyone"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt().unwrap();
        assert!(prompt.contains("男娘,femboy"));
        assert!(prompt.contains("摸摸,headpat"));
        assert!(!prompt.contains("面部追踪,face tracking"));
        assert!(snapshot.activation_matches("hi everyone").is_empty());
    }

    #[test]
    fn specialist_context_decays_back_to_vrchat_daily_chat() {
        let daily = corpus(
            "virtual-worlds.vrchat.community-language",
            65,
            CorpusActivation::OnEvidence,
            &[("VRChat", "VRChat")],
            &[("男娘", "femboy")],
        );
        let mut avatar = corpus(
            "virtual-worlds.vrchat.avatar-osc",
            55,
            CorpusActivation::OnEvidence,
            &[("Avatar", "Avatar")],
            &[("面部追踪", "face tracking")],
        );
        avatar.activation_context = vec![bilingual_term("VRChat", "VRChat")];
        let catalog = CorpusCatalog::from_definitions(vec![daily, avatar]).unwrap();
        catalog
            .dynamic_source()
            .replace_snapshot(
                "vrcx",
                vec![corpus(
                    "virtual-worlds.vrchat.runtime-game-mode",
                    1_100,
                    CorpusActivation::Always,
                    &[],
                    &[("VRChat", "VRChat")],
                )],
                None,
            )
            .unwrap();

        let mut context = PromptContextManager::new(config(), catalog).unwrap();
        let avatar_turn = context
            .select(
                "auto",
                "zh,en",
                &["My Avatar face tracking broke"],
                (1_000, 1_000),
            )
            .unwrap();
        assert!(
            avatar_turn
                .translation_prompt()
                .unwrap()
                .contains("面部追踪,face tracking")
        );

        for text in ["hello", "what are you doing", "nice to meet you"] {
            context.record_transcript(text);
            context.record_translation("en", "zh", text, "你好");
            let _ = context
                .select("auto", "zh,en", &[text], (1_000, 1_000))
                .unwrap();
        }

        let follow_up = context
            .select("auto", "zh,en", &["just chatting"], (1_000, 1_000))
            .unwrap();
        let prompt = follow_up.translation_prompt().unwrap();
        assert!(prompt.contains("男娘,femboy"));
        assert!(!prompt.contains("面部追踪,face tracking"));
    }

    #[test]
    fn active_corpus_term_match_refreshes_topic_decay_without_reactivating_it() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.overwatch.heroes",
            70,
            CorpusActivation::OnEvidence,
            &[("守望先锋", "Overwatch")],
            &[("天使", "Mercy")],
        )])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let activated = context
            .select("auto", "zh,en", &["Overwatch"], (1_000, 1_000))
            .unwrap();
        assert!(
            activated
                .translation_prompt()
                .unwrap()
                .contains("天使,Mercy")
        );

        for text in ["hello", "how are you"] {
            let _ = context
                .select("auto", "zh,en", &[text], (1_000, 1_000))
                .unwrap();
        }

        // Mercy is terminology, not an activation trigger. It keeps the
        // already-active specialist topic alive for this turn.
        let mercy_turn = context
            .select("auto", "zh,en", &["Mercy"], (1_000, 1_000))
            .unwrap();
        assert!(
            mercy_turn
                .translation_prompt()
                .unwrap()
                .contains("天使,Mercy")
        );
        assert!(mercy_turn.activation_matches("Mercy").is_empty());

        for text in ["nice", "thanks"] {
            let _ = context
                .select("auto", "zh,en", &[text], (1_000, 1_000))
                .unwrap();
        }
        let still_active = context
            .select("auto", "zh,en", &[], (1_000, 1_000))
            .unwrap();
        assert!(
            still_active
                .translation_prompt()
                .unwrap()
                .contains("天使,Mercy")
        );
    }

    #[test]
    fn streaming_revisions_advance_topic_decay_only_once_per_turn() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.overwatch.heroes",
            70,
            CorpusActivation::OnEvidence,
            &[("Overwatch", "Overwatch")],
            &[("Mercy-zh", "Mercy")],
        )])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        context
            .select_observation("auto", "zh,en", &["Overwatch"], (1_000, 1_000), true)
            .unwrap();
        context
            .select_observation("auto", "zh,en", &["hello"], (1_000, 1_000), true)
            .unwrap();

        // Repeated provisional windows from the same utterance must not spend
        // the remaining idle-turn allowance.
        for revision in ["hello there", "hello there friend", "hello there friend!"] {
            context
                .select_observation("auto", "zh,en", &[revision], (1_000, 1_000), false)
                .unwrap();
        }
        assert_eq!(context.active_corpus_ids(), vec!["games.overwatch.heroes"]);

        // A later revision containing an in-domain term refreshes the topic
        // even though it does not count as another turn.
        context
            .select_observation("auto", "zh,en", &["Mercy"], (1_000, 1_000), false)
            .unwrap();
        for text in ["one", "two"] {
            context
                .select_observation("auto", "zh,en", &[text], (1_000, 1_000), true)
                .unwrap();
        }
        assert_eq!(context.active_corpus_ids(), vec!["games.overwatch.heroes"]);
        context
            .select_observation("auto", "zh,en", &["three"], (1_000, 1_000), true)
            .unwrap();
        assert!(context.active_corpus_ids().is_empty());
    }

    #[test]
    fn prompt_selection_respects_the_model_token_budget() {
        let mut context = PromptContextManager::new(config(), catalog()).unwrap();
        context.record_transcript("VRChat avatar microphone");
        context.record_translation(
            "en",
            "zh",
            "A fairly long history turn",
            "一段较长的历史内容",
        );

        let snapshot = context.select("auto", "zh,en", &[], (48, 64)).unwrap();
        if let Some(prompt) = snapshot.asr_prompt() {
            assert!(estimated_prompt_tokens(&prompt) <= 48);
        }
        if let Some(prompt) = snapshot.translation_prompt() {
            assert!(estimated_prompt_tokens(&prompt) <= 64);
        }
    }

    #[test]
    fn trigger_alias_resolves_to_authoritative_term_and_only_target_is_highlighted() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.overwatch.heroes",
            70,
            CorpusActivation::OnEvidence,
            &[("Mercy", "Mercy")],
            &[
                ("\u{5929}\u{4f7f}", "Mercy"),
                ("\u{5362}\u{897f}\u{5965}", "Lúcio"),
            ],
        )])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select("auto", "zh,en", &["Do you play Mercy?"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt().unwrap();
        assert!(prompt.contains("\u{5929}\u{4f7f},Mercy"));
        assert!(!prompt.lines().any(|line| line == "Mercy,Mercy"));
        let segment_prompt = snapshot.translation_prompt_for("I love Mercy").unwrap();
        assert!(segment_prompt.contains("\u{5929}\u{4f7f},Mercy"));
        assert!(!segment_prompt.contains("\u{5362}\u{897f}\u{5965},Lúcio"));
        assert!(
            snapshot
                .translation_term_matches("I love Mercy", "Mercy", "zh")
                .is_empty()
        );
        assert_eq!(
            snapshot
                .translation_term_matches("I love Mercy", "\u{5929}\u{4f7f}", "zh")
                .len(),
            1
        );
        let (retry_prompt, missing) = snapshot
            .terminology_retry_prompt(
                "I love Mercy",
                "\u{6211}\u{559c}\u{6b22}\u{6885}\u{897f}",
                "en",
                "zh",
            )
            .unwrap();
        assert_eq!(missing, 1);
        assert!(retry_prompt.contains("\u{5929}\u{4f7f},Mercy"));
        assert_eq!(
            snapshot.missing_translation_term_count(
                "I love Mercy",
                "\u{6211}\u{559c}\u{6b22}\u{5929}\u{4f7f}",
                "en",
                "zh",
            ),
            0
        );
    }

    #[test]
    fn synonym_trigger_activates_corpus_without_becoming_prompt_terminology() {
        let mut definition = corpus(
            "technology.software-and-ai.frontier-models",
            60,
            CorpusActivation::OnEvidence,
            &[("人工智能", "artificial intelligence")],
            &[("开放人工智能", "OpenAI")],
        );
        definition.trigger_aliases = vec![bilingual_term("生成式人工智能", "generative AI")];
        let catalog = CorpusCatalog::from_definitions(vec![definition]).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select(
                "auto",
                "zh,en",
                &["A generative AI product"],
                (1_000, 1_000),
            )
            .unwrap();

        assert_eq!(snapshot.activation_matches("generative AI").len(), 1);
        let prompt = snapshot.translation_prompt().unwrap();
        assert!(prompt.contains("开放人工智能,OpenAI"));
        assert!(!prompt.contains("生成式人工智能,generative AI"));
    }

    #[test]
    fn checked_in_research_aliases_activate_on_essay_and_citation_language() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();

        let mut essay_context = PromptContextManager::new(config(), catalog.clone()).unwrap();
        let essay = essay_context
            .select(
                "auto",
                "zh,en",
                &["Did you finish your essay?"],
                (1_000, 1_000),
            )
            .unwrap();
        assert_eq!(
            essay.activation_matches("Did you finish your essay?")[0].text,
            "essay"
        );

        let mut citation_context = PromptContextManager::new(config(), catalog).unwrap();
        let citation = citation_context
            .select(
                "auto",
                "zh,en",
                &["I did not add a citation."],
                (1_000, 1_000),
            )
            .unwrap();
        assert_eq!(
            citation.activation_matches("I did not add a citation.")[0].text,
            "citation"
        );
    }

    #[test]
    fn checked_in_vrc_alias_does_not_activate_avatar_authoring_terms() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut casual = PromptContextManager::new(config(), catalog.clone()).unwrap();

        let casual_snapshot = casual
            .select("auto", "zh,en", &["Let's meet in VRC"], (1_000, 1_000))
            .unwrap();
        let casual_prompt = casual_snapshot.translation_prompt().unwrap();
        assert!(casual_prompt.contains("男娘,femboy"));
        assert!(!casual_prompt.contains("Avatar Dynamics"));

        let mut authoring = PromptContextManager::new(config(), catalog).unwrap();
        let authoring_snapshot = authoring
            .select(
                "auto",
                "zh,en",
                &["My VRChat Avatar has broken PhysBones"],
                (1_000, 1_000),
            )
            .unwrap();
        assert!(
            authoring_snapshot
                .translation_prompt()
                .unwrap()
                .contains("Avatar Dynamics")
        );
    }

    #[test]
    fn checked_in_vrc_alias_can_correct_rare_word_on_first_turn() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select(
                "auto",
                "zh,en",
                &["Yeah, you are a fanboy."],
                (1_000, 1_000),
            )
            .unwrap();
        let corrections = snapshot.recognition_corrections("Yeah, you are a fanboy.");

        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].original_text, "fanboy");
        assert_eq!(corrections[0].corrected_text, "femboy");
        assert_eq!(
            corrections[0].sources[0].corpus_id,
            "virtual-worlds.vrchat.community-language"
        );
        let prompt = snapshot
            .translation_prompt_for("Yeah, you are a fanboy.")
            .unwrap();
        assert!(prompt.contains("男娘,femboy"));
        assert!(!prompt.contains("fanboy"));
    }

    #[test]
    fn checked_in_lgbtq_spelled_letters_activate_identity_terms() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select("auto", "zh,en", &["Do you know L G B T Q?"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt().unwrap();
        let matches = snapshot.activation_matches("Do you know L G B T Q?");

        assert!(prompt.contains("LGBTQ,LGBTQ"));
        assert!(prompt.contains("跨性别,transgender"));
        assert_eq!(matches[0].text, "L G B T Q");
        assert_eq!(
            matches[0].sources[0].corpus_id,
            "identity-and-community.lgbtq.identity-and-language"
        );
    }

    #[test]
    fn checked_in_chinese_meme_seed_activates_small_teasing_cluster() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select("auto", "zh,en", &["今天满脑子咕咕嘎嘎。"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt().unwrap();

        assert!(prompt.contains("咕咕嘎嘎,Gugu Gaga"));
        assert!(prompt.contains("你充Q币吗,do you recharge Q coins"));
        let matches = snapshot.activation_matches("今天满脑子咕咕嘎嘎。");
        assert_eq!(matches[0].text, "咕咕嘎嘎");
        assert_eq!(
            matches[0].sources[0].corpus_id,
            "internet-culture.memes.chinese-casual-teasing"
        );
    }

    #[test]
    fn checked_in_chinese_meme_corpus_ignores_ordinary_teasing() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select("auto", "zh,en", &["你今天说话好搞笑。"], (1_000, 1_000))
            .unwrap();

        assert!(
            snapshot
                .translation_prompt()
                .is_none_or(|prompt| !prompt.contains("你充Q币吗"))
        );
        assert!(snapshot.activation_matches("你今天说话好搞笑。").is_empty());
    }

    #[test]
    fn checked_in_chinese_meme_alias_activates_without_becoming_prompt_term() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let snapshot = context
            .select("auto", "zh,en", &["你冲Q币吗？"], (1_000, 1_000))
            .unwrap();
        let prompt = snapshot.translation_prompt().unwrap();

        assert!(prompt.contains("你充Q币吗,do you recharge Q coins"));
        assert!(!prompt.contains("你冲Q币吗"));
        assert_eq!(
            snapshot.activation_matches("你冲Q币吗？")[0].text,
            "你冲Q币吗"
        );
    }

    #[test]
    fn checked_in_work_corpus_activates_from_title_or_character() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();

        let mut from_title = PromptContextManager::new(config(), catalog.clone()).unwrap();
        let title_snapshot = from_title
            .select("auto", "zh,en", &["Do you know 天降之物?"], (1_000, 1_000))
            .unwrap();
        let title_prompt = title_snapshot.translation_prompt().unwrap();
        assert!(title_prompt.contains("伊卡洛斯,Ikaros"));
        assert!(title_prompt.contains("阿斯特蕾亚,Astraea"));

        let mut from_character = PromptContextManager::new(config(), catalog).unwrap();
        let character_snapshot = from_character
            .select("auto", "zh,en", &["Ikaros is my favorite"], (1_000, 1_000))
            .unwrap();
        let character_prompt = character_snapshot.translation_prompt().unwrap();
        assert!(character_prompt.contains("天降之物,Heaven's Lost Property"));
        assert!(character_prompt.contains("妮姆芙,Nymph"));
    }

    #[test]
    fn checked_in_work_corpus_activates_from_non_route_language_alias() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();

        let mut from_japanese = PromptContextManager::new(config(), catalog.clone()).unwrap();
        let japanese_snapshot = from_japanese
            .select("auto", "zh,en", &["イカロス。"], (1_000, 1_000))
            .unwrap();
        let japanese_prompt = japanese_snapshot.translation_prompt().unwrap();
        assert!(japanese_prompt.contains("伊卡洛斯,Ikaros"));
        assert!(!japanese_prompt.contains("イカロス"));
        assert_eq!(
            japanese_snapshot.activation_matches("イカロス。")[0].text,
            "イカロス"
        );

        let mut from_mixed = PromptContextManager::new(config(), catalog).unwrap();
        let mixed_snapshot = from_mixed
            .select("auto", "zh,en", &["伊卡ロス。"], (1_000, 1_000))
            .unwrap();
        assert!(
            mixed_snapshot
                .translation_prompt()
                .unwrap()
                .contains("伊卡洛斯,Ikaros")
        );
        assert_eq!(
            mixed_snapshot.activation_matches("伊卡ロス。")[0].text,
            "伊卡ロス"
        );
    }

    #[test]
    fn checked_in_active_work_title_remains_recognition_context() {
        let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = CorpusCatalog::load(&PromptContextConfig::default(), &project_root).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let first = context
            .select("auto", "zh,en", &["你知道伊卡洛斯吗？"], (1_000, 1_000))
            .unwrap();
        assert_eq!(
            first.activation_matches("你知道伊卡洛斯吗？")[0].text,
            "伊卡洛斯"
        );
        context.record_transcript("你知道伊卡洛斯吗？");
        context.record_translation("zh", "en", "你知道伊卡洛斯吗？", "Do you know Ikaros?");

        let second = context
            .select("auto", "zh,en", &["嗯，天降之物的。"], (1_000, 1_000))
            .unwrap();
        let matches = second.recognition_context_matches("嗯，天降之物的。");
        assert_eq!(matches[0].text, "天降之物");
        assert!(
            matches[0].sources[0]
                .corpus_id
                .ends_with("sora-no-otoshimono.characters")
        );
    }

    #[test]
    fn activation_context_requires_country_and_topic_evidence() {
        let mut cuisine = corpus(
            "geography-and-culture.japan.cuisine",
            50,
            CorpusActivation::OnEvidence,
            &[("日本", "Japan")],
            &[("大阪烧", "okonomiyaki")],
        );
        cuisine.trigger_aliases = vec![bilingual_term("日本的", "Japanese")];
        cuisine.activation_context = vec![bilingual_term("美食", "food")];
        let catalog = CorpusCatalog::from_definitions(vec![cuisine]).unwrap();

        let mut country_only = PromptContextManager::new(config(), catalog.clone()).unwrap();
        assert!(
            country_only
                .select("auto", "zh,en", &["I visited Japan"], (1_000, 1_000))
                .unwrap()
                .translation_prompt()
                .is_none()
        );

        let mut topic_only = PromptContextManager::new(config(), catalog.clone()).unwrap();
        assert!(
            topic_only
                .select("auto", "zh,en", &["I like food"], (1_000, 1_000))
                .unwrap()
                .translation_prompt()
                .is_none()
        );

        let mut combined = PromptContextManager::new(config(), catalog).unwrap();
        let snapshot = combined
            .select(
                "auto",
                "zh,en",
                &["Tell me about Japanese food"],
                (1_000, 1_000),
            )
            .unwrap();
        assert!(
            snapshot
                .translation_prompt()
                .unwrap()
                .contains("大阪烧,okonomiyaki")
        );
    }

    #[test]
    fn country_and_topic_from_different_turns_do_not_form_a_new_activation() {
        let mut cuisine = corpus(
            "geography-and-culture.japan.cuisine",
            50,
            CorpusActivation::OnEvidence,
            &[("Japan", "Japan")],
            &[("okonomiyaki", "okonomiyaki")],
        );
        cuisine.activation_context = vec![bilingual_term("food", "food")];
        let catalog = CorpusCatalog::from_definitions(vec![cuisine]).unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        context.record_transcript("The food was good yesterday");
        let country_only = context
            .select("auto", "zh,en", &["Japan"], (1_000, 1_000))
            .unwrap();

        assert!(country_only.translation_prompt().is_none());
        assert!(country_only.activation_matches("Japan").is_empty());
    }

    #[test]
    fn activation_is_a_state_transition_and_matching_ignores_diacritics() {
        let catalog = CorpusCatalog::from_definitions(vec![corpus(
            "games.overwatch.heroes",
            70,
            CorpusActivation::OnEvidence,
            &[("L\u{00fa}cio", "L\u{00fa}cio")],
            &[("\u{5362}\u{897f}\u{5965}", "L\u{00fa}cio")],
        )])
        .unwrap();
        let mut context = PromptContextManager::new(config(), catalog).unwrap();

        let first = context
            .select("auto", "zh,en", &["I play Lucio"], (1_000, 1_000))
            .unwrap();
        let matches = first.activation_matches("I play Lucio");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].text, "Lucio");

        let repeated = context
            .select("auto", "zh,en", &["Lucio again"], (1_000, 1_000))
            .unwrap();
        assert!(repeated.activation_matches("Lucio again").is_empty());
        let context_matches = repeated.recognition_context_matches("Lucio again");
        assert_eq!(context_matches.len(), 1);
        assert_eq!(context_matches[0].text, "Lucio");
    }
}
