//! The declared apply: a host's declarative roster of identity entries,
//! applied by MobMachine ([`super::MobHandle::apply_declared_roster`]).
//!
//! The host's data is the authority. Each declared identity gets one typed
//! verdict from MobMachine (`DeclareIdentityEntry`), decided from the
//! declaration and the machine's member facts, and the handle realizes it
//! with the mob's existing steps: a Dormant seat (on the entry's archived
//! session when it has one), a same-session restamp
//! ([`super::MobHandle::respawn_member`]), a rotation
//! ([`super::MobHandle::rotate_member`]) or the hold of an undeclared role
//! move (its wakes refused, a Live member's run starts held until it is
//! released when idle). A complete roster is then sealed
//! (`SealDeclaredRoster`): every Declared entry it does not list is unlisted,
//! and its member is retired, the end of the retirement ending the entry.
//!
//! Each identity's answer is its own and typed: one identity's refusal or
//! failure never fails the pass. An unchanged roster writes nothing: no
//! journal row, no wake.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_core::types::SessionId;

use crate::MobError;
use crate::ids::{AgentIdentity, Generation, ProfileName};
use crate::machines::mob_machine as mob_dsl;

use super::HistoryUnavailable;
use super::handle::SpawnMemberSpec;
use super::state::{MobCommand, ScopeAdmissionSubject};

/// The most declared identities one MobMachine step decides, and the most
/// seats one seat step commits: the dormant seat's chunk.
pub const DECLARED_APPLY_CHUNK: usize = super::dormant_seat::DORMANT_SEAT_CHUNK;

/// A host's declared roster ([`super::MobHandle::apply_declared_roster`]).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DeclaredRoster {
    /// The identities the roster declares, each with its spec.
    pub entries: Vec<DeclaredEntry>,
    /// Whether the roster is the host's whole roster: a complete roster is
    /// sealed, and every Declared entry it does not list is unlisted (its
    /// member retired, the entry ended). An incomplete one declares its
    /// entries only (a registration, or an update of one identity).
    pub complete: bool,
    /// The role migrations the host declares, by identity: the role each
    /// identity's member may move from. A member whose role the roster
    /// changes without a declared migration from its current role is held,
    /// and so is an identity no member holds whose seat would resume a
    /// session last built under another role (its archived session, or its
    /// first-boot request's session) without a migration declared from that
    /// role. A migration for an identity the roster does not declare is
    /// ignored.
    pub role_migrations: BTreeMap<AgentIdentity, ProfileName>,
    /// The host's one-shot first-boot requests, by identity: the declared
    /// apply carries each out when it can (a seat on the request's session,
    /// a seated member that holds the identity already, or a fresh life) and
    /// names it in [`DeclaredRosterOutcome::first_boot_requests_carried_out`].
    /// The host consumes exactly those requests; any other stays the host's
    /// (a deferred, refused or failed answer keeps its request for the next
    /// apply, and the host's own creation carries out the request of an
    /// identity hosted on its first use or awaiting its held creation).
    pub first_boot_requests: BTreeMap<AgentIdentity, FirstBootRequest>,
}

impl DeclaredRoster {
    /// The host's whole roster: unlisted entries are retired.
    #[must_use]
    pub fn complete(entries: impl IntoIterator<Item = DeclaredEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            complete: true,
            ..Self::default()
        }
    }

    /// Part of the host's roster (a registration, an update): nothing is
    /// unlisted.
    #[must_use]
    pub fn partial(entries: impl IntoIterator<Item = DeclaredEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            complete: false,
            ..Self::default()
        }
    }

    /// Declare that `identity`'s member may move from the role `from` to the
    /// role its declaration names.
    #[must_use]
    pub fn with_role_migration(
        mut self,
        identity: impl Into<AgentIdentity>,
        from: impl Into<ProfileName>,
    ) -> Self {
        self.role_migrations.insert(identity.into(), from.into());
        self
    }

    /// Carry out `identity`'s first-boot request.
    #[must_use]
    pub fn with_first_boot_request(
        mut self,
        identity: impl Into<AgentIdentity>,
        request: FirstBootRequest,
    ) -> Self {
        self.first_boot_requests.insert(identity.into(), request);
        self
    }
}

/// One declared identity.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DeclaredEntry {
    /// The identity's spec: its identity, role and runtime mode (lifecycle
    /// facts MobMachine records), its placement or binding (a placed or
    /// externally bound identity is hosted on its first use), and its
    /// descriptive facets and build inputs (labels, context, instructions),
    /// which are no lifecycle fact: a change of them alone is no new life
    /// and no restamp. The spec's launch mode is not read: the apply decides
    /// the session.
    pub spec: SpawnMemberSpec,
    /// Whether the host holds the identity's creation for its held protocol
    /// ([`super::MobHandle::spawn_held_until`]): an identity never created
    /// then awaits its held creation and is never seated otherwise. The
    /// host's current policy decides; nothing is kept of an earlier one.
    pub held_creation: bool,
}

impl DeclaredEntry {
    /// A declared identity, created by the apply when it is not held.
    #[must_use]
    pub fn new(spec: SpawnMemberSpec) -> Self {
        Self {
            spec,
            held_creation: false,
        }
    }

    /// Whether the host holds the identity's creation for its held protocol.
    #[must_use]
    pub fn with_held_creation(mut self, held_creation: bool) -> Self {
        self.held_creation = held_creation;
        self
    }
}

/// A host's one-shot first-boot request for one identity
/// ([`DeclaredRoster::first_boot_requests`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FirstBootRequest {
    /// Seat the identity on this session, its committed session. An
    /// identity whose member is seated already settles the request.
    Resume { session_id: SessionId },
    /// Start the identity's fresh life on a new session: a seated member is
    /// rotated onto one, and an unseated identity is seated on one (its
    /// archived session is let go).
    Fresh,
}

/// Why the declared apply refused one identity
/// ([`DeclaredEntryOutcome::Refused`]). Nothing of it was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeclaredEntryRefusal {
    /// A worker member holds the name.
    NameHeldByWorker,
    /// The identity store manages the identity through an intent row (a
    /// host adoption): the declared apply does not drive it.
    HostAdopted,
    /// This composition of the mob is not authoritative (a candidate): it
    /// declares nothing.
    NonAuthoritative,
    /// The roster declares the identity more than once.
    DuplicateDeclaration,
    /// The roster carries a first-boot request for an identity it does not
    /// declare.
    UndeclaredFirstBootRequest,
}

