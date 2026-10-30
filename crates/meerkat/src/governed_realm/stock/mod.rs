//! Stock rkat's host composition for a governed realm.
//!
//! The realm's posture is selected durably and independently
//! ([`meerkat_store::govern_realm_in`]); stock rkat then opens the realm
//! through [`super::RealmGovernance::Stock`]. This module supplies the stock
//! host's trusted inputs to that composition:
//!
//! - the principals: the local operator authenticated by the operating
//!   system (the process's user), the CLI ingress actor and the realm's agent
//!   executor ([`StockPrincipals`]). Local OS identity is this stock
//!   adapter's authentication only; it is not a remote requester identity;
//! - the posture in which requesters are admitted ([`caller`]): the local
//!   operator alone, or callers a trusted host authenticated, each admitted
//!   by the host's exact-target decision while the operator stays the
//!   canonical owner of every session;
//! - the grants the operator deliberately issued and selected for the
//!   realm's executor ([`StockGrantSelection`], recorded by
//!   [`record_stock_grant_selection`]), re-read on every open and resolved by
//!   the restored grant owner at admission; a reference never grants by
//!   itself, and governing a realm issues none;
//! - the in-process originals ledger ([`originals::StockOriginalLedger`]);
//! - one invocation, admitted-work and operation policy object
//!   ([`policy::StockInvocationPolicy`]) used for both faces of
//!   [`ScopedInvocation::from_policy`], as the operation owner and for
//!   retained resume;
//! - the session's canonical destination ([`destination`]) and the actual
//!   pinned controller ([`controller::StockControllerPin`]), which also
//!   serves the retained resume of that session's work.
//!
//! A prompt of the local operator (from the CLI or a governed JSONL
//! connection) reaches the governed runtime only through
//! [`StockGovernance::bind_prompt`], the outcome of an administrative
//! command the operator's own process reports only through
//! [`StockGovernance::bind_host_outcome`], with the stock ingress itself as
//! its requester, and a host-authenticated caller's input only through
//! [`StockGovernance::bind_caller_prompt`]: the session is bound to its
//! destination, the original is authenticated and recorded, and the final
//! input is bound to it with [`bind_scoped_original`]. Every failure there
//! is a typed local refusal before the input is submitted.
//!
//! Retained work resumes through the scoped original composition's
//! retained-resume check, built from the owners that admitted the work. A
//! continuation of an earlier run of a session (the approved notice of
//! consent parked at that run's close, or a host producer's completion) is
//! admitted while this process's ledger still holds the run's originals, the
//! stock requester rule still admits their requester, the restored grant
//! owner still resolves their grants, and the session keeps the registration
//! that ran it, whose newest pinned controller has the run's exact
//! selection. The originals ledger is process memory: work admitted by an
//! earlier process is not held by it, so its resume is refused, never
//! re-submitted as if it were recovered. Every governed binding of a session
//! advances its destination, and a resume is admitted only at the
//! destination its run was admitted under: after a later prompt in the same
//! session, whichever controller that prompt used, the earlier run's resume
//! is refused for good. After the session is closed and reopened in the
//! same process, the earlier run's resume is not ready (its new registration
//! has no pin for it) until the session's destination is next bound, and is
//! then refused for good.

pub mod caller;
pub mod consent;
pub mod controller;
pub mod destination;
pub mod finalize;
pub mod issuance;
pub mod jsonl;
pub mod originals;
pub mod policy;
pub mod review;

#[cfg(all(test, feature = "mcp", feature = "workspace-tests"))]
mod checked_route_tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grants::{
    FileGrantOwnerStore, GrantDurabilityError, LocalGrantAuthority, LocalGrantConfiguration,
};
use meerkat_authorization::publication::{LocalAuthorizationPublication, PublicationError};
use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, ResourceDomain};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::connection::RealmId;
use meerkat_core::{
    ControllerModelClient, ControllerModelSelection, OperationAuthorizationError,
    OperationRefusalKind, OperationRefused, PrincipalKind, PrincipalRef, SessionId, TrustDomainId,
};
use meerkat_runtime::input::{ExternalEventInput, Input, InputDurability, InputOrigin};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, NativeRetainedResumeCheck,
    RetainedInvocationOwner, ScopedInvocation, ScopedRetainedResume, bind_scoped_original,
};
use meerkat_runtime::{IdempotencyKey, LogicalRuntimeId, MeerkatMachine, RuntimeDriverError};
use meerkat_store::RealmPaths;
use serde::{Deserialize, Serialize};

use self::caller::{StockAuthenticatedCaller, StockCallerAccess, StockCallerPosture};
use self::controller::StockResumePins;
use self::originals::{PendingOriginal, StockOriginalLedger};

/// The stock CLI ingress namespace of authenticated prompts.
const PROMPT_NAMESPACE: &str = "cli-prompts";
/// The grants the operator issued when governing the realm, beside the
/// realm's grant owner.
const STOCK_GRANTS_FILE: &str = "stock-grants.json";
/// The grant owner namespace of a stock-governed realm.
const STOCK_GRANT_NAMESPACE: &str = "stock-realm-grants";
/// How many times a caller's binding observes its admission again when the
/// composition's publication moved before the binding's commit (another
/// owner change, for example a concurrent original) or an owner change held
/// the publication at the commit.
const CALLER_BIND_ATTEMPTS: usize = 3;

