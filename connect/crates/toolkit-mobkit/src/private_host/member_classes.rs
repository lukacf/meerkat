//! Protected application commands over the existing inherited pipe. Callback
//! dispatch, roster ownership and native held settlement remain with their
//! existing owners; this endpoint stores only outstanding settlement receivers.
use super::*;
use crate::installed_host::member_mcp_consumer::{
    AcceptedMemberMcpRequirement, MemberMcpAdmissionSelection, MemberMcpApplication,
    MemberMcpRequirementContract, RetainedMemberMcpBindings,
};
use crate::{
    host_gateway::{
        MemberClassAddError, MemberClassAddOutcome, MemberClassAddStart, MemberClassSettlement,
        ToolkitGatewayHost,
    },
    installed_host::preparation::PreparedControllerIntent,
};
use meerkat_core::MobMemberBinding;
use meerkat_mobkit::identity_first::{DurableAgentSpec, RestoreFlowResult};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledMemberClassInstance, CompiledNativeSourceRoute,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock, Weak},
    time::Duration,
};

/// Exact prior policy read supplied by the application, never fetched or
/// retried on its behalf. It grants neither native identity nor authority.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberClassCheckpointWire {
    pub admission: Id,
    pub revision: u64,
}
impl From<MemberClassCheckpointWire>
    for crate::installed_host::dynamic_members::MemberClassCheckpoint
{
    fn from(value: MemberClassCheckpointWire) -> Self {
        Self {
            admission: value.admission,
            revision: value.revision,
        }
    }
}
impl From<crate::installed_host::dynamic_members::MemberClassCheckpoint>
    for MemberClassCheckpointWire
{
    fn from(value: crate::installed_host::dynamic_members::MemberClassCheckpoint) -> Self {
        Self {
            admission: value.admission,
            revision: value.revision,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "public wire type; boxing a variant would change its public API"
)]
pub enum MemberClassCommand {
    Add {
        id: Id,
        instance: CompiledMemberClassInstance,
        spec: DurableAgentSpec,
        route: CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        hold_timeout_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_predecessor: Option<MemberClassCheckpointWire>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mcp: Option<crate::installed_host::member_mcp_consumer::MemberMcpAddIntent>,
    },
    Observe {
        id: Id,
        member: MobMemberBinding,
    },
    Retire {
        id: Id,
        member: MobMemberBinding,
    },
    Join {
        settlement_id: u64,
    },
    McpResult {
        exchange_id: u64,
        selection: MemberMcpAdmissionSelection,
        retained: Option<RetainedMemberMcpBindings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requirement: Option<AcceptedMemberMcpRequirement>,
    },
    ListingResult {
        operation_id: u64,
        admission_id: Id,
        identity: String,
        class: Id,
        accepted: bool,
    },
}

/// One interim observation on the existing pipe, not another native task owner.
#[derive(Clone)]
pub(super) struct ListingChannel {
    pub replies: mpsc::Sender<Response>,
    pub closed: PrivateHostClosed,
    pub failed: watch::Sender<bool>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ListingSelection {
    pub admission_id: Id,
    pub identity: String,
    pub class: Id,
}
struct PendingListing {
    selection: ListingSelection,
    deadline: tokio::time::Instant,
    sender: oneshot::Sender<Result<(), HostLifetimeError>>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "public wire type; boxing a variant would change its public API"
)]
pub enum MemberClassResponse {
    /// No host operation accepted ownership. No admission was changed by this
    /// call; the original caller retains Observe/Retire recovery responsibility.
    ReservationNotAccepted {
        error: WireError,
    },
    /// Host-owned preflight refused before native entry. Cleanup is the joined
    /// canonical retirement result, not an assertion of native rollback.
    PreflightRefused {
        error: WireError,
        cleanup: MemberClassControlOutcome,
    },
    Unsettled {
        settlement_id: u64,
    },
    Settled {
        native: MemberClassNativeOutcome,
        authorization: MemberClassControlOutcome,
        cleanup: MemberClassControlOutcome,
    },
    NativeRefused {
        message: String,
        data: Option<serde_json::Value>,
    },
    Observed {
        record: crate::installed_host::dynamic_members::MemberClassAdmission,
        checkpoint: MemberClassCheckpointWire,
        authorization: MemberClassControlOutcome,
    },
    Retired,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberClassNativeOutcome {
    Result {
        result: serde_json::Value,
    },
    Refused {
        message: String,
        data: Option<serde_json::Value>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberClassControlOutcome {
    Succeeded,
    Refused { error: WireError },
}
impl From<Result<(), HostLifetimeError>> for MemberClassControlOutcome {
    fn from(value: Result<(), HostLifetimeError>) -> Self {
        match value {
            Ok(()) => Self::Succeeded,
            Err(error) => Self::Refused {
                error: error.into(),
            },
        }
    }
}

struct Endpoint {
    owner: Arc<dyn PrivateHostLifetimeControl>,
    logins: ConsoleLogins,
    host: OnceLock<Weak<ToolkitGatewayHost>>,
    // These are actual native observations, not a member registry. Removing
    // one transfers its sole observation to an existing pipe-owned join task.
    pending: Mutex<BTreeMap<u64, PendingSettlement>>,
    listings: Mutex<BTreeMap<u64, PendingListing>>,
    capacity: OnceLock<Arc<tokio::sync::Semaphore>>,
    transport: OnceLock<ListingChannel>,
    mcp_next: AtomicU64,
    mcp: Mutex<BTreeMap<u64, PendingMcp>>,
}
struct PendingMcp {
    selection: MemberMcpAdmissionSelection,
    expected: Option<RetainedMemberMcpBindings>,
    requirement_source: Option<crate::private_originals::PrivateSourceEvidence>,
    sender: oneshot::Sender<Result<McpReply, HostLifetimeError>>,
}
struct McpReply {
    retained: Option<RetainedMemberMcpBindings>,
    requirement: Option<AcceptedMemberMcpRequirement>,
}
// Cancellation only drops transport correlation; no application transaction
// or native grant is inferred to have rolled back.
struct McpWaiter {
    owner: Arc<Endpoint>,
    id: u64,
}
impl Drop for McpWaiter {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.owner.mcp.lock() {
            pending.remove(&self.id);
        }
    }
}
struct PendingSettlement {
    settlement: MemberClassSettlement,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Clone)]
pub struct MemberClassEndpoint(Arc<Endpoint>);

impl MemberClassEndpoint {
    pub(super) fn new(owner: Arc<dyn PrivateHostLifetimeControl>, logins: ConsoleLogins) -> Self {
        Self(Arc::new(Endpoint {
            owner,
            logins,
            host: OnceLock::new(),
            pending: Mutex::new(BTreeMap::new()),
            listings: Mutex::new(BTreeMap::new()),
            capacity: OnceLock::new(),
            transport: OnceLock::new(),
            mcp_next: AtomicU64::new(1),
            mcp: Mutex::new(BTreeMap::new()),
        }))
    }
    pub fn bind(&self, host: &Arc<ToolkitGatewayHost>) -> Result<(), HostLifetimeError> {
        host.member_class_policy()?;
        if !host.has_private_owner(&self.0.owner) {
            return Err(HostLifetimeError::WrongOwner);
        }
        host.bind_member_mcp_application(Arc::new(self.clone()))?;
        self.0
            .host
            .set(Arc::downgrade(host))
            .map_err(|_| HostLifetimeError::WrongOwner)
    }

    async fn settled(
        settlement: MemberClassSettlement,
    ) -> Result<MemberClassResponse, HostLifetimeError> {
        settlement.settled().await.map(settled_result)
    }

    pub(super) async fn execute(
        &self,
        request_id: u64,
        command: MemberClassCommand,
        listing_channel: ListingChannel,
    ) -> Result<MemberClassResponse, HostLifetimeError> {
        if let MemberClassCommand::Join { settlement_id } = command {
            let settlement = self
                .0
                .pending
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?
                .remove(&settlement_id)
                .ok_or(HostLifetimeError::Denied)?;
            return Self::settled(settlement.settlement).await;
        }
        let host = self
            .0
            .host
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        let policy = host.member_class_policy()?;
        match command {
            MemberClassCommand::Add {
                id,
                instance,
                spec,
                route,
                controller,
                hold_timeout_ms,
                expected_predecessor,
                mcp,
            } => {
                if !(1..=900_000).contains(&hold_timeout_ms) {
                    return Err(HostLifetimeError::InvalidSelection);
                }
                // Reserve observation capacity before native entry. Concurrent
                // Add calls are bounded by the existing private command limit;
                // the semaphore holds their possible settlement slots as well.
                let permit = self
                    .capacity()
                    .try_acquire_owned()
                    .map_err(|_| HostLifetimeError::Unavailable)?;
                let context = host.admit_member_class_context()?;
                let alias = meerkat_mobkit::member_comms_id::roster_member_id_for_identity(
                    spec.identity.as_str(),
                );
                if spec.initial_message.is_some() || spec.placement.is_some()
                    || spec.profile.as_str() != instance.member.member.role
                    || alias.as_str() != instance.member.member.member
                    || context.owner().handle().definition().id.as_str() != instance.member.member.mob_id
                    || route.application_parent.is_some() || route.controller_parent.is_some()
                    || !matches!(&route.origin, meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { member, .. }
                        if member == &instance.member.member)
                    || controller.target != (crate::installed_host::preparation::PreparedControllerTarget::OriginalRoute { route: route.id.clone() })
                { return Err(HostLifetimeError::WrongOwner); }
                let mcp = mcp
                    .map(|request| {
                        let login = self
                            .0
                            .logins
                            .parent_enrollment(request.console_enrollment_id)?;
                        Ok::<_, HostLifetimeError>((request, login))
                    })
                    .transpose()?;
                let listing_selection = ListingSelection {
                    admission_id: id.clone(),
                    identity: spec.identity.to_string(),
                    class: instance.class.clone(),
                };
                let admitted = Arc::new(match expected_predecessor {
                    Some(expected) => {
                        policy
                            .readmit_member_class_declaration(expected.into(), id, instance)
                            .await?
                    }
                    None => policy.reserve_member_class(id, instance).await?,
                });
                let listing_owner = self.clone();
                let deadline = tokio::time::Instant::now() + Duration::from_millis(hold_timeout_ms);
                match host
                    .add_member_class_with_mcp(
                        &context,
                        admitted,
                        spec,
                        route,
                        controller,
                        deadline,
                        move |deadline| async move {
                            listing_owner
                                .request_listing(
                                    request_id,
                                    listing_selection,
                                    deadline,
                                    listing_channel,
                                )
                                .await
                        },
                        mcp,
                    )
                    .await
                {
                    Ok(MemberClassAddStart::Settled(result)) => Ok(settled_result(result)),
                    Ok(MemberClassAddStart::Unsettled(settlement)) => {
                        self.0
                            .pending
                            .lock()
                            .map_err(|_| HostLifetimeError::Unavailable)?
                            .insert(
                                request_id,
                                PendingSettlement {
                                    settlement,
                                    _permit: permit,
                                },
                            );
                        Ok(MemberClassResponse::Unsettled {
                            settlement_id: request_id,
                        })
                    }
                    Err(MemberClassAddError::Host(error)) => Err(error),
                    Err(MemberClassAddError::ReservationNotAccepted { error }) => {
                        Ok(MemberClassResponse::ReservationNotAccepted {
                            error: error.into(),
                        })
                    }
                    Err(MemberClassAddError::Preflight { error, cleanup }) => {
                        Ok(MemberClassResponse::PreflightRefused {
                            error: error.into(),
                            cleanup: cleanup.into(),
                        })
                    }
                    Err(MemberClassAddError::Native(error)) => {
                        Ok(MemberClassResponse::NativeRefused {
                            data: error.structured_data(),
                            message: error.to_string(),
                        })
                    }
                }
            }
            MemberClassCommand::Observe { id, member } => {
                let observed = policy.observe_member_class(&member).await?;
                if observed.admission.id != id
                    || observed.checkpoint.admission != id
                    || observed.admission.instance.member.member != member
                {
                    return Err(HostLifetimeError::WrongOwner);
                }
                Ok(MemberClassResponse::Observed {
                    record: observed.admission,
                    checkpoint: observed.checkpoint.into(),
                    authorization: observed.authorization.into(),
                })
            }
            MemberClassCommand::Retire { id, member } => {
                policy.retire_member_class(&id, &member).await?;
                Ok(MemberClassResponse::Retired)
            }
            MemberClassCommand::Join { .. }
            | MemberClassCommand::ListingResult { .. }
            | MemberClassCommand::McpResult { .. } => unreachable!(),
        }
    }

    pub(super) fn check_workflow_publication(
        &self,
        admitted: &crate::installed_host::dynamic_members::AdmittedMemberClass,
    ) -> Result<(), HostLifetimeError> {
        let host = self
            .0
            .host
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        let current = host.member_class_policy()?.current_member_class(admitted)?;
        if current.class.purpose
            != meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow
            || current.workflow.is_none()
            || current.route.is_none()
            || current.native_surface.is_none()
            || !matches!(
                current.phase,
                crate::installed_host::dynamic_members::MemberClassAdmissionPhase::Prepared { .. }
            )
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    pub(super) async fn request_listing(
        &self,
        operation_id: u64,
        selection: ListingSelection,
        deadline: tokio::time::Instant,
        channel: ListingChannel,
    ) -> Result<(), HostLifetimeError> {
        if channel.closed.is_closed() {
            return Err(HostLifetimeError::Unavailable);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(HostLifetimeError::Denied);
        }
        let (send, receive) = oneshot::channel();
        {
            let mut listings = self
                .0
                .listings
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            if listings.len() >= MAX_PENDING_JOINS || listings.contains_key(&operation_id) {
                return Err(HostLifetimeError::Unavailable);
            }
            listings.insert(
                operation_id,
                PendingListing {
                    selection: selection.clone(),
                    deadline,
                    sender: send,
                },
            );
        }
        // This runs only when the real held callback invokes the application
        // finalizer after its canonical policy commit. A full writer closes the
        // existing connection instead of manufacturing a successful publication.
        if channel
            .replies
            .try_send(response(
                operation_id,
                Ok(Reply::MemberClassListingRequired {
                    operation_id,
                    admission_id: selection.admission_id,
                    identity: selection.identity,
                    class: selection.class,
                }),
            ))
            .is_err()
        {
            channel.failed.send_replace(true);
            self.0
                .listings
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?
                .remove(&operation_id);
            return Err(HostLifetimeError::Unavailable);
        }
        let result = tokio::select! { biased;
            _ = channel.closed.wait_closed() => Err(HostLifetimeError::Unavailable),
            // This is the same held deadline, not a new application timer.
            // Releasing this native waiter does not join a Python mutation;
            // the application must still join_listing before making it void.
            _ = tokio::time::sleep_until(deadline) => Err(HostLifetimeError::Denied),
            result = receive => result.unwrap_or(Err(HostLifetimeError::Unavailable)),
        };
        self.0
            .listings
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .remove(&operation_id);
        result
    }

    /// Synchronous control: never queue the ACK behind the Adds awaiting it.
    pub(super) fn listing_result(
        &self,
        operation_id: u64,
        selection: ListingSelection,
        accepted: bool,
    ) -> Result<(), HostLifetimeError> {
        let mut listings = self
            .0
            .listings
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let pending = listings
            .get(&operation_id)
            .ok_or(HostLifetimeError::Denied)?;
        if pending.selection != selection {
            return Err(HostLifetimeError::WrongOwner);
        }
        let late = accepted && tokio::time::Instant::now() >= pending.deadline;
        let pending = listings
            .remove(&operation_id)
            .ok_or(HostLifetimeError::Denied)?;
        drop(listings);
        pending
            .sender
            .send(if accepted && !late {
                Ok(())
            } else {
                Err(HostLifetimeError::Denied)
            })
            .map_err(|_| HostLifetimeError::Denied)?;
        if late {
            Err(HostLifetimeError::Denied)
        } else {
            Ok(())
        }
    }

    fn capacity(&self) -> Arc<tokio::sync::Semaphore> {
        self.0
            .capacity
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_JOINS)))
            .clone()
    }

    pub(super) async fn close(&self) -> Result<(), HostLifetimeError> {
        let pending = std::mem::take(
            &mut *self
                .0
                .pending
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?,
        );
        let mut failure = None;
        for (_, settlement) in pending {
            if let Err(error) = Self::settled(settlement.settlement).await {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

pub(super) fn settled_result(outcome: MemberClassAddOutcome) -> MemberClassResponse {
    MemberClassResponse::Settled {
        native: match outcome.native {
            Ok(result) => MemberClassNativeOutcome::Result {
                result: native_result(result),
            },
            Err(error) => MemberClassNativeOutcome::Refused {
                message: error.to_string(),
                data: error.structured_data(),
            },
        },
        authorization: outcome.authorization.into(),
        cleanup: outcome.cleanup.into(),
    }
}

fn native_result(result: RestoreFlowResult) -> serde_json::Value {
    let outcomes = result
        .outcomes
        .into_iter()
        .map(|(identity, outcome)| {
            (
                identity.to_string(),
                serde_json::json!({ "restore": outcome.restore_progress() }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({ "outcomes": outcomes, "managed_edges": result.managed_edges })
}

impl MemberClassEndpoint {
    pub(super) fn bind_transport(&self, channel: ListingChannel) -> Result<(), HostLifetimeError> {
        self.0
            .transport
            .set(channel)
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    async fn mcp_exchange(
        &self,
        selection: MemberMcpAdmissionSelection,
        expected: Option<RetainedMemberMcpBindings>,
        requirement_source: Option<crate::private_originals::PrivateSourceEvidence>,
    ) -> Result<McpReply, HostLifetimeError> {
        self.0.owner.validate_current_selection()?;
        if let Some(value) = &expected {
            value.check(&selection)?;
        }
        let channel = self
            .0
            .transport
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        if channel.closed.is_closed() {
            return Err(HostLifetimeError::Unavailable);
        }
        let (sender, receive) = oneshot::channel();
        let id = {
            let mut pending = self
                .0
                .mcp
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            if pending.len() >= MAX_PENDING_JOINS {
                return Err(HostLifetimeError::Unavailable);
            }
            // Allocate, register and enqueue under one existing correlation
            // lock. Python observes these ids in strict wire order.
            let id = self
                .0
                .mcp_next
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| HostLifetimeError::Unavailable)?;
            pending.insert(
                id,
                PendingMcp {
                    selection: selection.clone(),
                    expected: expected.clone(),
                    requirement_source: requirement_source.clone(),
                    sender,
                },
            );
            // Zero denotes an unsolicited request, not a command response.
            if channel
                .replies
                .try_send(response(
                    0,
                    Ok(Reply::MemberClassMcpRequired {
                        exchange_id: id,
                        selection,
                        accepted_requirement: expected
                            .as_ref()
                            .and_then(|value| value.accepted_requirement.clone()),
                        bindings: expected.map(|value| value.bindings),
                        requirement_source,
                    }),
                ))
                .is_err()
            {
                pending.remove(&id);
                channel.failed.send_replace(true);
                return Err(HostLifetimeError::Unavailable);
            }
            id
        };
        let _waiter = McpWaiter {
            owner: self.0.clone(),
            id,
        };
        tokio::select! { biased;
            _ = channel.closed.wait_closed() => Err(HostLifetimeError::Unavailable),
            value = receive => value.unwrap_or(Err(HostLifetimeError::Unavailable)),
        }
    }
    pub(super) fn mcp_result(
        &self,
        id: u64,
        selection: MemberMcpAdmissionSelection,
        retained: Option<RetainedMemberMcpBindings>,
        requirement: Option<AcceptedMemberMcpRequirement>,
    ) -> Result<(), HostLifetimeError> {
        let mut pending = self
            .0
            .mcp
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        // Every observed ACK is terminal for its exact exchange. Even a
        // changed payload must release that waiter before the typed refusal;
        // Python must not tear down unrelated calls for this native decision.
        let current = pending.remove(&id).ok_or(HostLifetimeError::Denied)?;
        drop(pending);
        let validation = (|| {
            if current.selection != selection {
                return Err(HostLifetimeError::WrongOwner);
            }
            if let Some(source) = &current.requirement_source {
                if retained.is_some() {
                    return Err(HostLifetimeError::Denied);
                }
                if let Some(value) = &requirement {
                    value.check(&selection, source)?;
                    if value.contract != MemberMcpRequirementContract::ObservedRequirement {
                        return Err(HostLifetimeError::Denied);
                    }
                }
                return Ok(());
            }
            if requirement.is_some() {
                return Err(HostLifetimeError::Denied);
            }
            if let Some(value) = &retained {
                value.check(&selection)?;
                if current
                    .expected
                    .as_ref()
                    .is_some_and(|expected| expected != value)
                {
                    return Err(HostLifetimeError::Denied);
                }
            }
            Ok(())
        })();
        if let Err(error) = validation {
            let _ = current.sender.send(Err(error));
            return Err(error);
        }
        let reply = if retained.is_some() || requirement.is_some() {
            Ok(McpReply {
                retained,
                requirement,
            })
        } else {
            Err(HostLifetimeError::Denied)
        };
        current
            .sender
            .send(reply)
            .map_err(|_| HostLifetimeError::Unavailable)
    }
}
#[async_trait::async_trait]
impl MemberMcpApplication for MemberClassEndpoint {
    fn requirement_contract(&self) -> MemberMcpRequirementContract {
        MemberMcpRequirementContract::ObservedRequirement
    }
    async fn accepted_requirement(
        &self,
        selection: MemberMcpAdmissionSelection,
        source: crate::private_originals::PrivateSourceEvidence,
    ) -> Result<AcceptedMemberMcpRequirement, HostLifetimeError> {
        self.mcp_exchange(selection, None, Some(source))
            .await?
            .requirement
            .ok_or(HostLifetimeError::Denied)
    }
    async fn retain(&self, value: RetainedMemberMcpBindings) -> Result<(), HostLifetimeError> {
        self.mcp_exchange(value.selection.clone(), Some(value), None)
            .await
            .map(|_| ())
    }
    async fn lookup(
        &self,
        selection: MemberMcpAdmissionSelection,
    ) -> Result<RetainedMemberMcpBindings, HostLifetimeError> {
        self.mcp_exchange(selection, None, None)
            .await?
            .retained
            .ok_or(HostLifetimeError::Denied)
    }
}
