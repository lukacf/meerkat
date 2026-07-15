#[cfg(feature = "runtime-adapter")]
use super::*;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::comms::TrustedPeerDescriptor;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationLifecycleAction, OperationLifecycleSnapshot,
    OperationPeerHandle, OperationProgressUpdate, OperationSource, OperationSpec, OperationStatus,
    OpsLifecycleError, OpsLifecycleRegistry,
};
#[cfg(feature = "runtime-adapter")]
use meerkat_core::types::SessionId;
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::sync::Mutex;

#[cfg(feature = "runtime-adapter")]
#[derive(Clone)]
struct MemberOpsBinding {
    /// Exact adapter-local binding incarnation. A same-session retry may
    /// reuse an identical binding but can never overwrite a different one.
    binding_id: uuid::Uuid,
    /// One exact in-process provisioning transaction may prepare this
    /// binding at a time. The claim is cleared only by exact commit/abort.
    provision_claim: Option<uuid::Uuid>,
    /// Session binding has not yet produced a published spawn receipt. An
    /// early claim failure may remove this exact incarnation.
    unpublished_attempt: bool,
    owner_bridge_session_id: SessionId,
    registry: Arc<dyn OpsLifecycleRegistry>,
    display_name: Option<String>,
    operation_source: OperationSource,
    /// Placed members pre-register one durable operation id before bridge
    /// dispatch, before the member endpoint (and therefore the ordinary
    /// operation source) exists.  Once ACK material arrives, the binding is
    /// pinned to that exact id instead of rediscovering by endpoint source.
    exact_operation_id: Option<OperationId>,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MemberOpsKey {
    Session(SessionId),
    BackendPeer { peer_id: String, address: String },
}

#[cfg(feature = "runtime-adapter")]
impl MemberOpsKey {
    fn from_member_ref(member_ref: &MemberRef) -> Option<Self> {
        match member_ref {
            MemberRef::Session { session_id } => Some(Self::Session(session_id.clone())),
            // Peer-shaped refs key by PEER identity regardless of a projected
            // session binding: registry bindings are installed with the
            // provision-time ref (`session_id: None` for external AND
            // host-materialized members), while delivery-time refs carry the
            // machine session binding projected in — for a placed member
            // that is the REMOTE session id, which must not alias a local
            // session key.
            MemberRef::BackendPeer {
                peer_id, address, ..
            } => Some(Self::BackendPeer {
                peer_id: peer_id.clone(),
                address: address.clone(),
            }),
        }
    }

    fn fallback_display_name(&self) -> String {
        match self {
            Self::Session(session_id) => format!("mob_member/{session_id}"),
            Self::BackendPeer { peer_id, address } => {
                format!("mob_member/backend_peer/{peer_id}@{address}")
            }
        }
    }

    fn child_session_id(&self) -> Option<SessionId> {
        match self {
            Self::Session(session_id) => Some(session_id.clone()),
            Self::BackendPeer { .. } => None,
        }
    }
}

#[cfg(feature = "runtime-adapter")]
pub(crate) struct MobOpsAdapter {
    member_bindings: Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
}

/// Exact adapter-local witness for one session operation-registry binding.
/// Used only when explicit resume has already proved that the corresponding
/// runtime attachment cannot be reused and must not clear a later binding.
#[cfg(feature = "runtime-adapter")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionOpsBindingWitness {
    binding_id: uuid::Uuid,
}

/// Exact, cancellation-safe claim over one session member operation while it
/// is still Provisioning (or an exact Running replay). Drop aborts only the
/// operation/binding incarnation carrying this claim.
#[cfg(feature = "runtime-adapter")]
pub(crate) struct PreparedMemberProvisionOperation {
    member_bindings: Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
    member_key: MemberOpsKey,
    binding_id: uuid::Uuid,
    claim_id: uuid::Uuid,
    registry: Arc<dyn OpsLifecycleRegistry>,
    operation_id: Option<OperationId>,
    display_name: String,
    owns_provisioning: bool,
    remove_binding_on_abort: bool,
    armed: bool,
}

/// Exact rollback authority retained after Provisioning -> Running until the
/// attachment transaction has produced its receipt. If publication fails or
/// is cancelled, Drop terminalizes the operation this claim started.
#[cfg(feature = "runtime-adapter")]
pub(crate) struct CommittedMemberProvisionOperation {
    member_bindings: Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
    member_key: MemberOpsKey,
    binding_id: uuid::Uuid,
    claim_id: uuid::Uuid,
    registry: Arc<dyn OpsLifecycleRegistry>,
    operation_id: OperationId,
    owns_provisioning: bool,
    armed: bool,
}

#[cfg(feature = "runtime-adapter")]
fn exact_claim_still_bound(
    member_bindings: &Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
    member_key: &MemberOpsKey,
    binding_id: uuid::Uuid,
    claim_id: uuid::Uuid,
) -> bool {
    member_bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(member_key)
        .is_some_and(|binding| {
            binding.binding_id == binding_id && binding.provision_claim == Some(claim_id)
        })
}

#[cfg(feature = "runtime-adapter")]
fn clear_exact_provision_claim(
    member_bindings: &Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
    member_key: &MemberOpsKey,
    binding_id: uuid::Uuid,
    claim_id: uuid::Uuid,
    remove_binding: bool,
) -> bool {
    let mut bindings = member_bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let exact = bindings.get(member_key).is_some_and(|binding| {
        binding.binding_id == binding_id && binding.provision_claim == Some(claim_id)
    });
    if !exact {
        return false;
    }
    if remove_binding {
        bindings.remove(member_key);
    } else if let Some(binding) = bindings.get_mut(member_key) {
        binding.provision_claim = None;
    }
    true
}

#[cfg(feature = "runtime-adapter")]
fn publish_exact_provision_claim(
    member_bindings: &Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
    member_key: &MemberOpsKey,
    binding_id: uuid::Uuid,
    claim_id: uuid::Uuid,
) -> bool {
    let mut bindings = member_bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(binding) = bindings.get_mut(member_key)
        && binding.binding_id == binding_id
        && binding.provision_claim == Some(claim_id)
    {
        binding.provision_claim = None;
        binding.unpublished_attempt = false;
        true
    } else {
        false
    }
}

