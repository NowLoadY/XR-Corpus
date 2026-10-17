use std::collections::VecDeque;

#[derive(Debug)]
struct TranscriptTurn {
    turn_id: Option<String>,
    text: String,
}

/// Bounded recognition evidence for vocabulary activation. Revisions replace
/// their logical turn; complete sentences never enter the ASR prompt.
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
