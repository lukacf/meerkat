//! Pruning ordinary System rows: one audited same-session rewrite that
//! removes ordinary System rows a host names, and, when asked, retires every
//! superseded instruction activation body in the same commit.
//!
//! A host names each row as a history read showed it: its index and the
//! digest of its exact content ([`SystemRowRef`]). Core never chooses the
//! rows: it cannot tell a stale prompt from an instruction a host appended
//! on purpose, since both are ordinary System rows. A row is removable when
//! it is a System row without a prompt version, an instruction activation or
//! an operator-instruction entry (each of those has its own lifecycle), and
//! no operator-instruction entry row follows it in its group of System rows.
//! Any other named row refuses the whole prune, typed
//! ([`SystemRowPruneRefusal`]), and nothing is written.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    RewriteCommitGuard, Session, TranscriptEditError, TranscriptRewriteCommit,
    TranscriptRewriteReason, TranscriptRewriteSelection,
};
use crate::types::{InstructionContentDigest, Message};

/// The reason kind of a system-row prune's rewrite commit; the request's
/// note is the reason's note.
pub const SYSTEM_ROW_PRUNE_REASON_KIND: &str = "system-row-prune";

/// One ordinary System row a prune removes, named as a history read showed
/// it: its index in the transcript and the digest of its exact content, so a
/// stale reference (the transcript changed since the read) is refused rather
/// than removing another row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SystemRowRef {
    /// The row's index in the transcript, as the history read showed it.
    pub index: usize,
    /// `sha256:<64 lowercase hex>` over the row's exact UTF-8 content.
    pub content_sha256: InstructionContentDigest,
}

impl SystemRowRef {
    /// The reference to the row at `index` whose content is `content`.
    pub fn for_row(index: usize, content: &str) -> Self {
        Self {
            index,
            content_sha256: InstructionContentDigest::for_body(content),
        }
    }
}

/// Why a system-row prune refused a named row. A refusal refuses the whole
/// prune: nothing is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SystemRowPruneRefusal {
    /// The index names no row of the transcript.
    OutOfBounds,
    /// The request names the index more than once.
    DuplicateIndex,
    /// The row is not a System row.
    NotASystemRow,
    /// The row is a keyed system-prompt version. A later version of its key
    /// supersedes it, and compaction drops a superseded version.
    PromptVersionRow,
    /// The row is an instruction activation row. Activation rows are
    /// immutable boundaries; a compaction, or a prune's retire option,
    /// retires a superseded body in place.
    InstructionActivationRow,
    /// The row is an operator-instruction entry row, which only a run start
    /// or a compaction fold writes or moves.
    OperatorInstructionEntryRow,
    /// The row's content does not match the named digest: the transcript
    /// changed since the history read.
    DigestMismatch,
    /// An operator-instruction entry row follows the row in its group of
    /// System rows, and removing the row would move the entry.
    BeforeOperatorInstructionEntry,
}

impl SystemRowPruneRefusal {
    /// The stable wire token of the cause.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutOfBounds => "out_of_bounds",
            Self::DuplicateIndex => "duplicate_index",
            Self::NotASystemRow => "not_a_system_row",
            Self::PromptVersionRow => "prompt_version_row",
            Self::InstructionActivationRow => "instruction_activation_row",
            Self::OperatorInstructionEntryRow => "operator_instruction_entry_row",
            Self::DigestMismatch => "digest_mismatch",
            Self::BeforeOperatorInstructionEntry => "before_operator_instruction_entry",
        }
    }
}

impl std::fmt::Display for SystemRowPruneRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A committed system-row prune.
#[derive(Debug, Clone)]
pub struct SystemRowPruneCommit {
    /// The one rewrite commit, with the reason kind
    /// [`SYSTEM_ROW_PRUNE_REASON_KIND`].
    pub commit: TranscriptRewriteCommit,
    /// How many ordinary System rows it removed.
    pub removed_rows: usize,
    /// How many superseded instruction activation bodies it retired.
    pub retired_bodies: usize,
}