/// The declared apply's answer for one identity.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeclaredEntryOutcome {
    /// Seated Dormant on `session_id`: its archived session (the identity
    /// comes back with its history), a first-boot request's session, or a
    /// fresh one. `history_unavailable` is set when the identity's archived
    /// session is gone from the store and the identity was seated on a
    /// fresh session.
    Seated {
        session_id: SessionId,
        history_unavailable: Option<HistoryUnavailable>,
    },
    /// A fresh spec that owes a kickoff was spawned, live, on `session_id`.
    /// `history_unavailable` is set when the identity's archived session is
    /// gone from the store and the identity was embodied on a fresh session.
    Embodied {
        session_id: SessionId,
        history_unavailable: Option<HistoryUnavailable>,
    },
    /// The identity's member keeps its seat: nothing was written. A change
    /// of descriptive facets alone answers this, and so does a change of
    /// placement alone (placement is read only for an identity no member
    /// holds).
    Kept,
    /// The member was restamped with its declared runtime mode on its own
    /// session, at `generation`.
    Restamped { generation: Generation },
    /// The member was restamped under its declared new role on its own
    /// session, at `generation`.
    RoleMigrated { generation: Generation },
    /// The member moved to a fresh session, `session_id`, under its declared
    /// role and runtime mode: its session lives in memory only.
    RoleRotated { session_id: SessionId },
    /// The identity's first-boot request started its fresh life on
    /// `session_id`.
    FreshLife { session_id: SessionId },
    /// The roster moves the member from role `from` to role `to` without a
    /// declared migration: the member is held until an apply declares the
    /// move or restores the role. Every wake of it is refused, and a Live
    /// member starts no new run (its run starts are held,
    /// [`crate::HostRunStartHoldReason::HostPreparation`]): its run in
    /// flight finishes, and it is released when idle. The hold ends in the
    /// step that commits the declared role (the restamp or rotation of a
    /// later apply), not at that apply's verdict. An identity no member holds
    /// whose seat would resume a session last built under `from` is not
    /// seated until then.
    RoleMigrationHeld { from: ProfileName, to: ProfileName },
    /// The identity awaits its held creation.
    AwaitingHeldCreation,
    /// The identity is hosted on its first use: its host creates the member
    /// (a spawn whose spec carries entry custody).
    HostedOnFirstUse,
    /// Left to the next apply, for a cause that clears on its own (a member
    /// that is busy, or whose spawn, wake or retirement is in flight, or a
    /// store that cannot serve now): nothing of it was done.
    Deferred { reason: MobError },
    /// Refused, typed: nothing of it was written.
    Refused { refusal: DeclaredEntryRefusal },
    /// Realizing the identity's verdict failed, typed: an apply runs it
    /// again only once the cause is fixed.
    Failed { error: MobError },
}

/// What the seal did with one Declared entry the complete roster does not
/// list ([`DeclaredRosterOutcome::unlisted`]).
#[derive(Debug)]
#[non_exhaustive]
pub enum UnlistedEntryOutcome {
    /// Its member was retired (the journal records the cause
    /// [`super::MemberRetirementCause::Unlisted`]), and the end of the
    /// retirement ended the entry.
    Retired,
    /// No member held it: the entry was removed at once.
    Removed,
    /// Its member's retirement failed: the entry stays marked, and the next
    /// complete apply retires it again.
    Failed { error: MobError },
}

/// The answer of [`super::MobHandle::apply_declared_roster`].
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct DeclaredRosterOutcome {
    /// Every declared identity's answer, and every identity a first-boot
    /// request names.
    pub entries: BTreeMap<AgentIdentity, DeclaredEntryOutcome>,
    /// What a complete roster's seal did with each Declared entry it does
    /// not list.
    pub unlisted: BTreeMap<AgentIdentity, UnlistedEntryOutcome>,
    /// The identities whose first-boot request this apply carried out: the
    /// host consumes exactly these requests
    /// ([`DeclaredRoster::first_boot_requests`]).
    pub first_boot_requests_carried_out: BTreeSet<AgentIdentity>,
}

/// One declaration MobMachine decides (`DeclareIdentityEntry`), with the
/// shell's raw observations. A spec that names no runtime mode declares its
/// role's, which the actor resolves.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct IdentityDeclaration {
    pub(in crate::runtime) agent_identity: AgentIdentity,
    pub(in crate::runtime) profile_name: ProfileName,
    pub(in crate::runtime) runtime_mode: Option<crate::MobRuntimeMode>,
    pub(in crate::runtime) placement: mob_dsl::EntryPlacementKind,
    pub(in crate::runtime) held_creation: bool,
    pub(in crate::runtime) declared_migration_from: Option<ProfileName>,
    pub(in crate::runtime) intent_managed: bool,
    pub(in crate::runtime) session_memory_only: bool,
    /// The role the session an unseated identity's seat would resume was
    /// last built under, as its owner records it: the identity's archived
    /// session, or its first-boot request's session. `None` for a seated
    /// identity, a fresh seat, a session no build created, or one that
    /// records no role.
    pub(in crate::runtime) resume_session_role: Option<ProfileName>,
    /// The kind of the host's first-boot request for the identity, if the
    /// roster carries one: the declaration's step ends the first-boot
    /// preparation the request carries out.
    pub(in crate::runtime) first_boot: mob_dsl::FirstBootRequestKind,
}

impl IdentityDeclaration {
    /// The declaration's machine input, with its resolved runtime mode.
    pub(in crate::runtime) fn machine_input(
        &self,
        runtime_mode: crate::MobRuntimeMode,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::DeclareIdentityEntry {
            agent_identity: mob_dsl::AgentIdentity::from_domain(&self.agent_identity),
            profile_name: self.profile_name.as_str().to_string(),
            runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::from(runtime_mode),
            placement: self.placement,
            held_creation: self.held_creation,
            declared_migration_from: self
                .declared_migration_from
                .as_ref()
                .map(|profile| profile.as_str().to_string()),
            intent_managed: self.intent_managed,
            session_memory_only: self.session_memory_only,
            resume_session_role: self
                .resume_session_role
                .as_ref()
                .map(|profile| profile.as_str().to_string()),
            first_boot: self.first_boot,
        }
    }
}

/// MobMachine's verdict for one declaration, with the runtime mode it was
/// declared with and the identity's archived session record after the step;
/// or the typed reason it was not decided.
pub(in crate::runtime) struct DeclarationVerdict {
    pub(in crate::runtime) agent_identity: AgentIdentity,
    pub(in crate::runtime) verdict: Result<DecidedDeclaration, MobError>,
}

