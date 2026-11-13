//! Cross-mob communication — peering and messaging between members in different mobs.

use meerkat_comms::{InprocRegistry, PeerMeta, PubKey};
use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::types::HandlingMode;
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobHandle, PeerTarget};

use crate::auth::peer_keys::GatewayPeerKeys;
use crate::contact_directory::{ContactDirectory, ContactEntry, MobTransport};
use crate::runtime::cross_mob_remote::{RemoteMemberInfo, RemoteMobError, RemoteMobProxy};

use super::UnifiedRuntime;

/// Dispatch a cross-mob operation to either an in-process `MobHandle`
/// (registered via `register_peer_mob`) or a [`RemoteMobProxy`] for
/// peers reachable over TCP/UDS.
///
/// The remote arm speaks the cross-process control protocol; see
/// `runtime/cross_mob_remote.rs` for the client and
/// `runtime/cross_mob_control.rs` for the wire shape and the listener.
enum LocalOrRemote {
    /// Same-process peer with its member plane and optional identity authority.
    Local(Box<PeerMobAuthority>),
    /// Cross-process peer reachable via TCP/UDS.
    Remote(RemoteMobProxy),
}

/// Authority registered for a same-process peer. A bare handle remains
/// supported for legacy member ids, but the reserved generated-alias
/// namespace fails closed unless the owning identity runtime is present.
#[derive(Clone)]
pub(crate) struct PeerMobAuthority {
    handle: MobHandle,
    identity_runtime: Option<std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    /// The peer mob's session service, when registered as a full runtime:
    /// it resolves a peer member's dialable address for a placed target.
    session_service: Option<std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
}

struct MemberPeerInfo {
    peer_id: String,
    comms_name: String,
    pubkey: [u8; 32],
    /// The member's transport-pubkey string (base64, optional `ed25519:`
    /// prefix): its roster row's, or, while no comms runtime of it is live,
    /// the one MobMachine records for it. Forwarded verbatim on remote wire
    /// requests so the far side can build a signed descriptor for this
    /// member.
    pubkey_b64: String,
}

/// A local member as a peer describes it: the answer of
/// `meta/cross_mob/peer_info`.
pub(crate) struct LocalMemberPeerFacts {
    pub(crate) peer_id: String,
    pub(crate) comms_name: String,
    /// `inproc://{comms_name}` for a member whose runtime is in this
    /// process; a placed member's durable host-acknowledged address.
    pub(crate) address: String,
    /// The member's transport public key, as its roster row publishes it.
    pub(crate) pubkey_b64: String,
    /// The member's socket endpoint, when a peer in another process can
    /// dial one; `None` for a member that only has in-process comms.
    pub(crate) dialable_address: Option<String>,
}

struct BilateralAliasRollback<'a> {
    local_namespace: &'a str,
    peer_namespace: &'a str,
    local_pubkey: [u8; 32],
    peer_pubkey: [u8; 32],
    local_alias_preexisting: bool,
    peer_alias_preexisting: bool,
}

struct BilateralWireRollback<'a> {
    local_handle: MobHandle,
    peer_handle: MobHandle,
    local_member_id: &'a str,
    peer_member_id: &'a str,
    peer_spec: &'a TrustedPeerDescriptor,
    local_spec: &'a TrustedPeerDescriptor,
    local_mutated: bool,
    peer_mutated: bool,
    aliases: Option<BilateralAliasRollback<'a>>,
}

/// Remove only physical state introduced by one bilateral wire attempt.
/// A repaired-but-degraded structural half is canonicalized to disconnected
/// on failure; already healthy trust and route aliases are preserved.
async fn rollback_bilateral_wire_attempt(attempt: BilateralWireRollback<'_>) -> Vec<String> {
    let BilateralWireRollback {
        local_handle,
        peer_handle,
        local_member_id,
        peer_member_id,
        peer_spec,
        local_spec,
        local_mutated,
        peer_mutated,
        aliases,
    } = attempt;
    let mut failures = Vec::new();
    if let Some(aliases) = aliases {
        let registry = InprocRegistry::global();
        if !aliases.local_alias_preexisting {
            registry.unregister_in_namespace(
                aliases.local_namespace,
                &PubKey::new(aliases.peer_pubkey),
            );
        }
        if !aliases.peer_alias_preexisting {
            registry.unregister_in_namespace(
                aliases.peer_namespace,
                &PubKey::new(aliases.local_pubkey),
            );
        }
    }
    if peer_mutated
        && let Err(error) = mutate_member_unchecked(
            peer_handle,
            peer_member_id,
            PeerTarget::External(local_spec.clone()),
            false,
        )
        .await
    {
        failures.push(format!("target trust rollback failed: {error}"));
    }
    if local_mutated
        && let Err(error) = mutate_member_unchecked(
            local_handle,
            local_member_id,
            PeerTarget::External(peer_spec.clone()),
            false,
        )
        .await
    {
        failures.push(format!("source trust rollback failed: {error}"));
    }
    failures
}

/// Errors from cross-mob operations.
#[derive(Debug)]
pub enum CrossMobError {
    /// No contact directory configured on this runtime.
    NoContactDirectory,
    /// Mob ID not found in the contact directory.
    UnknownMob(String),
    /// No peer mob handle registered for this mob (required for inproc).
    NoPeerHandle(String),
    /// Caller asked to wire a non-inproc peer but did not supply (or
    /// could not derive) a 32-byte Ed25519 pubkey. Meta refuses to
    /// build an unsigned descriptor on real transports — meerkat-comms
    /// would then admit any sender at ingress.
    MissingPeerPubkey { mob_id: Option<String> },
    /// Member not found in the target mob's roster.
    MemberNotFound { member_id: String, mob_id: String },
    /// Member has no comms runtime (not comms-enabled).
    NoCommsInfo { member_id: String, mob_id: String },
    /// The underlying mob operation failed.
    Mob(meerkat_mob::MobError),
    /// A generated member alias could not be admitted by its durable
    /// identity-generation authority.
    IdentityAuthority {
        member_id: String,
        mob_id: String,
        message: String,
    },
    /// Failed to build a trusted peer spec.
    PeerSpec(String),
    /// Same-process namespace alias installation failed. Namespace aliases
    /// are part of the physical edge: without them a structurally wired peer
    /// cannot receive messages across isolated mob realms.
    InprocAlias(String),
    /// A cross-process control-channel call failed: the peer gateway is
    /// unreachable, has no control listener bound, or rejected the
    /// request. The inner error carries the endpoint and the peer's
    /// rejection code when one was returned.
    Remote(RemoteMobError),
    /// The local cross-mob control listener could not be started (bind
    /// failure, double start, unsupported transport on this platform).
    ControlListener(String),
    /// Remote cross-mob wiring requires the LOCAL member to be reachable
    /// over a real transport: the descriptor shipped to the peer gateway
    /// must carry an address its comms router can dial. Members default to
    /// inproc comms; give them a socket listener (rpc_gateway
    /// `runtime_options.member_comms_address`, or `comms.mode = "tcp"`
    /// plus `comms.address` in the meerkat config) and bind the control
    /// listener (`control_listen` / `--control-listen`) on both gateways.
    LocalMemberNotRemotelyAddressable {
        member_id: String,
        mob_id: String,
        /// What the member's comms runtime advertised (`None` when the
        /// member has no comms runtime at all).
        advertised: Option<String>,
    },
    /// A placed (host-owned) member has no usable endpoint (Broken, or none
    /// registered). It is never downgraded to a local `inproc` descriptor.
    PlacedMemberUnavailable {
        member_id: String,
        mob_id: String,
        reason: String,
    },
    /// The placed member's edge is committed but its host has not yet
    /// acknowledged the trust row: meerkat reports the external route install
    /// outstanding. The edge is converging, not failed, and is not rolled
    /// back; the route drains when the host is reachable again.
    PlacedRouteInstallPending {
        member_id: String,
        mob_id: String,
        peer_id: String,
        host: String,
    },
    /// The remote gateway answered `LookupMember` without a dialable comms
    /// address or transport pubkey for the target member, so no routable
    /// signed descriptor can be built for it.
    RemoteMemberNotRemotelyAddressable {
        member_id: String,
        mob_id: String,
        detail: String,
    },
}

impl std::fmt::Display for CrossMobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoContactDirectory => write!(f, "no contact directory configured"),
            Self::UnknownMob(id) => write!(f, "unknown mob: {id}"),
            Self::NoPeerHandle(id) => write!(f, "no peer mob handle registered for: {id}"),
            Self::Remote(e) => write!(f, "cross-process cross-mob: {e}"),
            Self::MemberNotFound { member_id, mob_id } => {
                write!(f, "member '{member_id}' not found in mob '{mob_id}'")
            }
            Self::NoCommsInfo { member_id, mob_id } => {
                write!(
                    f,
                    "member '{member_id}' in mob '{mob_id}' has no comms runtime"
                )
            }
            Self::Mob(err) => write!(f, "mob error: {err}"),
            Self::IdentityAuthority {
                member_id,
                mob_id,
                message,
            } => write!(
                f,
                "identity authority rejected member '{member_id}' in mob '{mob_id}': {message}"
            ),
            Self::PeerSpec(reason) => write!(f, "peer spec error: {reason}"),
            Self::InprocAlias(reason) => write!(f, "inproc alias error: {reason}"),
            Self::ControlListener(reason) => {
                write!(f, "cross-mob control listener error: {reason}")
            }
            Self::PlacedMemberUnavailable {
                member_id,
                mob_id,
                reason,
            } => write!(
                f,
                "placed member '{member_id}' in mob '{mob_id}' has no usable endpoint: {reason}"
            ),
            Self::PlacedRouteInstallPending {
                member_id,
                mob_id,
                peer_id,
                host,
            } => write!(
                f,
                "placed member '{member_id}' in mob '{mob_id}' is wired to peer '{peer_id}', \
                 but host '{host}' has not yet acknowledged the trust row (converging)"
            ),
            Self::LocalMemberNotRemotelyAddressable {
                member_id,
                mob_id,
                advertised,
            } => write!(
                f,
                "local member '{member_id}' in mob '{mob_id}' is not remotely addressable \
                 (its comms runtime advertises {}); remote cross-mob wiring needs a member \
                 socket transport - set runtime_options.member_comms_address (rpc_gateway) or \
                 comms.mode=\"tcp\" with comms.address in the meerkat config, and bind \
                 control_listen / --control-listen on the peer gateways",
                match advertised {
                    Some(address) => format!("'{address}'"),
                    None => "no address (no comms runtime)".to_string(),
                }
            ),
            Self::RemoteMemberNotRemotelyAddressable {
                member_id,
                mob_id,
                detail,
            } => write!(
                f,
                "remote member '{member_id}' in mob '{mob_id}' is not remotely addressable: \
                 {detail}; the peer gateway must run its members with a socket comms transport \
                 and serve LookupMember with session-service access"
            ),
            Self::MissingPeerPubkey { mob_id } => match mob_id {
                Some(id) => write!(
                    f,
                    "non-inproc peer for mob '{id}' has no signing pubkey; \
                     bootstrap via meta/peer_pubkey or populate the contact \
                     directory's pubkey field before wiring"
                ),
                None => write!(
                    f,
                    "non-inproc peer has no signing pubkey; supply a 32-byte \
                     Ed25519 pubkey or use inproc transport"
                ),
            },
        }
    }
}

async fn member_lifecycle_target_with_authority(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_id: &str,
    mob_id: &str,
) -> Result<Option<crate::identity_first::runtime::MemberAliasLifecycleTarget>, CrossMobError> {
    if let Some(runtime) = identity_runtime {
        return runtime
            .member_alias_lifecycle_target(member_id)
            .await
            .map_err(|error| CrossMobError::IdentityAuthority {
                member_id: member_id.to_string(),
                mob_id: mob_id.to_string(),
                message: error.to_string(),
            });
    }
    if crate::member_comms_id::is_reserved_generated_alias(member_id) {
        return Err(CrossMobError::IdentityAuthority {
            member_id: member_id.to_string(),
            mob_id: mob_id.to_string(),
            message: "generated aliases require the owning IdentityRuntime".to_string(),
        });
    }
    Ok(None)
}

/// Resolve a public member alias to the concrete member generation used by a
/// cross-mob operation. Callers pass `identity_authoritative = true` only
/// while holding the target returned by `member_alias_lifecycle_target`.
/// Under that lock the continuity row is the sole generation authority; live
/// roster labels are an observation surface and may still contain both the
/// pre-reset and post-reset members.
async fn resolve_member_alias_under_authority(
    handle: &MobHandle,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    identity_authoritative: bool,
    member_alias: &str,
    mob_id: &str,
) -> Result<String, CrossMobError> {
    let member_alias = crate::member_comms_id::runtime_alias_str(member_alias).into_owned();
    if identity_authoritative {
        let runtime = identity_runtime.ok_or_else(|| CrossMobError::IdentityAuthority {
            member_id: member_alias.clone(),
            mob_id: mob_id.to_string(),
            message: "durable member target lost its IdentityRuntime authority".to_string(),
        })?;
        let identity = crate::identity_first::IdentityRuntime::identity_for_generated_member_alias(
            &member_alias,
        )
        .or_else(|| crate::identity_first::AgentIdentity::parse(&member_alias).ok())
        .ok_or_else(|| CrossMobError::IdentityAuthority {
            member_id: member_alias.clone(),
            mob_id: mob_id.to_string(),
            message: "durable member target is not a valid identity alias".to_string(),
        })?;
        let status =
            runtime
                .status(&identity)
                .await
                .map_err(|error| CrossMobError::IdentityAuthority {
                    member_id: member_alias.clone(),
                    mob_id: mob_id.to_string(),
                    message: error.to_string(),
                })?;
        // A runtime binding must EXIST for this target to be live, but it is
        // not the roster spelling. Callers encode this return value as the Mob
        // roster key for peer-info/wire/unwire, and since the stable-identity
        // lowering that key is the encoded durable identity; returning the
        // AgentRuntimeId named a row no roster has.
        if status.agent_runtime_id.is_none() {
            return Err(CrossMobError::IdentityAuthority {
                member_id: member_alias,
                mob_id: mob_id.to_string(),
                message: format!("identity {identity} has no current runtime member"),
            });
        }
        return Ok(identity.as_str().to_string());
    }

    let direct = crate::member_comms_id::mob_member_id(&member_alias);
    if handle
        .get_member(&direct)
        .await
        .map_err(CrossMobError::Mob)?
        .is_some()
    {
        return Ok(member_alias);
    }

    // Compatibility fallback for classic runtimes. Never select the first
    // matching durable label: reset cleanup is asynchronous, so two rows are
    // a normal transient and must fail closed without an IdentityRuntime.
    let candidates = handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .filter(|entry| {
            crate::member_comms_id::resolve_member_identity(
                handle,
                &entry.agent_identity,
                &entry.labels,
            )
            .is_some_and(|identity| identity == member_alias)
        })
        .map(|entry| {
            crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str()).into_owned()
        })
        .collect::<std::collections::BTreeSet<_>>();
    match candidates.len() {
        0 => Ok(member_alias),
        1 => candidates.into_iter().next().ok_or_else(|| {
            CrossMobError::PeerSpec("member alias candidate disappeared".to_string())
        }),
        _ => Err(CrossMobError::IdentityAuthority {
            member_id: member_alias.clone(),
            mob_id: mob_id.to_string(),
            message: format!(
                "ambiguous durable member alias {member_alias}: candidates [{}]",
                candidates.into_iter().collect::<Vec<_>>().join(", ")
            ),
        }),
    }
}

async fn run_member_authority_transaction<T, F, Fut>(
    targets: impl IntoIterator<
        Item = Option<crate::identity_first::runtime::MemberAliasLifecycleTarget>,
    >,
    member_context: impl Into<String>,
    mob_context: impl Into<String>,
    operation: F,
) -> Result<T, CrossMobError>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, CrossMobError>> + Send + 'static,
{
    let targets = targets.into_iter().flatten().collect::<Vec<_>>();
    if targets.is_empty() {
        return operation().await;
    }
    let member_context = member_context.into();
    let mob_context = mob_context.into();
    let operation_error = std::sync::Arc::new(std::sync::Mutex::new(None));
    let task_operation_error = std::sync::Arc::clone(&operation_error);
    let result =
        crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
            targets,
            move || async move {
                match operation().await {
                    Ok(value) => Ok(value),
                    Err(error) => {
                        *task_operation_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                        Err("cross-mob transaction failed".to_string())
                    }
                }
            },
        )
        .await;
    match result {
        Ok(value) => Ok(value),
        Err(authority_error) => {
            if let Some(operation_error) = operation_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                return Err(operation_error);
            }
            Err(CrossMobError::IdentityAuthority {
                member_id: member_context,
                mob_id: mob_context,
                message: authority_error.to_string(),
            })
        }
    }
}

async fn mutate_member_unchecked(
    handle: MobHandle,
    member_id: &str,
    peer: PeerTarget,
    wire: bool,
) -> Result<(), CrossMobError> {
    let member = local_roster_member(member_id);
    // Every wire and unwire of this body changes a member's row, so it keeps
    // the mob's part of the link table equal to the row
    // (`cross_mob_routes`), exactly as `wire_local_edge` and
    // `unwire_local_edge` do: a link is held once the wire is committed,
    // and goes before the unwire commits.
    if wire {
        let wired = handle
            .wire(member.clone(), peer)
            .await
            .map_err(CrossMobError::Mob);
        if wired.is_ok() {
            super::cross_mob_routes::sync_member_links(&handle, &member).await;
        }
        return wired;
    }
    let peer_name = match &peer {
        PeerTarget::External(spec) => Some(spec.name.as_str().to_string()),
        PeerTarget::ExternalName(name) => Some(name.as_str().to_string()),
        _ => None,
    };
    if let Some(peer_name) = &peer_name {
        super::cross_mob_routes::drop_member_link(handle.mob_id().as_ref(), &member, peer_name);
    }
    let unwired = handle
        .unwire(member.clone(), peer)
        .await
        .map_err(CrossMobError::Mob);
    if unwired.is_err() {
        // The edge is still in the row, so the link is still held.
        super::cross_mob_routes::sync_member_links(&handle, &member).await;
    }
    unwired
}