/// The indices of the rows `remove` names, each checked removable, in index
/// order. The first row that is not refuses the prune.
fn removable_rows(
    messages: &[Message],
    remove: &[SystemRowRef],
) -> Result<BTreeSet<usize>, TranscriptEditError> {
    let mut removed = BTreeSet::new();
    for row in remove {
        let refused = |cause| TranscriptEditError::SystemRowPruneRefused {
            index: row.index,
            cause,
        };
        if !removed.insert(row.index) {
            return Err(refused(SystemRowPruneRefusal::DuplicateIndex));
        }
        let Some(message) = messages.get(row.index) else {
            return Err(refused(SystemRowPruneRefusal::OutOfBounds));
        };
        let Message::System(system) = message else {
            return Err(refused(SystemRowPruneRefusal::NotASystemRow));
        };
        if system.prompt_version.is_some() {
            return Err(refused(SystemRowPruneRefusal::PromptVersionRow));
        }
        if system.instruction_activation.is_some() {
            return Err(refused(SystemRowPruneRefusal::InstructionActivationRow));
        }
        if system.instruction_entry.is_some() {
            return Err(refused(SystemRowPruneRefusal::OperatorInstructionEntryRow));
        }
        if InstructionContentDigest::for_body(&system.content) != row.content_sha256 {
            return Err(refused(SystemRowPruneRefusal::DigestMismatch));
        }
        // An entry row stands at its place among the System rows of its
        // group, so removing a System row before it in the group would move
        // it. Only a run start or a compaction fold moves an entry row.
        let entry_follows = messages[row.index + 1..]
            .iter()
            .take_while(|message| matches!(message, Message::System(_)))
            .any(|message| {
                matches!(message, Message::System(system) if system.instruction_entry.is_some())
            });
        if entry_follows {
            return Err(refused(
                SystemRowPruneRefusal::BeforeOperatorInstructionEntry,
            ));
        }
    }
    Ok(removed)
}

impl Session {
    /// Remove the named ordinary System rows in one audited rewrite and,
    /// with `retire_superseded_activation_bodies`, retire every superseded
    /// instruction activation body in the same commit, as a compaction does
    /// (`SessionDocumentMachine` decides which; a key's newest body is never
    /// retired). `Ok(None)` when the request names no row and there is no
    /// body to retire: nothing is written.
    ///
    /// Each [`SystemRowRef`] names a row by its 0-based index in this
    /// transcript (the index space of a history read) and the
    /// [`InstructionContentDigest`] of its content. The rows after a removed
    /// one move up; no row is renumbered otherwise, and the last ordinary
    /// System row is removable too: the host chooses. The commit's reason
    /// kind is [`SYSTEM_ROW_PRUNE_REASON_KIND`], with the request's note. An
    /// instruction activation row keeps its place among the conversation
    /// rows; a prune that moves one to another offset, or retires a body,
    /// writes the document at envelope 5.
    ///
    /// # Errors
    ///
    /// [`TranscriptEditError::SystemRowPruneRefused`] names the first row
    /// the prune refuses, and [`TranscriptEditError::RevisionConflict`]
    /// answers a stale `expected_parent_revision`, checked first; nothing is
    /// written.
    pub fn prune_system_rows(
        &mut self,
        request: crate::service::SystemRowPruneRequest,
    ) -> Result<Option<SystemRowPruneCommit>, TranscriptEditError> {
        let crate::service::SystemRowPruneRequest {
            remove,
            retire_superseded_activation_bodies,
            expected_parent_revision,
            note,
            actor,
        } = request;
        if let Some(expected) = expected_parent_revision.as_ref() {
            let actual = self
                .transcript_revision()
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            if *expected != actual {
                return Err(TranscriptEditError::RevisionConflict {
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        let removed = removable_rows(self.messages(), &remove)?;
        let retirement = if retire_superseded_activation_bodies {
            self.authorize_activation_body_retirement()
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?
        } else {
            None
        };

        // The one span that holds every removed row and every retired one.
        let mut changed = removed.clone();
        if let Some(retirement) = retirement.as_ref() {
            changed.extend(retirement.source_offsets());
        }
        let (Some(&start), Some(&last)) = (changed.first(), changed.last()) else {
            return Ok(None);
        };
        let end = last + 1;
        let replacement = self.messages()[start..end]
            .iter()
            .enumerate()
            .filter_map(|(position, message)| {
                let offset = start + position;
                if removed.contains(&offset) {
                    return None;
                }
                let retired = retirement
                    .as_ref()
                    .and_then(|retirement| retirement.retired_row(offset));
                Some(match retired {
                    Some(retired) => Message::System(retired.clone()),
                    None => message.clone(),
                })
            })
            .collect::<Vec<_>>();
        let reason = TranscriptRewriteReason {
            kind: SYSTEM_ROW_PRUNE_REASON_KIND.to_string(),
            note,
        };
        let guard = match retirement.as_ref() {
            Some(retirement) => RewriteCommitGuard::Retiring(retirement),
            None => RewriteCommitGuard::Generic,
        };
        let commit = self.commit_transcript_rewrite_bound(
            TranscriptRewriteSelection::MessageRange { start, end }.into_current_edit_semantic(),
            replacement,
            reason,
            actor,
            expected_parent_revision,
            guard,
        )?;
        let retired_bodies = match retirement.as_ref() {
            Some(retirement) => {
                // The document now holds a retired body, a fact only
                // envelope 5 reads. The commit read the stored state before
                // it was installed, so this records the marker or keeps the
                // state already recorded.
                self.record_activation_body_retirement(retirement)
                    .map_err(|error| {
                        TranscriptEditError::HistoryStateMalformed(error.to_string())
                    })?;
                retirement.retired_bodies()
            }
            None => 0,
        };
        Ok(Some(SystemRowPruneCommit {
            commit,
            removed_rows: removed.len(),
            retired_bodies,
        }))
    }
}