/// One decided declaration ([`DeclarationVerdict`]).
pub(in crate::runtime) struct DecidedDeclaration {
    pub(in crate::runtime) verdict: mob_dsl::IdentityEntryVerdict,
    pub(in crate::runtime) runtime_mode: crate::MobRuntimeMode,
    pub(in crate::runtime) archived_session: Option<SessionId>,
}

/// The seal's answer: its verdict, the Declared entries it marked (their
/// members are retired) and the ones it removed at once.
pub(in crate::runtime) struct SealAnswer {
    pub(in crate::runtime) verdict: mob_dsl::DeclaredRosterSealVerdict,
    pub(in crate::runtime) marked: Vec<AgentIdentity>,
    pub(in crate::runtime) removed: Vec<AgentIdentity>,
}

/// Where a declared spec's member lives: a placed member or one bound to an
/// external backend is hosted on its first use, any other is local.
fn placement_kind(spec: &SpawnMemberSpec) -> mob_dsl::EntryPlacementKind {
    match (&spec.placement, &spec.binding) {
        (Some(_), _) | (None, Some(crate::RuntimeBinding::HostMaterialized { .. })) => {
            mob_dsl::EntryPlacementKind::Placed
        }
        (None, Some(crate::RuntimeBinding::External { .. })) => {
            mob_dsl::EntryPlacementKind::External
        }
        (None, Some(crate::RuntimeBinding::Session) | None) => mob_dsl::EntryPlacementKind::Local,
    }
}

/// Whether a failure clears on its own: the identity is deferred to the next
/// apply rather than failed.
fn apply_error_is_retryable(error: &MobError) -> bool {
    super::builder::restore_error_is_retryable(error)
}

/// A declared identity's answer for an error of its realization.
fn failure_outcome(error: MobError) -> DeclaredEntryOutcome {
    if apply_error_is_retryable(&error) {
        DeclaredEntryOutcome::Deferred { reason: error }
    } else {
        DeclaredEntryOutcome::Failed { error }
    }
}

/// MobMachine's answer to the opening of a declared apply
/// (`OpenDeclaredApply`).
pub(in crate::runtime) enum DeclaredApplyOpening {
    /// The apply is open, under the lease of its open fact.
    Opened(DeclaredApplyLease),
    /// Another apply of the mob is open: refused, retryable.
    Busy,
}

/// The lease of an open declared apply's fact (MobMachine's
/// `declared_apply_open`), made by the actor in the step that opened it and
/// owned by the one call that opened it. Closing it, or dropping it anywhere
/// before (an opening answer that never reached its call, a call dropped in
/// its body or while it closes), closes the fact, so the mob's next apply is
/// never refused for a call that is gone. It stays armed until its close
/// command is on the actor's channel, never shorter.
pub(in crate::runtime) struct DeclaredApplyLease {
    close: Option<(
        crate::tokio::sync::mpsc::Sender<super::scope_gate::RoutedMobCommand>,
        crate::control_policy::CommandAuthority,
    )>,
}

impl DeclaredApplyLease {
    /// The lease of the fact the actor just opened: its close is sent on
    /// `command_tx` under `authority`, the opening call's own.
    pub(in crate::runtime) fn new(
        command_tx: crate::tokio::sync::mpsc::Sender<super::scope_gate::RoutedMobCommand>,
        authority: crate::control_policy::CommandAuthority,
    ) -> Self {
        Self {
            close: Some((command_tx, authority)),
        }
    }

    /// Give the close up: the actor closed the fact itself.
    pub(in crate::runtime) fn disarm(&mut self) {
        self.close = None;
    }

    fn close_command(
        authority: crate::control_policy::CommandAuthority,
    ) -> (
        super::scope_gate::RoutedMobCommand,
        crate::tokio::sync::oneshot::Receiver<Result<(), MobError>>,
    ) {
        let (reply_tx, reply_rx) = crate::tokio::sync::oneshot::channel();
        (
            super::scope_gate::RoutedMobCommand {
                authority,
                cmd: MobCommand::CloseDeclaredApply { reply_tx },
            },
            reply_rx,
        )
    }

    /// Close the open fact, and wait for the actor's answer. The lease is
    /// disarmed only once the close command is on the actor's channel, so a
    /// call dropped while it waits for room still closes.
    async fn close(mut self) {
        let Some((command_tx, authority)) = self.close.clone() else {
            return;
        };
        let (command, reply_rx) = Self::close_command(authority);
        let sent = command_tx.send(command).await;
        self.close = None;
        // An actor that is gone took its open fact with it.
        if sent.is_err() {
            return;
        }
        if let Ok(Err(error)) = reply_rx.await {
            tracing::warn!(
                %error,
                "closing the declared apply failed; the mob's next apply is refused until it closes"
            );
        }
    }
}

impl Drop for DeclaredApplyLease {
    fn drop(&mut self) {
        let Some((command_tx, authority)) = self.close.take() else {
            return;
        };
        let (command, _reply_rx) = Self::close_command(authority);
        let command = match command_tx.try_send(command) {
            Ok(()) | Err(crate::tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
            Err(crate::tokio::sync::mpsc::error::TrySendError::Full(command)) => command,
        };
        // A full channel: a task sends the close once there is room.
        let close = async move {
            let _ = command_tx.send(command).await;
        };
        // The browser runtime has one local executor and no runtime handle.
        #[cfg(target_arch = "wasm32")]
        {
            crate::tokio::spawn(close);
        }
        #[cfg(not(target_arch = "wasm32"))]
        match crate::tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(close);
            }
            Err(_) => {
                tracing::warn!("a dropped declared apply could not close: no async runtime at drop")
            }
        }
    }
}

/// One seat the apply owes, as the seat step takes it.
struct OwedSeat {
    spec: SpawnMemberSpec,
    /// The archived session this seat revives, if it does.
    revives: Option<SessionId>,
    /// The seat carries out a first-boot `Fresh` request.
    fresh_life: bool,
    /// The seat carries out the identity's first-boot request (`Resume` or
    /// `Fresh`).
    first_boot_request: bool,
}

