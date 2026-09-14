//! Trusted-grant record contracts. A decoded declaration/record is not a grant
//! handoff and cannot activate execution on its own.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use meerkat_core::RealmId;
use meerkat_core::execution_scope::{ExecutionGrantId, ExecutionGrantRef, ScopedExecutorBinding};
use meerkat_core::live_execution::activation::{
    LiveActivationDeclaration, LiveActivationId, LiveExecutorSelector,
};
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
mod issuer;
#[cfg(not(target_arch = "wasm32"))]
pub use issuer::{LiveExecutionGrantIssuer, LiveGrantActivationError, LiveGrantActivationRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveExecutionGrantFormat {
    V1,
}

/// The member owner supplies its canonical selector type and resolved binding.
/// This is content until the trusted issuer and generated activation owner
/// validate the actual current binding; equality alone never grants permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResolvedExecutorRecord<Member> {
    pub selector: LiveExecutorSelector<Member>,
    pub binding: ScopedExecutorBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "LiveExecutionGrantWire<Member>",
    bound(deserialize = "Member: Deserialize<'de> + PartialEq")
)]
pub struct LiveExecutionGrantRecord<Member> {
    format: LiveExecutionGrantFormat,
    grant_id: ExecutionGrantId,
    activation_id: LiveActivationId,
    declaration: LiveActivationDeclaration<Member>,
    requesting_realm: RealmId,
    executor: LiveResolvedExecutorRecord<Member>,
    issued_at: DateTime<Utc>,
}

impl<Member: PartialEq> LiveExecutionGrantRecord<Member> {
    pub fn new(
        grant_id: ExecutionGrantId,
        activation_id: LiveActivationId,
        declaration: LiveActivationDeclaration<Member>,
        requesting_realm: RealmId,
        executor: LiveResolvedExecutorRecord<Member>,
        issued_at: DateTime<Utc>,
    ) -> Result<Self, LiveGrantRecordError> {
        if declaration.executor != executor.selector {
            return Err(LiveGrantRecordError::SelectorMismatch);
        }
        if let LiveExecutorSelector::Session { session_id } = &executor.selector
            && session_id != &executor.binding.session_id
        {
            return Err(LiveGrantRecordError::SessionMismatch);
        }
        if !declaration.requesting_realms.contains(&requesting_realm) {
            return Err(LiveGrantRecordError::RequestingRealmDenied);
        }
        if declaration
            .expires_at
            .is_some_and(|expires| expires <= issued_at)
        {
            return Err(LiveGrantRecordError::ExpiredAtIssue);
        }
        Ok(Self {
            format: LiveExecutionGrantFormat::V1,
            grant_id,
            activation_id,
            declaration,
            requesting_realm,
            executor,
            issued_at,
        })
    }

    pub fn grant_ref(&self) -> ExecutionGrantRef {
        ExecutionGrantRef {
            id: self.grant_id,
            issuer_realm: self.declaration.issuer_realm.clone(),
            generation: self.declaration.generation,
        }
    }
}

impl<Member> LiveExecutionGrantRecord<Member> {
    pub fn declaration(&self) -> &LiveActivationDeclaration<Member> {
        &self.declaration
    }
    pub fn executor(&self) -> &LiveResolvedExecutorRecord<Member> {
        &self.executor
    }
    pub fn requesting_realm(&self) -> &RealmId {
        &self.requesting_realm
    }
    pub fn activation_id(&self) -> &LiveActivationId {
        &self.activation_id
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveExecutionGrantWire<Member> {
    format: LiveExecutionGrantFormat,
    grant_id: ExecutionGrantId,
    activation_id: LiveActivationId,
    declaration: LiveActivationDeclaration<Member>,
    requesting_realm: RealmId,
    executor: LiveResolvedExecutorRecord<Member>,
    issued_at: DateTime<Utc>,
}

impl<Member: PartialEq> TryFrom<LiveExecutionGrantWire<Member>>
    for LiveExecutionGrantRecord<Member>
{
    type Error = LiveGrantRecordError;
    fn try_from(value: LiveExecutionGrantWire<Member>) -> Result<Self, Self::Error> {
        match value.format {
            LiveExecutionGrantFormat::V1 => Self::new(
                value.grant_id,
                value.activation_id,
                value.declaration,
                value.requesting_realm,
                value.executor,
                value.issued_at,
            ),
        }
    }
}

/// Sealed receipt of trusted-host activation through the generated owner and
/// its exact durable commit, not an effect permit or a decoded record.
///
/// ```compile_fail
/// use meerkat_runtime::live_grant::LiveExecutionGrant;
/// let forged = serde_json::from_str::<LiveExecutionGrant<()>>("{}");
/// ```
#[derive(Debug, Clone)]
pub struct LiveExecutionGrant<Member> {
    record: Arc<LiveExecutionGrantRecord<Member>>,
    activation_commit: crate::live_ledger::transcript::LiveHeadReference,
}

impl<Member> LiveExecutionGrant<Member> {
    pub fn record(&self) -> &LiveExecutionGrantRecord<Member> {
        &self.record
    }
    pub fn activation_commit(&self) -> &crate::live_ledger::transcript::LiveHeadReference {
        &self.activation_commit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveGrantRecordError {
    #[error("live grant selector differs from the resolved executor selector")]
    SelectorMismatch,
    #[error("live grant exact session differs from its executor binding")]
    SessionMismatch,
    #[error("live grant does not list the requesting realm")]
    RequestingRealmDenied,
    #[error("live grant was expired when issued")]
    ExpiredAtIssue,
}
