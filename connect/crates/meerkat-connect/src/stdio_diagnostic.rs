//! Bounded observation of the original driver transition, never a cause or receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FenceOrigin {
    OtherNativeFailure,
    OtherNativeTransition,
    SendUnsettled,
    SendOwnerRetired,
    SendOwnerStatePoisoned,
    ApprovalPresenterClosed,
    ApprovalNativeDecisionFailed,
    ForgePresenterClosed,
    PairedHostPresenterClosed,
    ServicePresenterClosed,
    ServiceWaiterClosedBeforeLocate,
    ServiceLocateJoinFailed,
    ServiceLocateFailed,
    ServiceWaiterClosedAfterLocate,
    ServicePrepareFalseWhileLive,
    ServicePrepareError,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct FirstFence {
    pub(super) origin: FenceOrigin,
    pub(super) scope: Option<String>,
    pub(super) send_request_id: Option<String>,
    pub(super) cancellation_requested_before: Option<bool>,
}
