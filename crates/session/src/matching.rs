//! One literal matcher for activation, vocabulary ranking, corrections and highlights.
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub(super) fn fold_for_match(value: &str) -> String {
    value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

pub(super) struct FoldedText {
    text: String,
    original_boundaries: Vec<Option<usize>>,
}

impl FoldedText {
    pub(super) fn new(value: &str) -> Self {
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

    pub(super) fn spans(&self, term: &str) -> Vec<(usize, usize)> {
        folded_spans(&self.text, &fold_for_match(term))
            .into_iter()
            .filter_map(|(start, end)| self.original_span(start, end))
            .collect()
    }

    fn original_span(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        let original_start = self.original_boundaries.get(start).copied().flatten()?;
        let original_end = self.original_boundaries.get(end).copied().flatten()?;
        Some((original_start, original_end))
    }
}

pub(super) fn term_spans(text: &str, term: &str) -> Vec<(usize, usize)> {
    FoldedText::new(text).spans(term)
}

/// Both inputs are already folded by the session's evidence selection.
pub(super) fn evidence_contains_term(evidence: &str, term: &str) -> bool {
    !folded_spans(evidence, term).is_empty()
}

pub(super) fn match_key(value: &str) -> String {
    compact_word_separators(&fold_for_match(value))
}

fn word_separator(character: char) -> bool {
    matches!(character, ' ' | '\t' | '-' | '\u{2010}' | '\u{2011}')
}

/// ASR may split or join the same letters. Do not erase sentence punctuation,
/// line breaks or separators adjacent to numbers ("12 34" is not "1234").
fn compact_word_separators(value: &str) -> String {
    let mut compact = String::with_capacity(value.len());
    let mut characters = value.char_indices().peekable();
    while let Some((start, character)) = characters.next() {
        if !word_separator(character) {
            compact.push(character);
            continue;
        }
        while characters.peek().is_some_and(|(_, c)| word_separator(*c)) {
            characters.next();
        }
        if compact.chars().next_back().is_some_and(char::is_alphabetic)
            && characters.peek().is_some_and(|(_, c)| c.is_alphabetic())
        {
            continue;
        }
        let end = characters.peek().map_or(value.len(), |(index, _)| *index);
        compact.push_str(&value[start..end]);
    }
    compact
}

fn folded_spans(text: &str, term: &str) -> Vec<(usize, usize)> {
    let needle = compact_word_separators(term);
    let Some(first) = needle.chars().next() else {
        return Vec::new();
    };
    text.match_indices(first)
        .filter_map(|(start, _)| {
            let mut characters = text[start..].char_indices().peekable();
            let mut previous: Option<char> = None;
            let mut end = start;
            for expected in needle.chars() {
                if previous.is_some_and(char::is_alphabetic) && expected.is_alphabetic() {
                    while characters.peek().is_some_and(|(_, c)| word_separator(*c)) {
                        characters.next();
                    }
                }
                let (offset, actual) = characters.next()?;
                if actual != expected {
                    return None;
                }
                end = start + offset + actual.len_utf8();
                previous = Some(expected);
            }
            term_boundary_matches(text, start, end, &needle).then_some((start, end))
        })
        .collect()
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
