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

use serde::{Deserialize, Serialize};

use super::{Session, TranscriptEditError, TranscriptRewriteCommit};
use crate::types::InstructionContentDigest;

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

impl Session {
    /// Remove the named ordinary System rows in one audited rewrite and,
    /// with `retire_superseded_activation_bodies`, retire every superseded
    /// instruction activation body in the same commit. `Ok(None)` when the
    /// request names no row and there is no body to retire: nothing is
    /// written.
    ///
    /// # Errors
    ///
    /// [`TranscriptEditError::SystemRowPruneRefused`] names the first row
    /// the prune refuses, and
    /// [`TranscriptEditError::RevisionConflict`] answers a stale
    /// `expected_parent_revision`; nothing is written.
    pub fn prune_system_rows(
        &mut self,
        request: crate::service::SystemRowPruneRequest,
    ) -> Result<Option<SystemRowPruneCommit>, TranscriptEditError> {
        let _ = request;
        Err(TranscriptEditError::InvalidTranscriptShape(
            "system row prune is not implemented".to_string(),
        ))
    }
}
