//! Process-local services for identity-owned member materialization.
//!
//! [`crate::identity::DesiredMemberMaterial`] is the sole durable construction
//! authority.  This module deliberately exposes no mutable build draft: a
//! provider may only attach an in-process external-tool dispatcher to the
//! already sealed member material selected by the identity reconciler.

use std::fmt;
use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;

use crate::{AgentIdentity, DesiredLocalCallbackTool, MobId};

/// Exact sealed-intent key for one process-local materialization lookup.
///
/// This is a freshness/correlation key, not an actuation permit.  Supplying or
/// echoing it never authorizes a durable write; the actor still validates the
/// active identity lease and target-local CAS permit immediately before
/// materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityLocalMaterializationKey {
    mob_id: MobId,
    identity: AgentIdentity,
    intent_revision: u64,
    intent_digest: String,
    intent_authority_digest: String,
}

impl IdentityLocalMaterializationKey {
    /// Construct a process-local lookup key from already validated intent
    /// authority.  This constructor does not mint durable authority.
    #[must_use]
    pub fn new(
        mob_id: MobId,
        identity: AgentIdentity,
        intent_revision: u64,
        intent_digest: impl Into<String>,
        intent_authority_digest: impl Into<String>,
    ) -> Self {
        Self {
            mob_id,
            identity,
            intent_revision,
            intent_digest: intent_digest.into(),
            intent_authority_digest: intent_authority_digest.into(),
        }
    }

    #[must_use]
    pub fn mob_id(&self) -> &MobId {
        &self.mob_id
    }

    #[must_use]
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn intent_revision(&self) -> u64 {
        self.intent_revision
    }

    #[must_use]
    pub fn intent_digest(&self) -> &str {
        &self.intent_digest
    }

    #[must_use]
    pub fn intent_authority_digest(&self) -> &str {
        &self.intent_authority_digest
    }
}

impl fmt::Display for IdentityLocalMaterializationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}/{}@{}",
            self.mob_id, self.identity, self.intent_revision
        )
    }
}

/// Typed process-local lookup failure.
///
/// `Missing` and `Unavailable` are retryable observations.  An exact-key
/// mismatch is not: attaching a dispatcher compiled for different sealed
/// material would silently change the realized member and must be surfaced as
/// repair-blocked by the reconciler.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IdentityLocalExternalToolsError {
    #[error("no process-local external-tool services are registered")]
    Missing,
    #[error("process-local external-tool services are registered for stale authority {registered}")]
    AuthorityMismatch {
        registered: IdentityLocalMaterializationKey,
    },
    #[error(
        "process-local external-tool definitions do not match sealed desired material: {detail}"
    )]
    DefinitionMismatch { detail: String },
    #[error("process-local external-tool services are temporarily unavailable: {reason}")]
    Unavailable { reason: String },
}

/// Non-durable, identity-aware provider for in-process external-tool
/// dispatchers.
///
/// The provider receives the exact sealed intent key and returns only a
/// dispatcher overlay.  It cannot mutate the model, prompt, declarative tool
/// configuration, session target, lineage, initial input, or any other durable
/// desired material. The actor calls this provider only when the sealed
/// material requires callback tools; `Ok(None)` then means the required local
/// service is missing.
pub trait IdentityLocalExternalToolsProvider: Send + Sync {
    fn external_tools_for(
        &self,
        key: &IdentityLocalMaterializationKey,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, IdentityLocalExternalToolsError>;
}

impl<F> IdentityLocalExternalToolsProvider for F
where
    F: Fn(
            &IdentityLocalMaterializationKey,
        ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, IdentityLocalExternalToolsError>
        + Send
        + Sync,
{
    fn external_tools_for(
        &self,
        key: &IdentityLocalMaterializationKey,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, IdentityLocalExternalToolsError> {
        self(key)
    }
}

pub(super) fn resolve_identity_local_external_tools(
    provider: Option<&dyn IdentityLocalExternalToolsProvider>,
    key: &IdentityLocalMaterializationKey,
    required: &[DesiredLocalCallbackTool],
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, IdentityLocalExternalToolsError> {
    if required.is_empty() {
        return Ok(None);
    }
    let dispatcher = provider
        .ok_or(IdentityLocalExternalToolsError::Missing)?
        .external_tools_for(key)?
        .ok_or(IdentityLocalExternalToolsError::Missing)?;

    let dispatcher_tools = dispatcher.tools();
    if dispatcher_tools
        .iter()
        .any(|tool| tool.provenance.is_some())
    {
        return Err(IdentityLocalExternalToolsError::DefinitionMismatch {
            detail: "identity-local callback dispatchers must expose definitions without independently minted provenance".to_string(),
        });
    }
    let mut observed = dispatcher_tools
        .iter()
        .map(|tool| DesiredLocalCallbackTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        })
        .collect::<Vec<_>>();
    observed.sort_by(|left, right| left.name.cmp(&right.name));
    if observed != required {
        let expected_names = required
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let observed_names = observed
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(IdentityLocalExternalToolsError::DefinitionMismatch {
            detail: format!(
                "expected [{expected_names}], observed [{observed_names}] (name, description, and input schema must match exactly)"
            ),
        });
    }
    Ok(Some(dispatcher))
}
