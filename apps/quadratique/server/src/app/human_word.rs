//! The human's words that an agent gives when it moves an issue on the human's decision:
//! decision record 0002, rule 4.

use super::Refused;
use crate::domain::{Caller, Confidence};
use crate::store::Tx;

/// The most characters the history keeps of a reason.
const REASON_MAX: usize = 500;

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

    /// The reason of the history's change, within the 500 characters of a reason. Quoting can
    /// make the words longer than that: then the reason quotes as many of their first
    /// characters as fit, and says that the note has them whole.
    fn history_reason(&self) -> String {
        let head = format!(
            "The human's words, {} confidence: ",
            self.confidence.as_str()
        );
        let whole = format!("{head}{}", quoted(&self.words));
        if whole.chars().count() <= REASON_MAX {
            return whole;
        }
        let cut = " (cut: the note quotes them whole)";
        let room = REASON_MAX - head.chars().count() - cut.chars().count();
        let mut kept: String = self.words.clone();
        while quoted(&kept).chars().count() > room {
            kept.pop();
        }
        format!("{head}{}{cut}", quoted(&kept))
    }

    /// Records an agent's move on the human's words: the history's reason and a note.
    pub(super) fn record_moved(
        &self,
        tx: &mut Tx<'_>,
        id: &str,
        caller: &Caller,
        status: &str,
        reason: Option<&str>,
    ) -> Result<(), Refused> {
        let text = format!(
            "{} acted on the human's words, with {} confidence: {}. The issue is in {status}.",
            caller.actor.name,
            self.confidence.as_str(),
            quoted(&self.words)
        );
        self.record(tx, id, caller, text, reason)
    }

    /// Records an agent's call whose confidence was not high: the issue waits for the human.
    pub(super) fn record_left(
        &self,
        tx: &mut Tx<'_>,
        id: &str,
        caller: &Caller,
        reason: Option<&str>,
    ) -> Result<(), Refused> {
        let text = format!(
            "{} left the issue for the human: its confidence that the human's words decide it is {}. The words: {}.",
            caller.actor.name,
            self.confidence.as_str(),
            quoted(&self.words)
        );
        self.record(tx, id, caller, text, reason)
    }

    /// The call's own reason goes into the note, since the history's reason gives the words; a
    /// release that frees another agent's claim gives it in its own note instead.
    fn record(
        &self,
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
        tx.set_reason(self.history_reason());
        Ok(())
    }
}

/// The words as a JSON string: no quote, backslash, newline or control character inside them
/// can end the quotation.
fn quoted(words: &str) -> String {
    serde_json::Value::from(words).to_string()
}
