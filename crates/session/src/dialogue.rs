use std::collections::VecDeque;

#[derive(Debug)]
struct TranscriptTurn {
    turn_id: Option<String>,
    text: String,
}

/// Recognition evidence follows the same logical-turn replacement rule as
/// bilingual history, but remains source-only and never enters the ASR prompt.
#[derive(Debug)]
pub(super) struct TranscriptHistory {
    turns: VecDeque<TranscriptTurn>,
    capacity: usize,
}

impl TranscriptHistory {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            turns: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(super) fn clear(&mut self) {
        self.turns.clear();
    }

    pub(super) fn record(&mut self, turn_id: Option<String>, text: String) {
        if self.capacity == 0 {
            return;
        }
        if let Some(turn_id) = turn_id.as_deref()
            && let Some(index) = self
                .turns
                .iter()
                .position(|existing| existing.turn_id.as_deref() == Some(turn_id))
        {
            self.turns.remove(index);
            self.turns.push_back(TranscriptTurn {
                turn_id: Some(turn_id.to_owned()),
                text,
            });
            return;
        }
        if self
            .turns
            .back()
            .is_some_and(|existing| existing.text == text)
        {
            return;
        }
        if self.turns.len() == self.capacity {
            self.turns.pop_front();
        }
        self.turns.push_back(TranscriptTurn { turn_id, text });
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.turns.iter().map(|turn| turn.text.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BilingualTurn {
    pub(super) turn_id: Option<String>,
    pub(super) speaker_id: String,
    pub(super) source_language: String,
    pub(super) target_language: String,
    pub(super) source_text: String,
    pub(super) translated_text: String,
}

impl BilingualTurn {
    fn same_content(&self, other: &Self) -> bool {
        self.speaker_id == other.speaker_id
            && self.source_language == other.source_language
            && self.target_language == other.target_language
            && self.source_text == other.source_text
            && self.translated_text == other.translated_text
    }
}

/// Bounded history of completed logical speech turns.
///
/// A normal Speak utterance is inserted once after all of its translation
/// segments finish. Continuous recognition reuses a stable turn ID, so each
/// sliding-window result replaces that turn instead of consuming another
/// history entry with overlapping text.
#[derive(Debug)]
pub(super) struct DialogueHistory {
    turns: VecDeque<BilingualTurn>,
    capacity: usize,
}

impl DialogueHistory {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            turns: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(super) fn clear(&mut self) {
        self.turns.clear();
    }

    pub(super) fn record(&mut self, turn: BilingualTurn) {
        if self.capacity == 0 {
            return;
        }

        if let Some(turn_id) = turn.turn_id.as_deref()
            && let Some(index) = self
                .turns
                .iter()
                .position(|existing| existing.turn_id.as_deref() == Some(turn_id))
        {
            self.turns.remove(index);
            self.turns.push_back(turn);
            return;
        }

        if self
            .turns
            .back()
            .is_some_and(|existing| existing.same_content(&turn))
        {
            return;
        }
        if self.turns.len() == self.capacity {
            self.turns.pop_front();
        }
        self.turns.push_back(turn);
    }

    pub(super) fn relevant(&self, languages: &[String]) -> Vec<&BilingualTurn> {
        self.turns
            .iter()
            .filter(|turn| {
                languages.contains(&turn.source_language)
                    && languages.contains(&turn.target_language)
            })
            .collect()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &BilingualTurn> {
        self.turns.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(id: &str, source: &str) -> BilingualTurn {
        BilingualTurn {
            turn_id: (!id.is_empty()).then(|| id.to_owned()),
            speaker_id: "speaker-01".into(),
            source_language: "en".into(),
            target_language: "zh".into(),
            source_text: source.into(),
            translated_text: format!("translated {source}"),
        }
    }

    #[test]
    fn a_streaming_revision_replaces_its_logical_turn() {
        let mut history = DialogueHistory::new(2);
        history.record(turn("speech-1", "first window"));
        history.record(turn("speech-1", "revised window"));
        history.record(turn("speech-2", "next turn"));

        let relevant = history.relevant(&["en".into(), "zh".into()]);
        assert_eq!(relevant.len(), 2);
        assert_eq!(relevant[0].source_text, "revised window");
        assert_eq!(relevant[1].source_text, "next turn");
    }

    #[test]
    fn independent_speak_turns_remain_bounded_and_ordered() {
        let mut history = DialogueHistory::new(2);
        history.record(turn("speak-1", "one"));
        history.record(turn("speak-2", "two"));
        history.record(turn("speak-3", "three"));

        let relevant = history.relevant(&["en".into(), "zh".into()]);
        assert_eq!(
            relevant
                .iter()
                .map(|turn| turn.source_text.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
    }

    #[test]
    fn transcript_revisions_do_not_fill_recognition_evidence() {
        let mut history = TranscriptHistory::new(2);
        history.record(Some("speech-1".into()), "first window".into());
        history.record(Some("speech-1".into()), "revised window".into());
        history.record(Some("speech-2".into()), "next turn".into());
        assert_eq!(
            history.iter().collect::<Vec<_>>(),
            ["revised window", "next turn"]
        );
    }
}
