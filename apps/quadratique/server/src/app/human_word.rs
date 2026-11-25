//! The human's words that an agent gives when it moves an issue on the human's decision:
//! decision record 0002, rule 4.

use super::Refused;
use crate::domain::{Caller, Confidence};
use crate::store::Tx;

#[derive(Clone)]
pub(super) struct HumanWord {
    words: String,
    confidence: Confidence,
}

impl HumanWord {
    /// The words count only with a confidence, and only from an agent: the human's own calls
    /// do not change.
    pub(super) fn given(
        caller: &Caller,
        words: Option<String>,
        confidence: Option<Confidence>,
    ) -> Option<HumanWord> {
        if caller.is_human() {
            return None;
        }
        Some(HumanWord {
            words: words?,
            confidence: confidence?,
        })
    }

    pub(super) fn confidence(word: Option<&HumanWord>) -> Option<Confidence> {
        word.map(|word| word.confidence)
    }

    /// The reason of the history's change. The words are at most 400 characters, so it stays
    /// within the 500 of a reason.
    pub(super) fn history_reason(&self) -> String {
        format!(
            "The human's words, {} confidence: \"{}\"",
            self.confidence.as_str(),
            self.words
        )
    }

    /// The note of an agent that moved the issue on the human's words.
    pub(super) fn note_moved(
        &self,
        tx: &mut Tx<'_>,
        id: &str,
        caller: &Caller,
        status: &str,
        reason: Option<&str>,
    ) -> Result<(), Refused> {
        let text = format!(
            "{} acted on the human's words, with {} confidence: \"{}\". The issue is in {status}.",
            caller.actor.name,
            self.confidence.as_str(),
            self.words
        );
        add(tx, id, caller, text, reason)
    }

    /// The note of an agent whose confidence was not high: the issue waits for the human.
    pub(super) fn note_left(
        &self,
        tx: &mut Tx<'_>,
        id: &str,
        caller: &Caller,
        reason: Option<&str>,
    ) -> Result<(), Refused> {
        let text = format!(
            "{} left the issue for the human: its confidence that the human's words decide it is {}. The words: \"{}\".",
            caller.actor.name,
            self.confidence.as_str(),
            self.words
        );
        add(tx, id, caller, text, reason)
    }
}

fn add(
    tx: &mut Tx<'_>,
    id: &str,
    caller: &Caller,
    mut text: String,
    reason: Option<&str>,
) -> Result<(), Refused> {
    if let Some(reason) = reason {
        text.push_str(&format!(" Reason: {reason}"));
    }
    let author = serde_json::to_string(&caller.actor).map_err(Refused::defect)?;
    tx.add_note(id, &text, &author)?;
    Ok(())
}
