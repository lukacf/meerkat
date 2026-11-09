//! A stock governed realm staged before its native host is installed: the
//! fresh pre-open path of a standalone session whose controller must be
//! prepared, reviewed and granted before the session exists.
//!
//! [`stage_stock_realm_persistence_as`] opens the realm's stores and builds
//! its governed machine, which holds the realm's exclusive execution custody
//! and its sole auth lifecycle owner from then on. A competing opener of the
//! realm is refused promptly by the existing custody rules for as long as the
//! stage lives; nothing replaces, pauses or reacquires a live owner, and the
//! stage has no admitted session.
//!
//! The one, non-`Clone` [`StagedStockRealm`] then sequences, in order:
//!
//! 1. [`StagedStockRealm::prepare_controller`]: the controller of the session
//!    the caller will create is resolved, decorated, fallback-assembled and
//!    pinned under that same auth owner. Its route report is returned for the
//!    operator's review with a ticket for the session's one build; the client
//!    itself never leaves the stage's single shared cell (see
//!    [`PreparedControllerTicket`]).
//! 2. [`StagedStockRealm::grant_stock_agent`]: the operator's grant command,
//!    under the stage's own custody, on its owned task. A waiter that stops
//!    waiting leaves the exact entered command retained in the stage; the
//!    next grant, [`StagedStockRealm::finish`] and [`StagedStockRealm::abort`]
//!    join it first, and the step that settles it reports its exact outcome.
//! 3. [`StagedStockRealm::finish`]: once no entered command can still
//!    publish, the realm's grant owner is restored and the agent's selection
//!    snapshotted into the governed composition, the native host is installed
//!    on the staged machine exactly once, and the realm's bundle is
//!    assembled around it. The ticket stays valid for the session's build,
//!    whose session bindings clone the same auth owner.
//!
//! [`StagedStockRealm::abort`] retires and drains the prepared controller
//! (every ticket alias is inert at once), joins this stage's own entered
//! grant command through its owned task, drops the staged machine and
//! reports whether that release of the realm's custody actually completed.
//!
//! `finish` and `abort` borrow the stage and end it only when they complete.
//! A caller that stops waiting for either leaves the stage, and the entered
//! command it was joining, exactly as they were: calling the same step again
//! on the same stage rejoins that command and completes the step. Once a step
//! completed, the stage has ended and refuses every further step.
//!
//! Dropping the stage before it ended also retires the prepared controller;
//! an entered grant command then keeps running on its owned task, with its
//! own share of the custody, until it settles, and nothing waits for it.
//!
//! Sign-in and connector setup use the stage's own auth owner through its
//! existing provider-auth pair
//! ([`StagedStockRealm::provider_auth_runtime_authority`]), before the
//! controller is prepared under that same owner.
//!
//! Credential work the auth owner started (a refresh, a command credential
//! process, a pending OAuth flow) stays with its existing owners and is
//! never joined here. While any of it still holds the auth owner the realm's
//! custody is retained, and the abort outcome says so; its completion is
//! never claimed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::service::CreateSessionRequest;
use meerkat_runtime::{MeerkatMachine, StagedCustodyRelease, StagedMeerkatMachine};
use meerkat_store::{RealmBackend, RealmOrigin};

use super::RealmGovernance;
use super::stock::issuance::{
    StockGrantError, StockGrantObservation, StockGrantOutcome, StockGrantRequest, StockGrantSlot,
};
use super::stock::{StockGovernance, StockGovernanceError, StockPrincipals};
use crate::controller_route::ControllerRouteReport;
use crate::persistence::{PersistenceError, StagedRealmStores};
use crate::prepared_controller::{
    PreparedControllerCell, PreparedControllerDisposal, PreparedControllerTicket,
};
use crate::{BuildAgentError, FactoryAgentBuilder, PersistenceBundle};