/// A stock governed realm could not be set up or a prompt could not be bound.
/// Each is local and typed; none selects ungoverned behavior.
#[derive(Debug, thiserror::Error)]
pub enum StockGovernanceError {
    #[error("the local operator identity is unavailable on this platform")]
    OperatorUnavailable,
    #[error("invalid stock principal: {0}")]
    Principal(String),
    #[error("the operator has not selected the realm's stock grants, or they are unreadable")]
    GrantsUnavailable,
    #[error("the realm's grant owner is unavailable: {0}")]
    GrantOwner(String),
    #[error(
        "the realm has a grant owner but no stock grant selection; it was not governed by rkat"
    )]
    ForeignGrantOwner,
    #[error("governing this realm is unsupported: {0}")]
    Unsupported(String),
    #[error("the session's governed destination is unavailable: {0}")]
    Destination(String),
    #[error("the prompt's original could not be bound: {0}")]
    Binding(String),
    #[error("the realm is not governed")]
    NotGoverned,
    #[error("this stock composition does not admit that requester: {0}")]
    RequesterUnsupported(String),
    #[error("this caller is not admitted to the session now: {0}")]
    CallerNotAdmitted(OperationAuthorizationError),
    #[error("this grant scope is not representable by `rkat realm grant` yet: {0}")]
    ScopeNotRepresentable(String),
    #[error("the realm's MCP configuration is being changed or is unreadable; retry")]
    ConfigurationUnavailable,
    #[error("the realm's installation of MCP server '{0}' changed after it was written")]
    InstallationChanged(String),
    #[error("the administrative command did not complete; its effects are unknown")]
    CommandUncertain,
    #[error("the realm's review rules are unavailable: {0}")]
    Review(String),
    #[error(transparent)]
    Store(#[from] meerkat_store::StoreError),
    #[error(transparent)]
    Persistence(#[from] crate::persistence::PersistenceError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The stock host's principals in one realm's trust domain.
#[derive(Debug, Clone)]
pub struct StockPrincipals {
    /// The local operator: this process's operating-system user.
    pub operator: PrincipalRef,
    /// The CLI ingress actor that authenticated the operator.
    pub ingress_actor: PrincipalRef,
    /// The realm's agent executor.
    pub executor: PrincipalRef,
}

impl StockPrincipals {
    /// The principals of `realm` for this process's operating-system user.
    pub fn for_local_operator(realm: &RealmId) -> Result<Self, StockGovernanceError> {
        Self::for_user(realm, local_user_id()?)
    }

    /// The principals of `realm` for the operating-system user `uid`.
    pub fn for_user(realm: &RealmId, uid: u32) -> Result<Self, StockGovernanceError> {
        let domain = TrustDomainId::new(format!("rkat-realm-{}", realm.as_str()))
            .map_err(|error| StockGovernanceError::Principal(error.to_string()))?;
        let principal = |kind, name: String| {
            PrincipalRef::in_domain(kind, name, domain.clone())
                .map_err(|error| StockGovernanceError::Principal(error.to_string()))
        };
        Ok(Self {
            operator: principal(PrincipalKind::Human, format!("uid-{uid}"))?,
            ingress_actor: principal(PrincipalKind::RuntimeHost, "rkat-cli".to_string())?,
            executor: principal(PrincipalKind::PersonalAgent, "rkat-agent".to_string())?,
        })
    }

    /// The composition's ingress namespace: the stock ingress's
    /// `cli-prompts` domain, in which binding records every original and
    /// admission resolves it.
    fn ingress_domain(&self) -> ResourceDomain {
        ResourceDomain {
            authority: self.ingress_actor.clone(),
            namespace: PROMPT_NAMESPACE.into(),
        }
    }
}

#[cfg(unix)]
fn local_user_id() -> Result<u32, StockGovernanceError> {
    Ok(nix::unistd::getuid().as_raw())
}

#[cfg(not(unix))]
fn local_user_id() -> Result<u32, StockGovernanceError> {
    Err(StockGovernanceError::OperatorUnavailable)
}

/// The grants the operator issued to the realm's executor and selected for
/// its stock prompts: distinct worker (operation) and controller grants. They
/// are references: the restored grant owner resolves them at every
/// admission, and a revoked or foreign reference refuses.
#[derive(Clone, Serialize, Deserialize)]
pub struct StockGrantSelection {
    /// The operation grant the prompts' authority basis names.
    pub operation: GrantLineageRef,
    /// The controller grant the prompts' controller lineage names.
    pub controller: GrantLineageRef,
}

impl std::fmt::Debug for StockGrantSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StockGrantSelection")
            .finish_non_exhaustive()
    }
}

fn stock_grants_path(paths: &RealmPaths) -> PathBuf {
    meerkat_store::grant_owner_store_path(&paths.root).with_file_name(STOCK_GRANTS_FILE)
}

/// The realm's per-agent grant selection file, if recorded.
pub(crate) async fn read_stock_grant_file(
    paths: &RealmPaths,
) -> Result<Option<issuance::StockGrantFile>, StockGovernanceError> {
    match tokio::fs::read(stock_grants_path(paths)).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| StockGovernanceError::GrantsUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(StockGovernanceError::GrantsUnavailable),
    }
}

/// Record the default agent's pair, keeping every other agent's references.
async fn write_stock_grants(
    paths: &RealmPaths,
    selection: &StockGrantSelection,
) -> Result<(), StockGovernanceError> {
    let mut file = read_stock_grant_file(paths).await?.unwrap_or_default();
    #[cfg(test)]
    issuance::held_publication(&paths.root).await;
    let refs = file
        .agents
        .entry(issuance::DEFAULT_STOCK_AGENT.to_string())
        .or_default();
    refs.operation = Some(selection.operation.clone());
    refs.controller = Some(selection.controller.clone());
    // An agent already in per-target mode keeps it: the older worker grant
    // is recorded for cleanup, never selected (unless selected elsewhere).
    file.settle();
    Ok(write_stock_grant_file(paths, &file).await?)
}

/// Where a selection write stopped. Before the rename the selection on disk
/// is unchanged; after it the new selection is in place but may not be
/// durable.
#[derive(Debug)]
pub(crate) enum SelectionWriteFailure {
    NotWritten(std::io::Error),
    NotDurable(std::io::Error),
}

impl From<SelectionWriteFailure> for StockGovernanceError {
    fn from(failure: SelectionWriteFailure) -> Self {
        match failure {
            SelectionWriteFailure::NotWritten(error) | SelectionWriteFailure::NotDurable(error) => {
                Self::Io(error)
            }
        }
    }
}

pub(crate) async fn write_stock_grant_file(
    paths: &RealmPaths,
    file: &issuance::StockGrantFile,
) -> Result<(), SelectionWriteFailure> {
    let path = stock_grants_path(paths);
    let body = serde_json::to_vec_pretty(file)
        .map_err(|error| SelectionWriteFailure::NotWritten(error.into()))?;
    let tmp = path.with_extension(format!("tmp.{}", uuid::Uuid::now_v7()));
    let written = async {
        tokio::fs::write(&tmp, body).await?;
        tokio::fs::File::open(&tmp).await?.sync_all().await?;
        tokio::fs::rename(&tmp, &path).await
    };
    written.await.map_err(SelectionWriteFailure::NotWritten)?;
    if let Some(parent) = path.parent() {
        let synced = async { tokio::fs::File::open(parent).await?.sync_all().await };
        synced.await.map_err(SelectionWriteFailure::NotDurable)?;
    }
    Ok(())
}

/// The stock host's governed composition of one realm, shared by the realm's
/// machine (its originals) and the CLI prompt sites.
pub struct StockGovernance {
    realm: RealmId,
    principals: StockPrincipals,
    /// The restarted process's one publication: the restored grant owner
    /// and the originals ledger publish on it.
    publication: LocalAuthorizationPublication,
    ledger: Arc<StockOriginalLedger>,
    /// The agent's per-target references, as selected when the realm opened.
    agent_refs: OnceLock<issuance::AgentGrantRefs>,
    /// The realm owner's review rules, read from the realm's effective
    /// configuration when it opened. A composition without them is refused.
    review_rules: OnceLock<review::StockReviewRules>,
    /// The agent this process acts as (its executor and grant pair).
    agent: String,
    /// Who may be the requester of this composition's governed inputs.
    posture: StockCallerPosture,
    /// The latest composition's operation policy for attaching a configured
    /// MCP server. Held weakly: the realm's machine owns the policy (and the
    /// grant owner it holds), so closing the realm releases the owner even
    /// while this governance is still referenced.
    configured_policy: std::sync::Mutex<
        Option<std::sync::Weak<dyn meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority>>,
    >,
    /// The latest composition's grant owner, restored on this composition's
    /// publication: a caller's binding commits within its exclusion of owner
    /// changes. Held weakly, like the policy.
    composed_grants: std::sync::Mutex<Option<std::sync::Weak<LocalGrantAuthority>>>,
    /// The realm's own MCP installations, as its installation authority read
    /// them when the realm last opened: the current installation a resumed
    /// session's attachment restore resolves.
    installed_mcp: std::sync::Mutex<Vec<meerkat_core::McpServerConfig>>,
    /// Each session runtime's controller pin from its newest governed
    /// binding, which only the composition's retained-resume check reads: a
    /// capability cache of live clients, not lifecycle state (see
    /// `StockResumePins`).
    resume_pins: Arc<StockResumePins>,
}