/// The roster id of the local member a (resolved) alias names.
fn local_roster_member(member_id: &str) -> AgentIdentity {
    let member_id = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
    crate::member_comms_id::mob_member_id(&member_id)
}

async fn send_member_unchecked(
    handle: MobHandle,
    member_id: &str,
    mob_id: &str,
    content: meerkat_core::ContentInput,
) -> Result<String, CrossMobError> {
    let member_id = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
    let member = crate::member_comms_id::mob_member_id(&member_id);
    handle
        .member(&member)
        .await
        .map_err(CrossMobError::Mob)?
        .send(content, HandlingMode::Queue)
        .await
        .map_err(CrossMobError::Mob)?;
    handle
        .resolve_bridge_session_id(&member)
        .await
        .map(|session_id| session_id.to_string())
        .ok_or_else(|| CrossMobError::NoCommsInfo {
            member_id,
            mob_id: mob_id.to_string(),
        })
}

async fn member_peer_info(
    handle: &MobHandle,
    meerkat_id: &AgentIdentity,
    mob_id: &str,
) -> Result<MemberPeerInfo, CrossMobError> {
    let entry = handle
        .get_member(meerkat_id)
        .await
        .map_err(|err| {
            CrossMobError::PeerSpec(format!(
                "member lookup for '{meerkat_id}' in mob '{mob_id}' failed: {err}"
            ))
        })?
        .ok_or_else(|| CrossMobError::MemberNotFound {
            member_id: meerkat_id.to_string(),
            mob_id: mob_id.to_string(),
        })?;
    let (peer_id, pubkey_b64) = match entry.peer_id() {
        Some(peer_id) => {
            let pubkey_b64 = entry.transport_public_key().ok_or_else(|| {
                CrossMobError::PeerSpec(format!(
                    "member '{meerkat_id}' in mob '{mob_id}' has no transport public key"
                ))
            })?;
            (peer_id.to_string(), pubkey_b64.to_string())
        }
        // No comms runtime of the member is live (a seat no delivery has
        // woken yet, or a released member): the endpoint MobMachine records
        // for it, which a live peer trusts at once. Nothing is woken.
        None => {
            crate::runtime::cross_mob_control::recorded_member_comms_identity(handle, meerkat_id)
                .await
                .map_err(CrossMobError::Mob)?
                .ok_or_else(|| CrossMobError::NoCommsInfo {
                    member_id: meerkat_id.to_string(),
                    mob_id: mob_id.to_string(),
                })?
        }
    };
    let pubkey = crate::auth::peer_keys::decode_pubkey_b64(&pubkey_b64).map_err(|err| {
        CrossMobError::PeerSpec(format!(
            "member '{meerkat_id}' in mob '{mob_id}' has invalid transport public key: {err}"
        ))
    })?;
    let comms_name = meerkat_core::MemberCommsName::new(
        mob_id,
        entry.role.as_str(),
        meerkat_id.as_str(),
    )
    .map_err(|err| {
        CrossMobError::PeerSpec(format!(
            "member '{meerkat_id}' in mob '{mob_id}' has an invalid comms name component: {err}"
        ))
    })?
    .to_string();
    Ok(MemberPeerInfo {
        peer_id,
        comms_name,
        pubkey,
        pubkey_b64,
    })
}

/// The member's dialable comms address: its live comms runtime's advertised
/// address (`tcp://host:port` with the real bound port when configured with
/// port 0), or a placed member's host-acknowledged durable endpoint address.
/// `None` when neither exists. `inproc://...` comes back as-is; remote-wire
/// callers reject it fail-closed. A faulted endpoint query is an error.
async fn member_comms_advertised_address(
    mob_runtime: &crate::MobRuntime,
    handle: &MobHandle,
    member: &AgentIdentity,
) -> Result<Option<String>, CrossMobError> {
    crate::runtime::cross_mob_control::member_dialable_address(
        handle,
        mob_runtime.session_service(),
        member,
    )
    .await
    .map_err(|fault| plane_fault_error(fault, handle, member))
}

/// Map a plane-classification fault onto the typed cross-mob error, naming
/// the member's own mob.
fn plane_fault_error(
    fault: crate::runtime::cross_mob_control::MemberPlaneFault,
    handle: &MobHandle,
    member: &AgentIdentity,
) -> CrossMobError {
    match fault {
        crate::runtime::cross_mob_control::MemberPlaneFault::PlacedUnavailable { reason } => {
            CrossMobError::PlacedMemberUnavailable {
                member_id: member.to_string(),
                mob_id: handle.mob_id().to_string(),
                reason,
            }
        }
        crate::runtime::cross_mob_control::MemberPlaneFault::Mob(error) => {
            CrossMobError::Mob(error)
        }
    }
}

/// One plane snapshot for `member` (placement first; a placed member
/// without a usable endpoint is a typed error).
async fn member_plane(
    handle: &MobHandle,
    member: &AgentIdentity,
) -> Result<crate::runtime::cross_mob_control::MemberPlane, CrossMobError> {
    crate::runtime::cross_mob_control::member_plane(handle, member)
        .await
        .map_err(|fault| plane_fault_error(fault, handle, member))
}

/// Build the descriptor for a remote member from the facts its gateway
/// returned via `LookupMember`. Fails closed when the remote member is not
/// remotely addressable: a descriptor must carry an address the local
/// comms router can dial and the MEMBER's own transport pubkey (the
/// peer-id/pubkey consistency check rejects any other key, the contact
/// directory's gateway key included).
fn build_remote_member_spec(
    info: &RemoteMemberInfo,
    remote_member_id: &str,
    remote_mob_id: &str,
) -> Result<TrustedPeerDescriptor, CrossMobError> {
    let not_addressable = |detail: String| CrossMobError::RemoteMemberNotRemotelyAddressable {
        member_id: remote_member_id.to_string(),
        mob_id: remote_mob_id.to_string(),
        detail,
    };
    let address = info
        .advertised_address
        .as_deref()
        .filter(|address| !address.starts_with("inproc://"))
        .ok_or_else(|| {
            not_addressable(match &info.advertised_address {
                Some(address) => {
                    format!("advertised address '{address}' is not dialable across processes")
                }
                None => "the peer gateway reported no advertised comms address for this member"
                    .to_string(),
            })
        })?;
    let pubkey_b64 = info.pubkey_b64.as_deref().ok_or_else(|| {
        not_addressable("the peer gateway reported no transport pubkey for this member".to_string())
    })?;
    let pubkey = crate::auth::peer_keys::decode_pubkey_b64(pubkey_b64).map_err(|err| {
        CrossMobError::PeerSpec(format!(
            "remote member '{remote_member_id}' in mob '{remote_mob_id}' has an invalid \
             transport pubkey: {err}"
        ))
    })?;
    build_external_peer_spec(&info.comms_name, &info.peer_id, address, Some(pubkey))
}

/// Readiness of a PLACED member's edge to `expected_peer`, or `None` for a
/// member whose comms runtime is local (probe its live directory instead).
///
/// A placed member's peer directory lives on its host, not in this process.
/// meerkat commits the edge and then realizes the host trust row through a
/// route-install obligation that stays outstanding until the host ACKs.
/// Ready therefore means: the edge is committed AND meerkat reports no
/// outstanding external route install for it. Nothing here falls back to an
/// `inproc` probe.
async fn placed_member_edge_committed(
    handle: &MobHandle,
    member: &AgentIdentity,
    expected_peer: &MemberPeerInfo,
) -> Result<Option<PlacedEdgeReadiness>, CrossMobError> {
    if !member_is_placed(handle, member).await? {
        return Ok(None);
    }
    // The committed edge must carry an external descriptor naming this exact
    // peer (name AND peer id), not merely a peer of the same name.
    let committed = handle.get_member(member).await?.is_some_and(|entry| {
        entry
            .wired_to
            .iter()
            .any(|identity| identity.as_str() == expected_peer.comms_name)
            && entry
                .external_peer_descriptor(&expected_peer.comms_name)
                .is_some_and(|descriptor| descriptor.peer_id.to_string() == expected_peer.peer_id)
    });
    if !committed {
        return Ok(Some(PlacedEdgeReadiness::NotCommitted));
    }
    let installs = handle.route_installs().await?;
    Ok(Some(
        match external_route_install_pending_host(&installs, member, &expected_peer.peer_id) {
            Some(host) => PlacedEdgeReadiness::InstallPending { host },
            None => PlacedEdgeReadiness::Ready,
        },
    ))
}

/// Readiness of a placed member's edge to one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlacedEdgeReadiness {
    /// Committed and acknowledged by the host.
    Ready,
    /// No committed edge naming this exact peer.
    NotCommitted,
    /// Committed; the host has not yet acknowledged the trust row.
    InstallPending { host: String },
}

/// The host of an outstanding external route install for `member` ->
/// `peer_id`, if meerkat still reports one (a typed `route_installs()` fact).
fn external_route_install_pending_host(
    installs: &meerkat_contracts::wire::MobRouteInstallsResult,
    member: &AgentIdentity,
    peer_id: &str,
) -> Option<String> {
    installs
        .outstanding_external
        .iter()
        .find(|obligation| obligation.local == member.as_str() && obligation.peer_id == peer_id)
        .map(|obligation| obligation.host.0.clone())
}

async fn member_can_address_peer(
    mob_runtime: &crate::MobRuntime,
    handle: &MobHandle,
    local_member: &AgentIdentity,
    expected_peer: &MemberPeerInfo,
) -> Result<bool, CrossMobError> {
    if let Some(readiness) =
        placed_member_edge_committed(handle, local_member, expected_peer).await?
    {
        return Ok(readiness == PlacedEdgeReadiness::Ready);
    }
    let session_id = handle
        .resolve_bridge_session_id(local_member)
        .await
        .ok_or_else(|| CrossMobError::NoCommsInfo {
            member_id: local_member.to_string(),
            mob_id: handle.mob_id().to_string(),
        })?;
    let service = mob_runtime
        .session_service()
        .ok_or_else(|| CrossMobError::NoCommsInfo {
            member_id: local_member.to_string(),
            mob_id: handle.mob_id().to_string(),
        })?;
    let comms =
        service
            .comms_runtime(&session_id)
            .await
            .ok_or_else(|| CrossMobError::NoCommsInfo {
                member_id: local_member.to_string(),
                mob_id: handle.mob_id().to_string(),
            })?;
    let peers = comms.peers().await;
    Ok(peers.iter().any(|peer| {
        peer.peer_id.to_string() == expected_peer.peer_id
            && peer.name.as_str() == expected_peer.comms_name
            && peer
                .sendable_kinds
                .contains(&meerkat_core::comms::PeerSendability::PeerMessage)
    }))
}

/// Run `operation` on the member `member_id` names now, under the identity
/// authority that owns its alias when there is one: the operation gets the
/// handle and the member's current alias.
async fn with_member_under_authority<F, Fut>(
    handle: MobHandle,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_id: &str,
    mob_id: &str,
    operation: F,
) -> Result<(), CrossMobError>
where
    F: FnOnce(MobHandle, String) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), CrossMobError>> + Send + 'static,
{
    let operation_member_id = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
    let operation_mob_id = mob_id.to_string();
    let identity_runtime = identity_runtime.cloned();
    let target = member_lifecycle_target_with_authority(
        identity_runtime.as_ref(),
        &operation_member_id,
        &operation_mob_id,
    )
    .await?;
    let identity_authoritative = target.is_some();
    run_member_authority_transaction(
        [target],
        operation_member_id.clone(),
        operation_mob_id.clone(),
        move || async move {
            let current_member_id = resolve_member_alias_under_authority(
                &handle,
                identity_runtime.as_ref(),
                identity_authoritative,
                &operation_member_id,
                &operation_mob_id,
            )
            .await?;
            operation(handle, current_member_id).await
        },
    )
    .await
}

/// Whether `member` is placed, tolerating a placed member without a usable
/// endpoint (Broken or restore-failed is still placed): used where only the
/// placement matters, never an endpoint (alias bookkeeping, readiness gates).
async fn member_is_placed(
    handle: &MobHandle,
    member: &AgentIdentity,
) -> Result<bool, CrossMobError> {
    plane_result_is_placed(crate::runtime::cross_mob_control::member_plane(handle, member).await)
        .map_err(CrossMobError::Mob)
}

/// Pure placement verdict over a plane classification: `PlacedUnavailable`
/// is placed; only a genuine query fault is an error.
fn plane_result_is_placed(
    result: Result<
        crate::runtime::cross_mob_control::MemberPlane,
        crate::runtime::cross_mob_control::MemberPlaneFault,
    >,
) -> Result<bool, meerkat_mob::MobError> {
    match result {
        Ok(plane) => Ok(plane.is_placed()),
        Err(crate::runtime::cross_mob_control::MemberPlaneFault::PlacedUnavailable { .. }) => {
            Ok(true)
        }
        Err(crate::runtime::cross_mob_control::MemberPlaneFault::Mob(error)) => Err(error),
    }
}

/// How a target describes a member it installs, decided purely from the two
/// plane snapshots (placement first, the same order as
/// `member_dialable_address`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum InstallDescriptorChoice {
    /// The member is placed: its durable host-acknowledged endpoint.
    PlacedEndpoint(TrustedPeerDescriptor),
    /// The member is local but the target is placed on a remote host: the
    /// member's dialable non-`inproc` address, or a typed failure.
    DialableAddress,
    /// Both runtimes share this process: the same-process transport.
    SameProcess,
}

fn choose_install_descriptor(
    member: &crate::runtime::cross_mob_control::MemberPlane,
    target: &crate::runtime::cross_mob_control::MemberPlane,
) -> InstallDescriptorChoice {
    use crate::runtime::cross_mob_control::MemberPlane;
    match (member, target) {
        (MemberPlane::Placed(descriptor), _) => {
            InstallDescriptorChoice::PlacedEndpoint(descriptor.clone())
        }
        (MemberPlane::Local, MemberPlane::Placed(_)) => InstallDescriptorChoice::DialableAddress,
        (MemberPlane::Local, MemberPlane::Local) => InstallDescriptorChoice::SameProcess,
    }
}

/// The descriptor a target installs for `member`, from the operation's one
/// plane snapshot of each side (see [`choose_install_descriptor`]).
#[allow(clippy::too_many_arguments)]
async fn install_descriptor_for(
    member_handle: &MobHandle,
    member_sessions: Option<&std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
    member: &AgentIdentity,
    info: &MemberPeerInfo,
    same_process_transport: &MobTransport,
    member_plane_snapshot: &crate::runtime::cross_mob_control::MemberPlane,
    target_plane_snapshot: &crate::runtime::cross_mob_control::MemberPlane,
) -> Result<TrustedPeerDescriptor, CrossMobError> {
    match choose_install_descriptor(member_plane_snapshot, target_plane_snapshot) {
        InstallDescriptorChoice::PlacedEndpoint(descriptor) => {
            // The edge is keyed by the member's comms name on both sides; a
            // host endpoint registered under another name would never match.
            if descriptor.name.as_str() != info.comms_name {
                return Err(CrossMobError::PeerSpec(format!(
                    "placed member '{member}' has a durable endpoint named '{}' but its comms name is '{}'",
                    descriptor.name.as_str(),
                    info.comms_name
                )));
            }
            Ok(descriptor)
        }
        InstallDescriptorChoice::DialableAddress => {
            let address = match member_sessions {
                Some(service) => match member_handle.resolve_bridge_session_id(member).await {
                    Some(session_id) => service
                        .comms_runtime(&session_id)
                        .await
                        .and_then(|comms| comms.advertised_address()),
                    None => None,
                },
                None => None,
            };
            match address {
                Some(address) if !address.starts_with("inproc://") => build_external_peer_spec(
                    &info.comms_name,
                    &info.peer_id,
                    &address,
                    Some(info.pubkey),
                ),
                other => Err(CrossMobError::LocalMemberNotRemotelyAddressable {
                    member_id: member.to_string(),
                    mob_id: member_handle.mob_id().to_string(),
                    advertised: other,
                }),
            }
        }
        InstallDescriptorChoice::SameProcess => build_peer_spec(
            &info.comms_name,
            &info.peer_id,
            same_process_transport,
            Some(info.pubkey),
        ),
    }
}

/// Whether either side of a bilateral edge is placed. Same-process inproc
/// aliases only exist between two runtimes in this process; a placed side is
/// reached over its real transport, so alias installation is skipped and its
/// route is satisfied by the committed placed edge.
async fn any_side_placed(
    local_handle: &MobHandle,
    local: &AgentIdentity,
    peer_handle: &MobHandle,
    peer: &AgentIdentity,
) -> Result<bool, CrossMobError> {
    Ok(member_is_placed(local_handle, local).await? || member_is_placed(peer_handle, peer).await?)
}

