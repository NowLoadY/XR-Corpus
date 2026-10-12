use std::{collections::VecDeque, sync::Mutex};

use crate::dialogue::BilingualTurn;

/// Conversation history shared by input lanes, without sharing recognition
/// evidence, terminology activation, or mutable per-request snapshots.
#[derive(Debug)]
pub struct SharedDialogueHistory {
    state: Mutex<History>,
}

#[derive(Debug)]
struct History {
    capacity: usize,
    next_order: u64,
    observed: VecDeque<(String, u64)>,
    turns: Vec<(u64, BilingualTurn)>,
}

impl SharedDialogueHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(History {
                capacity,
                next_order: 0,
                observed: VecDeque::new(),
                turns: Vec::new(),
            }),
        }
    }

    pub(crate) fn observe(&self, turn_id: Option<&str>) {
        let Some(turn_id) = turn_id.filter(|id| !id.is_empty()) else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.capacity == 0
            || state.observed.iter().any(|(id, _)| id == turn_id)
            || state
                .turns
                .iter()
                .any(|(_, turn)| turn.turn_id.as_deref() == Some(turn_id))
        {
            return;
        }
        let order = state.next_order;
        state.next_order += 1;
        state.observed.push_back((turn_id.to_owned(), order));
        while state.observed.len() > state.capacity * 2 {
            state.observed.pop_front();
        }
    }

    pub(crate) fn record(&self, turn: BilingualTurn) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.capacity == 0 {
            return;
        }
        if let Some(id) = turn.turn_id.as_deref() {
            if let Some((_, existing)) = state
                .turns
                .iter_mut()
                .find(|(_, existing)| existing.turn_id.as_deref() == Some(id))
            {
                // A revision keeps the utterance's original position, even
                // when another input lane has spoken in the meantime.
                *existing = turn;
                return;
            }
            let Some(order) = state
                .observed
                .iter()
                .find_map(|(observed, order)| (observed == id).then_some(*order))
            else {
                // A result delayed beyond the bounded observation window
                // must not re-enter history as if it were new speech.
                return;
            };
            state.turns.push((order, turn));
        } else {
            let order = state.next_order;
            state.next_order += 1;
            state.turns.push((order, turn));
        }
        state.turns.sort_by_key(|(order, _)| *order);
        if state.turns.len() > state.capacity {
            state.turns.remove(0);
        }
    }

    pub(crate) fn relevant(&self, languages: &[String]) -> Vec<BilingualTurn> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .turns
            .iter()
            // Input lanes can use different language pairs. A shared
            // language is enough to retain useful conversational evidence.
            .filter(|(_, turn)| {
                languages.contains(&turn.source_language)
                    || languages.contains(&turn.target_language)
            })
            .map(|(_, turn)| turn.clone())
            .collect()
    }
}