impl StockGovernance {
    /// The local-operator composition of `realm`: the operator is its only
    /// requester ([`StockCallerPosture::LocalOperator`]).
    pub fn new(realm: RealmId, principals: StockPrincipals) -> Arc<Self> {
        Self::with_principals(
            realm,
            principals,
            issuance::DEFAULT_STOCK_AGENT.to_string(),
            StockCallerPosture::LocalOperator,
        )
    }

    /// The local-operator composition of `realm` acting as agent `agent`:
    /// the executor is that agent's principal and its grants are that
    /// agent's selection.
    pub fn new_for_agent(
        realm: RealmId,
        principals: StockPrincipals,
        agent: &str,
    ) -> Result<Arc<Self>, StockGovernanceError> {
        Self::for_agent(realm, principals, agent, StockCallerPosture::LocalOperator)
    }

    /// The composition of `realm` whose requesters are the callers a trusted
    /// host authenticates, each admitted by `access` for the exact session it
    /// invokes ([`StockCallerPosture::HostAuthenticated`]).
    /// `principals.operator` stays the canonical owner of every session and
    /// is no requester; inputs are bound with
    /// [`Self::bind_caller_prompt`].
    pub fn host_authenticated(
        realm: RealmId,
        principals: StockPrincipals,
        access: Arc<dyn StockCallerAccess>,
    ) -> Arc<Self> {
        Self::with_principals(
            realm,
            principals,
            issuance::DEFAULT_STOCK_AGENT.to_string(),
            StockCallerPosture::HostAuthenticated(access),
        )
    }

    /// [`Self::host_authenticated`] acting as agent `agent`, as
    /// [`Self::new_for_agent`] selects it.
    pub fn host_authenticated_for_agent(
        realm: RealmId,
        principals: StockPrincipals,
        agent: &str,
        access: Arc<dyn StockCallerAccess>,
    ) -> Result<Arc<Self>, StockGovernanceError> {
        Self::for_agent(
            realm,
            principals,
            agent,
            StockCallerPosture::HostAuthenticated(access),
        )
    }

    fn for_agent(
        realm: RealmId,
        mut principals: StockPrincipals,
        agent: &str,
        posture: StockCallerPosture,
    ) -> Result<Arc<Self>, StockGovernanceError> {
        principals.executor = issuance::stock_agent_principal(&realm, agent)?;
        Ok(Self::with_principals(
            realm,
            principals,
            agent.to_string(),
            posture,
        ))
    }

    fn with_principals(
        realm: RealmId,
        principals: StockPrincipals,
        agent: String,
        posture: StockCallerPosture,
    ) -> Arc<Self> {
        let publication = LocalAuthorizationPublication::new();
        Arc::new(Self {
            realm,
            principals,
            ledger: StockOriginalLedger::new(publication.clone()),
            publication,
            agent_refs: OnceLock::new(),
            review_rules: OnceLock::new(),
            agent,
            posture,
            configured_policy: std::sync::Mutex::new(None),
            composed_grants: std::sync::Mutex::new(None),
            installed_mcp: std::sync::Mutex::new(Vec::new()),
            resume_pins: Arc::default(),
        })
    }