/// Why a staged realm step did not complete. Each refusal stops this setup
/// only: nothing admitted elsewhere is touched.
#[derive(Debug, thiserror::Error)]
pub enum StagedRealmError {
    /// The realm's stores, admission or machine could not be opened, or its
    /// custody is held by another opener.
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    /// Only a governed realm is staged.
    #[error("realm '{realm_id}' is not governed")]
    NotGoverned { realm_id: String },
    /// The stock owners of this process are unavailable.
    #[error(transparent)]
    Governance(#[from] StockGovernanceError),
    /// The stage's controller was refused or could not be prepared.
    #[error(transparent)]
    Controller(#[from] BuildAgentError),
    /// The stage already prepared its controller.
    #[error("the stage already prepared its controller")]
    ControllerAlreadyPrepared,
    /// The prepared controller's pin reports no route, so it cannot be
    /// reviewed or granted. It was dropped.
    #[error("the prepared controller reports no route")]
    ControllerRouteUnavailable,
    /// The realm's governed owners did not restore when the stage finished:
    /// the realm was not installed with unavailable owners in their place.
    /// The stage is left as it was.
    #[error("the realm's governed owners are unavailable")]
    GovernanceUnavailable,
    /// The stage's grant command did not complete (see its outcomes).
    #[error(transparent)]
    Grant(#[from] StockGrantError),
    /// The stage already finished or aborted.
    #[error("the staged realm already finished or aborted")]
    Ended,
    /// The stage's abort has started (its controller is retired); only
    /// aborting it again completes it.
    #[error("the staged realm is being aborted")]
    Aborting,
}

/// What a stage's grant commands did to the realm: a classification of
/// their effects, not proof that any cleanup or custody release completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StagedIssuanceEffect {
    /// No grant command of the stage changed anything: none ran, or each was
    /// refused before any effect.
    NotEntered,
    /// Every grant command that changed the realm completed with a known
    /// outcome (an issued grant, and its selection when that was written).
    Committed,
    /// At least one command's outcome is unknown: it may have issued,
    /// selected or revoked.
    Uncertain,
}

impl StagedIssuanceEffect {
    fn of(outcome: &Result<StockGrantOutcome, StockGrantError>) -> Self {
        match outcome {
            Ok(_) | Err(StockGrantError::NotSelected { .. }) => Self::Committed,
            // A held or unusable admission fence refuses before any effect.
            Err(
                StockGrantError::Refused(_)
                | StockGrantError::AdmissionInProgress
                | StockGrantError::AdmissionFenceUnavailable(_),
            ) => Self::NotEntered,
            Err(
                StockGrantError::IssuanceUncertain { .. }
                | StockGrantError::SelectionUncertain { .. }
                | StockGrantError::CommandUncertain,
            ) => Self::Uncertain,
        }
    }
}

/// The controller a stage prepared, for the operator's review and the
/// session's one build.
#[derive(Debug)]
pub struct PreparedControllerOffer {
    /// The trusted report of the pin's actual lowered route, read from the
    /// pin itself: what a controller grant for it must name.
    pub report: ControllerRouteReport,
    /// The session's build consumes the prepared controller through this
    /// ticket (see [`PreparedControllerTicket::seat_build`]).
    pub ticket: PreparedControllerTicket,
}

/// The exact outcome of one grant command of a stage.
pub type StagedGrantOutcome = Result<StockGrantOutcome, StockGrantError>;

/// A finished stage: the realm open with its governed machine installed.
pub struct StagedRealmOpen {
    pub pin: meerkat_store::RealmManifestPin,
    pub bundle: PersistenceBundle,
    /// What the stage's grant commands did, every one of them settled.
    pub issuance: StagedIssuanceEffect,
    /// The exact outcomes of grant commands whose callers stopped waiting
    /// for them, in the order the stage settled them.
    pub unobserved_grants: Vec<StagedGrantOutcome>,
}

impl std::fmt::Debug for StagedRealmOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedRealmOpen")
            .field("issuance", &self.issuance)
            .field("unobserved_grants", &self.unobserved_grants)
            .finish_non_exhaustive()
    }
}