impl super::MobHandle {
    /// Apply a host's declared roster of identity entries: MobMachine decides
    /// each declared identity's verdict from the declaration and its member
    /// facts, and the handle realizes it; a complete roster is then sealed
    /// and every Declared entry it does not list is unlisted. A host verb: the
    /// agent lane (and a principal acting for an agent) is refused
    /// [`MobError::ScopeDenied`]. The handle's principal must hold
    /// [`crate::ControlScope::Retire`] (and
    /// [`crate::ControlScope::SendCommand`] for the seats).
    ///
    /// Each identity's answer is typed and its own
    /// ([`DeclaredEntryOutcome`]): one identity's refusal or failure never
    /// fails the pass. The call fails as a whole only for what no identity
    /// could be answered without: a mob that is not running, the identity
    /// store's intent rows that cannot be read now (retryable), or another
    /// apply of the mob that is running (retryable, see below).
    ///
    /// - A seated entry whose role and runtime mode match its declaration
    ///   keeps its seat, and nothing is written: its descriptive facets
    ///   (display name, labels, context, instructions) and build inputs are
    ///   the host's, and the apply stores none of them. The member's next
    ///   build applies the host's current ones only through the host's build
    ///   preparer or spawn customizer; the build inputs a seat recorded stay
    ///   as it recorded them. A change of placement alone keeps the seat too:
    ///   placement is read only for an identity no member holds.
    /// - An identity no member holds is seated Dormant: on the session its
    ///   member was last bound to when the identity entry's retirement
    ///   archived it (it comes back as the same identity, history kept, at
    ///   its next generation, and no wake), on a first-boot request's
    ///   session, or on a fresh session. When that archived session is gone
    ///   from the store, the identity is seated on a fresh session and the
    ///   answer carries [`HistoryUnavailable`]; a store that cannot be read
    ///   now defers it, never a fresh session. A seat that resumes a session
    ///   last built under another role than the declared one declares the
    ///   migration the roster declares from that role, so the session's next
    ///   build moves it; with no such migration declared the identity is
    ///   held ([`DeclaredEntryOutcome::RoleMigrationHeld`]) and not seated.
    ///   An archived session the store holds intact whose runtime refuses
    ///   revival is answered [`DeclaredEntryOutcome::Failed`]
    ///   ([`MobError::SessionUnavailableForResume`]) on every apply, since a
    ///   fresh session would lose its history: the host's `Fresh` first-boot
    ///   request starts the identity's fresh life instead. A worker spawned
    ///   under the name of an unlisted identity ends that identity's archived
    ///   session record, so listed again later it is seated fresh, with no
    ///   notice.
    /// - A placed or externally bound identity no member holds is hosted on
    ///   its first use, and one whose creation the host holds and that was
    ///   never created awaits its held creation: neither is seated.
    /// - A changed runtime mode is a restamp on the member's own session; a
    ///   role change with a migration declared from the member's current
    ///   role is a restamp under the new role; on a session that lives in
    ///   memory only either moves the member to a fresh session. Each waits
    ///   for the member to be idle: a busy member is deferred.
    /// - A role change with no migration declared is held
    ///   ([`crate::HostPreparationCause::RoleMigrationUndeclared`]): every
    ///   wake of the member is refused, and a Live member's run starts are
    ///   held ([`crate::HostRunStartHoldReason::HostPreparation`]), so its
    ///   run in flight finishes, never preempted, no new run starts under its
    ///   previous role, and the mob releases it when idle. The explicit
    ///   resume leaves it alone. An apply that restores the role ends the
    ///   hold at its verdict; one that declares the move ends it in the step
    ///   that commits the new role, so a deferred or refused restamp keeps it.
    /// - A legacy member listed under its own name is adopted as an entry
    ///   first ([`Self::adopt_legacy_identity_entry`]); a worker holding a
    ///   listed name refuses that identity alone.
    /// - A candidate composition answers every identity refused and writes
    ///   nothing.
    ///
    /// A complete roster is sealed after the last chunk: a Declared entry it
    /// does not list whose member is seated is retired (its start row records
    /// the cause [`super::MemberRetirementCause::Unlisted`]), and the end of
    /// the retirement ends the entry; one no member holds is removed at once.
    /// A worker is never unlisted. An entry whose member's retirement started
    /// for another reason is left to it: the entry outlives that retirement,
    /// unseated, and a later complete apply removes it.
    ///
    /// The apply decides and seats in chunks of [`DECLARED_APPLY_CHUNK`]
    /// identities, each one MobMachine step. Applies of one mob run one at a
    /// time: a call while another apply of the mob runs is refused
    /// [`MobError::LifecycleOperationPending`], retryable, and writes
    /// nothing.
    ///
    /// Between a restart and the first apply after it, MobMachine knows only
    /// the entries its journal rows record: the seated ones, and those whose
    /// member a retirement other than the seal's ended (unseated, with the
    /// session the retirement archived). An entry no member ever held and a
    /// role hold are declared again by that apply; until then, a member whose
    /// role the host moved while the process was down is not held, and a
    /// peer delivery can wake it under its previous role.
    pub async fn apply_declared_roster(
        &self,
        roster: DeclaredRoster,
    ) -> Result<DeclaredRosterOutcome, MobError> {
        self.admit_control_scope_for(
            mob_dsl::ControlScope::Retire,
            ScopeAdmissionSubject::DeclaredApply,
        )
        .await?;
        // One apply of the mob at a time (MobMachine's open fact): a second
        // one while this one runs is refused, typed and retryable. The open
        // fact is closed whatever the outcome, and when this call is dropped
        // at any point (the lease the actor answers with).
        let opening = self
            .send_actor_command(|reply_tx| MobCommand::OpenDeclaredApply {
                close_authority: self.command_authority.clone(),
                reply_tx,
            })
            .await??;
        let lease = match opening {
            DeclaredApplyOpening::Opened(lease) => lease,
            DeclaredApplyOpening::Busy => {
                return Err(MobError::LifecycleOperationPending {
                    intent: "apply a declared roster while another apply of this mob runs"
                        .to_string(),
                });
            }
        };
        let outcome = self.apply_open_declared_roster(roster).await;
        lease.close().await;
        outcome
    }

