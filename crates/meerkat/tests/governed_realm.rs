//! A governed realm opened by a fresh process: the posture the trusted host
//! selects, never the grant file's presence, decides governance; a governed
//! realm restores its grant owner or refuses locally and typed, and admits
//! only the realm's own persisted MCP installations with an explicit
//! operation ceiling.
#![cfg(all(
    feature = "session-store",
    feature = "local-authorization",
    feature = "mcp"
))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use meerkat::DiskStorageProvider;
use meerkat::governed_realm::{
    GovernedRealmBootstrap, RealmGovernance, open_governed_realm_persistence_with_provider,
};
use meerkat_authorization::clock::LocalAuthorizationTime;
use meerkat_authorization::clock::{LocalAuthorizationClock, LocalClockError};
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{
    FileGrantOwnerStore, GrantDurabilityError, LocalGrantAuthority, LocalGrantConfiguration,
};
use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, ResourceDomain};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef, QualifiedIngressNamespace,
};
use meerkat_core::authorization::{ControllerReadinessFailure, PreparedAuthorizationBinding};
use meerkat_core::mcp_attachment::McpAttachmentRefusal;
use meerkat_core::mcp_config::McpServerConfig;
use meerkat_core::{PrincipalKind, PrincipalRef, SessionId, TrustDomainId};
use meerkat_runtime::input_authority::{NativeAdmissionError, NativeIngressContext};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, ScopedInvocation, ScopedMandateOwner,
};
use meerkat_store::{RealmBackend, RealmPaths};

fn grant_owner_path(paths: &RealmPaths) -> std::path::PathBuf {
    meerkat_store::grant_owner_store_path(&paths.root)
}

struct Clock;
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: 100,
            monotonic: meerkat_core::time_compat::Instant::now(),
        })
    }
}

fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("governed-realm").expect("domain"),
    )
    .expect("principal")
}