    /// Test support: the next [`PendingOriginal::admit`] of a candidate that
    /// [`Self::bind_prompt`] recorded in THIS governance's own originals
    /// ledger fails exactly as if the promotion's publication could not
    /// begin: the candidate is dropped, nothing publishes, and `admit`
    /// returns `StockCustodyError::Unavailable`. One-shot; every other
    /// governance and every other ledger operation leave it armed.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn fail_next_original_promotion_for_test(&self) {
        self.ledger.fail_next_promotion_for_test();
    }

    /// The realm's own MCP installations, as its installation authority read
    /// them when the realm last opened (empty before it opened).
    pub fn installed_mcp_servers(&self) -> Vec<meerkat_core::McpServerConfig> {
        self.installed_mcp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Record the realm's own MCP installations its opening read.
    pub(crate) fn record_installed_mcp_servers(&self, servers: Vec<meerkat_core::McpServerConfig>) {
        *self
            .installed_mcp
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = servers;
    }

    /// The composition's operation policy for configured MCP attachment, if
    /// the stock owner supplies one.
    pub(crate) fn configured_mcp_policy(
        &self,
    ) -> Option<Arc<dyn meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority>> {
        self.configured_policy
            .lock()
            .ok()?
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
    }

    /// Test support: the latest composition's own grant owner, restored on
    /// this composition's publication, while the realm's machine still holds
    /// it, so a test can change the realm's policy live (a grant revoked
    /// while the realm is open). Compiled for test builds only: this crate's
    /// test support, or the MCP fixture feature the RPC test battery enables.
    #[cfg(any(test, feature = "test-support", feature = "test-mcp-oauth-fixtures"))]
    #[doc(hidden)]
    pub fn composed_grant_owner_for_test(&self) -> Option<Arc<LocalGrantAuthority>> {
        self.composed_grants()
    }

    /// The latest composition's grant owner, while the realm's machine
    /// still holds it.
    fn composed_grants(&self) -> Option<Arc<LocalGrantAuthority>> {
        self.composed_grants
            .lock()
            .ok()?
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
    }

    /// Bind this process's agent to the session's current registration, as
    /// the trusted host, before the session's agent is built: its executor;
    /// every MCP server the agent's selection names, each with exactly the
    /// lineage selected for it, whether or not the session configures it (so
    /// a live add of a selected server can be decided); and for an older
    /// selection's single worker grant, only the session's configured MCP
    /// `servers`. A selection that names no server binds nothing: the
    /// agent's catalog grant is base work, which its prompts' work basis
    /// selects ([`issuance::AgentGrantRefs::prompt_selection`]), not the
    /// registration. Until bound (or without a selection) every MCP server of
    /// the session is unavailable, and a server with no selected grant stays
    /// unavailable. Each selection is resolved for its own server at every
    /// decision, never combined with another's. The selection is the one
    /// read when the realm opened.
    pub async fn bind_session_agent<'a>(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        servers: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), StockGovernanceError> {
        let Some(refs) = self.agent_refs.get() else {
            return Ok(());
        };
        // Every explicitly selected server, configured or not, gets exactly
        // its own lineage; an older single worker grant applies only to the
        // session's configured servers. A server with no selection is absent
        // (and is refused).
        let mut selected: std::collections::BTreeMap<String, Vec<GrantLineageRef>> = refs
            .mcp
            .iter()
            .map(|(server, lineage)| (server.clone(), vec![lineage.clone()]))
            .collect();
        for server in servers {
            if !selected.contains_key(server)
                && let Some(lineage) = refs.server_lineage(server)
            {
                selected.insert(server.to_owned(), vec![lineage.clone()]);
            }
        }
        let servers = selected;
        if servers.is_empty() {
            return Ok(());
        }
        let registration = machine
            .current_session_registration_witness(session_id)
            .await
            .ok_or_else(|| {
                StockGovernanceError::Binding("the session has no current registration".into())
            })?;
        let mandate = meerkat_runtime::meerkat_machine::SessionAgentMandate::new(
            self.principals.executor.clone(),
            servers,
            None,
        )
        .map_err(|error| StockGovernanceError::Binding(error.to_string()))?;
        machine
            .bind_session_agent_mandate(&registration, mandate)
            .await
            .map_err(|error| StockGovernanceError::Binding(error.to_string()))
    }

    /// This composition's one authorization publication: the realm's grant
    /// owner is restored on it, and the restored owner and the originals
    /// ledger publish on it. A trusted host whose caller-access facts can
    /// change ([`StockCallerAccess`]) joins their owner to it once, at
    /// quiescent startup, before it exposes any writer of those facts, so
    /// that a change of them invalidates the decisions observed before it.
    #[must_use]
    pub fn authorization_publication(&self) -> LocalAuthorizationPublication {
        self.publication.clone()
    }

    /// Configure the realm owner's review rules, read from the realm's
    /// effective configuration when it opens, before any composition. The
    /// rules hold for this composition's lifetime: a configuration changed
    /// by the trusted host takes effect when the realm opens again.
    ///
    /// # Errors
    /// [`StockGovernanceError::Review`] when other rules are already
    /// configured.
    pub fn configure_review(
        &self,
        rules: review::StockReviewRules,
    ) -> Result<(), StockGovernanceError> {
        match self.review_rules.set(rules) {
            Ok(()) => Ok(()),
            Err(rules) if self.review_rules.get() == Some(&rules) => Ok(()),
            Err(_) => Err(StockGovernanceError::Review(
                "other review rules are already configured".to_string(),
            )),
        }
    }

    /// Apply `change`, a trusted host's change of facts it joined to this
    /// composition's publication (its callers' access, or the logins of its
    /// consent surface), inside an owner change on that publication, so
    /// that every decision and eligibility observed before the change stops
    /// being current. A publication that cannot change is already
    /// unavailable to every observation; `change` is applied either way.
    pub fn apply_host_change<T>(&self, change: impl FnOnce() -> T) -> T {
        let owner_change = self.publication.begin_owner_change();
        let applied = change();
        drop(owner_change);
        applied
    }

    /// The posture in which this composition admits requesters.
    #[must_use]
    pub fn caller_posture(&self) -> &StockCallerPosture {
        &self.posture
    }

    /// For an operator deciding what to grant: the trusted report of
    /// `controller`'s (the session's pinned controller) actual lowered route
    /// ([`controller::controller_route_report`]) and the command that
    /// grants this agent that route. `None` when that client reports no
    /// coherent facts for its selection. Naming the command grants nothing.
    pub fn controller_route_grant_hint(
        &self,
        controller: &ControllerModelClient,
    ) -> Option<String> {
        let report = controller::controller_route_report(controller)?;
        Some(format!(
            "the session's controller route is {report}. A controller grant naming this route is issued with: rkat realm grant {} --agent {} --controller --route {}",
            self.realm.as_str(),
            self.agent,
            report.key
        ))
    }

    pub fn principals(&self) -> &StockPrincipals {
        &self.principals
    }

    /// The realm this composition governs.
    pub fn realm(&self) -> &RealmId {
        &self.realm
    }

    /// Whether a realm machine's composition was already built from this
    /// governance ([`Self::composition`]); its agent's selection is then
    /// the one read for that machine and is kept.
    pub(crate) fn is_composed(&self) -> bool {
        self.composed_grants
            .lock()
            .map_or(true, |composed| composed.is_some())
    }

    /// Whether the open realm recorded the operator's grant selection; its
    /// governed prompts are refused locally until it does.
    pub fn has_grant_selection(&self) -> bool {
        self.prompt_selection().is_some()
    }

    /// Observe this governance's own agent's selected grants in `slots` as
    /// the open realm retained them: the selection its composition read when
    /// the realm opened, which its prompts use, resolved through that
    /// composition's own grant owner for the executor it uses, its
    /// principals' executor (see
    /// [`issuance::StockGrantObservationCoverage::RetainedSelection`]).
    /// Nothing is reopened, restored or read from disk, and the realm stays
    /// served.
    ///
    /// # Errors
    /// [`StockGovernanceError::GrantOwner`] when no open composition of this
    /// governance holds its grant owner: it never opened, or it closed.
    pub fn observe_retained_grants(
        &self,
        slots: &[issuance::StockGrantSlot],
    ) -> Result<issuance::StockGrantObservation, StockGovernanceError> {
        let owner = self.composed_grants().ok_or_else(|| {
            StockGovernanceError::GrantOwner(
                "no open composition of this realm holds it".to_string(),
            )
        })?;
        // The executor the composition uses is its principals' one, which a
        // caller of `StockGovernance::new` supplies: never re-derived here.
        let subject = issuance::ObservedAgent::live(
            self.realm.clone(),
            &self.agent,
            self.principals.executor.clone(),
        );
        Ok(issuance::observe_selection(
            &owner,
            subject,
            self.agent_refs.get(),
            slots,
            issuance::StockGrantObservationCoverage::RetainedSelection,
        ))
    }

    /// The agent's selection as the realm's composition read it.
    #[cfg(test)]
    pub(crate) fn composed_agent_refs(&self) -> Option<&issuance::AgentGrantRefs> {
        self.agent_refs.get()
    }

    fn prompt_selection(&self) -> Option<(WorkAuthorityBasis, GrantLineageRef)> {
        self.agent_refs.get()?.prompt_selection()
    }

    pub(crate) fn originals(&self) -> Arc<dyn AuthenticatedOriginalSource> {
        Arc::clone(&self.ledger) as Arc<dyn AuthenticatedOriginalSource>
    }

    /// The realm machine's composition over the restored grant owner, or why
    /// it is unavailable. Called once, while the realm opens.
    pub(crate) async fn composition(
        &self,
        grants: Arc<LocalGrantAuthority>,
        installations: Arc<dyn meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority>,
        paths: &RealmPaths,
    ) -> Result<meerkat_runtime::meerkat_machine::ScopedGrantComposition, StockGovernanceError>
    {
        // Without a recorded selection the composition still opens; its
        // prompts are refused locally until the operator records one.
        if let Some(refs) = read_stock_grant_file(paths)
            .await?
            .and_then(|mut file| file.agents.remove(&self.agent))
        {
            // A second open of the same composition keeps the first one.
            let _ = self.agent_refs.set(refs);
        }
        // The realm owner's review rules, read when the realm opened; a
        // composition without them would serve fewer, so it is refused.
        let review = self.review_rules.get().cloned().ok_or_else(|| {
            StockGovernanceError::Review("the realm's configuration was not read".to_string())
        })?;
        // One policy object: both invocation faces and the operation owner,
        // in this composition's requester posture.
        let policy = policy::StockInvocationPolicy::with_review_rules(
            Arc::clone(&grants),
            &self.principals,
            self.realm.clone(),
            installations,
            self.posture.clone(),
            review,
        );
        // The same policy decides configured MCP attachment for the
        // per-agent composition of this open.
        let configured: Arc<dyn meerkat_core::mcp_attachment::ConfiguredMcpAttachmentAuthority> =
            Arc::clone(&policy) as _;
        if let Ok(mut slot) = self.configured_policy.lock() {
            *slot = Some(Arc::downgrade(&configured));
        }
        // A caller's binding commits within this owner's exclusion of owner
        // changes on the composition's publication.
        if let Ok(mut slot) = self.composed_grants.lock() {
            *slot = Some(Arc::downgrade(&grants));
        }
        let retained_resume = self.retained_resume_check(
            Arc::clone(&grants),
            Arc::clone(&policy) as Arc<dyn RetainedInvocationOwner>,
        );
        Ok(meerkat_runtime::meerkat_machine::ScopedGrantComposition {
            grants,
            ingress_domain: self.principals.ingress_domain(),
            originals: self.originals(),
            invocation: ScopedInvocation::from_policy(Arc::clone(&policy)),
            operation_owner: policy,
            retained_resume: Some(retained_resume),
        })
    }

    /// The composition's retained-resume check: the scoped original
    /// composition's own check ([`ScopedRetainedResume`]) over the owners
    /// that admitted the work, the restored grant owner `grants`, this
    /// composition's ingress domain and originals ledger and the stock
    /// requester rule `invocation`, with, as its controller owner, the pin of
    /// the resumed session's own newest governed binding.
    fn retained_resume_check(
        &self,
        grants: Arc<LocalGrantAuthority>,
        invocation: Arc<dyn RetainedInvocationOwner>,
    ) -> Arc<NativeRetainedResumeCheck> {
        let ingress_domain = self.principals.ingress_domain();
        let originals = self.originals();
        let pins = Arc::clone(&self.resume_pins);
        Arc::new(
            move |runtime: &LogicalRuntimeId,
                  input: &Input,
                  evidence: &meerkat_runtime::retained_work::RetainedResumeEvidence| {
                ScopedRetainedResume::new(
                    Arc::clone(&grants),
                    ingress_domain.clone(),
                    Arc::clone(&originals),
                    Arc::clone(&invocation),
                    pins.controller_owner(runtime),
                )
                .check(runtime, input, evidence)
            },
        )
    }

    /// Bind one prompt of the local operator (the CLI's, or a governed JSONL
    /// connection's, [`Self::bind_jsonl_input`]) for `session_id` to an
    /// authenticated original on the realm's machine `machine`, with the
    /// session's actual pinned controller `controller`. The original names
    /// the operator as its requester, authenticated by this process, under
    /// the prompt labels: occurrence `prompt-<event>`, root event
    /// `prompt-events` and idempotency key `rkat-prompt-<event>`. The
    /// session is first bound to its canonical destination. The returned
    /// input is submitted through the ordinary runtime accept, and the
    /// returned candidate original is then admitted
    /// ([`PendingOriginal::admit`]) once the runtime accepted that exact
    /// input; dropping it withdraws the candidate. Each call is a new
    /// occurrence: an uncertain delivery is retried by resubmitting the exact
    /// input returned here, never by binding again.
    ///
    /// Every refusal here is local and typed, before anything is written.
    /// Any input but a prompt is refused
    /// ([`StockGovernanceError::RequesterUnsupported`]): the operator is the
    /// requester only of a prompt, and an administrative outcome the
    /// operator's own process reports binds with [`Self::bind_host_outcome`].
    /// A host-authenticated composition refuses every input here: its
    /// operator is no requester.
    pub async fn bind_prompt(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        input: Input,
        controller: ControllerModelClient,
    ) -> Result<(Input, PendingOriginal), StockGovernanceError> {
        if !matches!(self.posture, StockCallerPosture::LocalOperator) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "a host-authenticated composition binds each input with its authenticated caller"
                    .to_string(),
            ));
        }
        if !matches!(input, Input::Prompt(_)) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "the local operator is the requester only of a prompt; an administrative \
                 outcome its process reports binds with bind_host_outcome"
                    .to_string(),
            ));
        }
        self.bind_original(machine, session_id, input, controller, Requesting::Operator)
            .await
    }

    /// Bind one external event through which the local operator's own
    /// process reports the outcome of an administrative command, for
    /// `session_id`, to an authenticated original on the realm's machine
    /// `machine`, with the session's actual pinned controller `controller`.
    ///
    /// The original's requester is this composition's stock ingress itself
    /// ([`StockPrincipals::ingress_actor`], a runtime host), never the
    /// operator: no person requested this work, and it is not the operator's
    /// consent. It runs under the operator's authority, the same
    /// grant-lineage mandate as an operator prompt, at the operator's
    /// canonical session destination and with no represented subject, so it
    /// widens nothing. The original is authenticated by this process's own
    /// observation (evidence namespace `host-outcome-authentication`) under
    /// the host labels: occurrence `host-outcome-<event>`, root event
    /// `host-outcome-events` and idempotency key
    /// `rkat-host-outcome-<event>`, in the composition's `cli-prompts`
    /// ingress namespace. The review and consent of its operations state
    /// that requester, and its operations are reviewed under the operator's
    /// review tiers, the same as an operator prompt's. A turn's identity
    /// includes its requester, so an outcome never shares a turn with a
    /// queued operator prompt: each outcome runs its own turn.
    ///
    /// The event carries only the outcome that process retained, nothing
    /// from another source. The stock rules cannot verify that the command
    /// ran: the event's content is attributed to the ingress, never
    /// asserted. Its origin must be [`InputOrigin::External`] and its
    /// durability [`InputDurability::Durable`]. Any other origin is refused
    /// ([`StockGovernanceError::RequesterUnsupported`]), the runtime's own
    /// system origin included, and so is any other durability: the runtime
    /// refuses a derived external event only at its accept, after this
    /// binding's writes, and an ephemeral one may be lost while the outcome
    /// it reports stands. A host-authenticated composition is refused too:
    /// it admits its ingress as no requester. Every refusal here is local
    /// and typed, before anything is written.
    ///
    /// As for a prompt, the returned input is submitted through the ordinary
    /// runtime accept and the returned candidate original then admitted
    /// ([`PendingOriginal::admit`]) once the runtime accepted that exact
    /// input; dropping it withdraws the candidate. Each call is a new
    /// occurrence: an uncertain delivery is retried by resubmitting the exact
    /// input returned here, never by binding again.
    ///
    /// For example, a host whose setup flow finalizes a reviewed connector
    /// installation ([`finalize::ReviewedConnectorFinalization`]) and then
    /// tells the session's agent the outcome builds a durable
    /// [`ExternalEventInput`] from that retained outcome, of origin
    /// `InputOrigin::External { source_name: "connector-finalization" }`,
    /// and binds it here, while the operator's own prompts in the same
    /// session keep binding with [`Self::bind_prompt`].
    pub async fn bind_host_outcome(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        event: ExternalEventInput,
        controller: ControllerModelClient,
    ) -> Result<(Input, PendingOriginal), StockGovernanceError> {
        if !matches!(self.posture, StockCallerPosture::LocalOperator) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "a host-authenticated composition admits its ingress as no requester".to_string(),
            ));
        }
        // Exactly an external event source: every other origin names another
        // requester, and the runtime's own system origin may be derived work.
        if !matches!(event.header.source, InputOrigin::External { .. }) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "an administrative outcome binds only as an event of external origin".to_string(),
            ));
        }
        // Exactly a durable event: the runtime refuses a derived external
        // event only at its accept, after this binding's writes, and an
        // ephemeral one may be lost while the outcome it reports stands.
        if !matches!(event.header.durability, InputDurability::Durable) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "an administrative outcome binds only as a durable event".to_string(),
            ));
        }
        self.bind_original(
            machine,
            session_id,
            Input::ExternalEvent(event),
            controller,
            Requesting::Ingress,
        )
        .await
    }

    /// Bind one input of `caller`, a caller the trusted host authenticated,
    /// for `session_id`, as [`Self::bind_prompt`] binds an operator prompt:
    /// the session (owned by the operator) is bound to its canonical
    /// destination and the original, authenticated as the host recorded, is
    /// the caller's. The stock requester rule (never the operator or the
    /// stock ingress, otherwise the host's exact-target decision) is asked
    /// before the session's binding changes, and the binding commits only
    /// while that decision is still current, held current through the commit
    /// (`Self::bind_caller_session`); native admission and every operation
    /// ask again. A refused caller
    /// ([`StockGovernanceError::CallerNotAdmitted`]: Denied for the operator
    /// or the stock ingress named as a caller, otherwise the host's Denied
    /// verdict or unavailability) leaves the session's binding, and the
    /// disclosure of its earlier results, unchanged. The local-operator
    /// composition refuses it ([`StockGovernanceError::RequesterUnsupported`]):
    /// it admits no caller.
    pub async fn bind_caller_prompt(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        input: Input,
        controller: ControllerModelClient,
        caller: &StockAuthenticatedCaller,
    ) -> Result<(Input, PendingOriginal), StockGovernanceError> {
        if !matches!(self.posture, StockCallerPosture::HostAuthenticated(_)) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "the local-operator composition admits only its operator".to_string(),
            ));
        }
        self.bind_original(
            machine,
            session_id,
            input,
            controller,
            Requesting::Caller(caller),
        )
        .await
    }

    /// Bind one input of the `requesting` requester for `session_id`; see
    /// [`Self::bind_prompt`].
    async fn bind_original(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        input: Input,
        controller: ControllerModelClient,
        requesting: Requesting<'_>,
    ) -> Result<(Input, PendingOriginal), StockGovernanceError> {
        let (authority_basis, controller_grant) = self
            .prompt_selection()
            .ok_or(StockGovernanceError::GrantsUnavailable)?;
        let (destination, registration) = match requesting {
            Requesting::Operator | Requesting::Ingress => {
                destination::bind_stock_session_on_registration(
                    machine,
                    &self.principals.operator,
                    session_id,
                    |commit| {
                        commit();
                        Ok(())
                    },
                )
                .await
                .map_err(|error| StockGovernanceError::Destination(error.to_string()))?
            }
            Requesting::Caller(caller) => {
                self.bind_caller_session(machine, session_id, caller)
                    .await?
            }
        };
        let claims = StockOriginalClaims::new(
            &self.principals,
            &self.realm,
            &requesting,
            destination.target(),
            authority_basis.clone(),
            controller_grant,
            controller.selection().clone(),
            uuid::Uuid::now_v7(),
        )?;
        let pending = self
            .ledger
            .record_candidate(
                &claims.namespace,
                &claims.requester,
                AuthenticatedOriginal {
                    original_work: claims.original.clone(),
                    occurrence_scope: claims.namespace.occurrence_scope.clone(),
                    original_authentication: claims.authentication.clone(),
                    root_event: claims.root_event.clone(),
                    contributing_work: Vec::new(),
                    target: destination.target().clone(),
                    authority_basis,
                },
            )
            .map_err(|error| StockGovernanceError::Binding(error.to_string()))?;
        // The session's pin for a retained resume of its work: the client
        // this binding was made with, replacing the pin of an older binding
        // (a lower destination generation) only.
        let pinned = controller.clone();
        let generation = destination.target().context_generation;
        let input = bind_scoped_original(
            input,
            claims.association,
            IdempotencyKey::new(claims.key),
            claims.requester,
            self.principals.ingress_actor.clone(),
            self.realm.clone(),
            claims.authentication,
            controller,
        )
        .map_err(|error| StockGovernanceError::Binding(error.to_string()))?;
        self.resume_pins.record(registration, generation, pinned);
        Ok((input, pending))
    }

    /// Bind `session_id` to its canonical destination for an input of
    /// `caller`, only while the stock requester rule admits that exact caller
    /// there ([`policy::admit_stock_requester`]: never the operator or the
    /// stock ingress, otherwise the trusted host's decision for the caller,
    /// the realm's executor, the session's canonical owner and the session).
    /// The rule is observed under this composition's publication (to which
    /// the host joins its access facts) and a refusal returns typed before
    /// any binding. The binding then commits only within that observation: at
    /// the commit, with the session's machine authority held
    /// ([`destination::bind_stock_session_within`]), the observation must
    /// still be current, and it stays current through the commit because
    /// every owner change on the publication is excluded until the commit
    /// returns ([`LocalGrantAuthority::try_with_owner_changes_excluded`]). An
    /// observation the publication moved past, or an exclusion an owner
    /// change held, is taken again, a bounded number of times. Returns the
    /// destination with the exact registration it was bound on.
    async fn bind_caller_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        caller: &StockAuthenticatedCaller,
    ) -> Result<
        (
            destination::StockSessionDestination,
            meerkat_runtime::RuntimeSessionRegistrationWitness,
        ),
        StockGovernanceError,
    > {
        if !matches!(self.posture, StockCallerPosture::HostAuthenticated(_)) {
            return Err(StockGovernanceError::RequesterUnsupported(
                "the local-operator composition admits only its operator".to_string(),
            ));
        }
        let unavailable =
            || StockGovernanceError::CallerNotAdmitted(OperationAuthorizationError::Unavailable);
        for _ in 0..CALLER_BIND_ATTEMPTS {
            let observed = self.publication.observe(|| {
                policy::admit_stock_requester(
                    &self.posture,
                    &self.principals.operator,
                    &self.principals.ingress_actor,
                    &self.principals.executor,
                    caller.requester(),
                    Some(caller.authentication()),
                    session_id,
                )
            });
            // The binding commits only while the caller's exact login is
            // current: the observation's publication has not moved and the
            // login's own deadline has not passed, even before its host
            // retires it.
            let (stamp, login) = match observed {
                Ok((Ok((_, login)), stamp)) => (stamp, login),
                Ok((Err(refusal), _)) => {
                    return Err(StockGovernanceError::CallerNotAdmitted(refusal));
                }
                Err(PublicationError::Changed) => continue,
                Err(PublicationError::Unavailable) => return Err(unavailable()),
            };
            // The realm's grant owner, restored on this publication; without
            // it nothing is admitted.
            let grants = self.composed_grants().ok_or_else(unavailable)?;
            // Why the commit was refused: 1 moved or held, 2 unavailable,
            // 3 the caller's login past its deadline.
            let refused = AtomicU8::new(0);
            let bound = destination::bind_stock_session_on_registration(
                machine,
                &self.principals.operator,
                session_id,
                |commit| {
                    let held = grants.try_with_owner_changes_excluded(|| {
                        stamp.check_current().map(|()| {
                            if login.is_current_now() {
                                commit();
                                true
                            } else {
                                false
                            }
                        })
                    });
                    let reason = match held {
                        Ok(Ok(true)) => return Ok(()),
                        Ok(Ok(false)) => 3,
                        Ok(Err(PublicationError::Unavailable)) => 2,
                        // A publication since the observation, or an owner
                        // change holding the publication: observe again (a
                        // retired publication is then unavailable).
                        Ok(Err(PublicationError::Changed)) | Err(_) => 1,
                    };
                    refused.store(reason, Ordering::Relaxed);
                    Err(RuntimeDriverError::ValidationFailed {
                        reason: "the caller's admission is no longer current".to_string(),
                    })
                },
            )
            .await;
            match (bound, refused.load(Ordering::Relaxed)) {
                (Ok(bound), _) => return Ok(bound),
                (Err(_), 1) => continue,
                (Err(_), 2) => return Err(unavailable()),
                (Err(_), 3) => {
                    return Err(StockGovernanceError::CallerNotAdmitted(
                        OperationRefused::new(OperationRefusalKind::Denied).into(),
                    ));
                }
                (Err(error), _) => {
                    return Err(StockGovernanceError::Destination(error.to_string()));
                }
            }
        }
        Err(StockGovernanceError::CallerNotAdmitted(
            OperationRefused::new(OperationRefusalKind::ReprepareRequired).into(),
        ))
    }

    fn evidence(
        &self,
        namespace: &str,
        resource_id: String,
        event: uuid::Uuid,
    ) -> Result<HistoricalEvidenceRef, StockGovernanceError> {
        process_evidence(
            &self.principals.ingress_actor,
            namespace,
            resource_id,
            event,
        )
    }
}