    /// The body of [`Self::apply_declared_roster`], under its open fact.
    async fn apply_open_declared_roster(
        &self,
        roster: DeclaredRoster,
    ) -> Result<DeclaredRosterOutcome, MobError> {
        let DeclaredRoster {
            entries,
            complete,
            role_migrations,
            first_boot_requests,
        } = roster;
        let mut outcome = DeclaredRosterOutcome::default();

        // One declaration per identity: an identity declared twice is
        // refused, and stays listed.
        let mut listed = BTreeSet::new();
        let mut duplicated = BTreeSet::new();
        for entry in &entries {
            if !listed.insert(entry.spec.identity.clone()) {
                duplicated.insert(entry.spec.identity.clone());
            }
        }
        let mut declared: BTreeMap<AgentIdentity, DeclaredEntry> = BTreeMap::new();
        for entry in entries {
            if duplicated.contains(&entry.spec.identity) {
                continue;
            }
            declared.insert(entry.spec.identity.clone(), entry);
        }
        for identity in &duplicated {
            outcome.entries.insert(
                identity.clone(),
                DeclaredEntryOutcome::Refused {
                    refusal: DeclaredEntryRefusal::DuplicateDeclaration,
                },
            );
        }
        for identity in first_boot_requests.keys() {
            if !listed.contains(identity) {
                outcome.entries.insert(
                    identity.clone(),
                    DeclaredEntryOutcome::Refused {
                        refusal: DeclaredEntryRefusal::UndeclaredFirstBootRequest,
                    },
                );
            }
        }

        // The raw observations MobMachine decides by: the identity store's
        // intent rows (read once; a read that fails fails the pass, never
        // reads as no row) and the session service's durability.
        let intent_managed = self.declared_apply_intent_managed(&declared).await?;
        let session_memory_only = !self.session_service.keeps_durable_member_sessions();

        // A legacy member listed under its own name is adopted first, once:
        // MobMachine then declares it as the entry it is.
        let legacy: Vec<AgentIdentity> = {
            let state = self.machine_state_watch_rx.borrow();
            declared
                .keys()
                .filter(|identity| {
                    let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
                    state.identity_to_runtime.contains_key(&dsl_identity)
                        && !state.identity_entry_origins.contains_key(&dsl_identity)
                        && !intent_managed.contains(*identity)
                })
                .cloned()
                .collect()
        };
        for identity in legacy {
            if let Err(error) = self.adopt_legacy_identity_entry(&identity).await {
                declared.remove(&identity);
                outcome.entries.insert(identity, failure_outcome(error));
            }
        }

        // The role the session each unseated identity's seat would resume was
        // last built under (its first-boot request's session, or its archived
        // session): the raw observation MobMachine decides a role move by. A
        // store that cannot be read now defers the identity, never reads as
        // no role.
        let mut resume_session_roles: BTreeMap<AgentIdentity, ProfileName> = BTreeMap::new();
        for (identity, session_id) in
            self.declared_resume_sessions(&declared, &first_boot_requests, session_memory_only)
        {
            let observed = match session_id {
                Ok(session_id) => self.resume_session_role(&session_id).await,
                Err(error) => Err(error),
            };
            match observed {
                Ok(Some(role)) => {
                    resume_session_roles.insert(identity, role);
                }
                Ok(None) => {}
                Err(error) => {
                    declared.remove(&identity);
                    outcome.entries.insert(identity, failure_outcome(error));
                }
            }
        }

        let declare = |entry: &DeclaredEntry| IdentityDeclaration {
            agent_identity: entry.spec.identity.clone(),
            profile_name: entry.spec.role_name.clone(),
            runtime_mode: entry.spec.runtime_mode,
            placement: placement_kind(&entry.spec),
            held_creation: entry.held_creation,
            declared_migration_from: role_migrations.get(&entry.spec.identity).cloned(),
            intent_managed: intent_managed.contains(&entry.spec.identity),
            session_memory_only,
            resume_session_role: resume_session_roles.get(&entry.spec.identity).cloned(),
            first_boot: match first_boot_requests.get(&entry.spec.identity) {
                None => mob_dsl::FirstBootRequestKind::None,
                Some(FirstBootRequest::Resume { .. }) => mob_dsl::FirstBootRequestKind::Resume,
                Some(FirstBootRequest::Fresh) => mob_dsl::FirstBootRequestKind::Fresh,
            },
        };
        let declarations: Vec<IdentityDeclaration> = declared.values().map(declare).collect();
        let verdicts = self.declare_identity_entries(declarations).await?;

        // Realize each verdict.
        let mut seats: Vec<OwedSeat> = Vec::new();
        for DeclarationVerdict {
            agent_identity,
            verdict,
        } in verdicts
        {
            let Some(entry) = declared.get(&agent_identity) else {
                continue;
            };
            let request = first_boot_requests.get(&agent_identity);
            let DecidedDeclaration {
                verdict,
                runtime_mode,
                archived_session,
            } = match verdict {
                Ok(decided) => decided,
                Err(error) => {
                    outcome
                        .entries
                        .insert(agent_identity, failure_outcome(error));
                    continue;
                }
            };
            match (verdict, request) {
                // An unseated identity owed its seat: on a first-boot
                // request's session, on its archived session, or fresh.
                (mob_dsl::IdentityEntryVerdict::SeatOwed, request) => {
                    match self
                        .owed_seat(
                            entry,
                            request,
                            archived_session,
                            session_memory_only,
                            resume_session_roles.get(&agent_identity),
                        )
                        .await
                    {
                        Ok(seat) => seats.push(seat),
                        Err(error) => {
                            outcome
                                .entries
                                .insert(agent_identity, failure_outcome(error));
                        }
                    }
                }
                // A seated member with a first-boot Fresh request starts its
                // fresh life under the declared spec, once it is idle.
                (
                    mob_dsl::IdentityEntryVerdict::Kept
                    | mob_dsl::IdentityEntryVerdict::RuntimeModeRestamp
                    | mob_dsl::IdentityEntryVerdict::RoleMigrate
                    | mob_dsl::IdentityEntryVerdict::RoleRotate
                    | mob_dsl::IdentityEntryVerdict::RoleMigrationHeld,
                    Some(FirstBootRequest::Fresh),
                ) if self.seated_now(&agent_identity) => {
                    let answer = self.fresh_life_of_seated(entry).await;
                    if matches!(answer, DeclaredEntryOutcome::FreshLife { .. }) {
                        outcome
                            .first_boot_requests_carried_out
                            .insert(agent_identity.clone());
                    }
                    outcome.entries.insert(agent_identity, answer);
                }
                (verdict, request) => {
                    // A seated member carries out its first-boot Resume
                    // request: the declaration's own step ended its first-boot
                    // preparation, so no wake meets a window without a role
                    // hold. A hosted or held identity's request is its host's
                    // creation to carry out.
                    if matches!(request, Some(FirstBootRequest::Resume { .. }))
                        && matches!(
                            verdict,
                            mob_dsl::IdentityEntryVerdict::Kept
                                | mob_dsl::IdentityEntryVerdict::RuntimeModeRestamp
                                | mob_dsl::IdentityEntryVerdict::RoleMigrate
                                | mob_dsl::IdentityEntryVerdict::RoleRotate
                                | mob_dsl::IdentityEntryVerdict::RoleMigrationHeld
                        )
                        && self.seated_now(&agent_identity)
                    {
                        outcome
                            .first_boot_requests_carried_out
                            .insert(agent_identity.clone());
                    }
                    let answer = self
                        .realize_verdict(
                            entry,
                            verdict,
                            runtime_mode,
                            resume_session_roles.get(&agent_identity),
                        )
                        .await;
                    outcome.entries.insert(agent_identity, answer);
                }
            }
        }

        // The owed seats, in chunks; a seat whose archived session is gone
        // from the store is seated fresh, with the notice.
        let fresh_after_loss = self.realize_seats(seats, &mut outcome).await;
        if !fresh_after_loss.is_empty() {
            let lost = self.realize_seats(fresh_after_loss, &mut outcome).await;
            for seat in lost {
                outcome.entries.insert(
                    seat.spec.identity.clone(),
                    DeclaredEntryOutcome::Failed {
                        error: MobError::Internal(format!(
                            "the fresh seat of '{}' after the loss of its archived session \
                             found its session lost again",
                            seat.spec.identity
                        )),
                    },
                );
            }
        }

        if complete {
            self.seal_declared_roster(listed, &mut outcome).await?;
        }
        Ok(outcome)
    }