/// The host's originals source; these tests admit no work.
struct NoOriginals;
impl AuthenticatedOriginalSource for NoOriginals {
    fn authenticated_original(
        &self,
        _: &NativeIngressContext,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        Ok(None)
    }
    fn retained_custody(
        &self,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<meerkat_runtime::meerkat_machine::RetainedCustody, ControllerReadinessFailure> {
        Ok(meerkat_runtime::meerkat_machine::RetainedCustody::NotRetained)
    }
}

/// The host's invocation policy; these tests admit no work.
struct Policy;
impl ScopedMandateOwner for Policy {
    fn authorize_invocation(
        &self,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Err(NativeAdmissionError::Readiness(
            ControllerReadinessFailure::PolicyUnavailable,
        ))
    }
}
impl AdmittedWorkPolicyOwner for Policy {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

struct Operations;
impl OperationPolicyOwner for Operations {
    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

fn bootstrap() -> RealmGovernance {
    RealmGovernance::Governed(Box::new(GovernedRealmBootstrap {
        ingress_domain: ResourceDomain {
            authority: principal("ingress"),
            namespace: "prompts".into(),
        },
        originals: Arc::new(NoOriginals),
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Operations),
        retained_resume: None,
        publication: LocalAuthorizationPublication::new(),
        clock: Arc::new(Clock),
    }))
}

fn realm_paths(root: &Path) -> RealmPaths {
    meerkat_store::realm_paths_in(root, "governed")
}

async fn open(root: &Path, governance: RealmGovernance) -> meerkat::PersistenceBundle {
    open_governed_realm_persistence_with_provider(
        &DiskStorageProvider,
        root,
        "governed",
        Some(RealmBackend::Sqlite),
        None,
        None,
        governance,
    )
    .await
    .expect("the realm opens in every posture")
    .1
}

/// The configured-attachment verdict of a fresh session on the realm's
/// machine, before any discovery or spawn.
async fn configured_attachment(
    bundle: &meerkat::PersistenceBundle,
) -> Option<Result<(), McpAttachmentRefusal>> {
    let server = McpServerConfig::stdio("configured", "true", Vec::new(), HashMap::new());
    attachment_of(bundle, &server).await
}

async fn attachment_of(
    bundle: &meerkat::PersistenceBundle,
    server: &McpServerConfig,
) -> Option<Result<(), McpAttachmentRefusal>> {
    let machine = bundle.runtime_adapter();
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("registration");
    let bindings = machine.prepare_bindings(session).await.expect("bindings");
    bindings
        .configured_mcp_attachment()
        .map(|authority| authority.authorize_configured(server))
}

/// The realm's own config document, as `rkat mcp add --scope realm` writes it.
fn realm_config(paths: &RealmPaths) -> meerkat_core::ConfigRuntime {
    meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    )
}

/// Install `server` in the realm's own config; the new generation.
async fn install(paths: &RealmPaths, server: McpServerConfig) -> u64 {
    let runtime = realm_config(paths);
    meerkat_core::McpRealmPersistTarget::new(&runtime)
        .add(server, None)
        .await
        .expect("realm installation")
        .generation
}

async fn uninstall(paths: &RealmPaths, name: &str) {
    let runtime = realm_config(paths);
    meerkat_core::McpRealmPersistTarget::new(&runtime)
        .remove(name, None)
        .await
        .expect("realm removal");
}

/// The fixture server installed with a ceiling of `echo` only (a selected
/// read example; a ceiling never certifies an operation read-only).
fn read_only_fixture() -> McpServerConfig {
    let mut server = McpServerConfig::stdio(
        "reader",
        mcp_test_server::fixture_binary()
            .to_string_lossy()
            .to_string(),
        Vec::new(),
        HashMap::new(),
    );
    server.access = Some(meerkat_core::McpServerAccess::allowing(["echo"]));
    server
}

/// A realm with a provisioned grant owner and the ceilinged fixture
/// installed, plus an unceilinged installation; closed again.
async fn governed_realm_with_installations(root: &Path) -> RealmPaths {
    let paths = realm_paths(root);
    drop(open(root, RealmGovernance::Ungoverned).await);
    provision_grant_owner(&paths);
    install(&paths, read_only_fixture()).await;
    install(
        &paths,
        McpServerConfig::stdio("unceilinged", "true", Vec::new(), HashMap::new()),
    )
    .await;
    paths
}

/// Provision the realm's grant owner the way an operator would, then release
/// it so the realm can restore it.
fn provision_grant_owner(paths: &RealmPaths) {
    let path = grant_owner_path(paths);
    std::fs::create_dir_all(path.parent().expect("authorization dir")).expect("dir");
    let owner = LocalGrantAuthority::new_durable(
        LocalGrantConfiguration {
            root: principal("grant-owner"),
            namespace: EvidenceId::new("realm-grants").expect("id"),
            generation: 1,
        },
        Arc::new(FileGrantOwnerStore::new(path)),
        LocalAuthorizationPublication::new(),
        Arc::new(Clock),
    )
    .expect("provisioned owner");
    owner
        .issue_root(
            &principal("grant-owner"),
            EvidenceId::new("leaf").expect("id"),
            principal("executor"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("an issued grant");
}

fn try_restore(paths: &RealmPaths) -> Result<LocalGrantAuthority, GrantDurabilityError> {
    LocalGrantAuthority::restore(
        Arc::new(FileGrantOwnerStore::new(grant_owner_path(paths))),
        LocalAuthorizationPublication::new(),
        Arc::new(Clock),
    )
}

#[tokio::test]
async fn an_ungoverned_realm_installs_no_host() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = open(tmp.path(), RealmGovernance::Ungoverned).await;
    assert!(
        !bundle
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
    assert_eq!(configured_attachment(&bundle).await, None);
}

/// Governance is the host's selection: without host owners, or with owners
/// but no grant owner on disk, the realm still opens governed, every
/// configured MCP server is unavailable before discovery or spawn, and no
/// grant file is created.
#[tokio::test]
async fn a_governed_realm_without_its_owners_refuses_locally() {
    for (label, governance) in [
        ("no host owners", RealmGovernance::GovernedWithoutOwners),
        ("no grant owner on disk", bootstrap()),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = open(tmp.path(), governance).await;
        assert!(
            bundle
                .runtime_adapter()
                .has_native_work_authorization_host(),
            "{label}: governed, never ungoverned"
        );
        assert_eq!(
            configured_attachment(&bundle).await,
            Some(Err(McpAttachmentRefusal::Unavailable)),
            "{label}"
        );
        assert!(
            !grant_owner_path(&realm_paths(tmp.path())).exists(),
            "{label}: no replacement grant owner is created"
        );
        assert!(bundle.manifest().is_some() && bundle.store_path().is_some());
        assert!(
            bundle.event_projection().is_some(),
            "{label}: stores preserved"
        );
    }
}

/// A corrupt grant owner is not repaired, replaced or bypassed: the realm
/// opens governed and refuses locally, and the file is left as it was.
#[tokio::test]
async fn a_corrupt_grant_owner_is_refused_not_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    let path = grant_owner_path(&paths);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not a grant owner\n").unwrap();
    let bundle = open(tmp.path(), bootstrap()).await;
    assert!(
        bundle
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
    assert_eq!(
        configured_attachment(&bundle).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"not a grant owner\n");
}

/// With the host's owners and a provisioned grant owner, the realm's machine
/// is built from the restored owner: it holds the owner's exclusive lease for
/// as long as the realm is open, and releases it when the realm closes.
#[tokio::test]
async fn a_governed_realm_restores_its_grant_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    // Admission first materializes the realm; the owner is then provisioned
    // beside it and released before the governed open.
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    provision_grant_owner(&paths);

    let bundle = open(tmp.path(), bootstrap()).await;
    assert!(
        bundle
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
    assert!(
        try_restore(&paths).is_err(),
        "the open realm's machine holds the restored grant owner"
    );
    assert!(bundle.manifest().is_some() && bundle.event_projection().is_some());
    drop(bundle);
    assert!(
        try_restore(&paths).is_ok(),
        "closing the realm releases the restored owner"
    );
}

/// A governed realm admits exactly its own persisted ceilinged installation:
/// its connection then serves the allowed operation and refuses the other
/// before sending it. A server that is not the realm's installation, an
/// installation without a ceiling and a changed definition are unavailable.
#[tokio::test]
async fn a_governed_realm_admits_only_its_read_only_installation() {
    let tmp = tempfile::tempdir().unwrap();
    governed_realm_with_installations(tmp.path()).await;
    let bundle = open(tmp.path(), bootstrap()).await;
    let installed = read_only_fixture();
    assert_eq!(attachment_of(&bundle, &installed).await, Some(Ok(())));
    assert_eq!(
        configured_attachment(&bundle).await,
        Some(Err(McpAttachmentRefusal::Unavailable)),
        "not one of the realm's installations"
    );
    let unceilinged = McpServerConfig::stdio("unceilinged", "true", Vec::new(), HashMap::new());
    assert_eq!(
        attachment_of(&bundle, &unceilinged).await,
        Some(Err(McpAttachmentRefusal::Unavailable)),
        "an installation without an access ceiling"
    );
    let mut widened = installed.clone();
    widened.access = Some(meerkat_core::McpServerAccess::allowing(["echo", "add"]));
    assert_eq!(
        attachment_of(&bundle, &widened).await,
        Some(Err(McpAttachmentRefusal::Unavailable)),
        "a definition other than the installed one"
    );

    // The admitted definition's connection enforces its ceiling.
    let connection = meerkat_mcp::McpConnection::connect(&installed)
        .await
        .expect("connect the admitted installation");
    let echoed = connection
        .call_tool_text(
            "echo",
            &serde_json::json!({"message": "inside the ceiling"}),
        )
        .await
        .expect("the allowed operation is served");
    assert!(echoed.contains("inside the ceiling"), "{echoed}");
    let refused = connection
        .call_tool("add", &serde_json::json!({"a": 1, "b": 2}))
        .await
        .expect_err("an operation outside the ceiling is refused");
    assert!(
        matches!(
            &refused,
            meerkat_mcp::McpError::OperationNotPermitted { server, operation }
                if server == "reader" && operation == "add"
        ),
        "{refused:?}"
    );
    connection.close().await.expect("close");
}

/// The admission is of the document's generation read at open: once the
/// realm's config moves on, the open realm refuses the installation as not
/// current. A reopened realm (in this process; a restart reopens the same
/// way) reads the
/// current generation and admits it again; once it is removed, a reopened
/// realm finds it unavailable.
#[tokio::test]
async fn an_installation_is_admitted_at_the_current_generation_only() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = governed_realm_with_installations(tmp.path()).await;
    let installed = read_only_fixture();

    let bundle = open(tmp.path(), bootstrap()).await;
    assert_eq!(attachment_of(&bundle, &installed).await, Some(Ok(())));
    install(
        &paths,
        McpServerConfig::stdio("later", "true", Vec::new(), HashMap::new()),
    )
    .await;
    assert_eq!(
        attachment_of(&bundle, &installed).await,
        Some(Err(McpAttachmentRefusal::NotCurrent)),
        "a stale generation is refused"
    );
    drop(bundle);

    let reopened = open(tmp.path(), bootstrap()).await;
    assert_eq!(
        attachment_of(&reopened, &installed).await,
        Some(Ok(())),
        "a fresh open at the current generation admits it again"
    );
    drop(reopened);

    uninstall(&paths, "reader").await;
    let after_removal = open(tmp.path(), bootstrap()).await;
    assert_eq!(
        attachment_of(&after_removal, &installed).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
}

/// The installation is no authority of its own: a governed realm without
/// its owners, or with an unrestorable grant owner, still refuses it.
#[tokio::test]
async fn an_installation_without_restored_owners_stays_unavailable() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = governed_realm_with_installations(tmp.path()).await;
    let installed = read_only_fixture();
    let bundle = open(tmp.path(), RealmGovernance::GovernedWithoutOwners).await;
    assert_eq!(
        attachment_of(&bundle, &installed).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
    drop(bundle);
    std::fs::write(grant_owner_path(&paths), b"not a grant owner\n").unwrap();
    let bundle = open(tmp.path(), bootstrap()).await;
    assert_eq!(
        attachment_of(&bundle, &installed).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
}

/// `rkat realm govern`: the posture is selected durably on the manifest and
/// the grant owner is created once, with no grant issued; a second call
/// changes nothing, and a governed realm whose owner went missing is refused
/// rather than given a new one.
#[tokio::test]
async fn governing_a_realm_selects_the_posture_and_creates_its_owner_once() {
    use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    assert_eq!(
        meerkat_store::read_realm_posture_in(tmp.path(), "governed")
            .await
            .unwrap(),
        meerkat_store::RealmPosture::Ungoverned
    );
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(tmp.path(), "governed", &principals)
        .await
        .expect("governed");
    assert_eq!(
        meerkat_store::read_realm_posture_in(tmp.path(), "governed")
            .await
            .unwrap(),
        meerkat_store::RealmPosture::Governed
    );
    assert!(grant_owner_path(&paths).exists());
    assert!(
        !grant_owner_path(&paths)
            .with_file_name("stock-grants.json")
            .exists(),
        "governing selects no grant"
    );
    let owner_bytes = std::fs::read(grant_owner_path(&paths)).unwrap();
    govern_stock_realm(tmp.path(), "governed", &principals)
        .await
        .expect("idempotent");
    assert_eq!(
        std::fs::read(grant_owner_path(&paths)).unwrap(),
        owner_bytes,
        "the owner is neither replaced nor given new grants"
    );
    std::fs::remove_file(grant_owner_path(&paths)).unwrap();
    assert!(
        govern_stock_realm(tmp.path(), "governed", &principals)
            .await
            .is_err(),
        "a governed realm's missing owner is not recreated"
    );
    assert!(!grant_owner_path(&paths).exists());
}

/// A realm that was not governed cannot be governed before it exists, and a
/// grant owner that rkat did not provision is refused, not adopted.
#[tokio::test]
async fn governing_refuses_a_missing_realm_and_a_foreign_grant_owner() {
    use meerkat::governed_realm::stock::{
        StockGovernanceError, StockPrincipals, govern_stock_realm,
    };
    let tmp = tempfile::tempdir().unwrap();
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    assert!(
        govern_stock_realm(tmp.path(), "governed", &principals)
            .await
            .is_err(),
        "an unmaterialized realm is not governed"
    );
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    provision_grant_owner(&paths);
    assert!(matches!(
        govern_stock_realm(tmp.path(), "governed", &principals).await,
        Err(StockGovernanceError::ForeignGrantOwner)
    ));
}

/// A stock-governed realm opens with the stock composition over its
/// restored owner, carries it for the prompt sites and holds the owner's
/// lease while open. Its configured servers are decided per agent: a session
/// no agent is bound to attaches none, not even its ceilinged installation
/// (see `installation_does_not_grant_every_agent`).
#[tokio::test]
async fn a_stock_governed_realm_opens_with_its_composition() {
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    install(&paths, read_only_fixture()).await;
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(tmp.path(), "governed", &principals)
        .await
        .expect("governed");

    let stock = StockGovernance::new(realm, principals);
    let bundle = open(tmp.path(), RealmGovernance::Stock(stock)).await;
    assert!(
        bundle
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
    assert!(bundle.stock_governance().is_some());
    assert!(
        try_restore(&paths).is_err(),
        "the open realm holds its restored grant owner"
    );
    // Configured MCP is decided per agent: a session whose agent the host
    // has not bound gets no configured server, the installation included.
    assert_eq!(
        attachment_of(&bundle, &read_only_fixture()).await,
        Some(Err(McpAttachmentRefusal::Unavailable)),
        "no agent is bound to the session"
    );
    assert_eq!(
        configured_attachment(&bundle).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
}

/// The configured-attachment verdict of a fresh session whose agent the
/// stock host bound (as the CLI does right after preparing its bindings).
async fn agent_attachment_of(
    bundle: &meerkat::PersistenceBundle,
    stock: &meerkat::governed_realm::stock::StockGovernance,
    server: &McpServerConfig,
) -> Option<Result<(), McpAttachmentRefusal>> {
    let machine = bundle.runtime_adapter();
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("registration");
    let bindings = machine
        .prepare_bindings(session.clone())
        .await
        .expect("bindings");
    stock
        .bind_session_agent(&machine, &session, [server.name.as_str()])
        .await
        .expect("the host binds the session's agent");
    bindings
        .configured_mcp_attachment()
        .map(|authority| authority.authorize_configured(server))
}

/// Governed prompts need the operator's deliberate grant selection: without
/// one the stock composition opens and holds none; after the operator issues
/// distinct worker and controller grants and records their selection, a new
/// open holds it.
#[tokio::test]
async fn a_stock_realm_holds_only_the_operators_recorded_grant_selection() {
    use meerkat::governed_realm::stock::{
        StockGovernance, StockGrantSelection, StockPrincipals, govern_stock_realm,
        record_stock_grant_selection,
    };
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    install(&paths, read_only_fixture()).await;
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(tmp.path(), "governed", &principals)
        .await
        .expect("governed");

    let stock = StockGovernance::new(realm.clone(), principals.clone());
    let bundle = open(tmp.path(), RealmGovernance::Stock(Arc::clone(&stock))).await;
    assert!(
        bundle
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
    assert!(!stock.has_grant_selection());
    // Without a selection the host binds no agent: nothing is configured.
    assert_eq!(
        agent_attachment_of(&bundle, &stock, &read_only_fixture()).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
    // The composition (and so the grant owner) lives as long as the bundle
    // and the host's StockGovernance.
    drop(bundle);
    drop(stock);

    // The operator's explicit setup: distinct worker and controller grants.
    // The worker grant approves the reader installation's connector (it has
    // none) explicitly: a wildcard never approves one.
    let owner = try_restore(&paths).expect("the closed realm's owner restores");
    let issue = |grant: &str, restrictions: ExecutionRestrictions| {
        owner
            .issue_root(
                &principals.operator,
                EvidenceId::new(grant).unwrap(),
                principals.executor.clone(),
                None,
                restrictions,
            )
            .expect("issued")
    };
    let worker = meerkat::governed_realm::stock::issuance::stock_grant_restrictions(
        &principals.operator,
        &principals.executor,
        &meerkat::governed_realm::stock::issuance::StockGrantTarget::Mcp {
            server: read_only_fixture().name,
            operations: meerkat::governed_realm::stock::issuance::McpOperations::Exact(
                ["echo".to_string()].into(),
            ),
        },
        Some(&read_only_fixture().connector_approval()),
        None,
        0,
    )
    .expect("the reader's worker scope");
    let selection = StockGrantSelection {
        operation: issue("worker", worker),
        controller: issue("controller", ExecutionRestrictions::unrestricted()),
    };
    drop(owner);
    record_stock_grant_selection(tmp.path(), "governed", &selection)
        .await
        .expect("recorded");

    let stock = StockGovernance::new(realm, principals);
    let bundle = open(tmp.path(), RealmGovernance::Stock(Arc::clone(&stock))).await;
    assert!(stock.has_grant_selection());
    // The bound agent holds a grant covering the installation: its
    // configured installation is admitted, and only it.
    assert_eq!(
        agent_attachment_of(&bundle, &stock, &read_only_fixture()).await,
        Some(Ok(()))
    );
    let mut unceilinged = read_only_fixture();
    unceilinged.access = None;
    assert_eq!(
        agent_attachment_of(&bundle, &stock, &unceilinged).await,
        Some(Err(McpAttachmentRefusal::Unavailable))
    );
    drop(bundle);
}

/// Currentness is decided under the config document's own writer exclusion
/// against the actual document: a writer holding it refuses the decision as
/// not current, and a document changed by a write whose generation was never
/// published no longer certifies the removed installation.
#[tokio::test]
async fn an_installation_is_current_only_under_the_writer_exclusion() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = governed_realm_with_installations(tmp.path()).await;
    let installed = read_only_fixture();
    let bundle = open(tmp.path(), bootstrap()).await;
    assert_eq!(attachment_of(&bundle, &installed).await, Some(Ok(())));

    // A writer holds the document's custody: the mutation is pending.
    let lock = paths.config_path.with_file_name("config_state.lock");
    let writer = meerkat_core::config_runtime::ConfigWriterLock::try_hold(&lock)
        .unwrap()
        .expect("a config write in progress");
    assert_eq!(
        attachment_of(&bundle, &installed).await,
        Some(Err(McpAttachmentRefusal::NotCurrent))
    );
    drop(writer);
    assert_eq!(attachment_of(&bundle, &installed).await, Some(Ok(())));

    // The document lost the installation but its generation was never
    // published (a failed or cancelled generation write).
    let generation = std::fs::read(paths.config_path.with_file_name("config_state.json")).unwrap();
    std::fs::write(&paths.config_path, b"").unwrap();
    assert_eq!(
        std::fs::read(paths.config_path.with_file_name("config_state.json")).unwrap(),
        generation
    );
    assert_eq!(
        attachment_of(&bundle, &installed).await,
        Some(Err(McpAttachmentRefusal::NotCurrent))
    );
}

/// Governing takes the realm's exclusive custody before it publishes the
/// posture: while an ordinary owner is live, governing refuses as busy and
/// the posture stays ungoverned.
#[tokio::test]
async fn governing_while_an_ordinary_owner_is_live_is_busy_and_changes_nothing() {
    use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    let live = open(tmp.path(), RealmGovernance::Ungoverned).await;
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    assert!(
        govern_stock_realm(tmp.path(), "governed", &principals)
            .await
            .is_err(),
        "a live ordinary owner holds the realm"
    );
    assert_eq!(
        meerkat_store::read_realm_posture_in(tmp.path(), "governed")
            .await
            .unwrap(),
        meerkat_store::RealmPosture::Ungoverned
    );
    assert!(!grant_owner_path(&paths).exists());
    drop(live);
    govern_stock_realm(tmp.path(), "governed", &principals)
        .await
        .expect("governed once the owner closed");

    // A memory realm's custody belongs to each opened store, so another open
    // could not exclude a live ordinary owner: governing it is refused
    // before anything is published, while it opens ordinarily as before.
    let memory_root = tempfile::tempdir().unwrap();
    let live = open_governed_realm_persistence_with_provider(
        &DiskStorageProvider,
        memory_root.path(),
        "governed",
        Some(RealmBackend::Memory),
        None,
        None,
        RealmGovernance::Ungoverned,
    )
    .await
    .expect("an ordinary memory realm")
    .1;
    assert!(matches!(
        govern_stock_realm(memory_root.path(), "governed", &principals).await,
        Err(meerkat::governed_realm::stock::StockGovernanceError::Unsupported(_))
    ));
    assert_eq!(
        meerkat_store::read_realm_posture_in(memory_root.path(), "governed")
            .await
            .unwrap(),
        meerkat_store::RealmPosture::Ungoverned
    );
    drop(live);
    assert!(matches!(
        govern_stock_realm(memory_root.path(), "governed", &principals).await,
        Err(meerkat::governed_realm::stock::StockGovernanceError::Unsupported(_))
    ));
}

/// An ordinary open re-reads the posture under the custody it acquired: a
/// realm governed after a caller decided to open it ordinarily is refused by
/// every ordinary opener (in any build), and the posture-aware opener opens it
/// governed.
#[tokio::test]
async fn an_ordinary_open_of_a_governed_realm_is_refused_under_custody() {
    use meerkat::governed_realm::open_stock_realm_persistence;
    use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
    let tmp = tempfile::tempdir().unwrap();
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    govern_stock_realm(
        tmp.path(),
        "governed",
        &StockPrincipals::for_user(&realm, 4242).unwrap(),
    )
    .await
    .expect("governed");

    let ordinary = meerkat::open_realm_persistence_with_provider(
        &DiskStorageProvider,
        tmp.path(),
        "governed",
        Some(RealmBackend::Sqlite),
        None,
        None,
    )
    .await;
    assert!(matches!(
        ordinary,
        Err(meerkat::PersistenceError::RealmGoverned { .. })
    ));

    let (_, governed) = open_stock_realm_persistence(
        &DiskStorageProvider,
        tmp.path(),
        "governed",
        Some(RealmBackend::Sqlite),
        None,
        None,
    )
    .await
    .expect("the posture-aware open");
    assert!(
        governed
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
}

/// `rkat-rest` and `rkat-mcp` open their realm only through
/// `storage_provider::open_realm_persistence_with_layout`, so neither hosts a
/// stock-governed realm's schedules: that open refuses a governed realm, and
/// their startup fails. The control opens the same realm while it is
/// ungoverned. A host given a governed open would have to build its schedule
/// service with `PersistenceBundle::schedule_service()` for its fires to be
/// refused typed and early.
#[tokio::test]
async fn the_rest_and_mcp_hosts_open_refuses_a_governed_realm() {
    use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
    let tmp = tempfile::tempdir().unwrap();
    let layout = || {
        let realm = meerkat_core::RealmConfig {
            selection: meerkat_core::RealmSelection::Explicit {
                realm_id: "governed".to_string(),
            },
            state_root: Some(tmp.path().to_path_buf()),
            ..meerkat_core::RealmConfig::default()
        };
        let inputs = meerkat_core::StorageLayoutInputs {
            invocation_context: tmp.path().to_path_buf(),
            ..meerkat_core::StorageLayoutInputs::default()
        };
        meerkat_core::StorageLayout::resolve(inputs, &realm)
            .expect("layout")
            .layout
    };

    let (_, ungoverned) = meerkat::storage_provider::open_realm_persistence_with_layout(
        layout(),
        "governed",
        Some(RealmBackend::Sqlite),
        None,
    )
    .await
    .expect("the hosts' open opens an ungoverned realm");
    assert!(ungoverned.stock_governance().is_none());
    drop(ungoverned);

    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    govern_stock_realm(
        tmp.path(),
        "governed",
        &StockPrincipals::for_user(&realm, 4242).unwrap(),
    )
    .await
    .expect("governed");

    let refused = meerkat::storage_provider::open_realm_persistence_with_layout(
        layout(),
        "governed",
        Some(RealmBackend::Sqlite),
        None,
    )
    .await;
    assert!(matches!(
        refused,
        Err(meerkat::PersistenceError::RealmGoverned { .. })
    ));
}

/// One realm installation is not a grant for every agent. With the reader
/// installed (ceiling `echo`) and the operator's grant for it issued to agent
/// A only, A's session admits it once A is bound; agent B, granted only its
/// controller, has no grant selected for the reader and is refused it.
/// Before either is bound the session's configured servers are unavailable.
#[tokio::test]
async fn installation_does_not_grant_every_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    install(&paths, read_only_fixture()).await;
    let (realm, principals) = govern_as_operator(tmp.path()).await;
    grant(
        tmp.path(),
        &principals,
        "agent-a",
        mcp_target("reader", &["echo"]),
    )
    .await;
    for agent in ["agent-a", "agent-b"] {
        grant(
            tmp.path(),
            &principals,
            agent,
            meerkat::governed_realm::stock::issuance::StockGrantTarget::Controller {
                routes: [meerkat_core::ControllerRouteKey::parse(&format!(
                    "{}{}",
                    meerkat_core::ControllerRouteKey::PREFIX,
                    "0".repeat(64)
                ))
                .unwrap()]
                .into(),
            },
        )
        .await;
    }

    let reader = read_only_fixture();
    for (agent, granted) in [("agent-a", true), ("agent-b", false)] {
        let verdicts = agent_verdicts(tmp.path(), &realm, &principals, agent, &[&reader]).await;
        assert_eq!(
            verdicts.unbound,
            vec![Err(McpAttachmentRefusal::Unavailable)],
            "{agent}: not bound yet"
        );
        let expected = if granted {
            Ok(())
        } else {
            Err(McpAttachmentRefusal::Unavailable)
        };
        assert_eq!(verdicts.bound, vec![expected], "{agent}");
    }
}

/// Two installed servers with disjoint operations get two exact grants for
/// one agent; each is decided by its own. Revoking one unselects only it: in
/// a realm opened afterwards that server is refused and the other stays
/// usable.
#[tokio::test]
async fn per_server_grants_decide_each_server_by_its_own() {
    use meerkat::governed_realm::stock::issuance::{
        RevocationOutcome, StockGrantSlot, revoke_stock_agent,
    };
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    let reader = read_only_fixture();
    let adder = adder_fixture();
    install(&paths, reader.clone()).await;
    install(&paths, adder.clone()).await;
    let (realm, principals) = govern_as_operator(tmp.path()).await;
    grant(
        tmp.path(),
        &principals,
        "agent-a",
        mcp_target("reader", &["echo"]),
    )
    .await;
    grant(
        tmp.path(),
        &principals,
        "agent-a",
        mcp_target("adder", &["add"]),
    )
    .await;

    let verdicts = agent_verdicts(
        tmp.path(),
        &realm,
        &principals,
        "agent-a",
        &[&reader, &adder],
    )
    .await;
    assert_eq!(verdicts.bound, vec![Ok(()), Ok(())]);

    let revoked = revoke_stock_agent(
        tmp.path(),
        "governed",
        &principals,
        "agent-a",
        &StockGrantSlot::Mcp("reader".to_string()),
    )
    .await
    .expect("revoke");
    assert_eq!(revoked.outcome, RevocationOutcome::Revoked);
    let verdicts = agent_verdicts(
        tmp.path(),
        &realm,
        &principals,
        "agent-a",
        &[&reader, &adder],
    )
    .await;
    assert_eq!(
        verdicts.bound,
        vec![Err(McpAttachmentRefusal::Unavailable), Ok(())],
        "the adder grant never stands in for the reader"
    );
}

/// A selection that grants the agent only its tool catalog binds no session
/// agent. The catalog is base work, which the work basis authorizes; a
/// session's agent mandate carries MCP server lineages only. So every
/// configured server stays unavailable, and the registration's set-once
/// binding stays free for a later server mandate.
#[tokio::test]
async fn a_catalog_only_selection_binds_no_session_agent() {
    use meerkat::governed_realm::stock::StockGovernance;
    use meerkat::governed_realm::stock::issuance::{
        StockGrantRequest, StockGrantTarget, grant_stock_agent,
    };
    use meerkat_runtime::meerkat_machine::SessionAgentMandate;
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    let reader = read_only_fixture();
    install(&paths, reader.clone()).await;
    let (realm, principals) = govern_as_operator(tmp.path()).await;
    let catalog = grant_stock_agent(
        tmp.path(),
        "governed",
        &principals,
        &StockGrantRequest {
            agent: "agent-a".to_string(),
            target: StockGrantTarget::Catalog,
            expires_in_ms: None,
        },
    )
    .await
    .expect("issued and selected")
    .issued;

    let stock = StockGovernance::new_for_agent(realm, principals.clone(), "agent-a")
        .expect("agent principal");
    let bundle = open(tmp.path(), RealmGovernance::Stock(Arc::clone(&stock))).await;
    let machine = bundle.runtime_adapter();
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("registration");
    let bindings = machine
        .prepare_bindings(session.clone())
        .await
        .expect("bindings");
    stock
        .bind_session_agent(&machine, &session, [reader.name.as_str()])
        .await
        .expect("the host binds the agent's selection");
    assert_eq!(
        bindings
            .configured_mcp_attachment()
            .map(|authority| authority.authorize_configured(&reader)),
        Some(Err(McpAttachmentRefusal::Unavailable)),
        "a catalog grant never covers a configured server"
    );
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let servers = std::collections::BTreeMap::from([(reader.name.clone(), vec![catalog])]);
    machine
        .bind_session_agent_mandate(
            &registration,
            SessionAgentMandate::new(principals.executor.clone(), servers, None)
                .expect("a server mandate"),
        )
        .await
        .expect("the catalog-only selection left the registration unbound");
}

/// The fixture server installed under another name with a ceiling of `add`.
fn adder_fixture() -> McpServerConfig {
    let mut server = read_only_fixture();
    server.name = "adder".to_string();
    server.access = Some(meerkat_core::McpServerAccess::allowing(["add"]));
    server
}

async fn govern_as_operator(
    root: &Path,
) -> (
    meerkat_core::connection::RealmId,
    meerkat::governed_realm::stock::StockPrincipals,
) {
    use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
    let realm = meerkat_core::connection::RealmId::parse("governed").unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(root, "governed", &principals)
        .await
        .expect("governed");
    (realm, principals)
}

fn mcp_target(
    server: &str,
    operations: &[&str],
) -> meerkat::governed_realm::stock::issuance::StockGrantTarget {
    meerkat::governed_realm::stock::issuance::StockGrantTarget::Mcp {
        server: server.to_string(),
        operations: meerkat::governed_realm::stock::issuance::McpOperations::Exact(
            operations.iter().map(|op| (*op).to_string()).collect(),
        ),
    }
}

async fn grant(
    root: &Path,
    principals: &meerkat::governed_realm::stock::StockPrincipals,
    agent: &str,
    target: meerkat::governed_realm::stock::issuance::StockGrantTarget,
) {
    meerkat::governed_realm::stock::issuance::grant_stock_agent(
        root,
        "governed",
        principals,
        &meerkat::governed_realm::stock::issuance::StockGrantRequest {
            agent: agent.to_string(),
            target,
            expires_in_ms: None,
        },
    )
    .await
    .expect("issued and selected");
}

struct AgentVerdicts {
    unbound: Vec<Result<(), McpAttachmentRefusal>>,
    bound: Vec<Result<(), McpAttachmentRefusal>>,
}

/// Open the realm as `agent`, register a session and decide `servers`
/// before and after the host binds the agent (as the CLI does right after
/// preparing the session's bindings).
async fn agent_verdicts(
    root: &Path,
    realm: &meerkat_core::connection::RealmId,
    principals: &meerkat::governed_realm::stock::StockPrincipals,
    agent: &str,
    servers: &[&McpServerConfig],
) -> AgentVerdicts {
    use meerkat::governed_realm::stock::StockGovernance;
    let stock = StockGovernance::new_for_agent(realm.clone(), principals.clone(), agent)
        .expect("agent principal");
    let bundle = open(root, RealmGovernance::Stock(Arc::clone(&stock))).await;
    let machine = bundle.runtime_adapter();
    let session = SessionId::new();
    machine
        .register_session(session.clone())
        .await
        .expect("registration");
    let bindings = machine
        .prepare_bindings(session.clone())
        .await
        .expect("bindings");
    let authority = bindings
        .configured_mcp_attachment()
        .cloned()
        .expect("the governed host decides configured servers");
    let decide = || {
        servers
            .iter()
            .map(|server| authority.authorize_configured(server))
            .collect::<Vec<_>>()
    };
    let unbound = decide();
    stock
        .bind_session_agent(
            &machine,
            &session,
            servers.iter().map(|server| server.name.as_str()),
        )
        .await
        .expect("bound");
    let bound = decide();
    drop(bindings);
    drop(bundle);
    drop(stock);
    AgentVerdicts { unbound, bound }
}

/// The reader installed with a connector binding to `account`.
fn reader_bound_to(account: &str) -> McpServerConfig {
    let mut server = read_only_fixture();
    server.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: "governed".to_string(),
        slot_id: "reader-slot".to_string(),
        issuer: "https://issuer.example.test".to_string(),
        client: "reader-client".to_string(),
        resource: "https://api.example.test".to_string(),
        strategy_id: "oauth".to_string(),
        account: account.to_string(),
        operations: [(
            "echo".to_string(),
            meerkat_core::mcp_config::McpConnectorOperation {
                required_scopes: Default::default(),
                destination_origin: "https://api.example.test".to_string(),
                read_only: true,
            },
        )]
        .into(),
    });
    server
}

/// A grant approves the connector account of the installation the operator
/// approved. Reconfiguring the same server name to another account does not
/// inherit it: in a realm opened afterwards the old grant is refused, until
/// the operator explicitly grants the server again.
#[tokio::test]
async fn a_reconfigured_account_needs_an_explicit_regrant() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = realm_paths(tmp.path());
    drop(open(tmp.path(), RealmGovernance::Ungoverned).await);
    install(&paths, reader_bound_to("account-a")).await;
    let (realm, principals) = govern_as_operator(tmp.path()).await;
    grant(
        tmp.path(),
        &principals,
        "agent-a",
        mcp_target("reader", &["echo"]),
    )
    .await;
    grant(
        tmp.path(),
        &principals,
        "agent-a",
        meerkat::governed_realm::stock::issuance::StockGrantTarget::Controller {
            routes: [meerkat_core::ControllerRouteKey::parse(&format!(
                "{}{}",
                meerkat_core::ControllerRouteKey::PREFIX,
                "0".repeat(64)
            ))
            .unwrap()]
            .into(),
        },
    )
    .await;
    let reader_a = reader_bound_to("account-a");
    let verdicts = agent_verdicts(tmp.path(), &realm, &principals, "agent-a", &[&reader_a]).await;
    assert_eq!(verdicts.bound, vec![Ok(())], "the approved account");

    uninstall(&paths, "reader").await;
    install(&paths, reader_bound_to("account-b")).await;
    let reader_b = reader_bound_to("account-b");
    let verdicts = agent_verdicts(tmp.path(), &realm, &principals, "agent-a", &[&reader_b]).await;
    assert!(
        verdicts.bound[0].is_err(),
        "another account is not inherited: {:?}",
        verdicts.bound
    );

    grant(
        tmp.path(),
        &principals,
        "agent-a",
        mcp_target("reader", &["echo"]),
    )
    .await;
    let verdicts = agent_verdicts(tmp.path(), &realm, &principals, "agent-a", &[&reader_b]).await;
    assert_eq!(verdicts.bound, vec![Ok(())], "after an explicit regrant");
}
