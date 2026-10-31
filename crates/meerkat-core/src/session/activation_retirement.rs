//! The retirement of superseded instruction activation bodies, and the
//! session-document authority seam that decides it.
//!
//! An instruction activation row that a later activation row of its key
//! follows is superseded: the later row names it in `supersedes` and takes
//! effect from its own boundary. A compaction retires a superseded row's body
//! in place. The row keeps its identity, its timestamp, its place and its
//! activation ordinal; its content becomes the retired header
//! (`[meerkat-retired-instruction-v1]`), which names the key, the revision
//! and the body's length; and it carries the typed marker of the retired
//! body ([`crate::types::RetiredInstructionBody`]). The newest row of every
//! key keeps its body, so the session's effective activation of a key is
//! never retired, and a retired body stops counting against the activation
//! budget.
//!
//! `SessionDocumentMachine` decides which rows. Before every decision this
//! module recovers a fresh authority with the session's activation rows per
//! key: how many the transcript holds (every origin), how many of them carry
//! a retired body, and whether the key's newest row is the session's own.
//! The authority decides, and this module maps each key's retired count onto
//! that key's oldest rows. A key's retired rows always precede its live ones
//! (the row validator holds this), so the mapping is exact. Bodies, digests
//! and offsets stay here.

use std::borrow::Cow;
use std::collections::BTreeMap;

use super::instruction_activation::{
    ActivationRowBody, activation_row_body, retired_activation_row,
};
use super::{InstructionActivationError, InstructionStateError, Session};
use crate::generated::session_document as document;
use crate::types::{
    InstructionActivationIdentity, InstructionKey, InstructionNamespace, Message, SystemMessage,
};

/// The activation-body retirement region's per-session registry key: every
/// decision seeds one fresh authority for its one session.
const SESSION_DOCUMENT_ACTIVATION_KEY: &str = "instruction_activations";

/// The canonical key the session-document authority holds an activation key
/// under: the JSON array `[namespace, key]`, as the operator-instruction
/// region holds a slot.
fn activation_machine_key(namespace: &InstructionNamespace, key: &InstructionKey) -> String {
    serde_json::Value::Array(vec![
        serde_json::Value::String(namespace.as_str().to_owned()),
        serde_json::Value::String(key.as_str().to_owned()),
    ])
    .to_string()
}

/// The activation bodies a compaction retires, as the session-document
/// authority authorized them: the source offset of each row whose body it
/// retires, with the row's exact retired form. Only the validated
/// compaction path applies it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ActivationRetirement {
    rows: BTreeMap<usize, SystemMessage>,
}

impl ActivationRetirement {
    /// The retired form of the row at `source_offset`, when the retirement
    /// retires its body.
    pub(crate) fn retired_row(&self, source_offset: usize) -> Option<&SystemMessage> {
        self.rows.get(&source_offset)
    }

    /// The source offsets of the rows whose bodies the retirement retires,
    /// in order.
    pub(crate) fn source_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.keys().copied()
    }

    /// Whether the retirement retires the row with `identity`.
    fn retires(&self, identity: &InstructionActivationIdentity) -> bool {
        self.rows
            .values()
            .any(|row| row.instruction_activation.as_ref() == Some(identity))
    }

    /// A retirement of exactly `rows`, for the compaction validator's own
    /// rows; production retirements come from the authority.
    #[cfg(test)]
    pub(crate) fn from_rows(rows: BTreeMap<usize, SystemMessage>) -> Self {
        Self { rows }
    }
}

/// `model_messages`, a model projection of this session's rows, with each
/// activation row whose body `retirement` retires in its retired form: what
/// a summarizer reads, so a summary never restates a superseded body. Rows
/// a fork copied read as their inherited headers already and stay as they
/// are. Borrowed when nothing changes.
pub(crate) fn with_retired_activation_bodies<'a>(
    model_messages: &'a [Message],
    retirement: Option<&ActivationRetirement>,
) -> Cow<'a, [Message]> {
    let Some(retirement) = retirement else {
        return Cow::Borrowed(model_messages);
    };
    let retired = |message: &Message| match message {
        Message::System(system)
            if system
                .instruction_activation
                .as_ref()
                .is_some_and(|identity| retirement.retires(identity)) =>
        {
            retired_activation_row(system).map(Message::System)
        }
        _ => None,
    };
    if !model_messages
        .iter()
        .any(|message| retired(message).is_some())
    {
        return Cow::Borrowed(model_messages);
    }
    Cow::Owned(
        model_messages
            .iter()
            .map(|message| retired(message).unwrap_or_else(|| message.clone()))
            .collect(),
    )
}

