//! Trusted private host control. None of these commands admits request use.
//!
//! A Toolkit process composition binds this closed contract only to inherited
//! private endpoints. Ordinary RPC, SDK/model tool and remote callers cannot
//! select artifacts, open a component or construct an activity from this API.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use toolkit_package::Id;
use uuid::Uuid;

/// Exact selected bytes, not a permission or a mutable latest pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedHostArtifact {
    pub path: PathBuf,
    pub sha256: String,
}

/// Trusted bootstrap routes. The owner validates the actual current activation,
/// its policy manifest and this manifest-listed compiled local policy together.
/// No field here is a grant; the compiled artifact contains all semantic input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostBootstrapSelection {
    pub deployment_root: PathBuf,
    pub activation: SelectedHostArtifact,
    pub policy_manifest: SelectedHostArtifact,
    pub local_policy: SelectedHostArtifact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCloseReason {
    Shutdown,
    OperatorDisable,
    Quarantine,
    ActivationReplaced,
    HostLost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostCloseScope {
    Component { component: Id },
    Activation,
}

/// A correlation for this one concrete owner incarnation. Possession grants no
/// right to open, use or release anything. Stale incarnations always refuse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCloseHandle {
    pub incarnation: Uuid,
    pub close_id: Uuid,
    pub scope: HostCloseScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCloseState {
    Closing,
    Settled,
    Uncertain,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCloseFailure {
    PhysicalSettlementUncertain,
    PolicyUnavailable,
    OwnerFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCloseObservation {
    pub handle: HostCloseHandle,
    pub state: HostCloseState,
    pub failure: Option<HostCloseFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostComponentState {
    Staged,
    Open,
    Closing,
    Closed,
    Quarantined,
    Uncertain,
    Failed,
}

/// UI/projection only. Native current use checks the owner, never this value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostComponentObservation {
    pub incarnation: Uuid,
    pub component: Id,
    pub revision: u64,
    pub state: HostComponentState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostLifetimeError {
    #[error("host lifetime selection is invalid")]
    InvalidSelection,
    #[error("host lifetime owner is unavailable")]
    Unavailable,
    #[error("host lifetime operation is denied")]
    Denied,
    #[error("host lifetime observation is from another owner")]
    WrongOwner,
    #[error("host lifetime close is not settled")]
    NotSettled,
}

/// Private transport dispatch binds the actual concrete Toolkit owner to this
/// closed protocol. There is intentionally no enter/use/allow/execute command.
#[async_trait::async_trait]
pub trait PrivateHostLifetimeControl: Send + Sync {
    /// Synchronous admission observation of this actual owner's selected,
    /// open activation. It grants no operation permission and is not settlement.
    fn validate_current_selection(&self) -> Result<(), HostLifetimeError>;
    fn open_component(&self, component: &Id)
    -> Result<HostComponentObservation, HostLifetimeError>;
    fn begin_close_component(
        &self,
        component: &Id,
        reason: HostCloseReason,
    ) -> Result<HostCloseObservation, HostLifetimeError>;
    fn begin_close_activation(
        &self,
        reason: HostCloseReason,
    ) -> Result<HostCloseObservation, HostLifetimeError>;
    fn observe_close(
        &self,
        handle: &HostCloseHandle,
    ) -> Result<HostCloseObservation, HostLifetimeError>;
    async fn join_close(
        &self,
        handle: &HostCloseHandle,
    ) -> Result<HostCloseObservation, HostLifetimeError>;
    /// Synchronous irreversible admission closure on lost private endpoints.
    /// The returned observation can remain Closing or Uncertain, never inferred
    /// Settled merely because the host can no longer send another command.
    fn host_lost(&self) -> Result<HostCloseObservation, HostLifetimeError>;
}