    /// The session each declared identity no member holds would be seated
    /// on by resuming it: its first-boot `Resume` request's session, or (with
    /// no request, on a service that keeps durable sessions) the archived
    /// session MobMachine recorded for it. A seated identity, a `Fresh`
    /// request and a fresh seat resume none. Read from the published machine
    /// state; a record that does not parse is the identity's own error.
    fn declared_resume_sessions(
        &self,
        declared: &BTreeMap<AgentIdentity, DeclaredEntry>,
        first_boot_requests: &BTreeMap<AgentIdentity, FirstBootRequest>,
        session_memory_only: bool,
    ) -> Vec<(AgentIdentity, Result<SessionId, MobError>)> {
        let state = self.machine_state_watch_rx.borrow();
        declared
            .keys()
            .filter_map(|identity| {
                let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
                if state.identity_to_runtime.contains_key(&dsl_identity) {
                    return None;
                }
                let session = match first_boot_requests.get(identity) {
                    Some(FirstBootRequest::Resume { session_id }) => Ok(session_id.clone()),
                    Some(FirstBootRequest::Fresh) => return None,
                    None if session_memory_only => return None,
                    None => {
                        let archived = state.identity_entry_archived_sessions.get(&dsl_identity)?;
                        SessionId::parse(&archived.0).map_err(|error| {
                            MobError::Internal(format!(
                                "'{identity}' holds an invalid archived session record: {error}"
                            ))
                        })
                    }
                };
                Some((identity.clone(), session))
            })
            .collect()
    }