/// This process's observation `event` in the evidence `namespace` of its
/// stock ingress `ingress_actor`.
fn process_evidence(
    ingress_actor: &PrincipalRef,
    namespace: &str,
    resource_id: String,
    event: uuid::Uuid,
) -> Result<HistoricalEvidenceRef, StockGovernanceError> {
    let mut digest = [0_u8; 32];
    digest[..16].copy_from_slice(event.as_bytes());
    digest[16..].copy_from_slice(process_uuid().as_bytes());
    Ok(HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: ingress_actor.clone(),
                namespace: namespace.into(),
            },
            resource_id,
        },
        revision: EvidenceId::new(format!("process-{}", process_uuid()))
            .map_err(|error| StockGovernanceError::Binding(error.to_string()))?,
        // An identity of this observation, not a content digest: the
        // final input's replay digest binds its content.
        digest: EvidenceDigest::from_array(digest),
    })
}

/// The claims of one original the stock composition binds: its requester
/// and how it was authenticated, its occurrence and root event, the key of
/// its event and the association its input carries. The one builder of
/// [`StockGovernance::bind_original`], so what is read from these claims
/// is exactly what a binding records.
struct StockOriginalClaims {
    requester: PrincipalRef,
    authentication: HistoricalEvidenceRef,
    namespace: QualifiedIngressNamespace,
    original: OriginalWorkRef,
    root_event: HistoricalEvidenceRef,
    /// The event's idempotency key.
    key: String,
    association: InputAuthorityAssociation,
}