async fn wire_bilateral_transaction(
    local_runtime: crate::MobRuntime,
    peer_runtime: crate::MobRuntime,
    local_member_id: String,
    peer_member_id: String,
) -> Result<(), CrossMobError> {
    let local_handle = local_runtime.handle();
    let peer_handle = peer_runtime.handle();
    let local_mob_id = local_handle.mob_id().to_string();
    let peer_mob_id = peer_handle.mob_id().to_string();
    let local_mid = crate::member_comms_id::mob_member_id(&local_member_id);
    let peer_mid = crate::member_comms_id::mob_member_id(&peer_member_id);
    let local_info = member_peer_info(&local_handle, &local_mid, &local_mob_id).await?;
    let peer_info = member_peer_info(&peer_handle, &peer_mid, &peer_mob_id).await?;
    // One plane snapshot per side for the whole operation.
    let local_plane = member_plane(&local_handle, &local_mid).await?;
    let peer_plane = member_plane(&peer_handle, &peer_mid).await?;
    let peer_spec = install_descriptor_for(
        &peer_handle,
        peer_runtime.session_service(),
        &peer_mid,
        &peer_info,
        &MobTransport::Inproc,
        &peer_plane,
        &local_plane,
    )
    .await?;
    let local_spec = install_descriptor_for(
        &local_handle,
        local_runtime.session_service(),
        &local_mid,
        &local_info,
        &MobTransport::Inproc,
        &local_plane,
        &peer_plane,
    )
    .await?;
    let local_member = local_handle.get_member(&local_mid).await?.ok_or_else(|| {
        CrossMobError::MemberNotFound {
            member_id: local_member_id.clone(),
            mob_id: local_mob_id.clone(),
        }
    })?;
    let peer_member =
        peer_handle
            .get_member(&peer_mid)
            .await?
            .ok_or_else(|| CrossMobError::MemberNotFound {
                member_id: peer_member_id.clone(),
                mob_id: peer_mob_id.clone(),
            })?;
    let local_wired = local_member
        .wired_to
        .iter()
        .any(|identity| identity.as_str() == peer_info.comms_name);
    let peer_wired = peer_member
        .wired_to
        .iter()
        .any(|identity| identity.as_str() == local_info.comms_name);
    let local_agent_ready =
        member_can_address_peer(&local_runtime, &local_handle, &local_mid, &peer_info).await?;
    let peer_agent_ready =
        member_can_address_peer(&peer_runtime, &peer_handle, &peer_mid, &local_info).await?;
    // Inproc aliases only exist between two runtimes in this process; a
    // placed side is reached over its real transport.
    let placed_edge = local_plane.is_placed() || peer_plane.is_placed();
    let local_namespace = mob_inproc_namespace_for_id(&local_mob_id)?;
    let peer_namespace = mob_inproc_namespace_for_id(&peer_mob_id)?;
    let local_alias_preexisting = alias_already_installed(
        &local_namespace,
        &peer_info.comms_name,
        PubKey::new(peer_info.pubkey),
    );
    let peer_alias_preexisting = alias_already_installed(
        &peer_namespace,
        &local_info.comms_name,
        PubKey::new(local_info.pubkey),
    );
    // A key a member of either mob is seated under is never routed to the
    // other mob's member: refused before either row changes, as
    // `wire_local_edge` refuses it.
    if !local_wired || !local_agent_ready {
        super::cross_mob_routes::refuse_locally_seated_key(&local_mob_id, &peer_spec)?;
    }
    if !peer_wired || !peer_agent_ready {
        super::cross_mob_routes::refuse_key_seated_in_peer_mob(&peer_mob_id, &local_spec)?;
    }
    let mut local_mutated = false;
    let mut peer_mutated = false;
    if !local_wired || !local_agent_ready {
        mutate_member_unchecked(
            local_handle.clone(),
            &local_member_id,
            PeerTarget::External(peer_spec.clone()),
            true,
        )
        .await?;
        local_mutated = true;
    }
    if (!peer_wired || !peer_agent_ready)
        && let Err(error) = mutate_member_unchecked(
            peer_handle.clone(),
            &peer_member_id,
            PeerTarget::External(local_spec.clone()),
            true,
        )
        .await
    {
        let rollback_failures = rollback_bilateral_wire_attempt(BilateralWireRollback {
            local_handle: local_handle.clone(),
            peer_handle: peer_handle.clone(),
            local_member_id: &local_member_id,
            peer_member_id: &peer_member_id,
            peer_spec: &peer_spec,
            local_spec: &local_spec,
            local_mutated,
            peer_mutated: false,
            aliases: None,
        })
        .await;
        if rollback_failures.is_empty() {
            return Err(error);
        }
        return Err(CrossMobError::InprocAlias(format!(
            "target wire failed: {error}; rollback failures: {}",
            rollback_failures.join("; ")
        )));
    } else if !peer_wired || !peer_agent_ready {
        peer_mutated = true;
    }
    if !placed_edge
        && let Err(error) = register_cross_namespace_aliases(
            &local_namespace,
            &local_info.comms_name,
            local_info.pubkey,
            &peer_namespace,
            &peer_info.comms_name,
            peer_info.pubkey,
        )
    {
        let rollback_failures = rollback_bilateral_wire_attempt(BilateralWireRollback {
            local_handle: local_handle.clone(),
            peer_handle: peer_handle.clone(),
            local_member_id: &local_member_id,
            peer_member_id: &peer_member_id,
            peer_spec: &peer_spec,
            local_spec: &local_spec,
            local_mutated,
            peer_mutated,
            aliases: Some(BilateralAliasRollback {
                local_namespace: &local_namespace,
                peer_namespace: &peer_namespace,
                local_pubkey: local_info.pubkey,
                peer_pubkey: peer_info.pubkey,
                local_alias_preexisting,
                peer_alias_preexisting,
            }),
        })
        .await;
        if rollback_failures.is_empty() {
            return Err(CrossMobError::InprocAlias(error));
        }
        return Err(CrossMobError::InprocAlias(format!(
            "{error}; rollback failures: {}",
            rollback_failures.join("; ")
        )));
    }
    // A placed side whose edge is committed but whose host has not yet
    // acknowledged the trust row is converging, not failed: report it typed
    // and keep the edge (no rollback on every coordinator retry).
    for (handle, mid, member_id, mob_id, expected) in [
        (
            &local_handle,
            &local_mid,
            &local_member_id,
            &local_mob_id,
            &peer_info,
        ),
        (
            &peer_handle,
            &peer_mid,
            &peer_member_id,
            &peer_mob_id,
            &local_info,
        ),
    ] {
        if let Some(PlacedEdgeReadiness::InstallPending { host }) =
            placed_member_edge_committed(handle, mid, expected).await?
        {
            return Err(CrossMobError::PlacedRouteInstallPending {
                member_id: member_id.clone(),
                mob_id: mob_id.clone(),
                peer_id: expected.peer_id.clone(),
                host,
            });
        }
    }
    let readiness = async {
        let local_ready =
            member_can_address_peer(&local_runtime, &local_handle, &local_mid, &peer_info).await?;
        let peer_ready =
            member_can_address_peer(&peer_runtime, &peer_handle, &peer_mid, &local_info).await?;
        Ok::<_, CrossMobError>((local_ready, peer_ready))
    }
    .await;
    let readiness_error = match readiness {
        Ok((true, true)) => return Ok(()),
        Ok((local_ready, peer_ready)) => format!(
            "wire completed without agent-facing trust directory convergence: \
             local_ready={local_ready}, peer_ready={peer_ready}"
        ),
        Err(error) => format!("wire readiness inspection failed: {error}"),
    };
    let rollback_failures = rollback_bilateral_wire_attempt(BilateralWireRollback {
        local_handle,
        peer_handle,
        local_member_id: &local_member_id,
        peer_member_id: &peer_member_id,
        peer_spec: &peer_spec,
        local_spec: &local_spec,
        local_mutated,
        peer_mutated,
        aliases: (!placed_edge).then_some(BilateralAliasRollback {
            local_namespace: &local_namespace,
            peer_namespace: &peer_namespace,
            local_pubkey: local_info.pubkey,
            peer_pubkey: peer_info.pubkey,
            local_alias_preexisting,
            peer_alias_preexisting,
        }),
    })
    .await;
    if rollback_failures.is_empty() {
        return Err(CrossMobError::InprocAlias(readiness_error));
    }
    Err(CrossMobError::InprocAlias(format!(
        "{readiness_error}; rollback failures: {}",
        rollback_failures.join("; ")
    )))
}

async fn unwire_bilateral_transaction(
    local_runtime: crate::MobRuntime,
    peer_runtime: crate::MobRuntime,
    local_member_id: String,
    peer_member_id: String,
) -> Result<(), CrossMobError> {
    let local_handle = local_runtime.handle();
    let peer_handle = peer_runtime.handle();
    let local_mob_id = local_handle.mob_id().to_string();
    let peer_mob_id = peer_handle.mob_id().to_string();
    let local_mid = crate::member_comms_id::mob_member_id(&local_member_id);
    let peer_mid = crate::member_comms_id::mob_member_id(&peer_member_id);
    let local_member = local_handle.get_member(&local_mid).await?.ok_or_else(|| {
        CrossMobError::MemberNotFound {
            member_id: local_member_id.clone(),
            mob_id: local_mob_id.clone(),
        }
    })?;
    let peer_member =
        peer_handle
            .get_member(&peer_mid)
            .await?
            .ok_or_else(|| CrossMobError::MemberNotFound {
                member_id: peer_member_id.clone(),
                mob_id: peer_mob_id.clone(),
            })?;
    // Removal needs only the edge names: meerkat removes by name against the
    // stored edge, so a down or Broken runtime (no endpoint, no key) never
    // blocks an unwire. Descriptors are built only for a rollback re-wire.
    let local_comms_name = member_comms_name(
        &local_mob_id,
        local_member.role.as_str(),
        local_member.agent_identity.as_str(),
    )?;
    let peer_comms_name = member_comms_name(
        &peer_mob_id,
        peer_member.role.as_str(),
        peer_member.agent_identity.as_str(),
    )?;
    let local_wired = local_member
        .wired_to
        .iter()
        .any(|identity| identity.as_str() == peer_comms_name);
    let peer_wired = peer_member
        .wired_to
        .iter()
        .any(|identity| identity.as_str() == local_comms_name);
    if local_wired {
        mutate_member_unchecked(
            local_handle.clone(),
            &local_member_id,
            PeerTarget::ExternalName(peer_name(&peer_comms_name)?),
            false,
        )
        .await?;
    }
    if peer_wired
        && let Err(error) = mutate_member_unchecked(
            peer_handle.clone(),
            &peer_member_id,
            PeerTarget::ExternalName(peer_name(&local_comms_name)?),
            false,
        )
        .await
    {
        let rollback = if local_wired {
            match async {
                let peer_info = member_peer_info(&peer_handle, &peer_mid, &peer_mob_id).await?;
                let local_plane = member_plane(&local_handle, &local_mid).await?;
                let peer_plane = member_plane(&peer_handle, &peer_mid).await?;
                install_descriptor_for(
                    &peer_handle,
                    peer_runtime.session_service(),
                    &peer_mid,
                    &peer_info,
                    &MobTransport::Inproc,
                    &peer_plane,
                    &local_plane,
                )
                .await
            }
            .await
            {
                Ok(peer_spec) => {
                    mutate_member_unchecked(
                        local_handle.clone(),
                        &local_member_id,
                        PeerTarget::External(peer_spec),
                        true,
                    )
                    .await
                }
                Err(rollback_error) => Err(rollback_error),
            }
        } else {
            Ok(())
        };
        return match rollback {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(CrossMobError::InprocAlias(format!(
                "target unwire failed: {error}; source rollback failed: {rollback_error}"
            ))),
        };
    }
    if any_side_placed(&local_handle, &local_mid, &peer_handle, &peer_mid).await? {
        // No inproc aliases were installed for an edge with a placed side.
        return Ok(());
    }
    let (Some(local_key), Some(peer_key)) = (
        member_transport_key(local_member.transport_public_key()),
        member_transport_key(peer_member.transport_public_key()),
    ) else {
        // A member without a published key has no alias to unregister.
        return Ok(());
    };
    let local_namespace = mob_inproc_namespace_for_id(&local_mob_id)?;
    let peer_namespace = mob_inproc_namespace_for_id(&peer_mob_id)?;
    let local_still_references_peer = handle_references_peer(&local_handle, &peer_comms_name).await;
    let peer_still_references_local = handle_references_peer(&peer_handle, &local_comms_name).await;
    unregister_cross_namespace_aliases(
        &local_namespace,
        local_key,
        peer_key,
        local_still_references_peer,
        &peer_namespace,
        peer_still_references_local,
    );
    Ok(())
}

/// The member's comms name, rendered through the typed owner
/// (`meerkat_core::MemberCommsName`), from its roster entry alone.
fn member_comms_name(mob_id: &str, role: &str, member: &str) -> Result<String, CrossMobError> {
    meerkat_core::MemberCommsName::new(mob_id, role, member)
        .map(|name| name.to_string())
        .map_err(|error| {
            CrossMobError::PeerSpec(format!(
                "member '{member}' in mob '{mob_id}' has an invalid comms name component: {error}"
            ))
        })
}

/// The peer name another mob's member uses for `member`: its comms name from
/// the roster entry alone (no endpoint or key needed).
async fn roster_comms_peer_name(
    handle: &MobHandle,
    member: &AgentIdentity,
    mob_id: &str,
) -> Result<meerkat_core::comms::PeerName, CrossMobError> {
    let entry = handle
        .get_member(member)
        .await?
        .ok_or_else(|| CrossMobError::MemberNotFound {
            member_id: member.to_string(),
            mob_id: mob_id.to_string(),
        })?;
    peer_name(&member_comms_name(
        mob_id,
        entry.role.as_str(),
        entry.agent_identity.as_str(),
    )?)
}

/// The name of `local`'s committed edge to member `remote_member_id` of mob
/// `remote_mob_id`, found by the typed `MemberCommsName` components of the
/// edges it is wired to (no string matching), when the peer is unreachable.
async fn local_edge_name_for_remote(
    local_handle: &MobHandle,
    local: &AgentIdentity,
    remote_mob_id: &str,
    remote_member_id: &str,
) -> Result<Option<meerkat_core::comms::PeerName>, CrossMobError> {
    let Some(entry) = local_handle.get_member(local).await? else {
        return Ok(None);
    };
    // Deliberately the exact spelling (member-id alias rule 4): this fallback
    // runs when the peer cannot be asked, so nothing can confirm which
    // generation a generated alias names. Collapsing it onto the durable
    // member could let a stale alias remove the current edge; the lookup
    // error is reported instead.
    let remote_member = crate::member_comms_id::mob_member_id_str(remote_member_id);
    let matched = entry.wired_to.iter().find(|identity| {
        identity
            .as_str()
            .parse::<meerkat_core::MemberCommsName>()
            .is_ok_and(|name| name.mob_id() == remote_mob_id && name.member() == remote_member)
    });
    matched
        .map(|identity| peer_name(identity.as_str()))
        .transpose()
}

fn peer_name(comms_name: &str) -> Result<meerkat_core::comms::PeerName, CrossMobError> {
    meerkat_core::comms::PeerName::new(comms_name.to_string()).map_err(|error| {
        CrossMobError::PeerSpec(format!("invalid peer name {comms_name:?}: {error}"))
    })
}

fn member_transport_key(transport_public_key: Option<&str>) -> Option<[u8; 32]> {
    transport_public_key.and_then(|key| crate::auth::peer_keys::decode_pubkey_b64(key).ok())
}