    /// The role `session_id` was last built under, as its owner records it
    /// (archived sessions included). `None` for a session no build created,
    /// or one that records no role. A service without the retained metadata
    /// seam answers through the ordinary metadata read, which sees no
    /// archived session.
    async fn resume_session_role(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ProfileName>, MobError> {
        let metadata = match self
            .session_service
            .load_retained_session_metadata(session_id)
            .await
        {
            Ok(metadata) => metadata,
            Err(meerkat_core::service::SessionError::Unsupported(_)) => self
                .session_service
                .load_persisted_session_metadata(session_id)
                .await
                .map_err(MobError::SessionError)?,
            Err(error) => return Err(MobError::SessionError(error)),
        };
        Ok(metadata
            .and_then(|view| view.session_metadata)
            .and_then(|metadata| {
                crate::build::stored_member_role_name(&metadata).map(ProfileName::from)
            }))
    }

    /// The declared identities the identity store manages: an intent row,
    /// readable or not, names them. A mob without an identity store has none.
    async fn declared_apply_intent_managed(
        &self,
        declared: &BTreeMap<AgentIdentity, DeclaredEntry>,
    ) -> Result<BTreeSet<AgentIdentity>, MobError> {
        let Some(store) = self.identity_member.as_ref() else {
            return Ok(BTreeSet::new());
        };
        let history = self
            .member_binding_fold
            .read(store.as_ref(), self.mob_id())
            .await?;
        Ok(declared
            .keys()
            .filter(|identity| {
                history.identity_management(identity)
                    != super::MemberIdentityManagement::CommandDriven
            })
            .cloned()
            .collect())
    }

    /// MobMachine's verdicts for `declarations`, one chunk per actor step.
    async fn declare_identity_entries(
        &self,
        declarations: Vec<IdentityDeclaration>,
    ) -> Result<Vec<DeclarationVerdict>, MobError> {
        let mut verdicts = Vec::with_capacity(declarations.len());
        let mut declarations = declarations.into_iter().peekable();
        while declarations.peek().is_some() {
            let chunk: Vec<IdentityDeclaration> =
                declarations.by_ref().take(DECLARED_APPLY_CHUNK).collect();
            verdicts.extend(
                self.send_actor_command(|reply_tx| MobCommand::DeclareIdentityEntries {
                    declarations: chunk,
                    reply_tx,
                })
                .await??,
            );
        }
        Ok(verdicts)
    }

    /// The seat a SeatOwed verdict owes: on a first-boot request's session,
    /// on the identity's archived session (a revival), or fresh. A request
    /// for a fresh life lets the archived session record go first. A seat
    /// that resumes a session last built under another role
    /// (`resume_session_role`) declares the migration from that role, which
    /// MobMachine's verdict admitted (the roster declares it), so the
    /// session's next build moves the member to its declared role.
    async fn owed_seat(
        &self,
        entry: &DeclaredEntry,
        request: Option<&FirstBootRequest>,
        archived_session: Option<SessionId>,
        session_memory_only: bool,
        resume_session_role: Option<&ProfileName>,
    ) -> Result<OwedSeat, MobError> {
        let mut spec = entry.spec.clone().with_entry_custody(true);
        spec.history_unavailable = None;
        // The role the resumed session was last built under, when it is
        // another than the declared one.
        let moved_from = resume_session_role
            .filter(|role| **role != entry.spec.role_name)
            .cloned();
        let resume = |spec: SpawnMemberSpec, session_id: SessionId| match moved_from.clone() {
            Some(from) => spec.with_resume_bridge_session_id_from_role(session_id, from),
            None => spec.with_resume_bridge_session_id(session_id),
        };
        match request {
            Some(FirstBootRequest::Resume { session_id }) => {
                if let Some(archived) = archived_session.as_ref()
                    && archived != session_id
                {
                    return Err(MobError::WiringError(format!(
                        "'{}' has a first-boot request for session {session_id}, but its \
                         archived session is {archived}: start its fresh life with a Fresh \
                         request, or resume the archived session",
                        spec.identity
                    )));
                }
                Ok(OwedSeat {
                    spec: resume(spec, session_id.clone()),
                    revives: archived_session,
                    fresh_life: false,
                    first_boot_request: true,
                })
            }
            Some(FirstBootRequest::Fresh) => {
                if let Some(archived) = archived_session {
                    self.forget_archived_session(
                        &spec.identity,
                        &archived,
                        mob_dsl::ArchivedSessionForgetReason::FreshLifeRequested,
                    )
                    .await?;
                }
                spec.launch_mode = crate::launch::MemberLaunchMode::Fresh;
                Ok(OwedSeat {
                    spec,
                    revives: None,
                    fresh_life: true,
                    first_boot_request: true,
                })
            }
            None => match archived_session {
                Some(archived) if !session_memory_only => Ok(OwedSeat {
                    spec: resume(spec, archived.clone()),
                    revives: Some(archived),
                    fresh_life: false,
                    first_boot_request: false,
                }),
                _ => {
                    spec.launch_mode = crate::launch::MemberLaunchMode::Fresh;
                    Ok(OwedSeat {
                        spec,
                        revives: None,
                        fresh_life: false,
                        first_boot_request: false,
                    })
                }
            },
        }
    }

    /// Seat the owed seats in chunks and answer each. Returns the fresh
    /// seats owed for revivals whose archived session is gone from the
    /// store: the record is let go (`ForgetArchivedSession`), and each is
    /// seated again on a fresh session with the notice.
    async fn realize_seats(
        &self,
        seats: Vec<OwedSeat>,
        outcome: &mut DeclaredRosterOutcome,
    ) -> Vec<OwedSeat> {
        let mut fresh_after_loss = Vec::new();
        let mut seats = seats.into_iter().peekable();
        while seats.peek().is_some() {
            let chunk: Vec<OwedSeat> = seats.by_ref().take(DECLARED_APPLY_CHUNK).collect();
            let specs: Vec<SpawnMemberSpec> = chunk.iter().map(|seat| seat.spec.clone()).collect();
            let answers = match self.seat_dormant_members(specs).await {
                Ok(answers) => answers,
                Err(error) => {
                    // The chunk failed as a whole: each of its identities is
                    // answered with the chunk's error.
                    let retryable = apply_error_is_retryable(&error);
                    let error = std::sync::Arc::new(error);
                    for seat in chunk {
                        let shared =
                            MobError::SharedLifecycleFailure(std::sync::Arc::clone(&error));
                        outcome.entries.insert(
                            seat.spec.identity.clone(),
                            if retryable {
                                DeclaredEntryOutcome::Deferred { reason: shared }
                            } else {
                                DeclaredEntryOutcome::Failed { error: shared }
                            },
                        );
                    }
                    continue;
                }
            };
            for (seat, answer) in chunk.into_iter().zip(answers) {
                let identity = seat.spec.identity.clone();
                let history_unavailable = seat.spec.history_unavailable.clone();
                if seat.first_boot_request
                    && matches!(
                        answer,
                        super::DormantSeatOutcome::Seated { .. }
                            | super::DormantSeatOutcome::AlreadySeated { .. }
                            | super::DormantSeatOutcome::Embodied { .. }
                    )
                {
                    outcome
                        .first_boot_requests_carried_out
                        .insert(identity.clone());
                }
                let answer = match answer {
                    super::DormantSeatOutcome::Seated { session_id, .. }
                    | super::DormantSeatOutcome::AlreadySeated { session_id, .. }
                        if seat.fresh_life =>
                    {
                        DeclaredEntryOutcome::FreshLife { session_id }
                    }
                    super::DormantSeatOutcome::Seated { session_id, .. }
                    | super::DormantSeatOutcome::AlreadySeated { session_id, .. } => {
                        DeclaredEntryOutcome::Seated {
                            session_id,
                            history_unavailable,
                        }
                    }
                    super::DormantSeatOutcome::Embodied { session_id, .. } if seat.fresh_life => {
                        DeclaredEntryOutcome::FreshLife { session_id }
                    }
                    super::DormantSeatOutcome::Embodied { session_id, .. } => {
                        DeclaredEntryOutcome::Embodied {
                            session_id,
                            history_unavailable,
                        }
                    }
                    // A revival whose archived session is gone from the store:
                    // the record is let go, and the identity is seated fresh
                    // with the notice. Never for a store that cannot be read
                    // now (that is deferred above).
                    super::DormantSeatOutcome::Refused {
                        error:
                            MobError::SessionUnavailableForResume {
                                session_id,
                                reason: crate::error::SessionResumeUnavailableReason::Absent,
                                ..
                            },
                        ..
                    } if seat.revives.as_ref() == Some(&session_id) => {
                        match self
                            .forget_archived_session(
                                &identity,
                                &session_id,
                                mob_dsl::ArchivedSessionForgetReason::SessionAbsent,
                            )
                            .await
                        {
                            Ok(()) => {
                                let mut spec = seat.spec;
                                spec.launch_mode = crate::launch::MemberLaunchMode::Fresh;
                                spec.history_unavailable =
                                    Some(HistoryUnavailable::new(session_id));
                                // The request (if any) named the lost
                                // session: the fresh seat carries out none.
                                fresh_after_loss.push(OwedSeat {
                                    spec,
                                    revives: None,
                                    fresh_life: false,
                                    first_boot_request: false,
                                });
                                continue;
                            }
                            Err(error) => failure_outcome(error),
                        }
                    }
                    other => failure_outcome(other.into_unseated_error()),
                };
                outcome.entries.insert(identity, answer);
            }
        }
        fresh_after_loss
    }

    /// Let an unseated identity's archived session record go
    /// (`ForgetArchivedSession`).
    async fn forget_archived_session(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        reason: mob_dsl::ArchivedSessionForgetReason,
    ) -> Result<(), MobError> {
        self.apply_machine_input_effects(mob_dsl::MobMachineInput::ForgetArchivedSession {
            agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
            session_id: mob_dsl::SessionId::from_domain(session_id),
            reason,
        })
        .await
        .map(|_| ())
    }

    /// Whether a member holds `identity` in the published machine state.
    fn seated_now(&self, identity: &AgentIdentity) -> bool {
        super::identity_entry::machine_seats(&self.machine_state_watch_rx.borrow(), identity)
    }

    /// Close the open declared apply (`CloseDeclaredApply`).
    /// A seated member's first-boot `Fresh` request: its rotation onto a
    /// fresh session under the declared spec, once it is idle.
    async fn fresh_life_of_seated(&self, entry: &DeclaredEntry) -> DeclaredEntryOutcome {
        if let Err(error) = self.release_member(&entry.spec.identity).await {
            return failure_outcome(error);
        }
        match self
            .rotate_member(entry.spec.clone().with_entry_custody(true))
            .await
        {
            Ok(session_id) => DeclaredEntryOutcome::FreshLife { session_id },
            Err(error) => failure_outcome(error),
        }
    }

    /// Realize one verdict that needs no seat; `runtime_mode` is the one the
    /// identity was declared with, and `resume_session_role` the role the
    /// session an unseated identity's seat would resume was last built under.
    async fn realize_verdict(
        &self,
        entry: &DeclaredEntry,
        verdict: mob_dsl::IdentityEntryVerdict,
        runtime_mode: crate::MobRuntimeMode,
        resume_session_role: Option<&ProfileName>,
    ) -> DeclaredEntryOutcome {
        let identity = &entry.spec.identity;
        match verdict {
            mob_dsl::IdentityEntryVerdict::Kept => DeclaredEntryOutcome::Kept,
            mob_dsl::IdentityEntryVerdict::HostedOnFirstUse => {
                DeclaredEntryOutcome::HostedOnFirstUse
            }
            mob_dsl::IdentityEntryVerdict::AwaitHeldCreation => {
                DeclaredEntryOutcome::AwaitingHeldCreation
            }
            mob_dsl::IdentityEntryVerdict::RuntimeModeRestamp
            | mob_dsl::IdentityEntryVerdict::RoleMigrate => {
                // A restamp waits for the member to be idle: the release is
                // the idle check (refused, retryable, while it holds work).
                if let Err(error) = self.release_member(identity).await {
                    return failure_outcome(error);
                }
                let mut request = super::MemberRespawn::new();
                if verdict == mob_dsl::IdentityEntryVerdict::RoleMigrate {
                    request = request.with_profile(entry.spec.role_name.clone());
                }
                request = request.with_runtime_mode(runtime_mode);
                match self.respawn_member(identity.clone(), request).await {
                    Ok(receipt) => {
                        let generation = receipt.runtime_id().generation;
                        if verdict == mob_dsl::IdentityEntryVerdict::RoleMigrate {
                            DeclaredEntryOutcome::RoleMigrated { generation }
                        } else {
                            DeclaredEntryOutcome::Restamped { generation }
                        }
                    }
                    Err(error) => failure_outcome(error),
                }
            }
            mob_dsl::IdentityEntryVerdict::RoleRotate => {
                if let Err(error) = self.release_member(identity).await {
                    return failure_outcome(error);
                }
                match self
                    .rotate_member(entry.spec.clone().with_entry_custody(true))
                    .await
                {
                    Ok(session_id) => DeclaredEntryOutcome::RoleRotated { session_id },
                    Err(error) => failure_outcome(error),
                }
            }
            mob_dsl::IdentityEntryVerdict::RoleMigrationHeld => {
                let seated_role = {
                    let state = self.machine_state_watch_rx.borrow();
                    state
                        .member_profile_names
                        .get(&mob_dsl::AgentIdentity::from_domain(identity))
                        .map(|profile| ProfileName::from(profile.as_str()))
                };
                let from = match seated_role {
                    Some(role) => {
                        // The held member stops serving under its previous
                        // role once it is idle; a busy one finishes its work
                        // first, and the hold refuses its next wake either
                        // way.
                        match self.release_member(identity).await {
                            Ok(()) | Err(MobError::LifecycleOperationPending { .. }) => {}
                            Err(error) => return failure_outcome(error),
                        }
                        Some(role)
                    }
                    // No member holds the identity: its seat would resume a
                    // session last built under this role.
                    None => resume_session_role.cloned(),
                };
                match from {
                    Some(from) => DeclaredEntryOutcome::RoleMigrationHeld {
                        from,
                        to: entry.spec.role_name.clone(),
                    },
                    None => DeclaredEntryOutcome::Failed {
                        error: MobError::Internal(format!(
                            "'{identity}' is held for its role move but holds no role in the \
                             machine state"
                        )),
                    },
                }
            }
            mob_dsl::IdentityEntryVerdict::LifecycleInFlight => DeclaredEntryOutcome::Deferred {
                reason: MobError::LifecycleOperationPending {
                    intent: format!(
                        "declare '{identity}', whose member's spawn, wake or retirement is in \
                         flight"
                    ),
                },
            },
            mob_dsl::IdentityEntryVerdict::RefusedNameHeldByWorker => {
                DeclaredEntryOutcome::Refused {
                    refusal: DeclaredEntryRefusal::NameHeldByWorker,
                }
            }
            mob_dsl::IdentityEntryVerdict::RefusedHostAdopted => DeclaredEntryOutcome::Refused {
                refusal: DeclaredEntryRefusal::HostAdopted,
            },
            mob_dsl::IdentityEntryVerdict::RefusedNonAuthoritative => {
                DeclaredEntryOutcome::Refused {
                    refusal: DeclaredEntryRefusal::NonAuthoritative,
                }
            }
            // A seat verdict is realized by the seat step, and a mob that is
            // not running fails the declaration step itself.
            mob_dsl::IdentityEntryVerdict::SeatOwed
            | mob_dsl::IdentityEntryVerdict::MobNotRunning => DeclaredEntryOutcome::Failed {
                error: MobError::Internal(format!(
                    "the declared apply realized '{identity}''s verdict {verdict:?} outside its \
                     step"
                )),
            },
        }
    }

    /// Seal a complete roster and retire the member of every Declared entry
    /// the seal marked; answer every entry the seal unlisted.
    async fn seal_declared_roster(
        &self,
        listed: BTreeSet<AgentIdentity>,
        outcome: &mut DeclaredRosterOutcome,
    ) -> Result<(), MobError> {
        let answer = self
            .send_actor_command(|reply_tx| MobCommand::SealDeclaredRoster {
                desired: listed,
                reply_tx,
            })
            .await??;
        match answer.verdict {
            mob_dsl::DeclaredRosterSealVerdict::Sealed => {}
            // A candidate composition unlists nothing: every declared
            // identity was refused already.
            mob_dsl::DeclaredRosterSealVerdict::NonAuthoritative => return Ok(()),
            // The actor answers a mob that is not running with its own
            // invalid transition.
            mob_dsl::DeclaredRosterSealVerdict::MobNotRunning => {
                return Err(MobError::Internal(
                    "the seal of a declared roster answered a mob that is not running".to_string(),
                ));
            }
        }
        for identity in answer.removed {
            outcome
                .unlisted
                .insert(identity, UnlistedEntryOutcome::Removed);
        }
        for identity in answer.marked {
            let answer = match self.retire(identity.clone()).await {
                Ok(()) => UnlistedEntryOutcome::Retired,
                Err(error) => UnlistedEntryOutcome::Failed { error },
            };
            outcome.unlisted.insert(identity, answer);
        }
        Ok(())
    }
}