impl StockOriginalClaims {
    /// The claims of the original of `requesting` for event `event`, at
    /// the session destination `target` of the stock composition of
    /// `principals` in `realm`, under `authority_basis`, with the
    /// controller grant `controller_grant` and the actual controller
    /// selection `controller`.
    #[allow(clippy::too_many_arguments)]
    fn new(
        principals: &StockPrincipals,
        realm: &RealmId,
        requesting: &Requesting<'_>,
        target: &NativeWorkTarget,
        authority_basis: WorkAuthorityBasis,
        controller_grant: GrantLineageRef,
        controller: ControllerModelSelection,
        event: uuid::Uuid,
    ) -> Result<Self, StockGovernanceError> {
        let labels = requesting.labels();
        let occurrence = EvidenceId::new(format!("{}-{event}", labels.occurrence))
            .map_err(|error| StockGovernanceError::Binding(error.to_string()))?;
        let namespace = QualifiedIngressNamespace {
            realm: realm.clone(),
            ingress: principals.ingress_domain(),
            occurrence_scope: occurrence.clone(),
        };
        let (requester, authentication) = match requesting {
            Requesting::Operator => (
                principals.operator.clone(),
                process_evidence(
                    &principals.ingress_actor,
                    "operator-authentication",
                    process_id(),
                    event,
                )?,
            ),
            // Nothing about the operator is authenticated here: the process
            // observes itself, its own ingress.
            Requesting::Ingress => (
                principals.ingress_actor.clone(),
                process_evidence(
                    &principals.ingress_actor,
                    "host-outcome-authentication",
                    process_id(),
                    event,
                )?,
            ),
            Requesting::Caller(caller) => {
                (caller.requester().clone(), caller.authentication().clone())
            }
        };
        let root_event = process_evidence(
            &principals.ingress_actor,
            labels.root_events,
            event.to_string(),
            event,
        )?;
        let original = OriginalWorkRef {
            authority: principals.ingress_actor.clone(),
            work: occurrence,
        };
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: requester.clone(),
            ingress_actor: principals.ingress_actor.clone(),
            represented_subject: None,
            original_authentication: authentication.clone(),
            logical_executor: principals.executor.clone(),
            target: target.clone(),
            original_work: original.clone(),
            root_event: root_event.clone(),
            contributing_work: Vec::new(),
            authority_basis,
            controller_grant_lineage: vec![controller_grant],
            controller_model: Some(controller),
            // The grants and current policy narrow; the claims do not widen.
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: namespace.clone(),
            contract: ContractRequirements::local_governed_v1(),
        })
        .map_err(|error| StockGovernanceError::Binding(error.to_string()))?;
        Ok(Self {
            requester,
            authentication,
            namespace,
            original,
            root_event,
            key: format!("{}-{event}", labels.key),
            association,
        })
    }
}