/// What [`StagedStockRealm::abort`] did.
#[derive(Debug)]
pub struct StagedAbortOutcome {
    /// What the stage's grant commands did; an entered one was joined first.
    pub issuance: StagedIssuanceEffect,
    /// The prepared controller, as the first abort attempt retired it:
    /// drained (every ticket alias is inert), or already taken by the build
    /// that owns it now.
    pub controller: PreparedControllerDisposal,
    /// Whether this abort completed the release of the realm's execution
    /// custody. `Retained` means a handle sharing it is still alive (owned
    /// auth work holding the auth owner): the realm cannot be reopened until
    /// it ends, and nothing here waits for it.
    pub custody: StagedCustodyRelease,
    /// The exact outcomes of grant commands whose callers stopped waiting
    /// for them, in the order the stage settled them.
    pub unobserved_grants: Vec<StagedGrantOutcome>,
}

/// One stock governed realm staged before its native host is installed (see
/// the module docs). Not `Clone`: one owner sequences every step.
pub struct StagedStockRealm {
    realms_root: PathBuf,
    realm_id: String,
    stock: Arc<StockGovernance>,
    /// The realm's stores and staged machine, until a completed finish
    /// installs the host on that machine or a completed abort releases it.
    staged: Option<StagedParts>,
    controller: StagedController,
    /// The prepared controller's disposal, fixed by the first abort attempt,
    /// which retires it before anything else.
    controller_disposal: Option<PreparedControllerDisposal>,
    /// The grant command entered under the stage's custody, until it is
    /// joined: a waiter that stops waiting leaves it here.
    entered_grant: Option<tokio::task::JoinHandle<StagedGrantOutcome>>,
    issuance: StagedIssuanceEffect,
    /// Outcomes of commands settled after their callers stopped waiting,
    /// until the finishing or aborting step reports them.
    unobserved_grants: Vec<StagedGrantOutcome>,
}

/// What a stage holds until it ends.
struct StagedParts {
    stores: StagedRealmStores,
    machine: StagedMeerkatMachine,
}

/// A step the stage's own state refuses outright.
#[derive(Debug, Clone, Copy)]
enum StepRefusal {
    Ended,
    Aborting,
}

impl From<StepRefusal> for StagedRealmError {
    fn from(refusal: StepRefusal) -> Self {
        match refusal {
            StepRefusal::Ended => Self::Ended,
            StepRefusal::Aborting => Self::Aborting,
        }
    }
}

impl std::fmt::Debug for StagedStockRealm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedStockRealm")
            .field("realm_id", &self.realm_id)
            .field("ended", &self.staged.is_none())
            .field("controller_prepared", &self.controller.0.is_some())
            .field("grant_entered", &self.entered_grant.is_some())
            .field("issuance", &self.issuance)
            .finish_non_exhaustive()
    }
}

/// The stage's prepared controller cell. Dropped with an unfinished stage,
/// it retires the cell: an unfinished stage never leaves a usable prepared
/// controller behind.
struct StagedController(Option<Arc<PreparedControllerCell>>);

impl StagedController {
    fn retire(&mut self) -> PreparedControllerDisposal {
        self.0
            .take()
            .map_or(PreparedControllerDisposal::NotPrepared, |cell| {
                cell.retire()
            })
    }

    /// The finished stage hands the cell to its tickets alone.
    fn release_to_tickets(&mut self) {
        self.0 = None;
    }
}

impl Drop for StagedController {
    fn drop(&mut self) {
        self.retire();
    }
}

/// Stage the governed realm `realm_id` as stock rkat opens it for `agent`:
/// its stores admitted and its governed machine built with the realm's
/// exclusive execution custody and sole auth owner, before any grant is read
/// or any native host installed. Only a governed realm is staged, and only
/// with this process's local operator; a competing opener holding the realm
/// refuses the stage promptly. Without `layout` the realm opens under the
/// single-candidate layout of an explicit state root.
pub async fn stage_stock_realm_persistence_as(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
    agent: &str,
) -> Result<StagedStockRealm, StagedRealmError> {
    let realm = meerkat_core::connection::RealmId::parse(realm_id).map_err(|_| {
        StagedRealmError::NotGoverned {
            realm_id: realm_id.to_string(),
        }
    })?;
    let principals = StockPrincipals::for_local_operator(&realm)?;
    let stock = StockGovernance::new_for_agent(realm, principals, agent)?;
    stage_stock_realm_persistence_with(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        layout,
        stock,
    )
    .await
}