impl Session {
    /// Before a compaction rewrite: let the session-document authority
    /// decide which superseded instruction activation bodies the rewrite
    /// retires, and render their retired rows. `None` when the transcript
    /// holds no activation row (without consulting the authority), or when
    /// every superseded body is retired already.
    ///
    /// Nothing changes here: the compaction puts the retired rows in its
    /// rebuilt transcript, its validation checks them against the source,
    /// and the commit records the envelope-5 marker with the rewrite
    /// ([`Self::record_activation_body_retirement`]).
    ///
    /// # Errors
    ///
    /// [`InstructionActivationError::MalformedHistory`] when an activation
    /// row does not hold the bytes its identity names, or the authority
    /// refuses the recovered rows (a key's newest row of the session's own
    /// with a retired body, a retired row after a live one).
    pub(crate) fn authorize_activation_body_retirement(
        &self,
    ) -> Result<Option<ActivationRetirement>, InstructionActivationError> {
        // Every activation row, per key, in transcript order.
        let mut keys: BTreeMap<String, Vec<(usize, &SystemMessage, ActivationRowBody<'_>)>> =
            BTreeMap::new();
        for (offset, message) in self.messages().iter().enumerate() {
            let Message::System(system) = message else {
                continue;
            };
            let Some(identity) = system.instruction_activation.as_ref() else {
                continue;
            };
            let body = activation_row_body(identity, system)
                .map_err(InstructionActivationError::MalformedHistory)?;
            keys.entry(activation_machine_key(
                &identity.revision.namespace,
                &identity.revision.key,
            ))
            .or_default()
            .push((offset, system, body));
        }
        if keys.is_empty() {
            return Ok(None);
        }
        let mut rows = BTreeMap::new();
        let mut retired = BTreeMap::new();
        let mut head_origin = BTreeMap::new();
        for (key, key_rows) in &keys {
            rows.insert(key.clone(), key_rows.len() as u64);
            retired.insert(
                key.clone(),
                key_rows
                    .iter()
                    .filter(|(_, _, body)| body.live().is_none())
                    .count() as u64,
            );
            let own = key_rows.last().is_some_and(|(_, system, _)| {
                system
                    .instruction_activation
                    .as_ref()
                    .is_some_and(|identity| identity.origin_session_id == *self.id())
            });
            head_origin.insert(
                key.clone(),
                if own {
                    document::ActivationHeadOrigin::Own
                } else {
                    document::ActivationHeadOrigin::Copied
                },
            );
        }
        let refused = |error: document::SessionDocumentError| {
            InstructionActivationError::MalformedHistory(format!(
                "the session-document authority refused the instruction activation rows: {error}"
            ))
        };
        let mut authority = document::SessionDocumentMachineAuthority::new();
        let machine_key = document::SessionDocumentKey::new(SESSION_DOCUMENT_ACTIVATION_KEY);
        authority
            .recover_session_activation_bodies(machine_key.clone(), rows, retired, head_origin)
            .map_err(refused)?;
        let effects = authority
            .authorize_activation_body_retirement(machine_key.clone())
            .map_err(refused)?;
        if !effects.iter().any(|effect| {
            matches!(
                effect,
                document::SessionDocumentEffect::ActivationBodyRetirementAuthorized
            )
        }) {
            return Err(InstructionActivationError::MalformedHistory(
                "the session-document authority emitted no activation-body retirement verdict"
                    .to_string(),
            ));
        }
        let retired_after = authority
            .session_activation_retired_for(&machine_key)
            .unwrap_or_default();
        let mut retirement = BTreeMap::new();
        for (key, key_rows) in &keys {
            let count =
                usize::try_from(retired_after.get(key).copied().unwrap_or(0)).unwrap_or(usize::MAX);
            for (offset, system, body) in key_rows.iter().take(count) {
                if body.live().is_none() {
                    continue;
                }
                let row = retired_activation_row(system).ok_or_else(|| {
                    InstructionActivationError::MalformedHistory(format!(
                        "the instruction activation row at offset {offset} has no retired form"
                    ))
                })?;
                retirement.insert(*offset, row);
            }
        }
        Ok((!retirement.is_empty()).then_some(ActivationRetirement { rows: retirement }))
    }

    /// Record, in the session whose compaction rewrite retired activation
    /// bodies and in the same prepared session value, that its document
    /// carries a fact only envelope 5 reads: a release before retirement
    /// reads a retired row as a malformed envelope, so it must refuse the
    /// document typed at its envelope instead. The unowned instruction
    /// state is the marker; a session that records an instruction state
    /// already keeps it. Once 5, always 5: an activation row is never
    /// erased, and a retired body never turns live again.
    pub(crate) fn record_activation_body_retirement(
        &mut self,
        retirement: &ActivationRetirement,
    ) -> Result<(), InstructionStateError> {
        if retirement.rows.is_empty() {
            return Ok(());
        }
        self.record_unowned_instruction_state()
    }
}