/// A stock-governed host fires schedules for, and binds onto each session it
/// materializes or revives for a fire, the agent the realm was opened for
/// (its executor), exactly as [`StockGovernance::bind_session_agent`] binds
/// an interactive session.
#[async_trait::async_trait]
impl crate::surface::ScheduledSessionAgentBinding for StockGovernance {
    fn agent(&self) -> &PrincipalRef {
        &self.principals.executor
    }

    async fn bind_scheduled_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        configured_servers: &[meerkat_core::McpServerConfig],
    ) -> Result<(), meerkat_schedule::ScheduleDomainError> {
        self.bind_session_agent(
            machine,
            session_id,
            configured_servers.iter().map(|server| server.name.as_str()),
        )
        .await
        .map_err(|error| {
            meerkat_schedule::ScheduleDomainError::Internal(format!(
                "governed scheduled session agent: {error}"
            ))
        })
    }
}

/// Whose input [`StockGovernance::bind_original`] binds.
enum Requesting<'a> {
    /// The local operator, authenticated by this process.
    Operator,
    /// The composition's stock ingress itself, reporting the outcome of an
    /// administrative command its process ran.
    Ingress,
    /// A caller the trusted host authenticated.
    Caller(&'a StockAuthenticatedCaller),
}

/// The labels of one requester's originals.
struct OriginalLabels {
    /// The prefix of the occurrence.
    occurrence: &'static str,
    /// The namespace of the root event.
    root_events: &'static str,
    /// The prefix of the event's idempotency key.
    key: &'static str,
}