/// [`stage_stock_realm_persistence_as`] with the stock composition `stock`.
pub(crate) async fn stage_stock_realm_persistence_with(
    provider: &dyn crate::storage_provider::RealmStorageProvider,
    realms_root: &Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
    layout: Option<meerkat_core::StorageLayout>,
    stock: Arc<StockGovernance>,
) -> Result<StagedStockRealm, StagedRealmError> {
    let not_governed = || StagedRealmError::NotGoverned {
        realm_id: realm_id.to_string(),
    };
    let layout = match layout {
        Some(layout) => layout,
        None => crate::persistence::layout_for_explicit_state_root(realms_root, realm_id)?,
    };
    if meerkat_store::read_realm_posture_in(realms_root, realm_id)
        .await
        .map_err(PersistenceError::from)?
        != meerkat_store::RealmPosture::Governed
    {
        return Err(not_governed());
    }
    // The realm owner's review rules, from the realm's effective
    // configuration, before the stage can compose its owners. A stage exists
    // only when this read succeeded: its composition serves exactly these
    // rules (see `finish`).
    super::configure_stock_review(&stock, realms_root, realm_id, &layout).await?;
    let stores = crate::persistence::stage_realm_persistence_stores(
        provider,
        realms_root,
        realm_id,
        backend_hint,
        origin_hint,
        Some(layout),
    )
    .await?;
    let machine = MeerkatMachine::persistent_governed_staged(
        stores.runtime_store(),
        Some(stores.blob_store()),
    )
    .map_err(PersistenceError::from)?;
    // Re-read under the custody now held: the posture the stage proceeds
    // with holds for as long as it lives.
    if meerkat_store::read_realm_posture_in(realms_root, realm_id)
        .await
        .map_err(PersistenceError::from)?
        != meerkat_store::RealmPosture::Governed
    {
        return Err(not_governed());
    }
    Ok(StagedStockRealm {
        realms_root: realms_root.to_path_buf(),
        realm_id: realm_id.to_string(),
        stock,
        staged: Some(StagedParts { stores, machine }),
        controller: StagedController(None),
        controller_disposal: None,
        entered_grant: None,
        issuance: StagedIssuanceEffect::NotEntered,
        unobserved_grants: Vec::new(),
    })
}

impl StagedStockRealm {
    /// The staged realm's id.
    pub fn realm_id(&self) -> &str {
        &self.realm_id
    }

    /// The stage's stock composition (its principals and agent).
    pub fn stock_governance(&self) -> &Arc<StockGovernance> {
        &self.stock
    }

    fn parts(&self) -> Result<&StagedParts, StepRefusal> {
        self.staged.as_ref().ok_or(StepRefusal::Ended)
    }

    /// The stage's parts for a step other than abort: refused once an abort
    /// has started.
    fn steppable(&self) -> Result<&StagedParts, StepRefusal> {
        let parts = self.parts()?;
        if self.controller_disposal.is_some() {
            return Err(StepRefusal::Aborting);
        }
        Ok(parts)
    }

    /// The stage's current provider-auth pair, for sign-in and connector
    /// setup through the stage's own auth owner before its controller is
    /// prepared (for example with [`crate::HostAuthService::from_factory`]).
    /// It is the staged machine's existing pair, not a new runtime or auth
    /// owner: [`Self::prepare_controller`] resolves under that same owner,
    /// and the finished session's bindings clone it.
    ///
    /// A clone shares the stage's custody claim, never its machine, so
    /// holding one does not refuse [`Self::finish`]. While auth work still
    /// holds a clone when the stage is aborted, the realm's custody stays
    /// with it and the abort reports `Retained`; the abort does not wait for
    /// that work. Refused once an abort has started or the stage has ended.
    #[allow(clippy::result_large_err)] // the same typed refusal every step of the stage returns
    pub fn provider_auth_runtime_authority(
        &self,
    ) -> Result<meerkat_runtime::ProviderAuthRuntimeAuthority, StagedRealmError> {
        Ok(self.steppable()?.machine.provider_auth_runtime_authority())
    }