#[cfg(feature = "runtime-adapter")]
impl MobOpsAdapter {
    pub(crate) fn new() -> Self {
        Self {
            member_bindings: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn bind_session_registry(
        &self,
        child_session_id: SessionId,
        owner_bridge_session_id: SessionId,
        registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        let member_key = MemberOpsKey::Session(child_session_id.clone());
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = bindings.get(&member_key) {
            let exact_retry = existing.owner_bridge_session_id == owner_bridge_session_id
                && Arc::ptr_eq(&existing.registry, &registry)
                && existing.display_name.is_none()
                && existing.operation_source
                    == OperationSource::session_child(child_session_id.clone())
                && existing.exact_operation_id.is_none()
                && existing.provision_claim.is_none();
            if exact_retry {
                return Ok(());
            }
            return Err(MobError::Internal(format!(
                "session '{child_session_id}' already has a different operation-registry binding incarnation"
            )));
        }
        bindings.insert(
            member_key,
            MemberOpsBinding {
                binding_id: uuid::Uuid::new_v4(),
                provision_claim: None,
                unpublished_attempt: true,
                owner_bridge_session_id,
                registry,
                display_name: None,
                operation_source: OperationSource::session_child(child_session_id),
                exact_operation_id: None,
            },
        );
        Ok(())
    }

    pub(crate) fn capture_session_binding_witness(
        &self,
        child_session_id: &SessionId,
    ) -> Option<SessionOpsBindingWitness> {
        self.member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&MemberOpsKey::Session(child_session_id.clone()))
            .map(|binding| SessionOpsBindingWitness {
                binding_id: binding.binding_id,
            })
    }

    /// Compare-and-remove only the binding observed before exact attachment
    /// retirement. A replacement binding is never cleared, and an armed
    /// provision claim fails closed rather than being detached underneath its
    /// owner.
    pub(crate) fn clear_session_binding_for_explicit_resume(
        &self,
        child_session_id: &SessionId,
        expected: Option<SessionOpsBindingWitness>,
    ) -> Result<(), MobError> {
        let member_key = MemberOpsKey::Session(child_session_id.clone());
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(current) = bindings.get(&member_key) else {
            return Ok(());
        };
        let Some(expected) = expected else {
            return Err(MobError::Internal(format!(
                "operation-registry binding for session '{child_session_id}' appeared during explicit-resume retirement"
            )));
        };
        if current.binding_id != expected.binding_id {
            return Err(MobError::Internal(format!(
                "operation-registry binding for session '{child_session_id}' changed during explicit-resume retirement"
            )));
        }
        if current.provision_claim.is_some() {
            return Err(MobError::Internal(format!(
                "operation-registry binding for session '{child_session_id}' still has an exact provision claim during explicit resume"
            )));
        }
        bindings.remove(&member_key);
        Ok(())
    }

    pub(crate) fn bind_member_registry(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        registry: Arc<dyn OpsLifecycleRegistry>,
        display_name: impl Into<String>,
        operation_source: OperationSource,
    ) -> Result<(), MobError> {
        let member_key = MemberOpsKey::from_member_ref(member_ref).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot bind registry for member without canonical key: {member_ref:?}",
            ))
        })?;
        let display_name = display_name.into();
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let MemberOpsKey::BackendPeer {
            peer_id: incoming_peer_id,
            ..
        } = &member_key
        {
            let same_peer_keys = bindings
                .keys()
                .filter(|key| {
                    matches!(
                        key,
                        MemberOpsKey::BackendPeer { peer_id, .. }
                            if peer_id == incoming_peer_id
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            if let [existing_key] = same_peer_keys.as_slice() {
                let existing = bindings.get(existing_key).ok_or_else(|| {
                    MobError::Internal(format!(
                        "member peer '{incoming_peer_id}' binding disappeared during admission"
                    ))
                })?;
                let same_generic_binding = existing_key == &member_key
                    && existing.exact_operation_id.is_none()
                    && existing.owner_bridge_session_id == owner_bridge_session_id
                    && existing.display_name.as_deref() == Some(display_name.as_str())
                    && existing.operation_source == operation_source;
                if !same_generic_binding {
                    return Err(MobError::Internal(format!(
                        "member peer '{incoming_peer_id}' is already bound to a different or exact placed lifecycle operation"
                    )));
                }
            } else if !same_peer_keys.is_empty() {
                return Err(MobError::Internal(format!(
                    "member peer '{incoming_peer_id}' has duplicate live endpoint bindings"
                )));
            }
        }
        bindings.insert(
            member_key,
            MemberOpsBinding {
                binding_id: uuid::Uuid::new_v4(),
                provision_claim: None,
                unpublished_attempt: false,
                owner_bridge_session_id,
                registry,
                display_name: Some(display_name),
                operation_source,
                exact_operation_id: None,
            },
        );
        Ok(())
    }

    /// Atomically reserve one generic peer identity and pre-register its exact
    /// lifecycle operation before any BindMember/trust side effect. The
    /// operation id is the opaque attempt token: another attempt at the same
    /// PeerId cannot replay, overwrite, finalize, or clear this reservation.
    pub(crate) fn reserve_generic_member_provision(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        registry: Arc<dyn OpsLifecycleRegistry>,
        display_name: impl Into<String>,
        operation_source: OperationSource,
    ) -> Result<OperationId, MobError> {
        let member_key = Self::require_member_key(member_ref, "reserve generic provision for")?;
        let MemberOpsKey::BackendPeer {
            peer_id: incoming_peer_id,
            ..
        } = &member_key
        else {
            return Err(MobError::Internal(
                "generic peer reservation requires a backend peer key".to_string(),
            ));
        };
        let display_name = display_name.into();
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if bindings.keys().any(|key| {
            matches!(
                key,
                MemberOpsKey::BackendPeer { peer_id, .. } if peer_id == incoming_peer_id
            )
        }) {
            return Err(MobError::Internal(format!(
                "member peer '{incoming_peer_id}' is already reserved or bound"
            )));
        }
        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: owner_bridge_session_id.clone(),
                display_name: display_name.clone(),
                source_label: "mob_member".to_string(),
                operation_source: Some(operation_source.clone()),
                child_session_id: None,
                expect_peer_channel: true,
            })
            .map_err(|error| MobError::Internal(error.to_string()))?;
        bindings.insert(
            member_key,
            MemberOpsBinding {
                binding_id: uuid::Uuid::new_v4(),
                provision_claim: None,
                unpublished_attempt: false,
                owner_bridge_session_id,
                registry,
                display_name: Some(display_name),
                operation_source,
                exact_operation_id: Some(operation_id.clone()),
            },
        );
        Ok(operation_id)
    }

    pub(crate) fn complete_generic_member_provision_exact(
        &self,
        member_ref: &MemberRef,
        operation_owner_session_id: &SessionId,
        display_name: &str,
        operation_source: &OperationSource,
        operation_id: &OperationId,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "complete generic provision for")?;
        let binding = self.require_binding_for_key(&member_key, "complete generic provision")?;
        if binding.exact_operation_id.as_ref() != Some(operation_id)
            || &binding.owner_bridge_session_id != operation_owner_session_id
            || binding.display_name.as_deref() != Some(display_name)
            || &binding.operation_source != operation_source
        {
            return Err(MobError::Internal(format!(
                "generic provision reservation '{operation_id}' conflicts with its exact binding tuple"
            )));
        }
        let exact_running = |snapshot: &OperationLifecycleSnapshot| {
            snapshot.id == *operation_id
                && snapshot.kind == OperationKind::MobMemberChild
                && &snapshot.owner_session_id == operation_owner_session_id
                && snapshot.display_name == display_name
                && snapshot.source_label == "mob_member"
                && snapshot.operation_source.as_ref() == Some(operation_source)
                && snapshot.child_session_id.is_none()
                && snapshot.expect_peer_channel
                && !snapshot.terminal
        };
        let snapshot = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "generic provision reservation operation '{operation_id}' is missing"
                ))
            })?;
        if !exact_running(&snapshot)
            || !matches!(
                snapshot.status,
                OperationStatus::Provisioning | OperationStatus::Running
            )
        {
            return Err(MobError::Internal(format!(
                "generic provision reservation operation '{operation_id}' has incompatible tuple/status"
            )));
        }
        if snapshot.status == OperationStatus::Provisioning
            && let Err(error) = binding.registry.provisioning_succeeded(operation_id)
        {
            let current = binding
                .registry
                .snapshot(operation_id)
                .map_err(|read_error| MobError::Internal(read_error.to_string()))?;
            if !current.as_ref().is_some_and(|snapshot| {
                exact_running(snapshot) && snapshot.status == OperationStatus::Running
            }) {
                return Err(MobError::Internal(error.to_string()));
            }
        }
        let running = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "generic provision operation '{operation_id}' disappeared while starting"
                ))
            })?;
        if !exact_running(&running) || running.status != OperationStatus::Running {
            return Err(MobError::Internal(format!(
                "generic provision operation '{operation_id}' did not reach exact Running"
            )));
        }
        Ok(())
    }

    pub(crate) fn abort_generic_member_provision_exact(
        &self,
        member_ref: &MemberRef,
        operation_owner_session_id: &SessionId,
        display_name: &str,
        operation_source: &OperationSource,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "abort generic provision for")?;
        let binding = self.require_binding_for_key(&member_key, "abort generic provision")?;
        if binding.exact_operation_id.as_ref() != Some(operation_id)
            || &binding.owner_bridge_session_id != operation_owner_session_id
            || binding.display_name.as_deref() != Some(display_name)
            || &binding.operation_source != operation_source
        {
            return Err(MobError::Internal(format!(
                "generic provision reservation '{operation_id}' conflicts with cleanup authority"
            )));
        }
        let exact_tuple = |snapshot: &OperationLifecycleSnapshot| {
            snapshot.id == *operation_id
                && snapshot.kind == OperationKind::MobMemberChild
                && &snapshot.owner_session_id == operation_owner_session_id
                && snapshot.display_name == display_name
                && snapshot.source_label == "mob_member"
                && snapshot.operation_source.as_ref() == Some(operation_source)
                && snapshot.child_session_id.is_none()
                && snapshot.expect_peer_channel
        };
        let snapshot = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?;
        if let Some(snapshot) = snapshot {
            if !exact_tuple(&snapshot) {
                return Err(MobError::Internal(format!(
                    "generic provision operation '{operation_id}' conflicts with cleanup tuple"
                )));
            }
            if !snapshot.terminal {
                let result = match snapshot.status {
                    OperationStatus::Provisioning => binding
                        .registry
                        .abort_provisioning(operation_id, Some(reason.to_string())),
                    OperationStatus::Running => binding
                        .registry
                        .cancel_operation(operation_id, Some(reason.to_string())),
                    OperationStatus::Retiring => binding.registry.mark_retired(operation_id),
                    status => {
                        return Err(MobError::Internal(format!(
                            "generic provision operation '{operation_id}' has incompatible cleanup status {status:?}"
                        )));
                    }
                };
                if let Err(error) = result {
                    let current = binding
                        .registry
                        .snapshot(operation_id)
                        .map_err(|read_error| MobError::Internal(read_error.to_string()))?;
                    if !current.as_ref().is_some_and(|snapshot| {
                        exact_tuple(snapshot)
                            && snapshot.terminal
                            && matches!(
                                snapshot.status,
                                OperationStatus::Aborted
                                    | OperationStatus::Cancelled
                                    | OperationStatus::Retired
                            )
                    }) {
                        return Err(MobError::Internal(error.to_string()));
                    }
                }
            } else if !matches!(
                snapshot.status,
                OperationStatus::Aborted | OperationStatus::Cancelled | OperationStatus::Retired
            ) {
                return Err(MobError::Internal(format!(
                    "generic provision operation '{operation_id}' has incompatible terminal cleanup status {:?}",
                    snapshot.status
                )));
            }
            let terminal = binding
                .registry
                .snapshot(operation_id)
                .map_err(|error| MobError::Internal(error.to_string()))?
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "generic provision operation '{operation_id}' disappeared during cleanup"
                    ))
                })?;
            if !exact_tuple(&terminal)
                || !terminal.terminal
                || !matches!(
                    terminal.status,
                    OperationStatus::Aborted
                        | OperationStatus::Cancelled
                        | OperationStatus::Retired
                )
            {
                return Err(MobError::Internal(format!(
                    "generic provision operation '{operation_id}' did not reach exact cleanup terminal"
                )));
            }
        }
        self.clear_generic_member_binding_exact(
            member_ref,
            operation_owner_session_id,
            display_name,
            operation_source,
            operation_id,
        )
    }

    pub(crate) fn bind_member_registry_for_exact_operation(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        registry: Arc<dyn OpsLifecycleRegistry>,
        display_name: impl Into<String>,
        operation_source: OperationSource,
        operation_id: OperationId,
    ) -> Result<(), MobError> {
        let member_key = MemberOpsKey::from_member_ref(member_ref).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot bind exact registry for member without canonical key: {member_ref:?}",
            ))
        })?;
        let MemberOpsKey::BackendPeer {
            peer_id: incoming_peer_id,
            ..
        } = &member_key
        else {
            return Err(MobError::Internal(
                "exact placed operation binding requires a peer endpoint key".to_string(),
            ));
        };
        let display_name = display_name.into();
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let same_operation_keys = bindings
            .iter()
            .filter(|(_, binding)| binding.exact_operation_id.as_ref() == Some(&operation_id))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        if let [existing_key] = same_operation_keys.as_slice() {
            let existing = bindings.get(existing_key).ok_or_else(|| {
                MobError::Internal(format!(
                    "exact placed operation '{operation_id}' binding disappeared during exact admission"
                ))
            })?;
            let exact_replay = existing.owner_bridge_session_id == owner_bridge_session_id
                && existing.display_name.as_deref() == Some(display_name.as_str())
                && existing.operation_source == operation_source
                && existing_key == &member_key;
            if exact_replay {
                let existing = bindings.get_mut(existing_key).ok_or_else(|| {
                    MobError::Internal(format!(
                        "exact placed operation '{operation_id}' binding disappeared during registry refresh"
                    ))
                })?;
                // The owner registry object itself may be reconstructed while
                // the adapter survives. The exact carrier tuple authorizes an
                // atomic capability refresh; retaining the old Arc would send
                // later upcalls/cleanup to stale lifecycle state.
                existing.registry = registry;
                return Ok(());
            }
            return Err(MobError::Internal(format!(
                "exact placed operation '{operation_id}' is already bound to a different member endpoint/tuple"
            )));
        } else if !same_operation_keys.is_empty() {
            return Err(MobError::Internal(format!(
                "exact placed operation '{operation_id}' has duplicate live member bindings"
            )));
        }
        let same_peer_keys = bindings
            .keys()
            .filter(|key| {
                matches!(
                    key,
                    MemberOpsKey::BackendPeer { peer_id, .. }
                        if peer_id == incoming_peer_id
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if !same_peer_keys.is_empty() {
            // Endpoint keys are canonical transport identity.  A terminal old
            // binding is not implicitly replaceable either: its carrier-owned
            // cleanup must explicitly validate and clear it first.
            return Err(MobError::Internal(format!(
                "placed member endpoint '{member_key:?}' is already bound to a different lifecycle operation"
            )));
        }
        bindings.insert(
            member_key,
            MemberOpsBinding {
                binding_id: uuid::Uuid::new_v4(),
                provision_claim: None,
                unpublished_attempt: false,
                owner_bridge_session_id,
                registry,
                display_name: Some(display_name),
                operation_source,
                exact_operation_id: Some(operation_id),
            },
        );
        Ok(())
    }

    fn binding_for_key(&self, member_key: &MemberOpsKey) -> Option<MemberOpsBinding> {
        self.member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(member_key)
            .cloned()
    }

    fn require_binding_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
    ) -> Result<MemberOpsBinding, MobError> {
        self.binding_for_key(member_key).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot {operation} for member '{member_key:?}': missing generated owner operation binding"
            ))
        })
    }

    fn clear_member_binding(&self, member_key: &MemberOpsKey) -> Result<(), MobError> {
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if bindings
            .get(member_key)
            .is_some_and(|binding| binding.provision_claim.is_some())
        {
            return Err(MobError::Internal(format!(
                "cannot clear member binding '{member_key:?}' while an exact provision claim is armed"
            )));
        }
        bindings.remove(member_key);
        Ok(())
    }

    /// Remove the one live endpoint binding for an exact placed operation.
    /// The carrier tuple, not an endpoint rediscovery, authorizes removal.
    /// No binding is restart-idempotent; duplicate exact-id bindings or tuple
    /// drift fail closed so cleanup cannot silently detach another member.
    pub(crate) fn clear_placed_member_binding_exact(
        &self,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let exact_keys = bindings
            .iter()
            .filter(|(_, binding)| binding.exact_operation_id.as_ref() == Some(operation_id))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        match exact_keys.as_slice() {
            [] => Ok(()),
            [key] => {
                let binding = bindings.get(key).ok_or_else(|| {
                    MobError::Internal(format!(
                        "exact placed operation '{operation_id}' binding disappeared during cleanup"
                    ))
                })?;
                if &binding.owner_bridge_session_id != operation_owner_session_id
                    || binding.display_name.as_deref() != Some(display_name)
                {
                    return Err(MobError::Internal(format!(
                        "exact placed operation '{operation_id}' binding conflicts with its carrier owner/display tuple"
                    )));
                }
                bindings.remove(key);
                Ok(())
            }
            _ => Err(MobError::Internal(format!(
                "exact placed operation '{operation_id}' has duplicate live member bindings"
            ))),
        }
    }

    pub(crate) fn clear_generic_member_binding_exact(
        &self,
        member_ref: &MemberRef,
        operation_owner_session_id: &SessionId,
        display_name: &str,
        operation_source: &OperationSource,
        operation_id: &OperationId,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "clear generic reservation for")?;
        let mut bindings = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(binding) = bindings.get(&member_key) else {
            return Ok(());
        };
        if binding.exact_operation_id.as_ref() != Some(operation_id)
            || &binding.owner_bridge_session_id != operation_owner_session_id
            || binding.display_name.as_deref() != Some(display_name)
            || &binding.operation_source != operation_source
        {
            return Err(MobError::Internal(format!(
                "generic member reservation '{member_key:?}' conflicts with cleanup authority"
            )));
        }
        bindings.remove(&member_key);
        Ok(())
    }

    /// Resolve the live canonical mob-child lifecycle operation for a session.
    ///
    /// This is intentionally stricter than "latest operation for session":
    /// callers that need to mutate or re-bind active member lifecycle truth
    /// must never silently reuse a terminal operation snapshot.
    pub(crate) async fn active_operation_id_for_session(
        &self,
        session_id: &SessionId,
    ) -> Option<OperationId> {
        self.active_operation_id_for_key(&MemberOpsKey::Session(session_id.clone()))
            .await
    }

    pub(crate) async fn active_operation_id_for_member(
        &self,
        member_ref: &MemberRef,
    ) -> Option<OperationId> {
        let member_key = MemberOpsKey::from_member_ref(member_ref)?;
        self.active_operation_id_for_key(&member_key).await
    }

    async fn active_operation_id_for_key(&self, member_key: &MemberOpsKey) -> Option<OperationId> {
        let binding = self.binding_for_key(member_key)?;
        let snapshots = match Self::matching_operations_for_binding(member_key, &binding) {
            Ok(snapshots) => snapshots,
            Err(error) => {
                tracing::error!(
                    member_key = ?member_key,
                    error = %error,
                    "mob ops adapter failed to project operations through generated authority",
                );
                return None;
            }
        };
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => {
                let operation_id = active_ids[0].clone();
                if let Err(error) = self.ensure_running_before_live_update(
                    member_key,
                    &operation_id,
                    "resolve active operation",
                ) {
                    tracing::error!(
                        member_key = ?member_key,
                        operation_id = %operation_id,
                        error = %error,
                        "mob ops adapter failed to canonicalize active operation before returning it",
                    );
                    return None;
                }
                Some(operation_id)
            }
            0 => None,
            _ => {
                tracing::error!(
                    member_key = ?member_key,
                    operation_ids = %Self::format_operation_ids(&active_ids),
                    "mob ops adapter found ambiguous active mob child operations; refusing to pick one",
                );
                None
            }
        }
    }

    fn require_member_key(
        member_ref: &MemberRef,
        operation: &str,
    ) -> Result<MemberOpsKey, MobError> {
        MemberOpsKey::from_member_ref(member_ref).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot {operation} member without canonical lifecycle key: {member_ref:?}",
            ))
        })
    }

    fn matching_operations_for_key(
        &self,
        member_key: &MemberOpsKey,
    ) -> Result<Vec<OperationLifecycleSnapshot>, MobError> {
        let Some(binding) = self.binding_for_key(member_key) else {
            return Ok(Vec::new());
        };
        Self::matching_operations_for_binding(member_key, &binding)
    }

    fn snapshot_matches_binding(
        binding: &MemberOpsBinding,
        snapshot: &OperationLifecycleSnapshot,
    ) -> bool {
        snapshot.kind == OperationKind::MobMemberChild
            && binding.exact_operation_id.as_ref().map_or_else(
                || snapshot.operation_source.as_ref() == Some(&binding.operation_source),
                |operation_id| &snapshot.id == operation_id,
            )
    }

    fn matching_operations_for_binding(
        _member_key: &MemberOpsKey,
        binding: &MemberOpsBinding,
    ) -> Result<Vec<OperationLifecycleSnapshot>, MobError> {
        Ok(binding
            .registry
            .list_operations()
            .map_err(|error| MobError::Internal(error.to_string()))?
            .into_iter()
            .filter(|snapshot| Self::snapshot_matches_binding(binding, snapshot))
            .collect())
    }

    fn require_operation_for_binding(
        member_key: &MemberOpsKey,
        binding: &MemberOpsBinding,
        operation_id: &OperationId,
        operation: &str,
    ) -> Result<OperationLifecycleSnapshot, MobError> {
        let Some(snapshot) = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
        else {
            return Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation}: operation '{operation_id}' not found",
            )));
        };
        if !Self::snapshot_matches_binding(binding, &snapshot) {
            return Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation} for member '{member_key:?}': operation '{operation_id}' is not owned by generated operation source authority"
            )));
        }
        Ok(snapshot)
    }

    fn newest_operation_snapshot(
        snapshots: &[OperationLifecycleSnapshot],
    ) -> Option<&OperationLifecycleSnapshot> {
        snapshots.iter().max_by(|lhs, rhs| {
            lhs.created_at_ms
                .cmp(&rhs.created_at_ms)
                .then_with(|| lhs.id.0.cmp(&rhs.id.0))
        })
    }

    fn snapshot_is_terminal(snapshot: &OperationLifecycleSnapshot) -> bool {
        snapshot.terminal
    }

    fn active_operation_ids(snapshots: &[OperationLifecycleSnapshot]) -> Vec<OperationId> {
        let mut active_ids = Vec::new();
        for snapshot in snapshots {
            if !Self::snapshot_is_terminal(snapshot) {
                active_ids.push(snapshot.id.clone());
            }
        }
        active_ids
    }

    fn invalid_transition_is_idempotent(
        registry: &Arc<dyn OpsLifecycleRegistry>,
        action: OperationLifecycleAction,
        error: &OpsLifecycleError,
    ) -> Result<bool, MobError> {
        Self::invalid_transition_is_idempotent_dyn(registry.as_ref(), action, error)
    }

    fn invalid_transition_is_idempotent_dyn(
        registry: &dyn OpsLifecycleRegistry,
        action: OperationLifecycleAction,
        error: &OpsLifecycleError,
    ) -> Result<bool, MobError> {
        match error {
            OpsLifecycleError::InvalidTransition { id, .. } => registry
                .classify_operation_transition_idempotence(id, action)
                .map_err(|error| MobError::Internal(error.to_string())),
            _ => Ok(false),
        }
    }

    fn require_exact_existing_peer_handle(
        registry: &Arc<dyn OpsLifecycleRegistry>,
        operation_id: &OperationId,
        requested_peer_handle: &OperationPeerHandle,
    ) -> Result<(), MobError> {
        let snapshot = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "operation '{operation_id}' reported already peer-ready but has no lifecycle snapshot"
                ))
            })?;
        match snapshot.peer_handle {
            Some(existing) if &existing == requested_peer_handle => Ok(()),
            Some(existing) => Err(MobError::Internal(format!(
                "operation '{operation_id}' is already peer-ready with a different handle: existing={existing:?}, requested={requested_peer_handle:?}"
            ))),
            None => Err(MobError::Internal(format!(
                "operation '{operation_id}' reported already peer-ready without a persisted peer handle"
            ))),
        }
    }

    fn format_operation_ids(ids: &[OperationId]) -> String {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn operation_status_with_terminality(
        &self,
        session_id: &SessionId,
        operation_id: &OperationId,
    ) -> Result<Option<(OperationStatus, bool)>, MobError> {
        let Some(binding) = self.binding_for_key(&MemberOpsKey::Session(session_id.clone())) else {
            return Ok(None);
        };
        let Some(snapshot) = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .filter(|snapshot| Self::snapshot_matches_binding(&binding, snapshot))
        else {
            return Ok(None);
        };
        let terminal = binding
            .registry
            .classify_operation_terminality(operation_id, snapshot.status)
            .map_err(|error| MobError::Internal(error.to_string()))?;
        Ok(Some((snapshot.status, terminal)))
    }

    fn display_name_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
    ) -> Result<String, MobError> {
        let binding = self.require_binding_for_key(member_key, operation)?;
        Ok(binding
            .display_name
            .unwrap_or_else(|| member_key.fallback_display_name()))
    }

    fn registry_for_existing_binding(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
    ) -> Result<Arc<dyn OpsLifecycleRegistry>, MobError> {
        Ok(self
            .require_binding_for_key(member_key, operation)?
            .registry)
    }

    async fn resolve_or_register_active_operation_id_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let binding = self.require_binding_for_key(member_key, operation)?;
        let snapshots = Self::matching_operations_for_binding(member_key, &binding)?;
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => Ok(active_ids[0].clone()),
            0 => {
                self.ensure_operation_for_key(member_key, display_name)
                    .await
            }
            _ => Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation} for member '{member_key:?}': multiple active mob child operations [{}]",
                Self::format_operation_ids(&active_ids),
            ))),
        }
    }

    async fn ensure_operation_for_key(
        &self,
        member_key: &MemberOpsKey,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let binding = self.require_binding_for_key(member_key, "register operation")?;
        if let Some(exact_operation_id) = binding.exact_operation_id.as_ref() {
            let snapshot = binding
                .registry
                .snapshot(exact_operation_id)
                .map_err(|error| MobError::Internal(error.to_string()))?
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "exact placed provision operation '{exact_operation_id}' is not registered"
                    ))
                })?;
            if snapshot.kind != OperationKind::MobMemberChild
                || snapshot.owner_session_id != binding.owner_bridge_session_id
                || snapshot.source_label != "mob_member"
                || binding
                    .display_name
                    .as_ref()
                    .is_some_and(|display_name| &snapshot.display_name != display_name)
                || snapshot.terminal
                || !snapshot.expect_peer_channel
                || snapshot
                    .operation_source
                    .as_ref()
                    .is_some_and(|source| source != &binding.operation_source)
                || snapshot.child_session_id != member_key.child_session_id()
            {
                return Err(MobError::Internal(format!(
                    "exact placed provision operation '{exact_operation_id}' conflicts with its member binding"
                )));
            }
            let conflicting = binding
                .registry
                .list_operations()
                .map_err(|error| MobError::Internal(error.to_string()))?
                .into_iter()
                .filter(|candidate| {
                    &candidate.id != exact_operation_id
                        && !candidate.terminal
                        && candidate.kind == OperationKind::MobMemberChild
                        && candidate.operation_source.as_ref() == Some(&binding.operation_source)
                })
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>();
            if !conflicting.is_empty() {
                return Err(MobError::Internal(format!(
                    "exact placed provision operation '{exact_operation_id}' conflicts with active endpoint operations [{}]",
                    Self::format_operation_ids(&conflicting)
                )));
            }
            return Ok(exact_operation_id.clone());
        }
        let snapshots = Self::matching_operations_for_binding(member_key, &binding)?;
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => return Ok(active_ids[0].clone()),
            0 => {}
            _ => {
                return Err(MobError::Internal(format!(
                    "cannot provision member '{member_key:?}' with ambiguous active mob child operations [{}]",
                    Self::format_operation_ids(&active_ids)
                )));
            }
        }

        let operation_id = OperationId::new();
        let registry = binding.registry;
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: binding.owner_bridge_session_id,
                display_name: display_name.to_string(),
                source_label: "mob_member".to_string(),
                operation_source: Some(binding.operation_source.clone()),
                child_session_id: member_key.child_session_id(),
                expect_peer_channel: true,
            })
            .map_err(|error| MobError::Internal(error.to_string()))?;
        Ok(operation_id)
    }

    /// Register the canonical placed-member operation before any bridge send.
    /// `AlreadyRegistered` is accepted only when the exact id is the same
    /// non-terminal mob-child anchor in this owner registry.
    pub(crate) fn register_placed_provision_operation_exact(
        registry: &dyn OpsLifecycleRegistry,
        owner_session_id: SessionId,
        operation_id: OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let expected_owner_session_id = owner_session_id.clone();
        let spec = OperationSpec {
            id: operation_id.clone(),
            kind: OperationKind::MobMemberChild,
            owner_session_id,
            display_name: display_name.to_string(),
            source_label: "mob_member".to_string(),
            // The member endpoint does not exist until MaterializeMember ACK.
            // Exact-id binding below replaces endpoint-source discovery.
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: true,
        };
        match registry.register_operation(spec) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::AlreadyRegistered(_)) => {
                let snapshot = registry
                    .snapshot(&operation_id)
                    .map_err(|error| MobError::Internal(error.to_string()))?
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "placed provision operation '{operation_id}' reported AlreadyRegistered but has no snapshot"
                        ))
                    })?;
                if snapshot.kind != OperationKind::MobMemberChild
                    || snapshot.owner_session_id != expected_owner_session_id
                    || snapshot.display_name != display_name
                    || snapshot.source_label != "mob_member"
                    || snapshot.operation_source.is_some()
                    || snapshot.child_session_id.is_some()
                    || !snapshot.expect_peer_channel
                    || snapshot.status != OperationStatus::Provisioning
                    || snapshot.peer_ready
                    || snapshot.terminal
                {
                    return Err(MobError::Internal(format!(
                        "placed provision operation '{operation_id}' conflicts with an existing lifecycle anchor"
                    )));
                }
                Ok(())
            }
            Err(error) => Err(MobError::Internal(error.to_string())),
        }
    }

    /// Read-only Running postcondition for a committed placed operation.
    /// Exact-ID reconstruction is performed by
    /// `ensure_committed_placed_provision_operation_exact`; this helper is
    /// also used after ACK/binding to prove no endpoint-based substitution.
    pub(crate) fn verify_committed_placed_provision_operation_exact(
        registry: &dyn OpsLifecycleRegistry,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        Self::verify_committed_placed_provision_operation_exact_for_recovery(
            registry,
            operation_owner_session_id,
            operation_id,
            display_name,
            super::provisioner::PlacedOperationRecoveryExpectation::Active,
        )
    }

    fn verify_committed_placed_provision_operation_exact_for_recovery(
        registry: &dyn OpsLifecycleRegistry,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
        expectation: super::provisioner::PlacedOperationRecoveryExpectation,
    ) -> Result<(), MobError> {
        let snapshot = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "committed placed provision operation '{operation_id}' is missing"
                ))
            })?;
        if snapshot.kind != OperationKind::MobMemberChild
            || &snapshot.owner_session_id != operation_owner_session_id
            || snapshot.display_name != display_name
            || snapshot.source_label != "mob_member"
            || snapshot.operation_source.is_some()
            || snapshot.child_session_id.is_some()
            || !snapshot.expect_peer_channel
            || !match expectation {
                super::provisioner::PlacedOperationRecoveryExpectation::Active => {
                    snapshot.status == OperationStatus::Running
                }
                super::provisioner::PlacedOperationRecoveryExpectation::Retiring => matches!(
                    snapshot.status,
                    OperationStatus::Running | OperationStatus::Retiring
                ),
            }
            || snapshot.terminal
        {
            return Err(MobError::Internal(format!(
                "committed placed provision operation '{operation_id}' conflicts with its exact Running lifecycle anchor"
            )));
        }
        Ok(())
    }

    /// Reconstruct or resume the exact operation named by a Committed carrier.
    /// Nonterminal lifecycle rows are intentionally not durable across a true
    /// process restart, so absence is repaired by registering the SAME id and
    /// immutable owner/spec tuple, then starting it.  An exact Provisioning
    /// row is likewise started; exact Running is accepted.  Retiring,
    /// terminal, and tuple-conflicting rows fail closed.  This method never
    /// mints or discovers an id.
    pub(crate) fn ensure_committed_placed_provision_operation_exact(
        registry: &dyn OpsLifecycleRegistry,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        Self::ensure_committed_placed_provision_operation_exact_for_recovery(
            registry,
            operation_owner_session_id,
            operation_id,
            display_name,
            super::provisioner::PlacedOperationRecoveryExpectation::Active,
        )
    }

    pub(crate) fn ensure_committed_placed_provision_operation_exact_for_recovery(
        registry: &dyn OpsLifecycleRegistry,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
        expectation: super::provisioner::PlacedOperationRecoveryExpectation,
    ) -> Result<(), MobError> {
        let snapshot = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?;
        match snapshot {
            None => {
                Self::register_placed_provision_operation_exact(
                    registry,
                    operation_owner_session_id.clone(),
                    operation_id.clone(),
                    display_name,
                )?;
            }
            Some(snapshot) => {
                let exact_tuple = snapshot.kind == OperationKind::MobMemberChild
                    && &snapshot.owner_session_id == operation_owner_session_id
                    && snapshot.display_name == display_name
                    && snapshot.source_label == "mob_member"
                    && snapshot.operation_source.is_none()
                    && snapshot.child_session_id.is_none()
                    && snapshot.expect_peer_channel;
                if !exact_tuple || snapshot.terminal {
                    return Err(MobError::Internal(format!(
                        "committed placed provision operation '{operation_id}' conflicts with its exact lifecycle anchor"
                    )));
                }
                match snapshot.status {
                    OperationStatus::Running => {
                        return Self::verify_committed_placed_provision_operation_exact_for_recovery(
                            registry,
                            operation_owner_session_id,
                            operation_id,
                            display_name,
                            expectation,
                        );
                    }
                    OperationStatus::Retiring
                        if expectation
                            == super::provisioner::PlacedOperationRecoveryExpectation::Retiring =>
                    {
                        return Self::verify_committed_placed_provision_operation_exact_for_recovery(
                            registry,
                            operation_owner_session_id,
                            operation_id,
                            display_name,
                            expectation,
                        );
                    }
                    OperationStatus::Provisioning => {}
                    status => {
                        return Err(MobError::Internal(format!(
                            "committed placed provision operation '{operation_id}' has incompatible recovery status {status:?}"
                        )));
                    }
                }
            }
        }

        if let Err(start_error) = registry.provisioning_succeeded(operation_id) {
            // A same-id concurrent repair may have won the transition.  Only
            // the exact Running postcondition makes that error idempotent.
            if Self::verify_committed_placed_provision_operation_exact_for_recovery(
                registry,
                operation_owner_session_id,
                operation_id,
                display_name,
                expectation,
            )
            .is_err()
            {
                return Err(MobError::Internal(start_error.to_string()));
            }
        }
        Self::verify_committed_placed_provision_operation_exact_for_recovery(
            registry,
            operation_owner_session_id,
            operation_id,
            display_name,
            expectation,
        )
    }

    pub(crate) fn abort_placed_provision_operation_exact(
        registry: &dyn OpsLifecycleRegistry,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        let Some(snapshot) = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
        else {
            return Ok(());
        };
        if snapshot.kind != OperationKind::MobMemberChild
            || &snapshot.owner_session_id != operation_owner_session_id
            || snapshot.display_name != display_name
            || snapshot.source_label != "mob_member"
            || snapshot.operation_source.is_some()
            || snapshot.child_session_id.is_some()
            || !snapshot.expect_peer_channel
        {
            return Err(MobError::Internal(format!(
                "placed provision operation '{operation_id}' conflicts with its pre-dispatch anchor"
            )));
        }
        if snapshot.terminal {
            return if matches!(
                snapshot.status,
                OperationStatus::Aborted | OperationStatus::Cancelled | OperationStatus::Retired
            ) {
                Ok(())
            } else {
                Err(MobError::Internal(format!(
                    "placed provision operation '{operation_id}' has incompatible terminal Pending-cleanup status {:?}",
                    snapshot.status
                )))
            };
        }
        let (action, result) = match snapshot.status {
            OperationStatus::Provisioning => (
                OperationLifecycleAction::Abort,
                registry.abort_provisioning(operation_id, reason),
            ),
            OperationStatus::Running => (
                OperationLifecycleAction::Cancel,
                registry.cancel_operation(operation_id, reason),
            ),
            OperationStatus::Retiring => (
                OperationLifecycleAction::RetireCompleted,
                registry.mark_retired(operation_id),
            ),
            status => {
                return Err(MobError::Internal(format!(
                    "placed provision operation '{operation_id}' has non-terminal cleanup status {status:?}"
                )));
            }
        };
        match result {
            Ok(()) | Err(OpsLifecycleError::NotFound(_)) => {}
            Err(error) => {
                if !Self::invalid_transition_is_idempotent_dyn(registry, action, &error)? {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
        }
        let terminal = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "placed provision operation '{operation_id}' disappeared during Pending cleanup"
                ))
            })?;
        if terminal.kind != OperationKind::MobMemberChild
            || &terminal.owner_session_id != operation_owner_session_id
            || terminal.display_name != display_name
            || terminal.source_label != "mob_member"
            || terminal.operation_source.is_some()
            || terminal.child_session_id.is_some()
            || !terminal.expect_peer_channel
            || !terminal.terminal
            || !matches!(
                terminal.status,
                OperationStatus::Aborted | OperationStatus::Cancelled | OperationStatus::Retired
            )
        {
            return Err(MobError::Internal(format!(
                "placed provision operation '{operation_id}' did not reach an exact compatible Pending-cleanup terminal"
            )));
        }
        Ok(())
    }

    /// Close the exact lifecycle operation after a committed placed member's
    /// host release and durable terminal publication, before its carrier is
    /// deleted.  Missing is success across true restart (nonterminal rows are
    /// not durable); exact Running/Retiring is marked Retired; only exact
    /// Retired is terminal-idempotent.  Other terminals, Provisioning, and
    /// immutable tuple drift fail closed.  No operation is reconstructed or
    /// minted here.
    pub(crate) fn retire_committed_placed_provision_operation_exact(
        registry: &dyn OpsLifecycleRegistry,
        operation_owner_session_id: &SessionId,
        operation_id: &OperationId,
        display_name: &str,
    ) -> Result<(), MobError> {
        let exact_tuple = |snapshot: &OperationLifecycleSnapshot| {
            snapshot.kind == OperationKind::MobMemberChild
                && &snapshot.owner_session_id == operation_owner_session_id
                && snapshot.display_name == display_name
                && snapshot.source_label == "mob_member"
                && snapshot.operation_source.is_none()
                && snapshot.child_session_id.is_none()
                && snapshot.expect_peer_channel
        };
        let Some(snapshot) = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
        else {
            return Ok(());
        };
        if !exact_tuple(&snapshot) {
            return Err(MobError::Internal(format!(
                "committed placed provision operation '{operation_id}' conflicts with its retirement anchor"
            )));
        }
        if snapshot.terminal {
            return if snapshot.status == OperationStatus::Retired {
                Ok(())
            } else {
                Err(MobError::Internal(format!(
                    "committed placed provision operation '{operation_id}' has incompatible terminal retirement status {:?}",
                    snapshot.status
                )))
            };
        }
        if !matches!(
            snapshot.status,
            OperationStatus::Running | OperationStatus::Retiring
        ) {
            return Err(MobError::Internal(format!(
                "committed placed provision operation '{operation_id}' has incompatible retirement status {:?}",
                snapshot.status
            )));
        }
        if let Err(retire_error) = registry.mark_retired(operation_id) {
            let current = registry
                .snapshot(operation_id)
                .map_err(|error| MobError::Internal(error.to_string()))?;
            if !current.as_ref().is_some_and(|snapshot| {
                exact_tuple(snapshot) && snapshot.status == OperationStatus::Retired
            }) {
                return Err(MobError::Internal(retire_error.to_string()));
            }
        }
        let retired = registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "committed placed provision operation '{operation_id}' disappeared while retiring"
                ))
            })?;
        if !exact_tuple(&retired) || retired.status != OperationStatus::Retired {
            return Err(MobError::Internal(format!(
                "committed placed provision operation '{operation_id}' did not reach an exact terminal retirement state"
            )));
        }
        Ok(())
    }

    pub(crate) async fn verify_running_placed_member_operation_exact(
        &self,
        member_ref: &MemberRef,
        operation_owner_session_id: &SessionId,
        display_name: &str,
        operation_id: &OperationId,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "verify exact placed operation for")?;
        let binding = self.require_binding_for_key(&member_key, "verify exact placed operation")?;
        if binding.exact_operation_id.as_ref() != Some(operation_id)
            || &binding.owner_bridge_session_id != operation_owner_session_id
            || binding.display_name.as_deref() != Some(display_name)
        {
            return Err(MobError::Internal(format!(
                "placed member binding does not own exact operation '{operation_id}'"
            )));
        }
        // Also detects another active operation claiming the same returned
        // endpoint.  The exact branch never registers a replacement.
        let resolved = self
            .ensure_operation_for_key(&member_key, display_name)
            .await?;
        if &resolved != operation_id {
            return Err(MobError::Internal(format!(
                "placed member binding resolved operation '{resolved}' but carrier requires '{operation_id}'"
            )));
        }
        Self::verify_committed_placed_provision_operation_exact(
            binding.registry.as_ref(),
            operation_owner_session_id,
            operation_id,
            display_name,
        )
    }

    pub(crate) async fn mark_member_provisioned(
        &self,
        session_id: &SessionId,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let prepared = self
            .prepare_member_provision_operation(session_id, display_name)
            .await?;
        let committed = prepared.commit()?;
        committed.disarm()
    }

    /// Resolve or register the exact child operation while it is still in the
    /// non-serving materialization transaction. This deliberately stops at
    /// `Provisioning`: the transition to `Running` is committed synchronously
    /// at the executor-attachment linearization point.
    pub(crate) async fn prepare_member_provision_operation(
        &self,
        session_id: &SessionId,
        display_name: &str,
    ) -> Result<PreparedMemberProvisionOperation, MobError> {
        let member_key = MemberOpsKey::Session(session_id.clone());
        let binding = self.require_binding_for_key(&member_key, "prepare provision")?;
        let claim_id = uuid::Uuid::new_v4();
        {
            let mut bindings = self
                .member_bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let current = bindings.get_mut(&member_key).ok_or_else(|| {
                MobError::Internal(format!(
                    "mob provision binding for session '{session_id}' disappeared before exact claim"
                ))
            })?;
            if current.binding_id != binding.binding_id {
                return Err(MobError::Internal(format!(
                    "mob provision binding for session '{session_id}' changed before exact claim"
                )));
            }
            if current.provision_claim.is_some() {
                return Err(MobError::Internal(format!(
                    "mob provision binding for session '{session_id}' already has an in-flight exact operation claim"
                )));
            }
            current.provision_claim = Some(claim_id);
        }
        // Arm rollback before registry discovery/registration. Every `?`
        // below now drops an exact binding claim instead of leaking a
        // Provisioning operation or an unowned binding.
        let mut prepared = PreparedMemberProvisionOperation {
            member_bindings: Arc::clone(&self.member_bindings),
            member_key: member_key.clone(),
            binding_id: binding.binding_id,
            claim_id,
            registry: Arc::clone(&binding.registry),
            operation_id: None,
            display_name: display_name.to_string(),
            owns_provisioning: false,
            remove_binding_on_abort: binding.unpublished_attempt,
            armed: true,
        };
        let operation_id = match self
            .ensure_operation_for_key(&member_key, display_name)
            .await
        {
            Ok(operation_id) => operation_id,
            Err(error) => {
                return match prepared.abort("operation preparation failed before registration") {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(MobError::Internal(format!(
                        "{error}; exact operation binding cleanup also failed: {cleanup_error}"
                    ))),
                };
            }
        };
        prepared.operation_id = Some(operation_id.clone());
        let snapshot = match Self::require_operation_for_binding(
            &member_key,
            &binding,
            &operation_id,
            "prepare provision",
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return match prepared.abort("operation snapshot validation failed") {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(MobError::Internal(format!(
                        "{error}; exact operation rollback also failed: {cleanup_error}"
                    ))),
                };
            }
        };
        prepared.owns_provisioning = snapshot.status == OperationStatus::Provisioning;
        if snapshot.display_name != display_name
            || snapshot.terminal
            || !matches!(
                snapshot.status,
                OperationStatus::Provisioning | OperationStatus::Running
            )
        {
            let error = MobError::Internal(format!(
                "mob provision operation '{operation_id}' is not the exact nonterminal Provisioning/Running operation for session '{session_id}'"
            ));
            return match prepared.abort("operation tuple validation failed") {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(MobError::Internal(format!(
                    "{error}; exact operation rollback also failed: {cleanup_error}"
                ))),
            };
        }
        // A Running replay is established durable state, not an operation
        // this attempt may terminalize or remove on failure.
        if snapshot.status == OperationStatus::Running {
            prepared.remove_binding_on_abort = false;
        }
        Ok(prepared)
    }

    pub(crate) async fn mark_member_provisioned_for_member(
        &self,
        member_ref: &MemberRef,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let member_key = Self::require_member_key(member_ref, "mark provisioned for")?;
        self.mark_member_provisioned_for_key(&member_key, display_name)
            .await
    }

    pub(crate) async fn mark_member_provisioned_for_member_exact(
        &self,
        member_ref: &MemberRef,
        display_name: &str,
        operation_id: &OperationId,
    ) -> Result<OperationId, MobError> {
        let member_key = Self::require_member_key(member_ref, "mark exact provisioned for")?;
        let binding = self.require_binding_for_key(&member_key, "mark exact provisioned")?;
        if binding.exact_operation_id.as_ref() != Some(operation_id) {
            return Err(MobError::Internal(format!(
                "mob ops adapter exact placed binding does not own operation '{operation_id}'"
            )));
        }
        let resolved = self
            .mark_member_provisioned_for_key(&member_key, display_name)
            .await?;
        if &resolved != operation_id {
            return Err(MobError::Internal(format!(
                "mob ops adapter resolved operation '{resolved}' but placed carrier requires '{operation_id}'"
            )));
        }
        Ok(resolved)
    }

    async fn mark_member_provisioned_for_key(
        &self,
        member_key: &MemberOpsKey,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let operation_id = self
            .ensure_operation_for_key(member_key, display_name)
            .await?;
        let registry = self.registry_for_existing_binding(member_key, "mark provisioned")?;
        match registry.provisioning_succeeded(&operation_id) {
            Ok(()) => {}
            Err(error) => {
                if !Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::Start,
                    &error,
                )? {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
        }
        Ok(operation_id)
    }

    pub(crate) async fn report_member_progress(
        &self,
        member_ref: &MemberRef,
        message: impl Into<String>,
    ) -> Result<(), MobError> {
        tracing::debug!(
            member_ref = ?member_ref,
            "MobOpsAdapter::report_member_progress resolving member key"
        );
        let member_key = Self::require_member_key(member_ref, "report progress for")?;
        tracing::debug!(
            member_key = ?member_key,
            "MobOpsAdapter::report_member_progress resolving display name"
        );
        let display_name = self.display_name_for_key(&member_key, "report progress")?;
        tracing::debug!(
            member_key = ?member_key,
            display_name,
            "MobOpsAdapter::report_member_progress resolving active operation"
        );
        let operation_id = self
            .resolve_or_register_active_operation_id_for_key(
                &member_key,
                "report progress",
                &display_name,
            )
            .await?;
        tracing::debug!(
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress resolved active operation"
        );
        self.ensure_running_before_live_update(&member_key, &operation_id, "report progress")?;
        tracing::debug!(
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress operation running"
        );
        let registry = self.registry_for_existing_binding(&member_key, "report progress")?;
        tracing::debug!(
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress applying progress"
        );
        match registry.report_progress(
            &operation_id,
            OperationProgressUpdate {
                message: message.into(),
                percent: None,
            },
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::ProgressReported,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        }?;
        tracing::debug!(
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress applied progress"
        );
        Ok(())
    }

    pub(crate) fn report_member_progress_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        message: impl Into<String>,
    ) -> Result<(), MobError> {
        tracing::debug!(
            member_ref = ?member_ref,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress_for_operation resolving member key"
        );
        let member_key = Self::require_member_key(member_ref, "report progress for")?;
        let registry = self.registry_for_existing_binding(&member_key, "report progress")?;
        tracing::debug!(
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress_for_operation applying progress"
        );
        match registry.report_progress(
            operation_id,
            OperationProgressUpdate {
                message: message.into(),
                percent: None,
            },
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::ProgressReported,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        }?;
        tracing::debug!(
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::report_member_progress_for_operation applied progress"
        );
        Ok(())
    }

    pub(crate) async fn mark_member_peer_ready(
        &self,
        member_ref: &MemberRef,
        peer_name: &str,
        trusted_peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        tracing::debug!(
            peer_name,
            member_ref = ?member_ref,
            "MobOpsAdapter::mark_member_peer_ready resolving member key"
        );
        let member_key = Self::require_member_key(member_ref, "mark peer ready for")?;
        tracing::debug!(
            peer_name,
            member_key = ?member_key,
            "MobOpsAdapter::mark_member_peer_ready resolving active operation"
        );
        let operation_id = self
            .resolve_or_register_active_operation_id_for_key(
                &member_key,
                "mark peer ready",
                peer_name,
            )
            .await?;
        tracing::debug!(
            peer_name,
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::mark_member_peer_ready resolved active operation"
        );
        self.ensure_running_before_live_update(&member_key, &operation_id, "mark peer ready")?;
        tracing::debug!(
            peer_name,
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::mark_member_peer_ready operation running"
        );
        let registry = self.registry_for_existing_binding(&member_key, "mark peer ready")?;
        tracing::debug!(
            peer_name,
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::mark_member_peer_ready applying peer ready"
        );
        let requested_peer_handle = OperationPeerHandle {
            peer_name: meerkat_core::comms::PeerName::new(peer_name)
                .map_err(|e| MobError::Internal(format!("invalid peer name: {e}")))?,
            trusted_peer,
        };
        match registry.peer_ready(&operation_id, requested_peer_handle.clone()) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::AlreadyPeerReady(_)) => {
                Self::require_exact_existing_peer_handle(
                    &registry,
                    &operation_id,
                    &requested_peer_handle,
                )
            }
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::PeerReady,
                    &error,
                )? {
                    Self::require_exact_existing_peer_handle(
                        &registry,
                        &operation_id,
                        &requested_peer_handle,
                    )
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        }?;
        tracing::debug!(
            peer_name,
            member_key = ?member_key,
            operation_id = %operation_id,
            "MobOpsAdapter::mark_member_peer_ready applied peer ready"
        );
        Ok(())
    }

    pub(crate) fn mark_member_peer_ready_for_operation(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        peer_name: &str,
        trusted_peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        tracing::debug!(
            peer_name,
            operation_id = %operation_id,
            member_ref = ?member_ref,
            "MobOpsAdapter::mark_member_peer_ready_for_operation resolving member key"
        );
        let member_key = Self::require_member_key(member_ref, "mark peer ready for")?;
        tracing::debug!(
            peer_name,
            operation_id = %operation_id,
            member_key = ?member_key,
            "MobOpsAdapter::mark_member_peer_ready_for_operation getting registry"
        );
        let registry = self.registry_for_existing_binding(&member_key, "mark peer ready")?;
        tracing::debug!(
            peer_name,
            operation_id = %operation_id,
            member_key = ?member_key,
            "MobOpsAdapter::mark_member_peer_ready_for_operation applying peer ready"
        );
        let requested_peer_handle = OperationPeerHandle {
            peer_name: meerkat_core::comms::PeerName::new(peer_name)
                .map_err(|e| MobError::Internal(format!("invalid peer name: {e}")))?,
            trusted_peer,
        };
        match registry.peer_ready(operation_id, requested_peer_handle.clone()) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::AlreadyPeerReady(_)) => {
                Self::require_exact_existing_peer_handle(
                    &registry,
                    operation_id,
                    &requested_peer_handle,
                )
            }
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::PeerReady,
                    &error,
                )? {
                    Self::require_exact_existing_peer_handle(
                        &registry,
                        operation_id,
                        &requested_peer_handle,
                    )
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        }?;
        tracing::debug!(
            peer_name,
            operation_id = %operation_id,
            member_key = ?member_key,
            "MobOpsAdapter::mark_member_peer_ready_for_operation applied peer ready"
        );
        Ok(())
    }

    pub(crate) async fn mark_member_retired(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "mark retired for")?;
        let binding = self.require_binding_for_key(&member_key, "mark retired")?;
        let snapshots = Self::matching_operations_for_binding(&member_key, &binding)?;
        let active_ids = Self::active_operation_ids(&snapshots);
        let operation_id = match active_ids.len() {
            1 => active_ids[0].clone(),
            0 => {
                if let Some(latest) = Self::newest_operation_snapshot(&snapshots)
                    && Self::snapshot_is_terminal(latest)
                {
                    return Ok(());
                }
                return Ok(());
            }
            _ => {
                return Err(MobError::Internal(format!(
                    "cannot retire member '{member_key:?}': multiple active mob child operations [{}]",
                    Self::format_operation_ids(&active_ids)
                )));
            }
        };
        let registry = binding.registry;
        match registry.request_retire(&operation_id) {
            Ok(()) => {}
            Err(error) => {
                if !Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::RetireRequested,
                    &error,
                )? {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
        }
        let result = match registry.mark_retired(&operation_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::RetireCompleted,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        };
        if result.is_ok() {
            self.clear_member_binding(&member_key)?;
        }
        result
    }

    /// Complete the ops-lifecycle half of an archive-confirmed retirement.
    ///
    /// A missing binding remains fail-closed for a fresh archive. It is
    /// idempotent only when the typed disposal authority proves the durable
    /// archive already held: in that case an absent binding is the exact
    /// terminal-publication retry state produced after the prior ops
    /// retirement succeeded and cleared its binding.
    pub(crate) async fn mark_member_retired_after_disposal(
        &self,
        member_ref: &MemberRef,
        disposal: super::provisioner::MemberSessionDisposalVerdict,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "mark retired for")?;
        if disposal == super::provisioner::MemberSessionDisposalVerdict::AlreadyArchived
            && self.binding_for_key(&member_key).is_none()
        {
            tracing::debug!(
                member_key = ?member_key,
                "archive and mob-child retirement already terminal; continuing final publication retry"
            );
            return Ok(());
        }
        self.mark_member_retired(member_ref).await
    }

    pub(crate) async fn abort_member_provision(
        &self,
        session_id: &SessionId,
        operation_id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        self.abort_member_provision_for_key(
            &MemberOpsKey::Session(session_id.clone()),
            operation_id,
            reason,
        )
        .await
    }

    pub(crate) async fn abort_member_provision_for_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "abort provision for")?;
        self.abort_member_provision_for_key(&member_key, operation_id, reason)
            .await
    }

    async fn abort_member_provision_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation_id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        let binding = self.require_binding_for_key(member_key, "abort provision")?;
        let registry = Arc::clone(&binding.registry);
        Self::require_operation_for_binding(member_key, &binding, operation_id, "abort provision")?;
        let result = match registry.abort_provisioning(operation_id, reason) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::NotFound(_)) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::Abort,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        };
        if result.is_ok() {
            self.clear_member_binding(member_key)?;
        }
        result
    }

    fn ensure_running_before_live_update(
        &self,
        member_key: &MemberOpsKey,
        operation_id: &OperationId,
        operation: &str,
    ) -> Result<(), MobError> {
        let binding = self.require_binding_for_key(member_key, operation)?;
        let registry = Arc::clone(&binding.registry);
        let snapshot =
            Self::require_operation_for_binding(member_key, &binding, operation_id, operation)?;
        if snapshot.status != OperationStatus::Provisioning {
            return Ok(());
        }

        match registry.provisioning_succeeded(operation_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::Start,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(format!(
                        "mob ops adapter cannot {operation}: failed to canonicalize provisioning operation '{operation_id}' into running state: {error}",
                    )))
                }
            }
        }
    }
}