impl Requesting<'_> {
    /// The labels this requester's originals carry: the prompt labels for
    /// a person's input, the host-outcome labels for the ingress's own.
    fn labels(&self) -> OriginalLabels {
        match self {
            Self::Operator | Self::Caller(_) => OriginalLabels {
                occurrence: "prompt",
                root_events: "prompt-events",
                key: "rkat-prompt",
            },
            Self::Ingress => OriginalLabels {
                occurrence: "host-outcome",
                root_events: "host-outcome-events",
                key: "rkat-host-outcome",
            },
        }
    }
}

/// This process's one authentication observation of its operator.
fn process_uuid() -> uuid::Uuid {
    static PROCESS: OnceLock<uuid::Uuid> = OnceLock::new();
    *PROCESS.get_or_init(uuid::Uuid::now_v7)
}

fn process_id() -> String {
    format!("process-{}", process_uuid())
}

/// Select governance for an existing realm and create its grant owner, as
/// the local operator. No grant is issued: the operator issues the grants
/// that select an executor's scope deliberately, and records their selection
/// ([`record_stock_grant_selection`]); until then every governed prompt is
/// refused locally as unavailable.
///
/// Order: the realm's exclusive governed execution custody is taken first
/// (a live owner of the realm refuses as busy, and nothing changes). Only
/// while holding it is the manifest flipped to governed, so no ordinary
/// owner runs across the flip and every later open is governed even if a
/// later step fails. Then the grant owner is created only when the realm was
/// not governed before and has none. A realm that is already governed keeps
/// its owner: an existing one must restore, and a missing one is refused
/// rather than recreated, since a new owner would replace its lineage. This
/// is not a transaction: a failure after the flip leaves a governed realm
/// without owners, which refuses, and is reported here.
pub async fn govern_stock_realm(
    realms_root: &Path,
    realm_id: &str,
    principals: &StockPrincipals,
) -> Result<(), StockGovernanceError> {
    let clock: Arc<dyn LocalAuthorizationClock> =
        Arc::new(meerkat_authorization::clock::HostAuthorizationClock);
    // Only an existing realm is governed: opening it below would create it.
    if meerkat_store::read_realm_posture_in(realms_root, realm_id).await?
        == meerkat_store::RealmPosture::NotMaterialized
    {
        return Err(meerkat_store::StoreError::RealmNotMaterialized {
            realm_id: realm_id.to_string(),
        }
        .into());
    }
    // Exclusive governed custody before any posture change.
    let (pin, bundle) = super::open_governed_realm_persistence_with_provider(
        &crate::DiskStorageProvider,
        realms_root,
        realm_id,
        None,
        None,
        None,
        super::RealmGovernance::GovernedWithoutOwners,
    )
    .await?;
    // A memory realm's execution custody belongs to each opened store, not
    // to the realm: another open would not be excluded, so its posture is
    // never changed here.
    if pin
        .as_builtin()
        .is_some_and(|manifest| manifest.backend.as_str() == "memory")
    {
        return Err(StockGovernanceError::Unsupported(
            "a memory realm has no realm-wide execution custody to govern under".to_string(),
        ));
    }
    let was_governed = meerkat_store::read_realm_posture_in(realms_root, realm_id).await?
        == meerkat_store::RealmPosture::Governed;
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    let owner_path = meerkat_store::grant_owner_store_path(&paths.root);
    let owner_exists = tokio::fs::try_exists(&owner_path).await?;
    if !was_governed {
        // A grant owner this command did not create is refused before the
        // posture changes.
        if owner_exists {
            return Err(StockGovernanceError::ForeignGrantOwner);
        }
        meerkat_store::govern_realm_in(realms_root, realm_id).await?;
    }
    let store = Arc::new(FileGrantOwnerStore::new(owner_path.clone()));
    if owner_exists {
        LocalGrantAuthority::restore(store, LocalAuthorizationPublication::new(), clock)
            .map_err(|error| StockGovernanceError::GrantOwner(error.to_string()))?;
        drop(bundle);
        return Ok(());
    }
    if was_governed {
        return Err(StockGovernanceError::GrantOwner(
            "the governed realm's grant owner is missing; it is not recreated".to_string(),
        ));
    }
    if let Some(parent) = owner_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let owner = LocalGrantAuthority::new_durable(
        LocalGrantConfiguration {
            root: principals.operator.clone(),
            namespace: EvidenceId::new(STOCK_GRANT_NAMESPACE)
                .map_err(|error| StockGovernanceError::GrantOwner(error.to_string()))?,
            generation: 1,
        },
        store,
        LocalAuthorizationPublication::new(),
        clock,
    )
    .map_err(|error: GrantDurabilityError| StockGovernanceError::GrantOwner(error.to_string()))?;
    drop(owner);
    drop(bundle);
    Ok(())
}

/// Record the grants the operator issued and selected for the realm's
/// stock executor: trusted host configuration naming references in the
/// realm's grant owner, never the grants themselves. Admission resolves them
/// through the restored owner; a missing, revoked or mismatched reference
/// refuses. Written atomically; a later open reads it.
pub async fn record_stock_grant_selection(
    realms_root: &Path,
    realm_id: &str,
    selection: &StockGrantSelection,
) -> Result<(), StockGovernanceError> {
    // The same exclusive administrative custody as `rkat realm grant` and
    // `revoke-grant`, held across the read, the settled write and its
    // publication, on an owned task so it is never released while that
    // write can still land, even if the caller stops waiting.
    let (realms_root, realm_id) = (realms_root.to_path_buf(), realm_id.to_string());
    let selection = selection.clone();
    issuance::on_owned_task(async move {
        let bundle = issuance::administrative_custody(&realms_root, &realm_id).await?;
        let recorded = write_stock_grants(
            &meerkat_store::realm_paths_in(&realms_root, &realm_id),
            &selection,
        )
        .await;
        drop(bundle);
        recorded
    })
    .await
}