    /// Prepare the controller of the session `request` will create, for its
    /// pre-assigned `session_id`, under the stage's auth owner (see
    /// [`PreparedControllerTicket`]). `builder` is the builder the realm's
    /// persistent service will build that session with; the realm is its
    /// default realm, as the runtime-backed service sets it. No model request
    /// is sent. One controller per stage.
    pub async fn prepare_controller(
        &mut self,
        builder: &FactoryAgentBuilder,
        request: &CreateSessionRequest,
        session_id: SessionId,
    ) -> Result<PreparedControllerOffer, StagedRealmError> {
        let parts = self.steppable()?;
        if self.controller.0.is_some() {
            return Err(StagedRealmError::ControllerAlreadyPrepared);
        }
        let auth_lease = parts.machine.generated_auth_lease_handle();
        let mut builder = builder.clone();
        builder.default_realm_id = parts.stores.manifest_realm_id();
        let graph = builder
            .prepare_controller_graph(request, auth_lease, &session_id)
            .await?;
        let report = crate::controller_route::controller_route_report(&graph.controller)
            .ok_or(StagedRealmError::ControllerRouteUnavailable)?;
        let cell = PreparedControllerCell::new(session_id, graph);
        self.controller.0 = Some(Arc::clone(&cell));
        Ok(PreparedControllerOffer {
            report,
            ticket: PreparedControllerTicket::new(cell),
        })
    }

    /// Issue `request` as the realm's operator under the stage's own custody
    /// and select it for its target, exactly as
    /// [`crate::governed_realm::stock::issuance::grant_stock_agent`] does under an
    /// administrative custody (see its outcomes). The realm's admission
    /// fence is taken first, without waiting since the stage's custody is
    /// already held: an admission's hold refuses at once with
    /// [`StockGrantError::AdmissionInProgress`], before any effect.
    ///
    /// The command runs on its owned task. A caller that stops waiting leaves
    /// that exact entered command retained by the stage; this method, and
    /// [`Self::finish`] and [`Self::abort`], join it before anything else.
    pub async fn grant_stock_agent(
        &mut self,
        request: StockGrantRequest,
    ) -> Result<StockGrantOutcome, StagedRealmError> {
        self.steppable()?;
        self.settle_unobserved_grant().await;
        let machine = self.steppable()?.machine.administrative_machine();
        let command = tokio::spawn(super::stock::issuance::grant_under_staged_custody(
            machine,
            self.realms_root.clone(),
            self.realm_id.clone(),
            self.stock.principals().clone(),
            request,
        ));
        self.entered_grant = Some(command);
        self.settle_entered_grant()
            .await
            .unwrap_or(Err(StockGrantError::CommandUncertain))
            .map_err(StagedRealmError::Grant)
    }

    /// Observe `agent`'s selected grants in `slots` as the realm's persisted
    /// selection names them, under the stage's own custody, through the
    /// realm's grant owner restored for this read only: the read
    /// [`crate::governed_realm::stock::issuance::observe_stock_agent_grants`]
    /// makes under an administrative custody. The stage's entered grant
    /// command is joined first, so the read sees its outcome; a caller that
    /// stops waiting leaves that command with the stage. Once the stage
    /// finished, the open realm observes its retained selection instead
    /// ([`StockGovernance::observe_retained_grants`]).
    pub async fn observe_stock_agent_grants(
        &mut self,
        agent: &str,
        slots: &[StockGrantSlot],
    ) -> Result<StockGrantObservation, StagedRealmError> {
        self.steppable()?;
        self.settle_unobserved_grant().await;
        self.steppable()?;
        Ok(super::stock::issuance::observe_persisted_under_custody(
            &self.realms_root,
            &self.realm_id,
            agent,
            slots,
        )
        .await?)
    }

    /// Drain the exact outcomes of grant commands whose callers stopped
    /// waiting for them and that this stage has settled since, in the order
    /// it settled them. Each outcome is reported once: here, or by the step
    /// that completes the stage, whichever comes first. This reads them after
    /// a terminal finish error ended the stage too.
    pub fn take_unobserved_grants(&mut self) -> Vec<StagedGrantOutcome> {
        std::mem::take(&mut self.unobserved_grants)
    }