#[cfg(feature = "runtime-adapter")]
#[allow(clippy::too_many_arguments)]
fn rollback_exact_provision_operation(
    member_bindings: &Arc<Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>>,
    member_key: &MemberOpsKey,
    binding_id: uuid::Uuid,
    claim_id: uuid::Uuid,
    registry: &Arc<dyn OpsLifecycleRegistry>,
    operation_id: &OperationId,
    owns_provisioning: bool,
    remove_binding_on_abort: bool,
    reason: &str,
) -> Result<(), MobError> {
    if !exact_claim_still_bound(member_bindings, member_key, binding_id, claim_id) {
        return Err(MobError::Internal(format!(
            "operation rollback for '{operation_id}' lost its exact binding claim"
        )));
    }
    let snapshot = registry
        .snapshot(operation_id)
        .map_err(|error| MobError::Internal(error.to_string()))?;
    if let Some(snapshot) = snapshot
        && !snapshot.terminal
        && (owns_provisioning
            || (remove_binding_on_abort && snapshot.status == OperationStatus::Provisioning))
    {
        match snapshot.status {
            OperationStatus::Provisioning => {
                if let Err(error) =
                    registry.abort_provisioning(operation_id, Some(reason.to_string()))
                    && !MobOpsAdapter::invalid_transition_is_idempotent(
                        registry,
                        OperationLifecycleAction::Abort,
                        &error,
                    )?
                {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
            OperationStatus::Running | OperationStatus::Retiring => {
                if let Err(error) = registry.request_retire(operation_id)
                    && !MobOpsAdapter::invalid_transition_is_idempotent(
                        registry,
                        OperationLifecycleAction::RetireRequested,
                        &error,
                    )?
                {
                    return Err(MobError::Internal(error.to_string()));
                }
                if let Err(error) = registry.mark_retired(operation_id)
                    && !MobOpsAdapter::invalid_transition_is_idempotent(
                        registry,
                        OperationLifecycleAction::RetireCompleted,
                        &error,
                    )?
                {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
            _ => {
                return Err(MobError::Internal(format!(
                    "operation '{operation_id}' has incompatible rollback status {:?}",
                    snapshot.status
                )));
            }
        }
    }
    if !clear_exact_provision_claim(
        member_bindings,
        member_key,
        binding_id,
        claim_id,
        remove_binding_on_abort,
    ) {
        return Err(MobError::Internal(format!(
            "operation rollback for '{operation_id}' lost its exact binding claim during publication"
        )));
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
impl PreparedMemberProvisionOperation {
    pub(crate) fn operation_id(&self) -> &OperationId {
        self.operation_id.as_ref().unwrap_or_else(|| {
            unreachable!("published prepared operation is missing its operation id")
        })
    }

    pub(crate) fn commit(mut self) -> Result<CommittedMemberProvisionOperation, MobError> {
        let operation_id = self.operation_id.clone().ok_or_else(|| {
            MobError::Internal("prepared operation has no exact operation id".to_string())
        })?;
        if !exact_claim_still_bound(
            &self.member_bindings,
            &self.member_key,
            self.binding_id,
            self.claim_id,
        ) {
            return Err(MobError::Internal(format!(
                "prepared operation '{operation_id}' lost its exact binding claim before commit"
            )));
        }
        let binding = self
            .member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&self.member_key)
            .cloned()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "prepared operation '{operation_id}' lost its binding before commit"
                ))
            })?;
        let snapshot = MobOpsAdapter::require_operation_for_binding(
            &self.member_key,
            &binding,
            &operation_id,
            "commit prepared provision",
        )?;
        if snapshot.display_name != self.display_name || snapshot.terminal {
            return Err(MobError::Internal(format!(
                "prepared operation '{operation_id}' lost its exact display/terminal tuple"
            )));
        }
        match snapshot.status {
            OperationStatus::Provisioning => {
                if let Err(error) = self.registry.provisioning_succeeded(&operation_id)
                    && !MobOpsAdapter::invalid_transition_is_idempotent(
                        &self.registry,
                        OperationLifecycleAction::Start,
                        &error,
                    )?
                {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
            OperationStatus::Running => {}
            status => {
                return Err(MobError::Internal(format!(
                    "prepared operation '{operation_id}' has incompatible commit status {status:?}"
                )));
            }
        }
        self.armed = false;
        Ok(CommittedMemberProvisionOperation {
            member_bindings: Arc::clone(&self.member_bindings),
            member_key: self.member_key.clone(),
            binding_id: self.binding_id,
            claim_id: self.claim_id,
            registry: Arc::clone(&self.registry),
            operation_id,
            owns_provisioning: self.owns_provisioning,
            armed: true,
        })
    }

    pub(crate) fn abort(mut self, reason: &str) -> Result<(), MobError> {
        let result = match self.operation_id.as_ref() {
            Some(operation_id) => rollback_exact_provision_operation(
                &self.member_bindings,
                &self.member_key,
                self.binding_id,
                self.claim_id,
                &self.registry,
                operation_id,
                self.owns_provisioning,
                self.remove_binding_on_abort,
                reason,
            ),
            None => {
                if clear_exact_provision_claim(
                    &self.member_bindings,
                    &self.member_key,
                    self.binding_id,
                    self.claim_id,
                    self.remove_binding_on_abort,
                ) {
                    Ok(())
                } else {
                    Err(MobError::Internal(
                        "prepared operation lost its exact binding claim before abort".to_string(),
                    ))
                }
            }
        };
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for PreparedMemberProvisionOperation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let result = match self.operation_id.as_ref() {
            Some(operation_id) => rollback_exact_provision_operation(
                &self.member_bindings,
                &self.member_key,
                self.binding_id,
                self.claim_id,
                &self.registry,
                operation_id,
                self.owns_provisioning,
                self.remove_binding_on_abort,
                "prepared member provision was cancelled",
            ),
            None => {
                if clear_exact_provision_claim(
                    &self.member_bindings,
                    &self.member_key,
                    self.binding_id,
                    self.claim_id,
                    self.remove_binding_on_abort,
                ) {
                    Ok(())
                } else {
                    Err(MobError::Internal(
                        "prepared operation lost its exact binding claim during drop".to_string(),
                    ))
                }
            }
        };
        if let Err(error) = result {
            tracing::error!(operation_id = ?self.operation_id, %error, "drop-time prepared operation rollback failed");
        }
        self.armed = false;
    }
}

#[cfg(feature = "runtime-adapter")]
impl CommittedMemberProvisionOperation {
    pub(crate) fn disarm(mut self) -> Result<OperationId, MobError> {
        if !publish_exact_provision_claim(
            &self.member_bindings,
            &self.member_key,
            self.binding_id,
            self.claim_id,
        ) {
            return Err(MobError::Internal(format!(
                "committed operation '{}' lost its exact binding claim before receipt publication",
                self.operation_id
            )));
        }
        self.armed = false;
        Ok(self.operation_id.clone())
    }

    pub(crate) fn abort(mut self, reason: &str) -> Result<(), MobError> {
        let result = rollback_exact_provision_operation(
            &self.member_bindings,
            &self.member_key,
            self.binding_id,
            self.claim_id,
            &self.registry,
            &self.operation_id,
            self.owns_provisioning,
            self.owns_provisioning,
            reason,
        );
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

#[cfg(feature = "runtime-adapter")]
impl Drop for CommittedMemberProvisionOperation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = rollback_exact_provision_operation(
            &self.member_bindings,
            &self.member_key,
            self.binding_id,
            self.claim_id,
            &self.registry,
            &self.operation_id,
            self.owns_provisioning,
            self.owns_provisioning,
            "committed member provision was not published",
        ) {
            tracing::error!(operation_id = %self.operation_id, %error, "drop-time committed operation rollback failed");
        }
        self.armed = false;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::{PeerAddress, PeerId, TrustedPeerDescriptor};
    use meerkat_core::ops_lifecycle::OperationStatus;
    use meerkat_runtime::RuntimeOpsLifecycleRegistry;

    #[tokio::test]
    async fn session_registry_binding_rejects_incarnation_overwrite() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let owner_session_id = SessionId::new();
        let first = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let replacement = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_session_id.clone(),
                Arc::clone(&first) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind first registry incarnation");

        let error = adapter
            .bind_session_registry(
                session_id.clone(),
                owner_session_id,
                Arc::clone(&replacement) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect_err("different registry must not overwrite the exact binding");
        assert!(error.to_string().contains("different operation-registry"));

        let operation_id = adapter
            .mark_member_provisioned(&session_id, "mob/member/exact-binding")
            .await
            .expect("original binding remains usable");
        assert!(first.snapshot(&operation_id).unwrap().is_some());
        assert!(replacement.snapshot(&operation_id).unwrap().is_none());
    }

    #[test]
    fn explicit_resume_ops_cleanup_cannot_remove_replacement_binding() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let owner_session_id = SessionId::new();
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_session_id.clone(),
                Arc::new(RuntimeOpsLifecycleRegistry::new()) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind old registry incarnation");
        let old = adapter
            .capture_session_binding_witness(&session_id)
            .expect("capture old binding witness");
        adapter
            .clear_session_binding_for_explicit_resume(&session_id, Some(old))
            .expect("clear exact old binding");

        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_session_id,
                Arc::new(RuntimeOpsLifecycleRegistry::new()) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind replacement registry incarnation");
        let replacement = adapter
            .capture_session_binding_witness(&session_id)
            .expect("capture replacement binding witness");
        assert_ne!(replacement, old);

        let error = adapter
            .clear_session_binding_for_explicit_resume(&session_id, Some(old))
            .expect_err("stale explicit-resume cleanup must not clear replacement binding");
        assert!(error.to_string().contains("changed during explicit-resume"));
        assert_eq!(
            adapter.capture_session_binding_witness(&session_id),
            Some(replacement),
            "replacement binding remains installed"
        );
    }

    #[tokio::test]
    async fn dropping_prepared_operation_aborts_only_its_exact_attempt() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let owner_session_id = SessionId::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_session_id.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind prepared attempt");
        let prepared = adapter
            .prepare_member_provision_operation(&session_id, "mob/member/cancelled")
            .await
            .expect("prepare exact operation");
        let operation_id = prepared.operation_id().clone();

        drop(prepared);

        let snapshot = registry
            .snapshot(&operation_id)
            .unwrap()
            .expect("cancelled operation remains as terminal history");
        assert!(snapshot.terminal);
        let replacement = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id,
                owner_session_id,
                replacement as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("exact cancelled binding was removed");
    }

    #[tokio::test]
    async fn dropping_committed_operation_before_receipt_terminalizes_it() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let owner_session_id = SessionId::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind committed attempt");
        let prepared = adapter
            .prepare_member_provision_operation(&session_id, "mob/member/unpublished")
            .await
            .expect("prepare exact operation");
        let operation_id = prepared.operation_id().clone();
        let committed = prepared.commit().expect("commit operation to running");

        drop(committed);

        let snapshot = registry
            .snapshot(&operation_id)
            .unwrap()
            .expect("unpublished operation remains as terminal history");
        assert!(snapshot.terminal);
    }

    #[tokio::test]
    async fn ops_registry_integration_red_ok_member_adapter_tracks_peer_ready_and_retire() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");

        let operation_id = adapter
            .mark_member_provisioned(&session_id, "mob-a/orchestrator/member-alpha")
            .await
            .expect("register member");
        adapter
            .report_member_progress(&member_ref, "turn dispatched")
            .await
            .expect("progress");
        adapter
            .mark_member_peer_ready(
                &member_ref,
                "mob-a/orchestrator/member-alpha",
                TrustedPeerDescriptor::test_only_unsigned(
                    "mob-a/orchestrator/member-alpha",
                    "00000000-0000-4000-8000-000000000002",
                    "inproc://member-alpha",
                )
                .expect("trusted peer"),
            )
            .await
            .expect("peer ready");

        let running_snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(running_snapshot.status, OperationStatus::Running);
        assert!(running_snapshot.peer_ready);
        assert_eq!(running_snapshot.progress_count, 1);

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("retire member");

        let retired_snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(retired_snapshot.status, OperationStatus::Retired);
    }

    #[tokio::test]
    async fn operation_peer_ready_retry_requires_the_exact_existing_handle() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");
        let operation_id = adapter
            .mark_member_provisioned(&session_id, "mob-a/orchestrator/member-alpha")
            .await
            .expect("register member");
        let peer_name = "mob-a/orchestrator/member-alpha";
        let exact_peer = TrustedPeerDescriptor::test_only_unsigned(
            peer_name,
            "00000000-0000-4000-8000-000000000002",
            "inproc://member-alpha",
        )
        .expect("exact trusted peer");

        adapter
            .mark_member_peer_ready_for_operation(
                &member_ref,
                &operation_id,
                peer_name,
                exact_peer.clone(),
            )
            .expect("first peer-ready publication");
        adapter
            .mark_member_peer_ready_for_operation(
                &member_ref,
                &operation_id,
                peer_name,
                exact_peer.clone(),
            )
            .expect("same exact peer handle is idempotent");
        adapter
            .mark_member_peer_ready(&member_ref, peer_name, exact_peer.clone())
            .await
            .expect("resolved-operation path also accepts the exact existing handle");

        let different_peer = TrustedPeerDescriptor::test_only_unsigned(
            peer_name,
            "00000000-0000-4000-8000-000000000003",
            "inproc://member-alpha-rebound",
        )
        .expect("different trusted peer");
        let resolved_error = adapter
            .mark_member_peer_ready(&member_ref, peer_name, different_peer.clone())
            .await
            .expect_err("resolved-operation path must reject a different peer handle");
        assert!(
            resolved_error
                .to_string()
                .contains("already peer-ready with a different handle"),
            "unexpected resolved-operation mismatch error: {resolved_error}"
        );
        let error = adapter
            .mark_member_peer_ready_for_operation(
                &member_ref,
                &operation_id,
                peer_name,
                different_peer,
            )
            .expect_err("different peer handle must fail closed");
        assert!(
            error
                .to_string()
                .contains("already peer-ready with a different handle"),
            "unexpected mismatch error: {error}"
        );

        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(
            snapshot.peer_handle,
            Some(OperationPeerHandle {
                peer_name: meerkat_core::comms::PeerName::new(peer_name)
                    .expect("canonical peer name"),
                trusted_peer: exact_peer,
            }),
            "failed retry must not replace the operation's exact peer handle"
        );
    }

    #[tokio::test]
    async fn bound_session_registry_owns_child_operation_ids() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let bound_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                child_session_id.clone(),
                owner_bridge_session_id,
                Arc::clone(&bound_registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");

        let operation_id = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-bound")
            .await
            .expect("register bound child op");

        let bound_snapshot = bound_registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("bound registry should own exported child operation ids");
        assert_eq!(bound_snapshot.kind, OperationKind::MobMemberChild);
        assert_eq!(bound_snapshot.status, OperationStatus::Running);
        assert_eq!(bound_snapshot.child_session_id, Some(child_session_id));
    }

    #[tokio::test]
    async fn repeated_member_provision_uses_generated_idempotence_classifier() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                child_session_id.clone(),
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");

        let first = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-idempotent")
            .await
            .expect("first provision");
        let second = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-idempotent")
            .await
            .expect("generated idempotent start classification");

        assert_eq!(first, second);
        let snapshot = registry
            .snapshot(&first)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Running);
    }

    #[tokio::test]
    async fn already_retiring_member_retire_uses_generated_idempotence_classifier() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(child_session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                child_session_id.clone(),
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");

        let operation_id = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-retiring")
            .await
            .expect("provision member");
        registry
            .request_retire(&operation_id)
            .expect("generated retire request");

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("generated idempotent retire-request classification");

        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Retired);
    }

    #[tokio::test]
    async fn mark_member_retired_without_existing_operation_is_noop() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id,
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("missing lifecycle entry should retire as no-op");

        assert!(
            registry.list_operations().unwrap().is_empty(),
            "retire must not fabricate a provisioning operation"
        );
    }

    #[tokio::test]
    async fn member_operation_registration_without_owner_binding_fails_closed() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let err = adapter
            .mark_member_provisioned(&session_id, "mob/member-unbound")
            .await
            .expect_err("missing generated owner binding must fail closed");
        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );

        let member_ref = MemberRef::BackendPeer {
            peer_id: "peer-a".into(),
            address: "inproc://peer-a".into(),
            pubkey: [7u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let err = adapter
            .mark_member_provisioned_for_member(&member_ref, "mob/backend-peer")
            .await
            .expect_err("peer-only member without generated owner binding must fail closed");
        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );
        assert!(
            adapter
                .active_operation_id_for_member(&member_ref)
                .await
                .is_none(),
            "unbound peer-only member must not create fallback operation state"
        );
    }

    #[tokio::test]
    async fn peer_only_operation_identity_uses_generated_operation_source() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let peer_id = PeerId::new();
        let address = PeerAddress::parse("inproc://peer-a").expect("peer address");
        let operation_source = OperationSource::backend_peer(peer_id, address.clone());
        let member_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            pubkey: [7u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        adapter
            .bind_member_registry(
                &member_ref,
                owner_bridge_session_id.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "display-before",
                operation_source.clone(),
            )
            .expect("bind peer-only member registry");

        let operation_id = adapter
            .mark_member_provisioned_for_member(&member_ref, "display-before")
            .await
            .expect("register peer-only child operation");
        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.display_name, "display-before");
        assert_eq!(snapshot.operation_source, Some(operation_source.clone()));

        let relabel_error = adapter
            .bind_member_registry(
                &member_ref,
                owner_bridge_session_id.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "display-after",
                operation_source.clone(),
            )
            .expect_err("ordinary replay must not relabel a generic lifecycle operation");
        assert!(
            relabel_error.to_string().contains("different"),
            "unexpected relabel rejection: {relabel_error}"
        );

        assert_eq!(
            adapter.active_operation_id_for_member(&member_ref).await,
            Some(operation_id.clone())
        );

        // A byte-identical tuple may refresh the registry capability after
        // owner reconstruction. Seed that registry with the same canonical
        // operation, replay the binding, and prove subsequent progress reaches
        // only the replacement capability.
        let replacement_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        replacement_registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: owner_bridge_session_id.clone(),
                display_name: "display-before".to_string(),
                source_label: "mob_member".to_string(),
                operation_source: Some(operation_source.clone()),
                child_session_id: None,
                expect_peer_channel: true,
            })
            .expect("seed replacement registry");
        replacement_registry
            .provisioning_succeeded(&operation_id)
            .expect("start replacement operation");
        adapter
            .bind_member_registry(
                &member_ref,
                owner_bridge_session_id,
                Arc::clone(&replacement_registry) as Arc<dyn OpsLifecycleRegistry>,
                "display-before",
                operation_source,
            )
            .expect("exact generic replay refreshes registry capability");
        adapter
            .report_member_progress(&member_ref, "after generic registry refresh")
            .await
            .expect("progress uses refreshed registry capability");
        assert_eq!(
            registry
                .snapshot(&operation_id)
                .unwrap()
                .unwrap()
                .progress_count,
            0
        );
        assert_eq!(
            replacement_registry
                .snapshot(&operation_id)
                .unwrap()
                .unwrap()
                .progress_count,
            1
        );
    }

    #[tokio::test]
    async fn mark_member_retired_without_owner_binding_fails_closed() {
        let adapter = MobOpsAdapter::new();
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());

        let err = adapter
            .mark_member_retired(&member_ref)
            .await
            .expect_err("missing generated owner binding must fail closed");

        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn abort_member_provision_marks_provisioning_operation_aborted() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let operation_id = OperationId::new();
        let operation_source = OperationSource::session_child(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter
            .bind_session_registry(
                session_id.clone(),
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
            )
            .expect("bind session registry");
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: session_id.clone(),
                display_name: "mob/member-abort".into(),
                source_label: "mob_member".into(),
                operation_source: Some(operation_source),
                child_session_id: Some(session_id.clone()),
                expect_peer_channel: true,
            })
            .expect("register provisioning operation");

        adapter
            .abort_member_provision(&session_id, &operation_id, Some("mob is stopping".into()))
            .await
            .expect("abort provisioning should succeed");

        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Aborted);
    }

    #[tokio::test]
    async fn abort_member_provision_without_owner_binding_fails_closed() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let operation_id = OperationId::new();

        let err = adapter
            .abort_member_provision(&session_id, &operation_id, Some("mob is stopping".into()))
            .await
            .expect_err("missing generated owner binding must fail closed");

        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn placed_pending_anchor_registration_is_exact_and_idempotent() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/placed";

        MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner.clone(),
            operation_id.clone(),
            display_name,
        )
        .expect("register exact Pending anchor");
        MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner.clone(),
            operation_id.clone(),
            display_name,
        )
        .expect("same exact Pending anchor is idempotent");

        let conflicting_owner = MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            SessionId::new(),
            operation_id.clone(),
            display_name,
        )
        .expect_err("owner drift must fail closed");
        assert!(conflicting_owner.to_string().contains("conflicts"));
        let conflicting_display = MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner,
            operation_id,
            "mob/profile/different",
        )
        .expect_err("spec/display drift must fail closed");
        assert!(conflicting_display.to_string().contains("conflicts"));
    }

    #[test]
    fn committed_placed_recovery_reconstructs_same_id_and_reuses_running_anchor() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/revived";

        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("missing nonterminal row is reconstructed at the carrier id");
        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("committed revival accepts the existing Running anchor");

        let snapshots = registry.list_operations().expect("list operations");
        assert_eq!(snapshots.len(), 1, "recovery must never mint another id");
        assert_eq!(snapshots[0].id, operation_id);
        assert_eq!(snapshots[0].status, OperationStatus::Running);
        assert_eq!(snapshots[0].owner_session_id, owner);
    }

    #[test]
    fn committed_placed_recovery_promotes_exact_provisioning_but_rejects_terminal() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/recovery";
        MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner.clone(),
            operation_id.clone(),
            display_name,
        )
        .expect("register exact Provisioning row");

        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("exact Provisioning row is promoted");
        registry
            .cancel_operation(&operation_id, Some("test terminal".to_string()))
            .expect("terminalize exact operation");

        let error = MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect_err("terminal committed operation must not be resurrected");
        assert!(error.to_string().contains("conflicts"));
    }

    #[test]
    fn retiring_recovery_accepts_exact_retiring_but_active_revival_rejects_it() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/retiring-recovery";
        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("seed exact Running operation");
        registry
            .request_retire(&operation_id)
            .expect("enter exact Retiring state");

        MobOpsAdapter::ensure_committed_placed_provision_operation_exact_for_recovery(
            &registry,
            &owner,
            &operation_id,
            display_name,
            crate::runtime::provisioner::PlacedOperationRecoveryExpectation::Retiring,
        )
        .expect("retirement recovery accepts exact Retiring");
        let active = MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect_err("normal active revival must reject Retiring");
        assert!(active.to_string().contains("incompatible recovery status"));
    }

    #[test]
    fn pending_placed_cleanup_is_absence_idempotent_and_terminalizes_exact_running() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/pending-cleanup";

        MobOpsAdapter::abort_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
            Some("absent".to_string()),
        )
        .expect("missing Pending operation is cleanup-idempotent");
        MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner.clone(),
            operation_id.clone(),
            display_name,
        )
        .expect("register exact Pending operation");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("simulate lost completion after operation reached Running");
        MobOpsAdapter::abort_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
            Some("pending carrier cleanup".to_string()),
        )
        .expect("Running Pending anchor is cancelled exactly");
        MobOpsAdapter::abort_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
            Some("lost cleanup acknowledgement".to_string()),
        )
        .expect("exact Cancelled Pending cleanup is idempotent");

        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("exact operation remains as terminal history");
        assert_eq!(snapshot.status, OperationStatus::Cancelled);
        assert!(snapshot.terminal);
    }

    #[test]
    fn pending_placed_provisioning_abort_is_exactly_idempotent() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/pending-abort";
        MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner.clone(),
            operation_id.clone(),
            display_name,
        )
        .expect("register exact Provisioning Pending anchor");

        let display_conflict = MobOpsAdapter::abort_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            "mob/profile/wrong-member",
            Some("must not abort another member".to_string()),
        )
        .expect_err("display drift must fail before terminalization");
        assert!(display_conflict.to_string().contains("conflicts"));

        for reason in ["first abort", "lost abort acknowledgement"] {
            MobOpsAdapter::abort_placed_provision_operation_exact(
                &registry,
                &owner,
                &operation_id,
                display_name,
                Some(reason.to_string()),
            )
            .expect("exact Aborted Pending cleanup is idempotent");
        }
        let snapshot = registry
            .snapshot(&operation_id)
            .unwrap()
            .expect("terminal history");
        assert_eq!(snapshot.status, OperationStatus::Aborted);
    }

    #[test]
    fn committed_placed_retirement_is_exact_restart_idempotent_and_never_mints() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/retiring";

        MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("missing nonterminal row after restart is already quiescent");
        assert!(registry.list_operations().unwrap().is_empty());

        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("seed exact Running committed operation");
        MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("Running operation retires exactly");
        MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect("lost retirement acknowledgement is terminal-idempotent");

        let snapshots = registry.list_operations().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, operation_id);
        assert_eq!(snapshots[0].status, OperationStatus::Retired);
        assert!(snapshots[0].terminal);
    }

    #[test]
    fn committed_placed_retirement_rejects_provisioning_and_tuple_conflict() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/retirement-conflict";
        MobOpsAdapter::register_placed_provision_operation_exact(
            &registry,
            owner.clone(),
            operation_id.clone(),
            display_name,
        )
        .expect("seed exact Provisioning row");

        let provisioning = MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect_err("Committed cleanup must not hide a never-started operation");
        assert!(provisioning.to_string().contains("Provisioning"));
        let conflict = MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            &registry,
            &SessionId::new(),
            &operation_id,
            display_name,
        )
        .expect_err("owner drift must fail closed");
        assert!(conflict.to_string().contains("conflicts"));

        MobOpsAdapter::abort_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
            Some("divergent abort".to_string()),
        )
        .expect("terminalize Pending anchor as Aborted");
        let divergent_terminal = MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            &registry,
            &owner,
            &operation_id,
            display_name,
        )
        .expect_err("Committed cleanup must reject an Aborted lifecycle");
        assert!(
            divergent_terminal
                .to_string()
                .contains("terminal retirement status")
        );
    }

    #[tokio::test]
    async fn exact_placed_binding_never_mints_from_endpoint_source() {
        let adapter = MobOpsAdapter::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let peer_id = PeerId::new();
        let address = PeerAddress::parse("inproc://exact-placed").expect("peer address");
        let member_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            pubkey: [9u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        adapter
            .bind_member_registry_for_exact_operation(
                &member_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/exact-placed",
                OperationSource::backend_peer(peer_id, address),
                operation_id.clone(),
            )
            .expect("bind exact operation id");

        let error = adapter
            .verify_running_placed_member_operation_exact(
                &member_ref,
                &owner,
                "mob/profile/exact-placed",
                &operation_id,
            )
            .await
            .expect_err("missing exact id must not be replaced from endpoint source");
        assert!(error.to_string().contains("not registered"));
        assert!(registry.list_operations().unwrap().is_empty());
    }

    #[tokio::test]
    async fn exact_placed_bind_rejects_two_running_operations_for_one_peer_identity() {
        let adapter = MobOpsAdapter::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let owner = SessionId::new();
        let first_id = OperationId::new();
        let second_id = OperationId::new();
        let peer_id = PeerId::new();
        let first_address = PeerAddress::parse("inproc://placed-collision-a").unwrap();
        let second_address = PeerAddress::parse("inproc://placed-collision-b").unwrap();
        for (operation_id, display_name) in [
            (&first_id, "mob/profile/first"),
            (&second_id, "mob/profile/second"),
        ] {
            MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
                registry.as_ref(),
                &owner,
                operation_id,
                display_name,
            )
            .expect("seed exact Running operation");
        }
        let first_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: first_address.to_string(),
            pubkey: [1u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        adapter
            .bind_member_registry_for_exact_operation(
                &first_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/first",
                OperationSource::backend_peer(peer_id, first_address),
                first_id.clone(),
            )
            .expect("bind first exact peer identity");
        let colliding_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: second_address.to_string(),
            pubkey: [2u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let generic_collision = adapter
            .bind_member_registry(
                &colliding_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/generic-collision",
                OperationSource::backend_peer(peer_id, second_address.clone()),
            )
            .expect_err("generic binding must not overwrite an exact placed peer identity");
        assert!(generic_collision.to_string().contains("already bound"));
        let collision = adapter
            .bind_member_registry_for_exact_operation(
                &colliding_ref,
                owner,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/second",
                OperationSource::backend_peer(peer_id, second_address),
                second_id,
            )
            .expect_err("one live peer id cannot alias two exact operations");
        assert!(collision.to_string().contains("already bound"));
        assert_eq!(
            adapter.active_operation_id_for_member(&first_ref).await,
            Some(first_id)
        );
    }

    #[tokio::test]
    async fn exact_placed_retirement_clears_binding_before_same_peer_reuse() {
        let adapter = MobOpsAdapter::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let owner = SessionId::new();
        let first_id = OperationId::new();
        let display_name = "mob/profile/reusable";
        let peer_id = PeerId::new();
        let address = PeerAddress::parse("inproc://placed-reuse").unwrap();
        let member_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            pubkey: [3u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            registry.as_ref(),
            &owner,
            &first_id,
            display_name,
        )
        .expect("seed first exact Running operation");
        adapter
            .bind_member_registry_for_exact_operation(
                &member_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                display_name,
                OperationSource::backend_peer(peer_id, address.clone()),
                first_id.clone(),
            )
            .expect("bind first exact operation");
        MobOpsAdapter::retire_committed_placed_provision_operation_exact(
            registry.as_ref(),
            &owner,
            &first_id,
            display_name,
        )
        .expect("retire first exact operation");
        adapter
            .clear_placed_member_binding_exact(&owner, &first_id, display_name)
            .expect("clear exact terminal binding");
        assert!(
            adapter
                .active_operation_id_for_member(&member_ref)
                .await
                .is_none()
        );

        let replacement_id = OperationId::new();
        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            registry.as_ref(),
            &owner,
            &replacement_id,
            display_name,
        )
        .expect("seed replacement exact Running operation");
        adapter
            .bind_member_registry_for_exact_operation(
                &member_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                display_name,
                OperationSource::backend_peer(peer_id, address),
                replacement_id.clone(),
            )
            .expect("cleared peer identity admits replacement operation");
        assert_eq!(
            adapter.active_operation_id_for_member(&member_ref).await,
            Some(replacement_id)
        );
    }

    #[tokio::test]
    async fn exact_placed_replay_refreshes_reconstructed_owner_registry() {
        let adapter = MobOpsAdapter::new();
        let old_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let new_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let owner = SessionId::new();
        let operation_id = OperationId::new();
        let display_name = "mob/profile/registry-rotation";
        for registry in [&old_registry, &new_registry] {
            MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
                registry.as_ref(),
                &owner,
                &operation_id,
                display_name,
            )
            .expect("seed exact operation in reconstructed registry");
        }
        let peer_id = PeerId::new();
        let address = PeerAddress::parse("inproc://placed-registry-rotation").unwrap();
        let member_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            pubkey: [4u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let source = OperationSource::backend_peer(peer_id, address);
        adapter
            .bind_member_registry_for_exact_operation(
                &member_ref,
                owner.clone(),
                Arc::clone(&old_registry) as Arc<dyn OpsLifecycleRegistry>,
                display_name,
                source.clone(),
                operation_id.clone(),
            )
            .expect("bind old registry");
        adapter
            .bind_member_registry_for_exact_operation(
                &member_ref,
                owner.clone(),
                Arc::clone(&new_registry) as Arc<dyn OpsLifecycleRegistry>,
                display_name,
                source,
                operation_id.clone(),
            )
            .expect("exact replay refreshes registry capability");
        let projected_member_ref = match &member_ref {
            MemberRef::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                ..
            } => MemberRef::BackendPeer {
                peer_id: peer_id.clone(),
                address: address.clone(),
                pubkey: *pubkey,
                bootstrap_token: bootstrap_token.clone(),
                // This is the host-resident session projected for delivery
                // fencing. It must never become an operation-owner session.
                session_id: Some(SessionId::new()),
            },
            MemberRef::Session { .. } => unreachable!("placed fixture is peer-shaped"),
        };
        adapter
            .report_member_progress(&projected_member_ref, "after registry rotation")
            .await
            .expect("subsequent lookup uses refreshed registry");

        assert_eq!(
            old_registry
                .snapshot(&operation_id)
                .unwrap()
                .unwrap()
                .progress_count,
            0
        );
        let updated = new_registry.snapshot(&operation_id).unwrap().unwrap();
        assert_eq!(updated.progress_count, 1);
        assert_eq!(updated.owner_session_id, owner);
        assert_eq!(
            updated.child_session_id, None,
            "host-resident projected session must not become the operation owner/child key"
        );
    }

    #[test]
    fn generic_exact_reservation_linearizes_same_peer_attempts_and_cleans_by_token() {
        // A committed exact placed binding owns its PeerId before the generic
        // pre-side-effect reservation seam runs. Rejection must happen before
        // the candidate registry receives even a Provisioning operation.
        let placed_adapter = MobOpsAdapter::new();
        let placed_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let candidate_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let placed_owner = SessionId::new();
        let placed_peer_id = PeerId::new();
        let placed_address = PeerAddress::parse("inproc://placed-first").unwrap();
        let placed_source = OperationSource::backend_peer(placed_peer_id, placed_address.clone());
        let placed_ref = MemberRef::BackendPeer {
            peer_id: placed_peer_id.to_string(),
            address: placed_address.to_string(),
            pubkey: [4u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let placed_operation_id = OperationId::new();
        let placed_display_name = "mob/profile/placed-first";
        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            placed_registry.as_ref(),
            &placed_owner,
            &placed_operation_id,
            placed_display_name,
        )
        .expect("seed committed placed operation");
        placed_adapter
            .bind_member_registry_for_exact_operation(
                &placed_ref,
                placed_owner.clone(),
                Arc::clone(&placed_registry) as Arc<dyn OpsLifecycleRegistry>,
                placed_display_name,
                placed_source,
                placed_operation_id,
            )
            .expect("bind exact placed operation first");
        let generic_same_peer_ref = MemberRef::BackendPeer {
            peer_id: placed_peer_id.to_string(),
            address: "inproc://generic-collision".to_string(),
            pubkey: [9u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let generic_collision = placed_adapter
            .reserve_generic_member_provision(
                &generic_same_peer_ref,
                SessionId::new(),
                Arc::clone(&candidate_registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/generic-collision",
                OperationSource::backend_peer(
                    placed_peer_id,
                    PeerAddress::parse("inproc://generic-collision").unwrap(),
                ),
            )
            .expect_err("generic reservation cannot displace exact placed PeerId ownership");
        assert!(generic_collision.to_string().contains("already reserved"));
        assert!(
            candidate_registry.list_operations().unwrap().is_empty(),
            "placed-first collision must reject before registering a generic operation"
        );

        let adapter = MobOpsAdapter::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let owner = SessionId::new();
        let peer_id = PeerId::new();
        let first_address = PeerAddress::parse("inproc://generic-reservation-a").unwrap();
        let second_address = PeerAddress::parse("inproc://generic-reservation-b").unwrap();
        let first_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: first_address.to_string(),
            pubkey: [5u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let first_source = OperationSource::backend_peer(peer_id, first_address);
        let first_id = adapter
            .reserve_generic_member_provision(
                &first_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/generic-first",
                first_source.clone(),
            )
            .expect("first attempt reserves before BindMember");

        let competing_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: second_address.to_string(),
            pubkey: [6u8; 32],
            bootstrap_token: None,
            session_id: None,
        };
        let competing = adapter
            .reserve_generic_member_provision(
                &competing_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/generic-second",
                OperationSource::backend_peer(peer_id, second_address),
            )
            .expect_err("different identity/address cannot share a reserved PeerId");
        assert!(competing.to_string().contains("already reserved"));
        let replay = adapter
            .reserve_generic_member_provision(
                &first_ref,
                owner.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/generic-first",
                first_source.clone(),
            )
            .expect_err("same-identity concurrent attempt gets a different token and must wait");
        assert!(replay.to_string().contains("already reserved"));
        let snapshots = registry.list_operations().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, first_id);
        assert_eq!(snapshots[0].status, OperationStatus::Provisioning);

        // The retained generic token is also a quarantine against an exact
        // placed operation trying to claim the same PeerId. This is the local
        // state left by post-Bind compensation when remote retirement is not
        // confirmed.
        let exact_candidate_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let exact_candidate_id = OperationId::new();
        let exact_candidate_display = "mob/profile/exact-after-uncertain-generic";
        MobOpsAdapter::ensure_committed_placed_provision_operation_exact(
            exact_candidate_registry.as_ref(),
            &owner,
            &exact_candidate_id,
            exact_candidate_display,
        )
        .expect("seed exact candidate operation");
        let exact_collision = adapter
            .bind_member_registry_for_exact_operation(
                &competing_ref,
                owner.clone(),
                Arc::clone(&exact_candidate_registry) as Arc<dyn OpsLifecycleRegistry>,
                exact_candidate_display,
                OperationSource::backend_peer(
                    peer_id,
                    PeerAddress::parse("inproc://generic-reservation-b").unwrap(),
                ),
                exact_candidate_id,
            )
            .expect_err("retained generic reservation must quarantine PeerId from exact reuse");
        assert!(exact_collision.to_string().contains("already bound"));
        assert_eq!(
            registry.snapshot(&first_id).unwrap().unwrap().status,
            OperationStatus::Provisioning,
            "collision must not mutate the quarantining generic operation"
        );

        adapter
            .complete_generic_member_provision_exact(
                &first_ref,
                &owner,
                "mob/profile/generic-first",
                &first_source,
                &first_id,
            )
            .expect("only exact token finalizes");
        adapter
            .abort_generic_member_provision_exact(
                &first_ref,
                &owner,
                "mob/profile/generic-first",
                &first_source,
                &first_id,
                "test compensation",
            )
            .expect("exact token cancels and clears reservation");
        assert_eq!(
            registry.snapshot(&first_id).unwrap().unwrap().status,
            OperationStatus::Cancelled
        );
        let replacement = adapter
            .reserve_generic_member_provision(
                &first_ref,
                owner,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "mob/profile/generic-first",
                first_source,
            )
            .expect("cleared reservation admits a new exact attempt");
        assert_ne!(replacement, first_id);
    }
}