#[allow(clippy::too_many_arguments)]
async fn wire_cross_mob_transaction(
    entry: ContactEntry,
    remote: LocalOrRemote,
    local_runtime: crate::MobRuntime,
    local_mid: AgentIdentity,
    local_mob_id: String,
    local_member_id: String,
    remote_member_id: String,
    remote_mob_id: String,
) -> Result<(), CrossMobError> {
    let local_handle = local_runtime.handle();
    let local_info = member_peer_info(&local_handle, &local_mid, &local_mob_id).await?;
    match remote {
        LocalOrRemote::Local(remote_authority) => {
            let PeerMobAuthority {
                handle: remote_handle,
                session_service: remote_sessions,
                ..
            } = *remote_authority;
            let remote_mid = crate::member_comms_id::mob_member_id(&remote_member_id);
            let remote_info = member_peer_info(&remote_handle, &remote_mid, &remote_mob_id).await?;
            // One plane snapshot per side for the whole operation.
            let local_plane = member_plane(&local_handle, &local_mid).await?;
            let remote_plane = member_plane(&remote_handle, &remote_mid).await?;
            let remote_spec = install_descriptor_for(
                &remote_handle,
                remote_sessions.as_ref(),
                &remote_mid,
                &remote_info,
                &entry.transport,
                &remote_plane,
                &local_plane,
            )
            .await?;
            let local_spec = install_descriptor_for(
                &local_handle,
                local_runtime.session_service(),
                &local_mid,
                &local_info,
                &MobTransport::Inproc,
                &local_plane,
                &remote_plane,
            )
            .await?;
            // A key a member of either mob is seated under is never routed
            // to the other mob's member: refused before either row changes,
            // as `wire_local_edge` refuses it.
            super::cross_mob_routes::refuse_locally_seated_key(&local_mob_id, &remote_spec)?;
            super::cross_mob_routes::refuse_key_seated_in_peer_mob(&remote_mob_id, &local_spec)?;
            mutate_member_unchecked(
                local_handle.clone(),
                &local_member_id,
                PeerTarget::External(remote_spec.clone()),
                true,
            )
            .await?;
            if let Err(error) = mutate_member_unchecked(
                remote_handle,
                &remote_member_id,
                PeerTarget::External(local_spec),
                true,
            )
            .await
            {
                let _ = mutate_member_unchecked(
                    local_handle,
                    &local_member_id,
                    PeerTarget::External(remote_spec),
                    false,
                )
                .await;
                return Err(error);
            }
            Ok(())
        }
        LocalOrRemote::Remote(proxy) => {
            // Resolve the LOCAL member's dialable comms address before
            // mutating anything: the far side installs it as the reply
            // route, and an inproc:// (or absent) address can never
            // deliver an envelope across processes. Failing here leaves
            // both rosters untouched.
            let local_address =
                member_comms_advertised_address(&local_runtime, &local_handle, &local_mid).await?;
            let local_address = match local_address {
                Some(address) if !address.starts_with("inproc://") => address,
                other => {
                    return Err(CrossMobError::LocalMemberNotRemotelyAddressable {
                        member_id: local_member_id.clone(),
                        mob_id: local_mob_id.clone(),
                        advertised: other,
                    });
                }
            };
            let remote_info = proxy
                .lookup_member(&remote_member_id)
                .await
                .map_err(CrossMobError::Remote)?;
            let remote_spec =
                build_remote_member_spec(&remote_info, &remote_member_id, &remote_mob_id)?;
            mutate_member_unchecked(
                local_handle.clone(),
                &local_member_id,
                PeerTarget::External(remote_spec),
                true,
            )
            .await?;
            if let Err(remote_error) = proxy
                .wire_remote(
                    &remote_member_id,
                    &local_address,
                    &local_info.comms_name,
                    &local_info.peer_id,
                    Some(local_info.pubkey_b64.clone()),
                )
                .await
            {
                if let Ok(spec) =
                    build_remote_member_spec(&remote_info, &remote_member_id, &remote_mob_id)
                {
                    let _ = mutate_member_unchecked(
                        local_handle,
                        &local_member_id,
                        PeerTarget::External(spec),
                        false,
                    )
                    .await;
                }
                return Err(CrossMobError::Remote(remote_error));
            }
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn unwire_cross_mob_transaction(
    // Unwire removes by edge name, so the contact's transport is not needed;
    // the entry stays in the signature to match the wire transaction.
    _entry: ContactEntry,
    remote: LocalOrRemote,
    local_runtime: crate::MobRuntime,
    local_mid: AgentIdentity,
    local_mob_id: String,
    local_member_id: String,
    remote_member_id: String,
    remote_mob_id: String,
) -> Result<(), CrossMobError> {
    let local_handle = local_runtime.handle();
    let mut first_error = None;
    match remote {
        LocalOrRemote::Local(remote_authority) => {
            let remote_handle = remote_authority.handle;
            let remote_mid = crate::member_comms_id::mob_member_id(&remote_member_id);
            // Removal needs only the edge names: meerkat removes by name
            // against the stored edge, so a down or Broken runtime never
            // blocks it, and every failure is reported.
            match roster_comms_peer_name(&remote_handle, &remote_mid, &remote_mob_id).await {
                Ok(remote_name) => {
                    if let Err(error) = mutate_member_unchecked(
                        local_handle.clone(),
                        &local_member_id,
                        PeerTarget::ExternalName(remote_name),
                        false,
                    )
                    .await
                    {
                        first_error.get_or_insert(error);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
            match roster_comms_peer_name(&local_handle, &local_mid, &local_mob_id).await {
                Ok(local_name) => {
                    if let Err(error) = mutate_member_unchecked(
                        remote_handle,
                        &remote_member_id,
                        PeerTarget::ExternalName(local_name),
                        false,
                    )
                    .await
                    {
                        first_error.get_or_insert(error);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        LocalOrRemote::Remote(proxy) => {
            // Local half: remove by edge name. The name comes from the peer
            // gateway's lookup, or, when the peer is unreachable, from the
            // local member's own committed edge whose typed comms name
            // names that mob and member. Every failure is reported.
            let remote_name = match proxy.lookup_member(&remote_member_id).await {
                Ok(remote_info) => peer_name(&remote_info.comms_name),
                Err(lookup_error) => match local_edge_name_for_remote(
                    &local_handle,
                    &local_mid,
                    &remote_mob_id,
                    &remote_member_id,
                )
                .await
                {
                    Ok(Some(name)) => Ok(name),
                    Ok(None) => Err(CrossMobError::Remote(lookup_error)),
                    Err(error) => Err(error),
                },
            };
            match remote_name {
                Ok(name) => {
                    if let Err(error) = mutate_member_unchecked(
                        local_handle.clone(),
                        &local_member_id,
                        PeerTarget::ExternalName(name),
                        false,
                    )
                    .await
                    {
                        first_error.get_or_insert(error);
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
            // Far half: the peer gateway removes its trust row by the
            // descriptor the wire installed (its strict handler rejects
            // inproc://), so it needs the local member's dialable address and
            // key. Missing material is reported, never skipped silently.
            match member_peer_info(&local_handle, &local_mid, &local_mob_id).await {
                Err(error) => {
                    first_error.get_or_insert(error);
                }
                Ok(local_info) => {
                    match member_comms_advertised_address(&local_runtime, &local_handle, &local_mid)
                        .await
                    {
                        Err(error) => {
                            first_error.get_or_insert(error);
                        }
                        Ok(Some(local_address)) if !local_address.starts_with("inproc://") => {
                            if let Err(error) = proxy
                                .unwire_remote(
                                    &remote_member_id,
                                    &local_address,
                                    &local_info.comms_name,
                                    &local_info.peer_id,
                                    Some(local_info.pubkey_b64.clone()),
                                )
                                .await
                            {
                                first_error.get_or_insert(CrossMobError::Remote(error));
                            }
                        }
                        Ok(other) => {
                            first_error.get_or_insert(
                                CrossMobError::LocalMemberNotRemotelyAddressable {
                                    member_id: local_member_id.clone(),
                                    mob_id: local_mob_id.clone(),
                                    advertised: other,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

impl std::error::Error for CrossMobError {}

impl From<meerkat_mob::MobError> for CrossMobError {
    fn from(err: meerkat_mob::MobError) -> Self {
        Self::Mob(err)
    }
}

impl From<RemoteMobError> for CrossMobError {
    fn from(err: RemoteMobError) -> Self {
        Self::Remote(err)
    }
}

impl UnifiedRuntime {
    /// Same-process bilateral primitive used by the durable topology
    /// coordinator. Unlike the public contact-directory RPC this takes both
    /// runtime authorities explicitly, so it cannot silently fall through to
    /// an unauthenticated remote control channel.
    pub(crate) async fn wire_bilateral_same_process(
        &self,
        peer: &UnifiedRuntime,
        local_member_id: &str,
        peer_member_id: &str,
    ) -> Result<(), CrossMobError> {
        // The links this wire declares are held in each runtime's part of
        // the link table, which its start-time read creates: a no-op for
        // every runtime whose mob was started through its bootstrap.
        self.restore_cross_mob_wake_routes().await;
        peer.restore_cross_mob_wake_routes().await;
        let local_member_id = self.topology_concrete_member_id(local_member_id).await?;
        let peer_member_id = peer.topology_concrete_member_id(peer_member_id).await?;
        let local_target = member_lifecycle_target_with_authority(
            self.identity_runtime_ref(),
            &local_member_id,
            &self.mob_id(),
        )
        .await?;
        let peer_target = member_lifecycle_target_with_authority(
            peer.identity_runtime_ref(),
            &peer_member_id,
            &peer.mob_id(),
        )
        .await?;
        let local_authoritative = local_target.is_some();
        let peer_authoritative = peer_target.is_some();
        let local_identity_runtime = self.identity_runtime_ref().cloned();
        let peer_identity_runtime = peer.identity_runtime_ref().cloned();
        let local_mob_id = self.mob_id();
        let peer_mob_id = peer.mob_id();
        let local_runtime = self.mob_runtime.clone();
        let peer_runtime = peer.mob_runtime.clone();
        run_member_authority_transaction(
            [local_target, peer_target],
            format!("{local_member_id} <-> {peer_member_id}"),
            format!("{local_mob_id} <-> {peer_mob_id}"),
            move || async move {
                // Member-id alias rule: under the lifecycle targets this
                // transaction holds, the topology's runtime id names the
                // durable roster row of its identity, as in the other
                // cross-mob operations.
                let local_member_id = resolve_member_alias_under_authority(
                    &local_runtime.handle(),
                    local_identity_runtime.as_ref(),
                    local_authoritative,
                    &local_member_id,
                    &local_mob_id,
                )
                .await?;
                let peer_member_id = resolve_member_alias_under_authority(
                    &peer_runtime.handle(),
                    peer_identity_runtime.as_ref(),
                    peer_authoritative,
                    &peer_member_id,
                    &peer_mob_id,
                )
                .await?;
                wire_bilateral_transaction(
                    local_runtime,
                    peer_runtime,
                    local_member_id,
                    peer_member_id,
                )
                .await
            },
        )
        .await
    }

    pub(crate) async fn unwire_bilateral_same_process(
        &self,
        peer: &UnifiedRuntime,
        local_member_id: &str,
        peer_member_id: &str,
    ) -> Result<(), CrossMobError> {
        let local_member_id = self.topology_concrete_member_id(local_member_id).await?;
        let peer_member_id = peer.topology_concrete_member_id(peer_member_id).await?;
        let local_target = member_lifecycle_target_with_authority(
            self.identity_runtime_ref(),
            &local_member_id,
            &self.mob_id(),
        )
        .await?;
        let peer_target = member_lifecycle_target_with_authority(
            peer.identity_runtime_ref(),
            &peer_member_id,
            &peer.mob_id(),
        )
        .await?;
        let local_authoritative = local_target.is_some();
        let peer_authoritative = peer_target.is_some();
        let local_identity_runtime = self.identity_runtime_ref().cloned();
        let peer_identity_runtime = peer.identity_runtime_ref().cloned();
        let local_mob_id = self.mob_id();
        let peer_mob_id = peer.mob_id();
        let local_runtime = self.mob_runtime.clone();
        let peer_runtime = peer.mob_runtime.clone();
        run_member_authority_transaction(
            [local_target, peer_target],
            format!("{local_member_id} <-> {peer_member_id}"),
            format!("{local_mob_id} <-> {peer_mob_id}"),
            move || async move {
                // Member-id alias rule: under the lifecycle targets this
                // transaction holds, the topology's runtime id names the
                // durable roster row of its identity, as in the other
                // cross-mob operations.
                let local_member_id = resolve_member_alias_under_authority(
                    &local_runtime.handle(),
                    local_identity_runtime.as_ref(),
                    local_authoritative,
                    &local_member_id,
                    &local_mob_id,
                )
                .await?;
                let peer_member_id = resolve_member_alias_under_authority(
                    &peer_runtime.handle(),
                    peer_identity_runtime.as_ref(),
                    peer_authoritative,
                    &peer_member_id,
                    &peer_mob_id,
                )
                .await?;
                unwire_bilateral_transaction(
                    local_runtime,
                    peer_runtime,
                    local_member_id,
                    peer_member_id,
                )
                .await
            },
        )
        .await
    }

    pub(crate) async fn bilateral_same_process_state(
        &self,
        peer: &UnifiedRuntime,
        local_member_id: &str,
        peer_member_id: &str,
    ) -> Result<(bool, bool), CrossMobError> {
        let local_member_id = self.topology_concrete_member_id(local_member_id).await?;
        let peer_member_id = peer.topology_concrete_member_id(peer_member_id).await?;
        let local_handle = self.mob_runtime.handle();
        let peer_handle = peer.mob_runtime.handle();
        // A read under the member-id alias rule: the topology's runtime id
        // came from the identity runtime just now, and it names the durable
        // roster row of its identity.
        let local_mid = crate::member_comms_id::roster_member_id_for_supplied_id(&local_member_id);
        let peer_mid = crate::member_comms_id::roster_member_id_for_supplied_id(&peer_member_id);
        let local_info = self
            .get_member_peer_info(&local_handle, &local_mid, &self.mob_id())
            .await?;
        let peer_info = self
            .get_member_peer_info(&peer_handle, &peer_mid, &peer.mob_id())
            .await?;
        let local = local_handle.get_member(&local_mid).await?.ok_or_else(|| {
            CrossMobError::MemberNotFound {
                member_id: local_member_id.clone(),
                mob_id: self.mob_id(),
            }
        })?;
        let remote = peer_handle.get_member(&peer_mid).await?.ok_or_else(|| {
            CrossMobError::MemberNotFound {
                member_id: peer_member_id.clone(),
                mob_id: peer.mob_id(),
            }
        })?;
        let local_structural = local
            .wired_to
            .iter()
            .any(|identity| identity.as_str() == peer_info.comms_name);
        let remote_structural = remote
            .wired_to
            .iter()
            .any(|identity| identity.as_str() == local_info.comms_name);
        let local_namespace = mob_inproc_namespace(self)?;
        let peer_namespace = mob_inproc_namespace(peer)?;
        // A placed side is reached over its real transport: no inproc alias
        // exists or is needed, and its route is the committed placed edge
        // (which the agent readiness below checks through meerkat).
        let placed_edge =
            any_side_placed(&local_handle, &local_mid, &peer_handle, &peer_mid).await?;
        let local_route = placed_edge
            || alias_already_installed(
                &local_namespace,
                &peer_info.comms_name,
                PubKey::new(peer_info.pubkey),
            );
        let remote_route = placed_edge
            || alias_already_installed(
                &peer_namespace,
                &local_info.comms_name,
                PubKey::new(local_info.pubkey),
            );
        let local_agent = self
            .agent_can_address_peer(&local_handle, &local_mid, &peer_info)
            .await?;
        let remote_agent = peer
            .agent_can_address_peer(&peer_handle, &peer_mid, &local_info)
            .await?;
        Ok((
            local_structural && local_route && local_agent,
            remote_structural && remote_route && remote_agent,
        ))
    }

    async fn topology_concrete_member_id(
        &self,
        logical_identity: &str,
    ) -> Result<String, CrossMobError> {
        let Some(context) = self.identity_first_context.as_ref() else {
            return Ok(logical_identity.to_string());
        };
        let identity = crate::identity_first::AgentIdentity::parse(logical_identity)
            .map_err(|error| CrossMobError::PeerSpec(error.to_string()))?;
        context
            .runtime
            .runtime_id_for(&identity)
            .await
            .map(|runtime_id| runtime_id.to_string())
            .map_err(|error| CrossMobError::MemberNotFound {
                member_id: format!("{logical_identity}: {error}"),
                mob_id: self.mob_id(),
            })
    }

    /// Register an external mob's handle for same-process cross-mob communication.
    /// Generated `rt:*` aliases fail closed through this legacy registration;
    /// use [`Self::register_peer_runtime`] or
    /// [`Self::register_peer_mob_with_identity_runtime`] for identity-first peers.
    pub async fn register_peer_mob(&self, mob_id: &str, handle: MobHandle) {
        self.peer_mob_handles.write().await.insert(
            mob_id.to_string(),
            PeerMobAuthority {
                handle,
                identity_runtime: None,
                session_service: None,
            },
        );
    }

    /// Register a same-process peer together with the durable authority that
    /// owns its generated member aliases.
    pub async fn register_peer_mob_with_identity_runtime(
        &self,
        mob_id: &str,
        handle: MobHandle,
        identity_runtime: std::sync::Arc<crate::identity_first::IdentityRuntime>,
    ) {
        self.peer_mob_handles.write().await.insert(
            mob_id.to_string(),
            PeerMobAuthority {
                handle,
                identity_runtime: Some(identity_runtime),
                session_service: None,
            },
        );
    }

    /// Test probe: whether the registered same-process peer mob carries its
    /// session service (so its members' dialable addresses resolve).
    #[cfg(test)]
    pub(crate) async fn peer_mob_carries_session_service(&self, mob_id: &str) -> bool {
        self.peer_mob_handles
            .read()
            .await
            .get(mob_id)
            .is_some_and(|authority| authority.session_service.is_some())
    }

    /// Register all authority needed for direct same-process cross-mob calls.
    pub async fn register_peer_runtime(&self, peer: &UnifiedRuntime) {
        self.peer_mob_handles.write().await.insert(
            peer.mob_id(),
            PeerMobAuthority {
                handle: peer.mob_handle(),
                identity_runtime: peer.identity_runtime_ref().cloned(),
                session_service: peer.mob_runtime.session_service().cloned(),
            },
        );
    }

    /// Set the contact directory for cross-mob address resolution.
    pub fn set_contact_directory(&mut self, directory: ContactDirectory) {
        self.contact_directory = Some(directory);
    }

    /// Bind the cross-mob control listener and start serving control RPC
    /// (wire/unwire/inject/lookup) from remote gateways.
    ///
    /// Returns the dialable bound address (`tcp://ip:port` with the real
    /// kernel-assigned port for `tcp://host:0`, or `uds:///path`) - this is
    /// what peers put in their contact directories. The serve task is owned
    /// by the runtime and aborted on [`UnifiedRuntime::shutdown`].
    ///
    /// The handler re-reads the identity authority per request, so calling
    /// this before `attach_identity_first_context` is safe: generated
    /// `rt:*` aliases fail closed until the authority is attached and are
    /// admitted afterwards.
    ///
    /// This convenience form is fail-closed: it binds with an empty grant
    /// table, so every request is refused. Use
    /// [`Self::start_control_listener_with_authorizer`] with a populated
    /// table to admit scoped callers. An intentionally open listener must
    /// name [`ControlAuthorizer::open`](crate::runtime::cross_mob_control::ControlAuthorizer::open)
    /// explicitly at that lower-level seam.
    pub async fn start_control_listener(
        &self,
        addr: &crate::runtime::cross_mob_control::ControlListenAddr,
    ) -> Result<String, CrossMobError> {
        tracing::warn!(%addr, "cross-mob control listener has no caller grants; refusing every request");
        self.start_control_listener_with_authorizer(
            addr,
            std::sync::Arc::new(
                crate::runtime::cross_mob_control::ControlAuthorizer::with_grants_for_audience(
                    crate::runtime::cross_mob_control::ControlGrantTable::new(),
                    self.mob_id(),
                ),
            ),
        )
        .await
    }

    /// Bind the cross-mob control listener with caller authorization.
    ///
    /// `authorizer` is supplied by the host rather than held on the
    /// runtime because grants come from configuration the caller already
    /// holds before it starts the listener - unlike the gateway keypair,
    /// which hosts install late and which therefore needs a live slot.
    pub async fn start_control_listener_with_authorizer(
        &self,
        addr: &crate::runtime::cross_mob_control::ControlListenAddr,
        authorizer: std::sync::Arc<crate::runtime::cross_mob_control::ControlAuthorizer>,
    ) -> Result<String, CrossMobError> {
        let mut task_slot = self.cross_mob_control_task.lock().await;
        if task_slot.is_some() {
            return Err(CrossMobError::ControlListener(
                "control listener already running".to_string(),
            ));
        }
        let bound = crate::runtime::cross_mob_control::BoundControlListener::bind(addr)
            .await
            .map_err(|error| CrossMobError::ControlListener(format!("bind {addr}: {error}")))?;
        let advertised = bound.advertised_address().to_string();
        let mut handler =
            crate::runtime::cross_mob_control::MobHandleControlHandler::with_shared_identity_authority(
                // The runtime-owned listener is not a public owner.
                self.mob_runtime.handle(),
                std::sync::Arc::clone(&self.implicit_delegate_identity_runtime),
            );
        if let Some(service) = self.mob_runtime.session_service() {
            handler = handler.with_session_service(std::sync::Arc::clone(service));
        }
        self.install_remote_host_facts(&advertised);
        handler = handler.with_host_facts_slot(std::sync::Arc::clone(&self.remote_host_facts));
        handler = handler.with_keyed_inject_slot(std::sync::Arc::clone(&self.keyed_inject_slot));
        // A candidate launch's listener rewires no member and admits no
        // member input: a peer's Wire, Unwire and Inject are refused.
        handler = handler.with_candidate_launch(!self.speaks_for_composition());
        let handler: std::sync::Arc<dyn crate::runtime::cross_mob_control::ControlHandler> =
            std::sync::Arc::new(handler);
        *task_slot = Some(tokio::spawn(bound.serve_with_authorizer(
            handler,
            std::sync::Arc::clone(&self.gateway_peer_keys),
            authorizer,
        )));
        *self
            .cross_mob_control_advertised
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(advertised.clone());
        Ok(advertised)
    }

    /// The dialable address of the running cross-mob control listener, or
    /// `None` when no listener was started. For `tcp://host:0` binds this
    /// carries the real kernel-assigned port.
    pub fn control_listener_advertised_address(&self) -> Option<String> {
        self.cross_mob_control_advertised
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn install_remote_host_facts(&self, advertised: &str) {
        let Some(keys) = self.gateway_peer_keys() else {
            return;
        };
        let mut options = meerkat::surface::RuntimeHostSurfaceOptions::process(
            "meerkat-meta",
            env!("CARGO_PKG_VERSION"),
        );
        options.runtime_backed_sessions = self.mob_runtime.session_service().is_some();
        options.mobs = true;
        options.multi_host_mobs = true;
        options.comms = true;
        options.session_events = true;
        options.session_streams = true;
        options.rpc_transport = Some("meta_control".to_string());
        options.rpc_methods = vec!["host/describe".to_string(), "host/health".to_string()];
        let capabilities = meerkat::surface::build_runtime_host_capabilities(&options);
        let facts = crate::runtime::remote_host::HostFacts::new(
            self.mob_id(),
            keys.pubkey_b64(),
            advertised,
            capabilities,
        )
        .with_endpoints(meerkat_contracts::RuntimeHostEndpointProjection {
            rpc_transport: Some("meta_control".to_string()),
            rest_base_url: None,
            rpc_methods: vec!["host/describe".to_string(), "host/health".to_string()],
            rest_paths: Vec::new(),
        });
        let health = meerkat_contracts::RuntimeHostHealth {
            contract_version: meerkat_contracts::ContractVersion::CURRENT,
            status: meerkat_contracts::RuntimeHostHealthStatus::Ok,
            checks: std::collections::BTreeMap::new(),
        };
        *self
            .remote_host_facts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::sync::Arc::new(
            crate::runtime::remote_host::StaticHostFacts::new(facts, health),
        ));
    }

    /// Install the long-lived Ed25519 keypair this gateway advertises via
    /// `meta/peer_pubkey` and signs cross-mob control responses with.
    ///
    /// Inproc-only deployments and most tests skip this — the in-process
    /// router authorises by identity map and signature verification is
    /// moot. Production gateways and any cross-process integration test
    /// must call this. Takes effect immediately for a control listener
    /// that is already serving (the serve task re-reads the slot per
    /// request).
    pub fn set_gateway_peer_keys(&mut self, keys: GatewayPeerKeys) {
        // Key replacement would have to migrate the signer, pairing store,
        // and caller identity used by an in-flight reconnect controller as one
        // joined transition. This synchronous composition API cannot provide
        // that boundary, so it remains explicitly one-shot and never detaches
        // the already-owned controller task.
        if self.gateway_peer_keys().is_some() {
            tracing::warn!("gateway peer keys are already installed; refusing live replacement");
            return;
        }
        let state_root = keys.state_root().map(std::path::Path::to_path_buf);
        *self
            .gateway_peer_keys
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::sync::Arc::new(keys));
        if let Some(advertised) = self.control_listener_advertised_address() {
            self.install_remote_host_facts(&advertised);
        }
        let Some(state_root) = state_root else {
            return;
        };
        let path = state_root.join(crate::runtime::remote_host::HOST_PAIRING_FILE_NAME);
        match crate::runtime::remote_host::RemoteHostLifecycle::load(
            path,
            crate::runtime::remote_host::HostReconnectPolicy::default(),
        ) {
            Ok(lifecycle) => {
                let lifecycle = std::sync::Arc::new(lifecycle);
                *self
                    .remote_host_lifecycle
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(std::sync::Arc::clone(&lifecycle));
                *self
                    .remote_host_lifecycle_error
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.start_remote_host_reconnect_task(lifecycle);
            }
            Err(error) => {
                *self
                    .remote_host_lifecycle
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                *self
                    .remote_host_lifecycle_error
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
            }
        }
    }

    fn start_remote_host_reconnect_task(
        &mut self,
        lifecycle: std::sync::Arc<crate::runtime::remote_host::RemoteHostLifecycle>,
    ) {
        // `set_gateway_peer_keys` is normally a one-shot composition step.
        // Refuse to detach a prior controller if an embedder calls it twice:
        // the original remains owned by `shutdown` and no duplicate probes run.
        if self.remote_host_reconnect_task.get_mut().is_some() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let caller_keys = self.gateway_peer_keys();
        let task = handle.spawn(async move {
            // Only already-durable pairings are eligible for reconnect. A
            // pinned contact alone is not enrollment authority: the explicit
            // `pair_runtime_host` boundary must first authenticate and commit
            // its endpoint identity.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                for outcome in lifecycle
                    .probe_due(
                        caller_keys.clone(),
                        crate::runtime::remote_host::unix_now_secs(),
                    )
                    .await
                {
                    if let Err(error) = outcome.result {
                        tracing::debug!(
                            host_key_b64 = %outcome.host_key_b64,
                            %error,
                            "runtime-host reconnect probe failed"
                        );
                    }
                }
            }
        });
        *self.remote_host_reconnect_task.get_mut() = Some(task);
    }

    fn remote_host_lifecycle(
        &self,
    ) -> Result<
        std::sync::Arc<crate::runtime::remote_host::RemoteHostLifecycle>,
        crate::runtime::remote_host::RemoteHostLifecycleError,
    > {
        if let Some(error) = self
            .remote_host_lifecycle_error
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Err(error.into());
        }
        self.remote_host_lifecycle
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                crate::runtime::remote_host::RemoteHostLifecycleError::Pairing(
                    crate::runtime::remote_host::HostPairingError::Io {
                        path: crate::runtime::remote_host::HOST_PAIRING_FILE_NAME.to_string(),
                        reason: "gateway has no durable state root".to_string(),
                    },
                )
            })
    }

    /// Explicitly pair a pinned remote contact and return its exact Meerkat
    /// placement carrier. This never binds the host or mutates a member;
    /// Meerkat's host authority must admit the returned `WireHostRef` later.
    pub async fn pair_runtime_host(
        &self,
        contact_id: &str,
    ) -> Result<
        crate::runtime::remote_host::RuntimeHostPlacement,
        crate::runtime::remote_host::RemoteHostLifecycleError,
    > {
        let contact = self
            .contact_directory
            .as_ref()
            .and_then(|directory| directory.get(contact_id))
            .cloned()
            .ok_or_else(|| {
                crate::runtime::remote_host::RemoteHostLifecycleError::UnknownContact {
                    contact: contact_id.to_string(),
                }
            })?;
        self.remote_host_lifecycle()?
            .pair_contact(
                &contact,
                self.gateway_peer_keys(),
                crate::runtime::remote_host::unix_now_secs(),
            )
            .await
    }

    /// Resolve an already-paired host to an exact placement carrier. Unknown,
    /// never-authenticated, unreachable, unhealthy, and incapable hosts all
    /// refuse typed; there is no `Option` that a caller could treat as local.
    pub async fn runtime_host_placement(
        &self,
        host_key_b64: &str,
    ) -> Result<
        crate::runtime::remote_host::RuntimeHostPlacement,
        crate::runtime::remote_host::RemoteHostLifecycleError,
    > {
        self.remote_host_lifecycle()?.placement(host_key_b64).await
    }

    /// The local gateway keypair if one was installed.
    pub fn gateway_peer_keys(&self) -> Option<std::sync::Arc<GatewayPeerKeys>> {
        self.gateway_peer_keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Wire a local member to a member in an external mob.
    ///
    /// Resolves both members' peer IDs from roster entries, builds peer specs
    /// using the transport scheme advertised by the contact directory entry
    /// (`inproc`, `tcp`, or `uds`), and registers the peer on both sides to
    /// establish bidirectional trust.
    ///
    /// # Local vs remote dispatch
    ///
    /// The destination mob is dispatched as either `LocalOrRemote::Local`
    /// (when an `Arc<MobHandle>` was registered via [`Self::register_peer_mob`])
    /// or `LocalOrRemote::Remote` (when only a contact-directory TCP/UDS
    /// entry exists). The remote arm performs real cross-process control
    /// RPC against the peer gateway's control listener - see
    /// `runtime::cross_mob_remote::RemoteMobProxy`.
    pub async fn wire_cross_mob(
        &self,
        local_member_id: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
    ) -> Result<(), CrossMobError> {
        self.wire_cross_mob_with_identity_runtime(
            local_member_id,
            remote_member_id,
            remote_mob_id,
            self.identity_runtime_ref(),
        )
        .await
    }

    pub(crate) async fn wire_cross_mob_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        local_identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), CrossMobError> {
        let local_member_id =
            crate::member_comms_id::runtime_alias_str(local_member_id).into_owned();
        let remote_member_id =
            crate::member_comms_id::runtime_alias_str(remote_member_id).into_owned();
        // The link this wire declares is held in the runtime's part of the
        // link table, which its start-time read creates: a no-op for every
        // runtime whose mob was started through its bootstrap. The other
        // mob's runtime is reached through its handle here and ran its own
        // read when its mob started.
        self.restore_cross_mob_wake_routes().await;

        let local_handle = self.mob_runtime.handle();
        let local_mob_id = local_handle.mob_id().to_string();
        let local_identity_runtime = local_identity_runtime.cloned();
        let local_target = member_lifecycle_target_with_authority(
            local_identity_runtime.as_ref(),
            &local_member_id,
            &local_mob_id,
        )
        .await?;
        let local_identity_authoritative = local_target.is_some();
        let entry = self.resolve_contact(remote_mob_id)?;
        let remote = self.dispatch_for(&entry).await?;
        let remote_target = match &remote {
            LocalOrRemote::Local(authority) => {
                member_lifecycle_target_with_authority(
                    authority.identity_runtime.as_ref(),
                    &remote_member_id,
                    remote_mob_id,
                )
                .await?
            }
            LocalOrRemote::Remote(_) => None,
        };
        let remote_identity_authoritative = remote_target.is_some();
        let remote_mob_id = remote_mob_id.to_string();
        let local_runtime = self.mob_runtime.clone();
        run_member_authority_transaction(
            [local_target, remote_target],
            format!("{local_member_id} <-> {remote_member_id}"),
            format!("{local_mob_id} <-> {remote_mob_id}"),
            move || async move {
                let local_member_id = resolve_member_alias_under_authority(
                    &local_handle,
                    local_identity_runtime.as_ref(),
                    local_identity_authoritative,
                    &local_member_id,
                    &local_mob_id,
                )
                .await?;
                let local_mid = crate::member_comms_id::mob_member_id(&local_member_id);
                let remote_member_id = match &remote {
                    LocalOrRemote::Local(authority) => {
                        resolve_member_alias_under_authority(
                            &authority.handle,
                            authority.identity_runtime.as_ref(),
                            remote_identity_authoritative,
                            &remote_member_id,
                            &remote_mob_id,
                        )
                        .await?
                    }
                    LocalOrRemote::Remote(_) => remote_member_id,
                };
                wire_cross_mob_transaction(
                    entry,
                    remote,
                    local_runtime,
                    local_mid,
                    local_mob_id,
                    local_member_id,
                    remote_member_id,
                    remote_mob_id,
                )
                .await
            },
        )
        .await
    }

    /// Unwire a cross-mob peering.
    ///
    /// Best-effort on both sides — attempts to unwire both the local and
    /// remote members. Partial cleanup is better than aborting after one
    /// side fails, which would leave asymmetric peering.
    pub async fn unwire_cross_mob(
        &self,
        local_member_id: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
    ) -> Result<(), CrossMobError> {
        self.unwire_cross_mob_with_identity_runtime(
            local_member_id,
            remote_member_id,
            remote_mob_id,
            self.identity_runtime_ref(),
        )
        .await
    }

    pub(crate) async fn unwire_cross_mob_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        local_identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), CrossMobError> {
        let local_member_id =
            crate::member_comms_id::runtime_alias_str(local_member_id).into_owned();
        let remote_member_id =
            crate::member_comms_id::runtime_alias_str(remote_member_id).into_owned();
        let local_handle = self.mob_runtime.handle();
        let local_mob_id = local_handle.mob_id().to_string();
        let local_identity_runtime = local_identity_runtime.cloned();
        let local_target = member_lifecycle_target_with_authority(
            local_identity_runtime.as_ref(),
            &local_member_id,
            &local_mob_id,
        )
        .await?;
        let local_identity_authoritative = local_target.is_some();
        let entry = self.resolve_contact(remote_mob_id)?;
        let remote = self.dispatch_for(&entry).await?;
        let remote_target = match &remote {
            LocalOrRemote::Local(authority) => {
                member_lifecycle_target_with_authority(
                    authority.identity_runtime.as_ref(),
                    &remote_member_id,
                    remote_mob_id,
                )
                .await?
            }
            LocalOrRemote::Remote(_) => None,
        };
        let remote_identity_authoritative = remote_target.is_some();
        let remote_mob_id = remote_mob_id.to_string();
        let local_runtime = self.mob_runtime.clone();
        run_member_authority_transaction(
            [local_target, remote_target],
            format!("{local_member_id} <-> {remote_member_id}"),
            format!("{local_mob_id} <-> {remote_mob_id}"),
            move || async move {
                let local_member_id = resolve_member_alias_under_authority(
                    &local_handle,
                    local_identity_runtime.as_ref(),
                    local_identity_authoritative,
                    &local_member_id,
                    &local_mob_id,
                )
                .await?;
                let local_mid = crate::member_comms_id::mob_member_id(&local_member_id);
                let remote_member_id = match &remote {
                    LocalOrRemote::Local(authority) => {
                        resolve_member_alias_under_authority(
                            &authority.handle,
                            authority.identity_runtime.as_ref(),
                            remote_identity_authoritative,
                            &remote_member_id,
                            &remote_mob_id,
                        )
                        .await?
                    }
                    LocalOrRemote::Remote(_) => remote_member_id,
                };
                unwire_cross_mob_transaction(
                    entry,
                    remote,
                    local_runtime,
                    local_mid,
                    local_mob_id,
                    local_member_id,
                    remote_member_id,
                    remote_mob_id,
                )
                .await
            },
        )
        .await
    }

    /// Inject a message into a remote mob member's session.
    ///
    /// This is an **app-level injection** — the remote agent receives the
    /// message as an external turn but does not know who sent it. For
    /// agent-to-agent communication with sender identity and reply path,
    /// use `wire_cross_mob` to set up peering, then agents communicate
    /// directly via their comms `send` tool.
    ///
    /// `from_local_member` is recorded for audit/logging but does not
    /// affect delivery — the message is injected via the remote mob handle.
    pub async fn send_cross_mob(
        &self,
        from_local_member: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        content: impl Into<meerkat_core::ContentInput>,
    ) -> Result<String, CrossMobError> {
        self.send_cross_mob_with_identity_runtime(
            from_local_member,
            remote_member_id,
            remote_mob_id,
            content,
            self.identity_runtime_ref(),
        )
        .await
    }

    pub(crate) async fn send_cross_mob_with_identity_runtime(
        &self,
        from_local_member: &str,
        remote_member_id: &str,
        remote_mob_id: &str,
        content: impl Into<meerkat_core::ContentInput>,
        local_identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<String, CrossMobError> {
        let content = content.into();
        let from_local_member =
            crate::member_comms_id::runtime_alias_str(from_local_member).into_owned();
        let remote_member_id =
            crate::member_comms_id::runtime_alias_str(remote_member_id).into_owned();
        let local_mob_id = self.mob_id();
        let local_identity_runtime = local_identity_runtime.cloned();
        let local_target = member_lifecycle_target_with_authority(
            local_identity_runtime.as_ref(),
            &from_local_member,
            &local_mob_id,
        )
        .await?;
        let entry = self.resolve_contact(remote_mob_id)?;
        let remote = self.dispatch_for(&entry).await?;
        let remote_target = match &remote {
            LocalOrRemote::Local(authority) => {
                member_lifecycle_target_with_authority(
                    authority.identity_runtime.as_ref(),
                    &remote_member_id,
                    remote_mob_id,
                )
                .await?
            }
            LocalOrRemote::Remote(_) => None,
        };
        let remote_identity_authoritative = remote_target.is_some();
        let remote_mob_id = remote_mob_id.to_string();
        // A durable identity of a same-process peer mob is reached through
        // its identity door, whatever its residency: its admission guards
        // (addressability, a pending first-boot row, an owed held completion,
        // the registered row) and its delivery preparation apply, and Core
        // wakes it when it is dormant. The door takes the identity's own
        // lifecycle lock and checks a generated alias against its current
        // incarnation, so the send holds no lock of the target itself.
        if let (LocalOrRemote::Local(authority), Some(target)) = (&remote, remote_target.as_ref())
            && let Some(identity_runtime) = authority.identity_runtime.clone()
        {
            let identity = target.durable_identity().clone();
            // The sender is checked current under its own lock first; it
            // names the message's origin and does not take the delivery.
            // The door checks it again under the target's lock, just before
            // delivery, without the sender's own: taking it there would nest
            // one identity's lock inside another's.
            let sender = local_target.clone();
            run_member_authority_transaction(
                [local_target],
                from_local_member.clone(),
                local_mob_id.clone(),
                || async { Ok(()) },
            )
            .await?;
            let sent = match sender {
                Some(sender) => {
                    identity_runtime
                        .send_through_identity_door_as(
                            &identity,
                            &remote_member_id,
                            &content,
                            sender,
                        )
                        .await
                }
                None => {
                    identity_runtime
                        .send_through_identity_door(&identity, &remote_member_id, &content)
                        .await
                }
            };
            return sent.map_err(|error| CrossMobError::IdentityAuthority {
                member_id: remote_member_id.clone(),
                mob_id: remote_mob_id.clone(),
                message: error.to_string(),
            });
        }
        run_member_authority_transaction(
            [local_target, remote_target],
            format!("{from_local_member} -> {remote_member_id}"),
            format!("{local_mob_id} -> {remote_mob_id}"),
            move || async move {
                match remote {
                    LocalOrRemote::Local(authority) => {
                        let remote_member_id = resolve_member_alias_under_authority(
                            &authority.handle,
                            authority.identity_runtime.as_ref(),
                            remote_identity_authoritative,
                            &remote_member_id,
                            &remote_mob_id,
                        )
                        .await?;
                        send_member_unchecked(
                            authority.handle,
                            &remote_member_id,
                            &remote_mob_id,
                            content,
                        )
                        .await
                    }
                    LocalOrRemote::Remote(proxy) => {
                        let content_json = serde_json::to_value(&content).map_err(|err| {
                            CrossMobError::PeerSpec(format!(
                                "failed to serialize content for remote inject: {err}"
                            ))
                        })?;
                        proxy
                            .inject_message(&remote_member_id, content_json)
                            .await
                            .map_err(CrossMobError::Remote)
                    }
                }
            },
        )
        .await
    }

    /// List external mobs from the contact directory.
    pub fn list_external_mobs(&self) -> Vec<ContactEntry> {
        self.contact_directory
            .as_ref()
            .map(|d| d.list().into_iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether a contact directory is configured (cross-mob operations available).
    pub fn has_contact_directory(&self) -> bool {
        self.contact_directory.is_some()
    }

    /// Whether any peer mob handles are registered (required for
    /// high-level cross-mob wire/unwire/send).
    pub async fn has_peer_mob_handles(&self) -> bool {
        !self.peer_mob_handles.read().await.is_empty()
    }

    /// Whether the contact directory has any inproc entries.
    pub fn has_inproc_contacts(&self) -> bool {
        self.contact_directory.as_ref().is_some_and(|d| {
            d.list()
                .iter()
                .any(|e| matches!(e.transport, MobTransport::Inproc))
        })
    }

    /// Whether the contact directory has any cross-process (TCP/UDS) entries.
    /// Useful for opting into the remote-mob proxy code path.
    pub fn has_remote_contacts(&self) -> bool {
        self.contact_directory.as_ref().is_some_and(|d| {
            d.list()
                .iter()
                .any(|e| matches!(e.transport, MobTransport::Tcp(_) | MobTransport::Uds(_)))
        })
    }

    /// Return the local mob's ID.
    pub fn mob_id(&self) -> String {
        self.mob_runtime.handle().mob_id().to_string()
    }

    /// Get comms peer info for a member of this mob.
    /// Returns `(peer_id, comms_name, address)`. The address is
    /// `inproc://{comms_name}` for a member whose runtime is in this process
    /// (a peer in another process wires the member by its own socket
    /// endpoint and key, which the host RPC's peer info reports), and the
    /// durable host-acknowledged address for a
    /// placed member. A placed member without a usable endpoint is
    /// `CrossMobError::PlacedMemberUnavailable`.
    pub async fn local_member_peer_info(
        &self,
        member_id: &str,
    ) -> Result<(String, String, String), CrossMobError> {
        let facts = self.local_member_peer_facts(member_id).await?;
        Ok((facts.peer_id, facts.comms_name, facts.address))
    }

    /// [`Self::local_member_peer_info`] with what a peer in ANOTHER process
    /// needs to describe the member: its transport public key, and its
    /// socket endpoint when it has one (`member_comms_address`, or a placed
    /// member's host-acknowledged address).
    pub(crate) async fn local_member_peer_facts(
        &self,
        member_id: &str,
    ) -> Result<LocalMemberPeerFacts, CrossMobError> {
        let handle = self.mob_runtime.handle();
        let mob_runtime = self.mob_runtime.clone();
        let mob_id = handle.mob_id().to_string();
        let member_alias = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
        let identity_runtime = self.identity_runtime_ref().cloned();
        let target = member_lifecycle_target_with_authority(
            identity_runtime.as_ref(),
            &member_alias,
            &mob_id,
        )
        .await?;
        let identity_authoritative = target.is_some();
        let operation_alias = member_alias.clone();
        let operation_mob_id = mob_id.clone();
        run_member_authority_transaction([target], member_alias, mob_id, move || async move {
            let current_alias = resolve_member_alias_under_authority(
                &handle,
                identity_runtime.as_ref(),
                identity_authoritative,
                &operation_alias,
                &operation_mob_id,
            )
            .await?;
            let mid = crate::member_comms_id::mob_member_id(&current_alias);
            let info = member_peer_info(&handle, &mid, &operation_mob_id).await?;
            // A placed member reports its durable host address; a local
            // member its in-process route.
            let address = crate::runtime::cross_mob_control::member_peer_info_address(
                &handle,
                &mid,
                &info.comms_name,
            )
            .await
            .map_err(|fault| plane_fault_error(fault, &handle, &mid))?;
            // An in-process address reaches only this process; a member
            // without a socket endpoint has no dialable address to report.
            let dialable_address = member_comms_advertised_address(&mob_runtime, &handle, &mid)
                .await?
                .filter(|advertised| !advertised.starts_with("inproc://"));
            Ok(LocalMemberPeerFacts {
                peer_id: info.peer_id,
                comms_name: info.comms_name,
                address,
                pubkey_b64: info
                    .pubkey_b64
                    .trim()
                    .trim_start_matches("ed25519:")
                    .to_string(),
                dialable_address,
            })
        })
        .await
    }

    /// Wire a local member to an external peer using provided comms info.
    /// Only wires the local side — for the bidirectional wire, call this
    /// on both gateways.
    ///
    /// `remote_address` is the comms transport address (e.g. `"inproc://name"`
    /// for same-process, `"tcp://host:port"` for cross-process).
    /// `remote_pubkey` is the remote MEMBER's 32-byte Ed25519 transport
    /// key, the key its peer info reports and its peer id is derived from
    /// (never a gateway's key). An inproc descriptor of a live member of
    /// another mob in this process may pass `None`. Non-inproc transports
    /// MUST supply a non-zero pubkey: this call fails closed with
    /// [`CrossMobError::MissingPeerPubkey`] otherwise so unsigned
    /// descriptors never reach a real transport.
    ///
    /// When the in-process peer is a live member of another mob in this
    /// process, the call also installs the route by which this mob's
    /// namespace reaches that member, and describes the member by its own
    /// registered key (so `remote_pubkey` may be `None`): two such calls,
    /// one on each mob, are a working edge. The registered key must derive
    /// `remote_peer_id`; another peer id under that name is refused with
    /// [`CrossMobError::InprocAlias`] and nothing changes.
    ///
    /// Wiring the identical edge again succeeds and changes nothing. A wire
    /// is never replaced: when the member is already wired to that peer
    /// name with another endpoint, the call is refused with
    /// [`CrossMobError::PeerSpec`] (unwire both sides first) and nothing
    /// changes.
    pub async fn wire_local(
        &self,
        local_member_id: &str,
        remote_comms_name: &str,
        remote_peer_id: &str,
        remote_address: &str,
        remote_pubkey: Option<[u8; 32]>,
    ) -> Result<(), CrossMobError> {
        self.wire_local_with_identity_runtime(
            local_member_id,
            remote_comms_name,
            remote_peer_id,
            remote_address,
            remote_pubkey,
            None,
        )
        .await
    }

    pub(crate) async fn wire_local_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_comms_name: &str,
        remote_peer_id: &str,
        remote_address: &str,
        remote_pubkey: Option<[u8; 32]>,
        identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), CrossMobError> {
        let spec = build_external_peer_spec(
            remote_comms_name,
            remote_peer_id,
            remote_address,
            remote_pubkey,
        )?;
        let member_alias = local_member_id.to_string();
        // The link this wire declares is held in the runtime's part of the
        // link table, which its start-time read creates: a no-op here for
        // every runtime whose mob was started through its bootstrap.
        self.restore_cross_mob_wake_routes().await;
        with_member_under_authority(
            self.mob_runtime.handle(),
            identity_runtime.or_else(|| self.identity_runtime_ref()),
            local_member_id,
            &self.mob_id(),
            move |handle, current_member_id| async move {
                wire_local_edge(
                    &handle,
                    local_roster_member(&current_member_id),
                    &member_alias,
                    spec,
                    SameProcessPeers::Resolve,
                )
                .await
            },
        )
        .await
    }

    /// Undo a `wire_local` — unwire a local member from a previously wired peer.
    /// Only affects the local side; the remote side is left unchanged. The
    /// route this mob holds for a same-process peer goes with the trust,
    /// unless another member of this mob is still wired to that peer.
    pub async fn unwire_local(
        &self,
        local_member_id: &str,
        remote_comms_name: &str,
        remote_peer_id: &str,
        remote_address: &str,
        remote_pubkey: Option<[u8; 32]>,
    ) -> Result<(), CrossMobError> {
        self.unwire_local_with_identity_runtime(
            local_member_id,
            remote_comms_name,
            remote_peer_id,
            remote_address,
            remote_pubkey,
            None,
        )
        .await
    }

    pub(crate) async fn unwire_local_with_identity_runtime(
        &self,
        local_member_id: &str,
        remote_comms_name: &str,
        remote_peer_id: &str,
        remote_address: &str,
        remote_pubkey: Option<[u8; 32]>,
        identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<(), CrossMobError> {
        let spec = build_external_peer_spec(
            remote_comms_name,
            remote_peer_id,
            remote_address,
            remote_pubkey,
        )?;
        with_member_under_authority(
            self.mob_runtime.handle(),
            identity_runtime.or_else(|| self.identity_runtime_ref()),
            local_member_id,
            &self.mob_id(),
            move |handle, current_member_id| async move {
                unwire_local_edge(&handle, local_roster_member(&current_member_id), spec).await
            },
        )
        .await
    }

    // -- internal helpers --

    fn resolve_contact(&self, mob_id: &str) -> Result<ContactEntry, CrossMobError> {
        let dir = self
            .contact_directory
            .as_ref()
            .ok_or(CrossMobError::NoContactDirectory)?;
        dir.get(mob_id)
            .cloned()
            .ok_or_else(|| CrossMobError::UnknownMob(mob_id.to_string()))
    }

    /// Pick the appropriate dispatch arm for a contact entry.
    ///
    /// Order of preference:
    /// 1. **Local** — if a `MobHandle` was registered via
    ///    [`Self::register_peer_mob`] for this mob_id, use it directly
    ///    (covers the inproc + same-process-test paths).
    /// 2. **Remote** — for TCP/UDS contact entries with no registered
    ///    handle, build a [`RemoteMobProxy`].
    /// 3. **Error** — inproc entry with no registered handle, or unknown
    ///    transport.
    async fn dispatch_for(&self, entry: &ContactEntry) -> Result<LocalOrRemote, CrossMobError> {
        if let Some(authority) = self
            .peer_mob_handles
            .read()
            .await
            .get(&entry.mob_id)
            .cloned()
        {
            return Ok(LocalOrRemote::Local(Box::new(authority)));
        }
        // Authenticate ourselves to the peer with this gateway's keypair
        // so a peer that enforces scoped control grants can attribute the
        // call. `None` (no keypair installed) still dispatches: peers that
        // do not enforce grants are unaffected, and peers that do refuse
        // us typed rather than silently.
        match RemoteMobProxy::from_entry_with_caller(entry, self.gateway_peer_keys())? {
            Some(proxy) => Ok(LocalOrRemote::Remote(proxy)),
            None => Err(CrossMobError::NoPeerHandle(entry.mob_id.clone())),
        }
    }

    /// Resolve a member's peer_id, comms name, and transport key from the roster entry.
    ///
    /// Returns peer id, comms name, and the member transport key. The comms
    /// name is built through `meerkat_core::MemberCommsName::new`, the single
    /// fail-closed owner meerkat-mob routes all such names through
    /// (`render_member_comms_name`). It validates each of the three components
    /// against the identifier-safe slug rule and renders `{mob_id}/{role}/{member}`.
    /// Routing through the typed owner (rather than a raw `format!`) means a
    /// slug-invalid `mob_id`/`role` is rejected here with a clear error instead
    /// of minting a descriptor that silently fails to match at comms ingress.
    async fn get_member_peer_info(
        &self,
        handle: &MobHandle,
        meerkat_id: &AgentIdentity,
        mob_id: &str,
    ) -> Result<MemberPeerInfo, CrossMobError> {
        let entry = handle
            .get_member(meerkat_id)
            .await
            .map_err(|err| {
                CrossMobError::PeerSpec(format!(
                    "member lookup for '{meerkat_id}' in mob '{mob_id}' failed: {err}"
                ))
            })?
            .ok_or_else(|| CrossMobError::MemberNotFound {
                member_id: meerkat_id.to_string(),
                mob_id: mob_id.to_string(),
            })?;
        let peer_id = entry
            .peer_id()
            .ok_or_else(|| CrossMobError::NoCommsInfo {
                member_id: meerkat_id.to_string(),
                mob_id: mob_id.to_string(),
            })?
            .to_string();
        let pubkey_b64 = entry.transport_public_key().ok_or_else(|| {
            CrossMobError::PeerSpec(format!(
                "member '{meerkat_id}' in mob '{mob_id}' has no transport public key"
            ))
        })?;
        let pubkey = crate::auth::peer_keys::decode_pubkey_b64(pubkey_b64).map_err(|err| {
            CrossMobError::PeerSpec(format!(
                "member '{meerkat_id}' in mob '{mob_id}' has invalid transport public key: {err}"
            ))
        })?;
        let comms_name = meerkat_core::MemberCommsName::new(
            mob_id,
            entry.role.as_str(),
            meerkat_id.as_str(),
        )
        .map_err(|err| {
            CrossMobError::PeerSpec(format!(
                "member '{meerkat_id}' in mob '{mob_id}' has an invalid comms name component: {err}"
            ))
        })?
        .to_string();
        Ok(MemberPeerInfo {
            peer_id,
            comms_name,
            pubkey,
            pubkey_b64: pubkey_b64.to_string(),
        })
    }

    /// Observe the exact directory consumed by the member's `peers` and
    /// `send_message` tools. Roster `wired_to` is durable graph projection,
    /// not proof that the live session trust store accepted the descriptor.
    async fn agent_can_address_peer(
        &self,
        handle: &MobHandle,
        local_member: &AgentIdentity,
        expected_peer: &MemberPeerInfo,
    ) -> Result<bool, CrossMobError> {
        if let Some(readiness) =
            placed_member_edge_committed(handle, local_member, expected_peer).await?
        {
            return Ok(readiness == PlacedEdgeReadiness::Ready);
        }
        let session_id = handle
            .resolve_bridge_session_id(local_member)
            .await
            .ok_or_else(|| CrossMobError::NoCommsInfo {
                member_id: local_member.to_string(),
                mob_id: handle.mob_id().to_string(),
            })?;
        let service =
            self.mob_runtime
                .session_service()
                .ok_or_else(|| CrossMobError::NoCommsInfo {
                    member_id: local_member.to_string(),
                    mob_id: handle.mob_id().to_string(),
                })?;
        let comms =
            service
                .comms_runtime(&session_id)
                .await
                .ok_or_else(|| CrossMobError::NoCommsInfo {
                    member_id: local_member.to_string(),
                    mob_id: handle.mob_id().to_string(),
                })?;
        let peers = comms.peers().await;
        Ok(peers.iter().any(|peer| {
            peer.peer_id.to_string() == expected_peer.peer_id
                && peer.name.as_str() == expected_peer.comms_name
                && peer
                    .sendable_kinds
                    .contains(&meerkat_core::comms::PeerSendability::PeerMessage)
        }))
    }
}

/// Build a `TrustedPeerDescriptor` whose address reflects the supplied
/// transport. **Routes through [`build_external_peer_spec`] so the
/// pubkey requirement is enforced**: inproc descriptors stay unsigned
/// (the in-process router authorizes via its identity map), but TCP and
/// UDS peers must have a non-zero 32-byte pubkey or the call fails
/// closed with `CrossMobError::MissingPeerPubkey`.
///
/// `pubkey` is `None` for the local-half descriptor (always inproc) and
/// `entry.pubkey` for the remote-half descriptor.
fn build_peer_spec(
    comms_name: &str,
    peer_id: &str,
    transport: &MobTransport,
    pubkey: Option<[u8; 32]>,
) -> Result<TrustedPeerDescriptor, CrossMobError> {
    let address = match transport {
        MobTransport::Inproc => format!("inproc://{comms_name}"),
        MobTransport::Tcp(addr) => format!("tcp://{addr}"),
        MobTransport::Uds(path) => format!("uds://{path}"),
    };
    build_external_peer_spec(comms_name, peer_id, &address, pubkey)
}

/// Build a [`TrustedPeerDescriptor`] for an external (non-inproc) peer.
///
/// The address scheme decides the policy:
///
/// * `inproc://...` — keep behaviour aligned with
///   [`build_inproc_peer_spec`]: pubkey is optional and an unsigned
///   descriptor is acceptable.
/// * `tcp://...` / `uds://...` — fail closed unless a non-zero 32-byte
///   pubkey is supplied. meerkat-comms keys its trust store by pubkey;
///   admitting an all-zero pubkey would let any sender in.
fn build_external_peer_spec(
    comms_name: &str,
    peer_id: &str,
    address: &str,
    pubkey: Option<[u8; 32]>,
) -> Result<TrustedPeerDescriptor, CrossMobError> {
    let is_inproc = address.starts_with("inproc://");
    match (is_inproc, pubkey) {
        (true, None) => TrustedPeerDescriptor::test_only_unsigned(comms_name, peer_id, address)
            .map_err(CrossMobError::PeerSpec),
        (true, Some(bytes)) => {
            TrustedPeerDescriptor::unsigned_with_pubkey(comms_name, peer_id, bytes, address)
                .map_err(CrossMobError::PeerSpec)
        }
        (false, None) => Err(CrossMobError::MissingPeerPubkey { mob_id: None }),
        (false, Some(bytes)) => {
            if bytes == [0u8; 32] {
                return Err(CrossMobError::MissingPeerPubkey { mob_id: None });
            }
            TrustedPeerDescriptor::unsigned_with_pubkey(comms_name, peer_id, bytes, address)
                .map_err(CrossMobError::PeerSpec)
        }
    }
}

/// Build an UNSIGNED TCP peer descriptor (test/fixture helper).
///
/// Uses the comms-layer address scheme `tcp://host:port` and goes through
/// [`TrustedPeerDescriptor::test_only_unsigned`], so the result carries no
/// pubkey and is rejected by every fail-closed wire path in this module.
/// Production callers get signed descriptors from the wire/lookup flow
/// (which routes through `build_external_peer_spec` with a real pubkey);
/// this helper exists for tests that assert address canonicalization.
pub fn build_tcp_peer_spec(
    comms_name: &str,
    peer_id: &str,
    address: &str,
) -> Result<TrustedPeerDescriptor, CrossMobError> {
    TrustedPeerDescriptor::test_only_unsigned(comms_name, peer_id, format!("tcp://{address}"))
        .map_err(CrossMobError::PeerSpec)
}

/// Build an UNSIGNED UDS peer descriptor (test/fixture helper).
///
/// Uses the comms-layer address scheme `uds:///path` (triple slash -
/// `uds://` + absolute path). See [`build_tcp_peer_spec`] for why this
/// stays unsigned and what production callers use instead.
pub fn build_uds_peer_spec(
    comms_name: &str,
    peer_id: &str,
    path: &str,
) -> Result<TrustedPeerDescriptor, CrossMobError> {
    let normalized = if let Some(stripped) = path.strip_prefix('/') {
        stripped
    } else {
        path
    };
    TrustedPeerDescriptor::test_only_unsigned(comms_name, peer_id, format!("uds:///{normalized}"))
        .map_err(CrossMobError::PeerSpec)
}

fn mob_inproc_namespace(runtime: &UnifiedRuntime) -> Result<String, CrossMobError> {
    mob_inproc_namespace_for_id(&runtime.mob_id())
}

pub(super) fn mob_inproc_namespace_for_id(mob_id: &str) -> Result<String, CrossMobError> {
    meerkat_core::mob_realm_id(mob_id)
        .map(|realm| realm.as_str().to_string())
        .map_err(|error| CrossMobError::InprocAlias(error.to_string()))
}

fn alias_already_installed(namespace: &str, name: &str, pubkey: PubKey) -> bool {
    InprocRegistry::global()
        .peers_in_namespace(namespace)
        .into_iter()
        .any(|peer| peer.name == name && peer.pubkey == pubkey)
}

fn ensure_alias_slot(
    namespace: &str,
    canonical_namespace: &str,
    name: &str,
    pubkey: PubKey,
) -> Result<(), String> {
    let registry = InprocRegistry::global();
    for peer in InprocRegistry::global().peers_in_namespace(namespace) {
        if peer.name == name && peer.pubkey != pubkey {
            let old_route_is_live = registry
                .peers_in_namespace(canonical_namespace)
                .into_iter()
                .any(|canonical| canonical.name == name && canonical.pubkey == peer.pubkey);
            if old_route_is_live {
                return Err(format!(
                    "namespace {namespace:?} already binds name {name:?} to another live peer"
                ));
            }
            // A clean runtime restart rotates the member pubkey and removes
            // its canonical route, but an opposite-namespace alias can
            // survive. Remove only when the old pubkey is no longer canonical
            // in the target namespace; never displace a live peer.
            registry.unregister_in_namespace(namespace, &peer.pubkey);
            continue;
        }
        if peer.pubkey == pubkey && peer.name != name {
            let canonical_name_matches = registry
                .peers_in_namespace(canonical_namespace)
                .into_iter()
                .any(|canonical| canonical.name == name && canonical.pubkey == pubkey);
            if !canonical_name_matches {
                return Err(format!(
                    "namespace {namespace:?} already binds peer {pubkey:?} as {:?}",
                    peer.name
                ));
            }
            registry.unregister_in_namespace(namespace, &peer.pubkey);
        }
    }
    Ok(())
}

/// Install the route by which `namespace` reaches the member `name` of
/// `canonical_namespace`: an alias of that member's own inbox. `Ok(true)`
/// when this call installed it, `Ok(false)` when it was already there (it is
/// then re-pointed at the member's inbox of now).
/// Refuses displacement: a wire must never evict an unrelated inproc route.
///
/// The one owner of a cross-namespace route install: the bilateral
/// same-process wire installs both directions through it, and a one-sided
/// local wire installs its own direction.
pub(super) fn install_route_alias(
    namespace: &str,
    canonical_namespace: &str,
    name: &str,
    pubkey: PubKey,
    side: &str,
) -> Result<bool, RouteNotInstalled> {
    let registry = InprocRegistry::global();
    ensure_alias_slot(namespace, canonical_namespace, name, pubkey)
        .map_err(RouteNotInstalled::Refused)?;
    let sender = registry
        .get_by_pubkey_in_namespace(canonical_namespace, &pubkey)
        .ok_or_else(|| member_not_there(side, name, canonical_namespace))?;
    register_route_alias(namespace, canonical_namespace, name, pubkey, sender, side)
}

/// Why [`install_route_alias`] installed no route.
pub(super) enum RouteNotInstalled {
    /// The member is not registered in its own namespace now: it is not
    /// built, or its inbox left while the route was being installed (the
    /// registry refuses a route to an inbox that left). Nothing is wrong
    /// with the installer: the member's next wake installs its route.
    MemberNotThere(String),
    /// The route would displace another entry, or the registry refused it
    /// for another reason.
    Refused(String),
}

impl RouteNotInstalled {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::MemberNotThere(message) | Self::Refused(message) => message,
        }
    }
}

fn member_not_there(side: &str, name: &str, canonical_namespace: &str) -> RouteNotInstalled {
    RouteNotInstalled::MemberNotThere(format!(
        "{side} peer {name:?} is not registered in {canonical_namespace:?}"
    ))
}

/// The second step of [`install_route_alias`]: register `sender`, the inbox
/// read for the member, as the route of `namespace`. The member can leave
/// between the read and this step (it is released to rest, or replaced);
/// the registry then refuses the route, and the answer is the one for a
/// member that is not there.
fn register_route_alias(
    namespace: &str,
    canonical_namespace: &str,
    name: &str,
    pubkey: PubKey,
    sender: meerkat_comms::InboxSender,
    side: &str,
) -> Result<bool, RouteNotInstalled> {
    let registry = InprocRegistry::global();
    // A route that is already there is re-pointed at the member's inbox of
    // now. The registry removes a route with the inbox it leads to when that
    // inbox leaves through its own runtime, so what is found here is the
    // member's current inbox or a route somebody registered to another one.
    // It stays the earlier wire's route (`Ok(false)`).
    let already_installed = alias_already_installed(namespace, name, pubkey);
    if already_installed {
        registry.unregister_in_namespace(namespace, &pubkey);
    }
    let outcome = registry.register_with_meta_in_namespace(
        namespace,
        name,
        pubkey,
        sender,
        PeerMeta::default(),
    );
    match outcome {
        meerkat_comms::RegistrationOutcome::Registered => Ok(!already_installed),
        // The member's inbox left after it was read: the member is not
        // there now, as when the read finds nothing.
        meerkat_comms::RegistrationOutcome::Rejected {
            reason: meerkat_comms::RegistrationRejection::InboxGone,
        } => Err(member_not_there(side, name, canonical_namespace)),
        // 0.8.23: name-occupied displacement fails closed as
        // Rejected(NameOccupied); ReplacedPubkey is the one surviving
        // displacement shape (same pubkey evicting its own old name) and
        // keeps meta's fail-closed posture here. Any other refusal of the
        // registry is not installed either.
        outcome => Err(RouteNotInstalled::Refused(format!(
            "{side} alias installation failed: {outcome:?}"
        ))),
    }
}

/// Install the routing aliases that make two isolated mob namespaces able to
/// deliver the structurally trusted cross-runtime edge. Refuses displacement:
/// topology mutation must never evict an unrelated inproc route. A failed
/// second direction removes the first when this call installed it.
fn register_cross_namespace_aliases(
    source_namespace: &str,
    source_comms_name: &str,
    source_pubkey: [u8; 32],
    target_namespace: &str,
    target_comms_name: &str,
    target_pubkey: [u8; 32],
) -> Result<(), String> {
    let source_pubkey = PubKey::new(source_pubkey);
    let target_pubkey = PubKey::new(target_pubkey);
    let target_installed = install_route_alias(
        source_namespace,
        target_namespace,
        target_comms_name,
        target_pubkey,
        "target",
    )
    .map_err(RouteNotInstalled::into_message)?;
    if let Err(error) = install_route_alias(
        target_namespace,
        source_namespace,
        source_comms_name,
        source_pubkey,
        "source",
    )
    .map_err(RouteNotInstalled::into_message)
    {
        if target_installed {
            InprocRegistry::global().unregister_in_namespace(source_namespace, &target_pubkey);
        }
        return Err(error);
    }
    Ok(())
}

/// The route a one-sided local wire (`wire_local`) installed in the local
/// mob's namespace, so a failed trust install can take back exactly that.
pub(crate) struct LocalWireRoute {
    namespace: String,
    pubkey: PubKey,
    installed: bool,
}

impl LocalWireRoute {
    /// Remove the route when [`install_local_wire_route`] installed it. A
    /// route that was already there belongs to an earlier wire and stays.
    pub(crate) fn roll_back(self) {
        if self.installed {
            InprocRegistry::global().unregister_in_namespace(&self.namespace, &self.pubkey);
        }
    }

    /// Remove the route after the local member was unwired from `peer_name`.
    /// The route stays while another member of the local mob is still wired
    /// to that peer, as on the bilateral unwire.
    pub(crate) async fn remove_unless_referenced(self, handle: &MobHandle, peer_name: &str) {
        if !handle_references_peer(handle, peer_name).await {
            InprocRegistry::global().unregister_in_namespace(&self.namespace, &self.pubkey);
        }
    }
}

/// The namespace of the mob an in-process descriptor's comms name belongs
/// to: `None` for a socket address or a name that is no member comms name.
pub(super) fn inproc_peer_namespace(spec: &TrustedPeerDescriptor) -> Option<String> {
    if !matches!(
        spec.address.transport,
        meerkat_core::comms::PeerTransport::Inproc
    ) {
        return None;
    }
    let name = spec
        .name
        .as_str()
        .parse::<meerkat_core::MemberCommsName>()
        .ok()?;
    mob_inproc_namespace_for_id(name.mob_id()).ok()
}

/// The local mob's namespace and the namespace of the mob `spec` names, when
/// `spec` is an in-process descriptor of a member of ANOTHER mob. A member of
/// the local mob is routed by its own registration and needs no alias.
fn other_mob_namespaces(
    local_mob_id: &str,
    spec: &TrustedPeerDescriptor,
) -> Result<Option<(String, String)>, CrossMobError> {
    let Some(peer_namespace) = inproc_peer_namespace(spec) else {
        return Ok(None);
    };
    let local_namespace = mob_inproc_namespace_for_id(local_mob_id)?;
    if peer_namespace == local_namespace {
        return Ok(None);
    }
    Ok(Some((local_namespace, peer_namespace)))
}

/// The route half of a one-sided local wire. Trust alone does not deliver
/// between two isolated mobs of one process: the sending mob's namespace
/// must route the peer's name to the peer's inbox, as the bilateral
/// same-process wire arranges for both directions.
///
/// When `spec` names a live member of another mob in this process, this
/// installs that route in the local mob's namespace and returns the
/// descriptor carrying the member's own key (a caller that only has the
/// member's peer info has no key to supply). The registered key must derive
/// the peer id the caller named, and equal a key the caller supplied: a name
/// alone never selects an inbox. A mismatch is refused before anything is
/// installed.
///
/// A member comms name that no mob of this process has live is left as it
/// is when the caller supplied a key (no route, the descriptor unchanged).
/// Without a key it is refused in plain words: an in-process address
/// reaches no other process, and there is no key to describe the member by.
pub(crate) fn install_local_wire_route(
    local_mob_id: &str,
    spec: TrustedPeerDescriptor,
) -> Result<(TrustedPeerDescriptor, Option<LocalWireRoute>), CrossMobError> {
    let Some(peer_namespace) = inproc_peer_namespace(&spec) else {
        return Ok((spec, None));
    };
    let local_namespace = mob_inproc_namespace_for_id(local_mob_id)?;
    if peer_namespace == local_namespace {
        // A member of this mob: its own registration routes it, and its
        // mob's own wiring wires it. Without a key there is nothing to
        // describe it by here either.
        if spec.pubkey == [0u8; 32] {
            return Err(CrossMobError::PeerSpec(format!(
                "{:?} is a member of this mob: wire_local wires a member to a peer of another \
                 mob, and two members of one mob are wired by the mob's own wiring",
                spec.name.as_str()
            )));
        }
        return Ok((spec, None));
    }
    let Some(member) = InprocRegistry::global()
        .peers_in_namespace(&peer_namespace)
        .into_iter()
        .find(|peer| peer.name == spec.name.as_str())
    else {
        if spec.pubkey == [0u8; 32] {
            return Err(not_live_in_this_process(&spec));
        }
        return Ok((spec, None));
    };
    let key = *member.pubkey.as_bytes();
    if member.pubkey.to_peer_id() != spec.peer_id
        || (spec.pubkey != [0u8; 32] && spec.pubkey != key)
    {
        return Err(CrossMobError::InprocAlias(format!(
            "{:?} is a live member of this process under another peer id or key; read its peer info again",
            spec.name.as_str()
        )));
    }
    // The route is an entry of the local namespace under the member's key.
    // A key the local namespace holds under another name is somebody else's
    // entry (a member of this mob, or another route): the wire is refused
    // and that entry stays.
    if InprocRegistry::global()
        .peers_in_namespace(&local_namespace)
        .into_iter()
        .any(|peer| peer.pubkey == member.pubkey && peer.name != spec.name.as_str())
    {
        return Err(CrossMobError::InprocAlias(format!(
            "this mob already holds that key under another name, so {:?} cannot be wired in-process",
            spec.name.as_str()
        )));
    }
    let installed = match install_route_alias(
        &local_namespace,
        &peer_namespace,
        spec.name.as_str(),
        member.pubkey,
        "remote",
    ) {
        Ok(installed) => installed,
        // The member left while its route was being installed (it was
        // released to rest): the answer is the one for a member that is not
        // built, above.
        Err(RouteNotInstalled::MemberNotThere(_)) => {
            if spec.pubkey == [0u8; 32] {
                return Err(not_live_in_this_process(&spec));
            }
            return Ok((spec, None));
        }
        Err(RouteNotInstalled::Refused(error)) => return Err(CrossMobError::InprocAlias(error)),
    };
    Ok((
        spec.with_pubkey(key),
        Some(LocalWireRoute {
            namespace: local_namespace,
            pubkey: member.pubkey,
            installed,
        }),
    ))
}

/// The refusal of a keyless in-process wire to a member comms name that no
/// mob of this process has live. It is the answer for a name no mob has at
/// all, too: the two are not told apart.
fn not_live_in_this_process(spec: &TrustedPeerDescriptor) -> CrossMobError {
    CrossMobError::PeerSpec(format!(
        "{:?} is not a live member of a mob in this process. If this process hosts it and it is \
         not running, pass its key (remote_pubkey_b64, from its peer info). If another process \
         hosts it, an in-process address does not reach it: wire it by the dialable address and \
         key its peer info reports (a member gets a dialable address from its gateway's \
         member_comms_address; member_comms_tcp() or memberCommsTcp() in the SDKs)",
        spec.name.as_str()
    ))
}

/// [`install_local_wire_route`] for a caller that must learn nothing about
/// the other mobs of this process: the descriptor is treated as naming a
/// member no mob of this process has. No route is installed and no key is
/// filled in; a keyless in-process descriptor of a member comms name gets
/// the refusal of [`not_live_in_this_process`], word for word.
pub(crate) fn local_wire_without_route(
    local_mob_id: &str,
    spec: TrustedPeerDescriptor,
) -> Result<(TrustedPeerDescriptor, Option<LocalWireRoute>), CrossMobError> {
    if other_mob_namespaces(local_mob_id, &spec)?.is_some() && spec.pubkey == [0u8; 32] {
        return Err(not_live_in_this_process(&spec));
    }
    Ok((spec, None))
}

/// Whether a one-sided local wire may look at the other mobs of this
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SameProcessPeers {
    /// The operator's wire: a live member of another mob in this process is
    /// described by its own key and routed.
    Resolve,
    /// A caller that must learn nothing about another mob: every name is
    /// treated as one no mob of this process has
    /// ([`local_wire_without_route`]).
    Unresolved,
}

/// A wire is never replaced. When `member` is already wired to `spec`'s peer
/// name with ANOTHER endpoint (another peer id, so another key, or another
/// address), the wire is refused in plain words that name the remedy, before
/// anything is installed. The identical edge passes: wiring it again changes
/// nothing.
async fn refuse_replacing_wire(
    handle: &MobHandle,
    member: &AgentIdentity,
    member_alias: &str,
    spec: &TrustedPeerDescriptor,
) -> Result<(), CrossMobError> {
    let Some(entry) = handle
        .get_member(member)
        .await
        .map_err(CrossMobError::Mob)?
    else {
        // No such member: the wire itself answers that.
        return Ok(());
    };
    let Some(existing) = entry.external_peer_descriptor(spec.name.as_str()) else {
        return Ok(());
    };
    // A keyless in-process descriptor names its key by its peer id.
    let same_key = spec.pubkey == [0u8; 32] || existing.pubkey == spec.pubkey;
    if existing.peer_id == spec.peer_id && existing.address == spec.address && same_key {
        return Ok(());
    }
    Err(CrossMobError::PeerSpec(format!(
        "{member_alias:?} is already wired to {:?} with another endpoint (peer id {} at {}), and \
         a wire is never replaced: unwire both sides first (unwire_local on each side, with the \
         other member's current peer info), then wire again",
        spec.name.as_str(),
        existing.peer_id,
        existing.address,
    )))
}

/// The local half of a one-sided wire, for a roster member already resolved
/// under its authority: refuse a wire that would replace an edge, install
/// the route of a same-process peer, then the trust; a failed trust takes
/// back the route this call installed. Once the wire is committed, the
/// member's links to in-process members of other mobs are read from its row
/// (`cross_mob_routes`): a wire to a member that is not built holds its link
/// from here on, so a peer message can wake that member. The one body of
/// `UnifiedRuntime::wire_local` and the Console's `wire_local`.
pub(crate) async fn wire_local_edge(
    handle: &MobHandle,
    member: AgentIdentity,
    member_alias: &str,
    spec: TrustedPeerDescriptor,
    peers: SameProcessPeers,
) -> Result<(), CrossMobError> {
    refuse_replacing_wire(handle, &member, member_alias, &spec).await?;
    let mob_id = handle.mob_id().to_string();
    // Route first, then trust: a route without trust delivers nothing, and
    // taking it back needs no second member mutation.
    let (spec, route) = match peers {
        SameProcessPeers::Resolve => install_local_wire_route(&mob_id, spec)?,
        SameProcessPeers::Unresolved => local_wire_without_route(&mob_id, spec)?,
    };
    // A key a member of this mob is seated under is never routed to
    // another mob's member, built or not.
    let wired = match super::cross_mob_routes::refuse_locally_seated_key(&mob_id, &spec) {
        Ok(()) => handle
            .wire(member.clone(), PeerTarget::External(spec))
            .await
            .map_err(CrossMobError::Mob),
        Err(refusal) => Err(refusal),
    };
    match &wired {
        Ok(()) => super::cross_mob_routes::sync_member_links(handle, &member).await,
        Err(_) => {
            if let Some(route) = route {
                route.roll_back();
            }
        }
    }
    wired
}

/// The local half of an unwire, for a roster member already resolved under
/// its authority: the trust goes, and the route this mob's namespace holds
/// under the peer's name goes with it unless another member of the mob is
/// still wired to that peer. The member's link to an in-process member of
/// another mob goes first, before the row changes (`cross_mob_routes`); an
/// unwire that fails reads the link back from the row. The one body of
/// `UnifiedRuntime::unwire_local` and the Console's `unwire_local`.
pub(crate) async fn unwire_local_edge(
    handle: &MobHandle,
    member: AgentIdentity,
    spec: TrustedPeerDescriptor,
) -> Result<(), CrossMobError> {
    let mob_id = handle.mob_id().to_string();
    let (spec, route) = installed_local_wire_route(&mob_id, spec)?;
    let peer_name = spec.name.as_str().to_string();
    super::cross_mob_routes::drop_member_link(&mob_id, &member, &peer_name);
    if let Err(error) = handle
        .unwire(member.clone(), PeerTarget::External(spec))
        .await
    {
        // The edge is still in the row, so the link is still held.
        super::cross_mob_routes::sync_member_links(handle, &member).await;
        return Err(CrossMobError::Mob(error));
    }
    if let Some(route) = route {
        route.remove_unless_referenced(handle, &peer_name).await;
    }
    Ok(())
}

/// The route this mob's namespace holds under `spec`'s peer name, found
/// before the local member is unwired from it, and the descriptor carrying
/// that route's key when the caller supplied none and named the route's own
/// peer id (the wire stored the member under its own key). The route is
/// found by name: the trust is removed by name too, so an unwire with a peer
/// id of another incarnation of the member (a read after a respawn) takes
/// the route with the trust. `None` when this mob's namespace routes no such
/// peer.
pub(crate) fn installed_local_wire_route(
    local_mob_id: &str,
    spec: TrustedPeerDescriptor,
) -> Result<(TrustedPeerDescriptor, Option<LocalWireRoute>), CrossMobError> {
    let Some((local_namespace, _)) = other_mob_namespaces(local_mob_id, &spec)? else {
        return Ok((spec, None));
    };
    let Some(route) = InprocRegistry::global()
        .peers_in_namespace(&local_namespace)
        .into_iter()
        .find(|peer| peer.name == spec.name.as_str())
    else {
        return Ok((spec, None));
    };
    let spec = if spec.pubkey == [0u8; 32] && route.pubkey.to_peer_id() == spec.peer_id {
        spec.with_pubkey(*route.pubkey.as_bytes())
    } else {
        spec
    };
    Ok((
        spec,
        Some(LocalWireRoute {
            namespace: local_namespace,
            pubkey: route.pubkey,
            installed: true,
        }),
    ))
}

async fn handle_references_peer(handle: &MobHandle, peer_id: &str) -> bool {
    handle
        .list_members_including_retiring()
        .await
        .iter()
        .any(|member| member.wired_to.iter().any(|peer| peer.as_str() == peer_id))
}

fn unregister_cross_namespace_aliases(
    source_namespace: &str,
    source_pubkey: [u8; 32],
    target_pubkey: [u8; 32],
    source_still_references_target: bool,
    target_namespace: &str,
    target_still_references_source: bool,
) {
    let registry = InprocRegistry::global();
    if !source_still_references_target {
        registry.unregister_in_namespace(source_namespace, &PubKey::new(target_pubkey));
    }
    if !target_still_references_source {
        registry.unregister_in_namespace(target_namespace, &PubKey::new(source_pubkey));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn installs_with_external(
        local: &str,
        peer_id: &str,
    ) -> meerkat_contracts::wire::MobRouteInstallsResult {
        meerkat_contracts::wire::MobRouteInstallsResult {
            outstanding: Vec::new(),
            outstanding_external: vec![
                meerkat_contracts::wire::WireExternalRouteInstallObligation {
                    local: local.to_string(),
                    peer_id: peer_id.to_string(),
                    peer_name: "remote-mob/worker/remote".to_string(),
                    host: meerkat_contracts::wire::WireHostRef("host-a".to_string()),
                },
            ],
            complete: false,
        }
    }

    fn placed_plane(address: &str) -> crate::runtime::cross_mob_control::MemberPlane {
        let pubkey = [9u8; 32];
        crate::runtime::cross_mob_control::MemberPlane::Placed(
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "remote-mob/worker/b2",
                meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey).to_string(),
                pubkey,
                address,
            )
            .expect("placed descriptor"),
        )
    }

    #[test]
    fn a_placed_member_is_always_described_by_its_durable_endpoint() {
        use crate::runtime::cross_mob_control::MemberPlane;
        let placed = placed_plane("tcp://placed-host.test:4200");
        for target in [
            MemberPlane::Local,
            placed_plane("tcp://other-host.test:4300"),
        ] {
            let choice = choose_install_descriptor(&placed, &target);
            assert!(
                matches!(
                    &choice,
                    InstallDescriptorChoice::PlacedEndpoint(descriptor)
                        if descriptor.address.to_string() == "tcp://placed-host.test:4200"
                ),
                "a placed member must be described by its durable endpoint, never inproc: {choice:?}"
            );
        }
    }

    #[test]
    fn a_local_member_installed_on_a_placed_target_needs_its_dialable_address() {
        use crate::runtime::cross_mob_control::MemberPlane;
        assert_eq!(
            choose_install_descriptor(&MemberPlane::Local, &placed_plane("tcp://h.test:1")),
            InstallDescriptorChoice::DialableAddress
        );
    }

    #[test]
    fn local_to_local_still_installs_the_inproc_descriptor() {
        use crate::runtime::cross_mob_control::MemberPlane;
        assert_eq!(
            choose_install_descriptor(&MemberPlane::Local, &MemberPlane::Local),
            InstallDescriptorChoice::SameProcess
        );
        let spec = build_peer_spec(
            "authors/coordinator/alice",
            TEST_PEER_ID,
            &MobTransport::Inproc,
            None,
        )
        .expect("same-process descriptor");
        assert_eq!(
            spec.address.to_string(),
            "inproc://authors/coordinator/alice"
        );
    }

    #[test]
    fn placed_readiness_waits_for_its_outstanding_external_route_install() {
        let installs = installs_with_external("b2", "peer-x");
        assert_eq!(
            external_route_install_pending_host(&installs, &AgentIdentity::from("b2"), "peer-x"),
            Some("host-a".to_string())
        );
        assert_eq!(
            external_route_install_pending_host(&installs, &AgentIdentity::from("b2"), "peer-y"),
            None,
            "another peer's install does not gate this edge"
        );
        assert_eq!(
            external_route_install_pending_host(&installs, &AgentIdentity::from("c3"), "peer-x"),
            None,
            "another member's install does not gate this edge"
        );
    }

    #[test]
    fn a_broken_placed_member_is_still_placed_for_alias_bookkeeping() {
        use crate::runtime::cross_mob_control::{MemberPlane, MemberPlaneFault};
        assert!(
            plane_result_is_placed(Err(MemberPlaneFault::PlacedUnavailable {
                reason: "member is Broken".to_string(),
            }))
            .expect("placed-but-unavailable is a verdict, not a fault"),
            "a Broken placed side skips inproc aliases, so its unwire can succeed"
        );
        assert!(!plane_result_is_placed(Ok(MemberPlane::Local)).expect("local"));
        assert!(
            plane_result_is_placed(Ok(placed_plane("tcp://placed-host.test:4200")))
                .expect("placed")
        );
        assert!(
            plane_result_is_placed(Err(MemberPlaneFault::Mob(meerkat_mob::MobError::Internal(
                "query fault".to_string()
            ))))
            .is_err(),
            "only a genuine query fault is an error"
        );
    }

    #[test]
    fn peer_info_reports_a_placed_members_durable_address() {
        use crate::runtime::cross_mob_control::{MemberPlane, peer_info_address_for};
        assert_eq!(
            peer_info_address_for(&placed_plane("tcp://placed-host.test:4200"), "m/w/b2"),
            "tcp://placed-host.test:4200"
        );
        assert_eq!(
            peer_info_address_for(&MemberPlane::Local, "m/w/alice"),
            "inproc://m/w/alice"
        );
    }

    #[test]
    fn lookup_reports_a_placed_member_without_endpoint_typed() {
        use crate::runtime::cross_mob_control::{MemberPlaneFault, lookup_address_fault};
        let (code, message) = lookup_address_fault(MemberPlaneFault::PlacedUnavailable {
            reason: "member is Broken".to_string(),
        });
        assert_eq!(code, "placed_member_unavailable");
        assert!(message.contains("member is Broken"));
        let (code, _) = lookup_address_fault(MemberPlaneFault::Mob(
            meerkat_mob::MobError::Internal("query fault".to_string()),
        ));
        assert_eq!(code, "mob_error");
    }

    #[test]
    fn route_installs_without_external_routes_decode_as_none_outstanding() {
        let installs: meerkat_contracts::wire::MobRouteInstallsResult =
            serde_json::from_str(r#"{"outstanding":[],"complete":true}"#)
                .expect("the pre-external projection shape still decodes");
        assert!(installs.outstanding_external.is_empty());
        assert_eq!(
            external_route_install_pending_host(&installs, &AgentIdentity::from("b2"), "peer-x"),
            None
        );
    }

    const TEST_PEER_ID: &str = "00000000-0000-4000-8000-000000000001";

    /// Non-zero placeholder pubkey for the trust-required tests.
    const TEST_PUBKEY: [u8; 32] = [42u8; 32];

    /// `TrustedPeerDescriptor::validate_pubkey_for_peer_id` (post-LUC-*)
    /// requires the descriptor's `peer_id` to be UUIDv5-derived from its
    /// pubkey. Tests that pass a non-zero pubkey must therefore use the
    /// derived id, not an arbitrary placeholder.
    fn derived_peer_id() -> String {
        meerkat_core::comms::PeerId::from_ed25519_pubkey(&TEST_PUBKEY).to_string()
    }

    #[test]
    fn peer_spec_inproc_uses_comms_name_address() {
        let spec = build_peer_spec(
            "authors/coordinator/alice",
            TEST_PEER_ID,
            &MobTransport::Inproc,
            None,
        )
        .expect("spec");
        assert_eq!(spec.address.endpoint(), "authors/coordinator/alice");
    }

    #[test]
    fn peer_spec_tcp_uses_tcp_scheme() {
        let id = derived_peer_id();
        let spec = build_peer_spec(
            "authors/coordinator/alice",
            &id,
            &MobTransport::Tcp("127.0.0.1:9001".to_string()),
            Some(TEST_PUBKEY),
        )
        .expect("spec");
        assert_eq!(spec.address.endpoint(), "127.0.0.1:9001");
    }

    #[test]
    fn peer_spec_uds_uses_uds_scheme() {
        let id = derived_peer_id();
        let spec = build_peer_spec(
            "authors/coordinator/alice",
            &id,
            &MobTransport::Uds("/tmp/x.sock".to_string()),
            Some(TEST_PUBKEY),
        )
        .expect("spec");
        assert_eq!(spec.address.endpoint(), "/tmp/x.sock");
    }

    /// Regression: cross-mob TCP wires must fail closed without a pubkey.
    /// Pre-fix `build_peer_spec` produced an unsigned descriptor for any
    /// transport; meerkat-comms would have admitted any sender.
    #[test]
    fn peer_spec_tcp_without_pubkey_rejected() {
        let result = build_peer_spec(
            "authors/coordinator/alice",
            TEST_PEER_ID,
            &MobTransport::Tcp("127.0.0.1:9001".to_string()),
            None,
        );
        assert!(
            matches!(result, Err(CrossMobError::MissingPeerPubkey { .. })),
            "TCP peer spec without pubkey must fail closed, got {result:?}"
        );
    }

    /// Regression: cross-mob UDS wires must also fail closed without a pubkey.
    #[test]
    fn peer_spec_uds_without_pubkey_rejected() {
        let result = build_peer_spec(
            "authors/coordinator/alice",
            TEST_PEER_ID,
            &MobTransport::Uds("/tmp/x.sock".to_string()),
            None,
        );
        assert!(
            matches!(result, Err(CrossMobError::MissingPeerPubkey { .. })),
            "UDS peer spec without pubkey must fail closed, got {result:?}"
        );
    }

    #[test]
    fn build_uds_peer_spec_handles_leading_slash() {
        // Caller may pass the path with or without a leading slash; both
        // produce the canonical `uds:///path` form.
        let with = build_uds_peer_spec("a", "00000000-0000-4000-8000-000000000001", "/tmp/x.sock")
            .expect("spec");
        let without =
            build_uds_peer_spec("a", "00000000-0000-4000-8000-000000000001", "tmp/x.sock")
                .expect("spec");
        assert_eq!(with.address.endpoint(), "/tmp/x.sock");
        assert_eq!(without.address.endpoint(), "/tmp/x.sock");
    }

    /// The installer reads the member's inbox and registers the route in two
    /// steps. A member that is released between them is a member that is
    /// not there: no route is installed, the name stays free, and the answer
    /// is `MemberNotThere`, the one a member that is not built gets.
    #[test]
    fn a_member_released_while_its_route_is_installed_is_a_member_that_is_not_there() {
        let (namespace, canonical) = ("xm-late-route-a", "xm-late-route-b");
        let name = "xm-late-route-b/worker/bob";
        let runtime =
            meerkat_comms::CommsRuntime::inproc_only_scoped(name, Some(canonical.to_string()))
                .expect("bob's runtime publishes");
        let key = runtime.public_key();
        let registry = InprocRegistry::global();
        // Step one of the installer.
        let read = registry
            .get_by_pubkey_in_namespace(canonical, &key)
            .expect("bob is routed in his own namespace");
        // Bob is released to rest.
        assert!(runtime.retire_inproc_route());
        // Step two.
        let answer = register_route_alias(namespace, canonical, name, key, read, "remote");
        assert!(
            matches!(&answer, Err(RouteNotInstalled::MemberNotThere(_))),
            "the answer must be the one for a member that is not there: {:?}",
            answer.map_err(RouteNotInstalled::into_message)
        );
        assert!(
            registry
                .get_by_pubkey_in_namespace(namespace, &key)
                .is_none(),
            "a route to an inbox that left was installed"
        );
        assert!(registry.peer_names_in_namespace(namespace).is_empty());
        // The whole installer afterwards finds nobody, with the same answer.
        assert!(matches!(
            install_route_alias(namespace, canonical, name, key, "remote"),
            Err(RouteNotInstalled::MemberNotThere(_))
        ));
    }
}