    /// Join the stage's entered grant command, if any, and record its effect.
    /// Cancelled while waiting, the command stays retained.
    async fn settle_entered_grant(&mut self) -> Option<StagedGrantOutcome> {
        let entered = self.entered_grant.as_mut()?;
        let joined = entered.await;
        self.entered_grant = None;
        let outcome = joined.unwrap_or(Err(StockGrantError::CommandUncertain));
        self.issuance = self.issuance.max(StagedIssuanceEffect::of(&outcome));
        Some(outcome)
    }

    /// Join an entered command whose caller stopped waiting and keep its
    /// exact outcome for the step that reports it. Cancelled while waiting,
    /// the command stays retained.
    async fn settle_unobserved_grant(&mut self) {
        if let Some(outcome) = self.settle_entered_grant().await {
            self.unobserved_grants.push(outcome);
        }
    }

    /// Finish the stage: join its entered grant command, restore the realm's
    /// grant owner and snapshot the agent's selection into the governed
    /// composition, install the native host on the staged machine once, and
    /// assemble the realm's bundle around it. A prepared controller's ticket
    /// stays valid for the session's build.
    ///
    /// A caller that stops waiting leaves the stage and its entered command
    /// as they were; finishing it again rejoins that command. Refused with
    /// the stage left as it was when the composition's owners are
    /// unavailable (abort it to release the realm). Refused with the stage
    /// ended, its controller retired, when the host cannot be installed or
    /// the bundle cannot be assembled; the grant outcomes it settled before
    /// that stay readable through [`Self::take_unobserved_grants`].
    pub async fn finish(&mut self) -> Result<StagedRealmOpen, StagedRealmError> {
        self.steppable()?;
        // Nothing is snapshotted while an entered command can still publish.
        self.settle_unobserved_grant().await;
        // The stage's own read of the review rules succeeded when it was
        // staged (it is refused otherwise).
        let composition = super::governed_composition(
            RealmGovernance::Stock(Arc::clone(&self.stock)),
            Some(Ok(())),
            self.parts()?.stores.paths(),
        )
        .await;
        if matches!(composition, super::GovernedComposition::Unavailable) {
            return Err(StagedRealmError::GovernanceUnavailable);
        }
        // No await from here on: the stage ends with this step.
        let StagedParts { stores, machine } = self.staged.take().ok_or(StagedRealmError::Ended)?;
        let machine = match super::install_governed_composition(machine, composition) {
            Ok(machine) => machine,
            Err(error) => {
                self.controller_disposal = Some(self.controller.retire());
                return Err(PersistenceError::from(error).into());
            }
        };
        let (pin, mut bundle) = match crate::persistence::assemble_realm_persistence_bundle(
            stores,
            Arc::new(machine),
        ) {
            Ok(assembled) => assembled,
            Err(error) => {
                self.controller_disposal = Some(self.controller.retire());
                return Err(error.into());
            }
        };
        bundle.set_stock_governance(Some(Arc::clone(&self.stock)));
        self.controller.release_to_tickets();
        Ok(StagedRealmOpen {
            pin,
            bundle,
            issuance: self.issuance,
            unobserved_grants: std::mem::take(&mut self.unobserved_grants),
        })
    }

    /// Abandon the stage: retire and drain its prepared controller so every
    /// ticket alias is inert, join its own entered grant command, drop the
    /// staged machine and report whether that completed the release of the
    /// realm's custody.
    ///
    /// A caller that stops waiting leaves the stage, with its controller
    /// already retired, and its entered command as they were; aborting it
    /// again rejoins that command.
    pub async fn abort(&mut self) -> Result<StagedAbortOutcome, StagedRealmError> {
        self.parts()?;
        let controller = match self.controller_disposal {
            Some(disposal) => disposal,
            None => {
                let disposal = self.controller.retire();
                self.controller_disposal = Some(disposal);
                disposal
            }
        };
        self.settle_unobserved_grant().await;
        // No await from here on: the stage ends with this step.
        let StagedParts { stores, machine } = self.staged.take().ok_or(StagedRealmError::Ended)?;
        drop(stores);
        Ok(StagedAbortOutcome {
            issuance: self.issuance,
            controller,
            custody: machine.release(),
            unobserved_grants: std::mem::take(&mut self.unobserved_grants),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
