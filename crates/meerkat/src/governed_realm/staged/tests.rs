//! The staged pre-open path of a stock governed realm: exclusive custody
//! from the start, the stage's own grant under it, finish only after every
//! entered grant command settled, and an abort that retires every ticket
//! alias and reports the custody it actually released.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat_core::auth::{PersistedAuthMode, PersistedTokens};
use meerkat_core::handles::LeaseKey;
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
use meerkat_core::{
    AuthBindingRef, AuthStatusPhase, BindingId, Config, OAuthProviderIdentity, Provider,
    RealmConfigSection, RealmId, ToolCategoryOverride,
};
use meerkat_session::ephemeral::SessionAgentBuilder as _;
use tokio::sync::{mpsc, oneshot};

use super::*;
use crate::governed_realm::stock::issuance::{
    DEFAULT_STOCK_AGENT, PUBLICATION_HOLDS, StockGrantConclusion, StockGrantObservationCoverage,
    StockGrantTarget, StockSelectionSource, grant_stock_agent,
};
use crate::prepared_controller::PreparedControllerRefusal;
use crate::{HostAuthService, HostAuthTarget, ProviderAuthPersistence};

const REALM: &str = "governed";

fn realm() -> RealmId {
    RealmId::parse(REALM).unwrap()
}

fn principals() -> StockPrincipals {
    StockPrincipals::for_user(&realm(), 4242).unwrap()
}

/// An ordinary realm the operator then governed.
async fn governed_realm() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    drop(
        super::super::open_governed_realm_persistence_with_provider(
            &crate::DiskStorageProvider,
            tmp.path(),
            REALM,
            Some(RealmBackend::Sqlite),
            None,
            None,
            RealmGovernance::Ungoverned,
        )
        .await
        .expect("an ordinary realm"),
    );
    super::super::stock::govern_stock_realm(tmp.path(), REALM, &principals())
        .await
        .expect("governed");
    tmp
}

async fn stage(root: &Path) -> Result<StagedStockRealm, StagedRealmError> {
    stage_stock_realm_persistence_with(
        &crate::DiskStorageProvider,
        root,
        REALM,
        None,
        None,
        None,
        StockGovernance::new_for_agent(realm(), principals(), DEFAULT_STOCK_AGENT).unwrap(),
    )
    .await
}

/// The realm stages again (and is aborted): its custody was released. A
/// released custody can trail the last handle's drop on another task, so
/// this waits a bounded time.
async fn reopens(root: &Path) {
    for _ in 0..500 {
        match stage(root).await {
            Ok(mut again) => {
                let outcome = again.abort().await.expect("aborted");
                assert_eq!(outcome.custody, StagedCustodyRelease::Released);
                return;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    panic!("the realm's custody was never released");
}

fn catalog() -> StockGrantRequest {
    StockGrantRequest {
        agent: DEFAULT_STOCK_AGENT.to_string(),
        target: StockGrantTarget::Catalog,
        expires_in_ms: None,
    }
}

async fn selected_catalog(
    root: &Path,
) -> Option<meerkat_authorization_contracts::work_association::GrantLineageRef> {
    crate::governed_realm::stock::read_stock_grant_file(&meerkat_store::realm_paths_in(root, REALM))
        .await
        .unwrap()
        .and_then(|mut file| file.agents.remove(DEFAULT_STOCK_AGENT))
        .and_then(|refs| refs.catalog)
}

/// Hold the realm's next selection write after it acknowledges that its
/// command holds custody and is about to publish.
fn hold_publication(root: &Path) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (acknowledge, acknowledged) = oneshot::channel();
    let (release, released) = oneshot::channel();
    PUBLICATION_HOLDS.lock().unwrap().push((
        meerkat_store::realm_paths_in(root, REALM).root,
        acknowledge,
        released,
    ));
    (acknowledged, release)
}

/// Start the stage's grant and stop waiting for it once its command is
/// inside its held selection write: the exact entered command stays with the
/// stage.
async fn abandon_after_entry(
    stage: &mut StagedStockRealm,
    request: StockGrantRequest,
    acknowledged: oneshot::Receiver<()>,
) {
    let grant = stage.grant_stock_agent(request);
    tokio::pin!(grant);
    tokio::select! {
        biased;
        outcome = &mut grant => panic!("the grant waits on its held publication: {outcome:?}"),
        acknowledged = acknowledged => {
            acknowledged.expect("the command holds custody inside its selection write");
        }
    }
}

/// A factory whose anthropic route resolves an inline key of the realm:
/// nothing is requested.
fn controller_builder(root: &Path) -> crate::FactoryAgentBuilder {
    let factory = crate::AgentFactory::new(root.join("factory-sessions"))
        .without_provider_auth_persistence()
        .builtins(false);
    let mut config = Config::default();
    config.realm.insert(
        REALM.to_string(),
        RealmConfigSection::from_inline_api_keys(&[("anthropic", "test-staged-key")]),
    );
    crate::FactoryAgentBuilder::new(factory, config)
}

fn controller_request() -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "claude-sonnet-4-5".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: crate::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            provider: Some(Provider::Anthropic),
            override_builtins: ToolCategoryOverride::Disable,
            override_image_generation: ToolCategoryOverride::Disable,
            ..Default::default()
        }),
        labels: None,
    }
}

/// External tools whose update poll never finishes: a build that waits for
/// its tool connections stops there, after it took its controller.
struct HeldUpdates {
    entered: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for HeldUpdates {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::not_found(call.name))
    }

    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        std::future::pending().await
    }
}

/// The realm's ChatGPT binding: a managed-store account that has a
/// credential only once the operator signs in to it.
const SIGN_IN_BINDING: &str = "chatgpt";
const SIGN_IN_ACCOUNT: &str = "operator";
const SIGN_IN_MODEL: &str = "gpt-5.4";
const SIGN_IN_REDIRECT: &str = "http://localhost:1455/auth/callback";

fn sign_in_config() -> Config {
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        SIGN_IN_BINDING.to_string(),
        meerkat_core::BackendProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            backend_kind: "chatgpt_backend".to_string(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        SIGN_IN_BINDING.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            auth_method: "managed_chatgpt_oauth".to_string(),
            source: meerkat_core::CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        SIGN_IN_BINDING.to_string(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: SIGN_IN_BINDING.to_string(),
            auth_profile: SIGN_IN_BINDING.to_string(),
            credential_account: Some(
                meerkat_core::CredentialAccountId::parse(SIGN_IN_ACCOUNT).unwrap(),
            ),
            default_model: Some(SIGN_IN_MODEL.to_string()),
            policy: Default::default(),
            provider_default: true,
        },
    );
    let mut config = Config::default();
    config.realm.insert(REALM.to_string(), section);
    config
}

fn sign_in_target() -> HostAuthTarget {
    HostAuthTarget {
        provider: OAuthProviderIdentity::OpenAiChatGpt,
        realm_id: realm(),
        binding_id: BindingId::parse(SIGN_IN_BINDING).unwrap(),
        profile_id: None,
    }
}

/// The account credential the operator signs in to.
fn sign_in_credential() -> meerkat_core::AuthCredentialIdentity {
    meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
        realm: realm(),
        account: meerkat_core::CredentialAccountId::parse(SIGN_IN_ACCOUNT).unwrap(),
    })
}

/// The controller of the session, on the signed-in binding.
fn sign_in_request() -> CreateSessionRequest {
    let mut request = controller_request();
    request.model = SIGN_IN_MODEL.to_string();
    request.build = Some(SessionBuildOptions {
        provider: Some(Provider::OpenAI),
        auth_binding: Some(AuthBindingRef {
            realm: realm(),
            binding: BindingId::parse(SIGN_IN_BINDING).unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }),
        override_builtins: ToolCategoryOverride::Disable,
        override_image_generation: ToolCategoryOverride::Disable,
        ..Default::default()
    });
    request
}

/// A factory whose provider resolution and a host's sign-in share one
/// provider-auth persistence (as `HostAuthService::from_factory` reads it).
fn sign_in_factory(root: &Path) -> (crate::AgentFactory, ProviderAuthPersistence) {
    let persistence = ProviderAuthPersistence::new(
        Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
        Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
    );
    let factory = crate::AgentFactory::new(root.join("factory-sessions"))
        .with_provider_auth_persistence(persistence.clone())
        .builtins(false);
    (factory, persistence)
}

/// Sign the operator in through `service`, as a host does: the service
/// starts the browser sign-in on its pair's flow owner, and the tokens the
/// provider returns for that attempt are committed exactly as
/// `HostAuthService::login_complete` commits them after its code exchange
/// (the one network step, left out here), into that pair's owner and the
/// factory's persistence.
async fn sign_in(
    service: &HostAuthService,
    pair: &meerkat_runtime::ProviderAuthRuntimeAuthority,
    persistence: &ProviderAuthPersistence,
    config: &Config,
) {
    let started = service
        .login_start(config, &sign_in_target(), SIGN_IN_REDIRECT)
        .await
        .expect("the service starts the sign-in on its pair's flow owner");
    let now = chrono::Utc::now();
    let tokens = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("signed-in-access".to_string()),
        refresh_token: Some("signed-in-refresh".to_string()),
        id_token: None,
        expires_at: Some(now + chrono::Duration::hours(1)),
        last_refresh: Some(now),
        scopes: Vec::new(),
        account_id: Some("chatgpt-provider-account".to_string()),
        metadata: serde_json::Value::Null,
    };
    meerkat_providers::browser_login::save_oauth_tokens_and_consume_browser_flow(
        persistence.clone(),
        pair.generated_auth_lease_handle(),
        sign_in_credential(),
        tokens,
        meerkat_providers::browser_login::BrowserOAuthFlowCommit {
            authority: pair.oauth_flow_authority(),
            state: started.state,
            completion: OAuthProviderIdentity::OpenAiChatGpt.into(),
            redirect_uri: started.redirect_uri,
        },
    )
    .await
    .expect("the sign-in commits into the pair's owner");
}

/// The stage holds the realm's exclusive custody from the start: a
/// competing opener and a separately opened administrative grant are refused
/// promptly, while the stage's own grant runs under its custody. Finishing
/// snapshots that issued selection into the installed composition, and the
/// finished bundle keeps the realm until it is dropped.
#[tokio::test]
async fn a_staged_realm_refuses_competing_openers_and_composes_its_own_grant() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "a competing stage is refused"
    );
    assert!(
        super::super::open_stock_realm_persistence(
            &crate::DiskStorageProvider,
            root,
            REALM,
            None,
            None,
            None,
        )
        .await
        .is_err(),
        "a competing opener is refused"
    );
    assert!(
        matches!(
            grant_stock_agent(root, REALM, &principals(), &catalog()).await,
            Err(StockGrantError::Refused(_))
        ),
        "an administrative custody opened beside the stage is refused"
    );

    let granted = staged
        .grant_stock_agent(catalog())
        .await
        .expect("the stage's own grant runs under its custody");
    let open = staged.finish().await.expect("finished");
    assert_eq!(open.issuance, StagedIssuanceEffect::Committed);
    assert!(open.unobserved_grants.is_empty(), "its waiter observed it");
    assert_eq!(selected_catalog(root).await, Some(granted.issued.clone()));
    assert_eq!(
        open.bundle
            .stock_governance()
            .and_then(|stock| stock.composed_agent_refs())
            .and_then(|refs| refs.catalog.clone()),
        Some(granted.issued),
        "the composition snapshotted the selection the stage issued"
    );
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "the finished bundle keeps the realm"
    );
    drop(open);
    reopens(root).await;
}

/// An admission's hold of the realm's admission fence refuses the stage's
/// grant at once: the grant runs under the stage's custody, so it takes the
/// fence without waiting and answers `AdmissionInProgress` well inside the
/// interactive wait, with nothing appended to the grant log or selected.
/// Once the hold ends, the stage's grant goes through.
#[tokio::test]
async fn a_held_fence_refuses_the_staged_grant_at_once_with_nothing_issued() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let fence = meerkat_core::AdmissionFence::realm(root, &realm()).expect("the realm's fence");
    fence.ensure_exists().expect("the fence a holder needs");
    let mut hold = meerkat_core::AdmissionHold::try_acquire(vec![fence])
        .expect("an admission holds the realm's fence");
    let log = meerkat_store::realm_paths_in(root, REALM)
        .root
        .join(meerkat_store::GRANT_OWNER_STORE_RELATIVE_PATH);
    let before = std::fs::read(&log).ok();

    let started = std::time::Instant::now();
    let refused = staged.grant_stock_agent(catalog()).await;
    let waited = started.elapsed();
    assert!(
        matches!(
            refused,
            Err(StagedRealmError::Grant(
                StockGrantError::AdmissionInProgress
            ))
        ),
        "{refused:?}"
    );
    assert!(
        waited < meerkat_core::realm_admission::INTERACTIVE_FENCE_WAIT / 2,
        "under the stage's custody the realm's fence is taken without waiting: {waited:?}"
    );
    assert_eq!(std::fs::read(&log).ok(), before, "nothing was appended");
    assert_eq!(selected_catalog(root).await, None, "nothing was selected");

    hold.release();
    let granted = staged
        .grant_stock_agent(catalog())
        .await
        .expect("the stage's grant goes through once the hold ended");
    let open = staged.finish().await.expect("finished");
    assert_eq!(open.issuance, StagedIssuanceEffect::Committed);
    assert_eq!(selected_catalog(root).await, Some(granted.issued));
}

/// Under the stage's own custody the stage observes the persisted selection
/// its grant wrote, through the realm's grant owner restored for that read
/// only (finishing restores it again for the composition); the finished
/// realm then observes the same lineage as the selection its composition
/// retained, through that composition's own owner.
#[tokio::test]
async fn a_stage_observes_its_grant_and_the_finished_realm_its_retained_selection() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let granted = staged.grant_stock_agent(catalog()).await.expect("granted");

    let observation = staged
        .observe_stock_agent_grants(DEFAULT_STOCK_AGENT, &[StockGrantSlot::Catalog])
        .await
        .expect("observed under the stage's custody");
    assert_eq!(
        observation.coverage,
        StockGrantObservationCoverage::PersistedSelection
    );
    let [catalog_slot] = observation.slots.as_slice() else {
        panic!("one entry per requested slot: {observation:?}");
    };
    let selected = catalog_slot
        .selected
        .as_ref()
        .expect("the catalog is selected");
    assert_eq!(selected.lineage, granted.issued);
    assert_eq!(selected.source, StockSelectionSource::PerTarget);
    assert!(matches!(
        selected.conclusion,
        StockGrantConclusion::Current(_)
    ));

    let open = staged.finish().await.expect("finished after the read");
    let retained = open
        .bundle
        .stock_governance()
        .expect("the finished realm's stock composition")
        .observe_retained_grants(&[StockGrantSlot::Catalog])
        .expect("observed through the composition's owner");
    assert_eq!(
        retained.coverage,
        StockGrantObservationCoverage::RetainedSelection
    );
    let retained_selected = retained.slots[0]
        .selected
        .as_ref()
        .expect("the catalog is retained");
    assert_eq!(retained_selected.lineage, granted.issued);
    assert!(matches!(
        retained_selected.conclusion,
        StockGrantConclusion::Current(_)
    ));
}

/// A grant whose waiter stopped waiting inside its entered selection write
/// stays with the stage: finishing waits for that exact command to publish
/// before it snapshots anything, and the composition then holds its grant.
#[tokio::test]
async fn finish_waits_for_the_entered_grant_its_waiter_abandoned() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let (acknowledged, release) = hold_publication(root);
    abandon_after_entry(&mut staged, catalog(), acknowledged).await;

    let finish = staged.finish();
    tokio::pin!(finish);
    tokio::select! {
        biased;
        finished = &mut finish => panic!(
            "finish snapshotted while the entered command could still publish: {:?}",
            finished.map(|open| open.issuance)
        ),
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }
    assert_eq!(selected_catalog(root).await, None, "nothing published yet");
    release.send(()).unwrap();
    let open = finish.await.expect("finished once the command settled");
    assert_eq!(open.issuance, StagedIssuanceEffect::Committed);
    let published = selected_catalog(root)
        .await
        .expect("the abandoned command published its selection");
    let [Ok(settled)] = open.unobserved_grants.as_slice() else {
        panic!(
            "finish reports the abandoned command's outcome: {:?}",
            open.unobserved_grants
        );
    };
    assert_eq!(settled.issued, published);
    assert_eq!(
        open.bundle
            .stock_governance()
            .and_then(|stock| stock.composed_agent_refs())
            .and_then(|refs| refs.catalog.clone()),
        Some(published),
        "the composition holds the grant published after the waiter left"
    );
}

/// Aborting joins the stage's own entered grant command (and nothing else)
/// before it releases the realm: the abort is still pending while that
/// command holds its write, then reports its committed effect and the
/// released custody, and the realm reopens.
#[tokio::test]
async fn abort_joins_the_entered_grant_before_releasing_custody() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let (acknowledged, release) = hold_publication(root);
    abandon_after_entry(&mut staged, catalog(), acknowledged).await;

    let abort = staged.abort();
    tokio::pin!(abort);
    tokio::select! {
        biased;
        outcome = &mut abort => panic!("abort did not join the entered command: {outcome:?}"),
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }
    release.send(()).unwrap();
    let outcome = abort.await.expect("aborted");
    assert_eq!(outcome.issuance, StagedIssuanceEffect::Committed);
    assert_eq!(outcome.controller, PreparedControllerDisposal::NotPrepared);
    assert_eq!(outcome.custody, StagedCustodyRelease::Released);
    let published = selected_catalog(root)
        .await
        .expect("the abandoned command published its selection");
    let [Ok(settled)] = outcome.unobserved_grants.as_slice() else {
        panic!(
            "abort reports the abandoned command's outcome: {:?}",
            outcome.unobserved_grants
        );
    };
    assert_eq!(settled.issued, published);
    reopens(root).await;
}

/// Aborting retires and drains the prepared controller: every cloned ticket
/// alias is refused as retired at once. The realm's custody is actually
/// released, so it reopens.
#[tokio::test]
async fn abort_retires_every_ticket_alias_and_releases_the_realm() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let offer = staged
        .prepare_controller(
            &controller_builder(root),
            &controller_request(),
            SessionId::new(),
        )
        .await
        .expect("prepared under the stage's auth owner");
    assert!(
        matches!(
            staged
                .prepare_controller(
                    &controller_builder(root),
                    &controller_request(),
                    SessionId::new()
                )
                .await,
            Err(StagedRealmError::ControllerAlreadyPrepared)
        ),
        "one controller per stage"
    );
    let alias = offer.ticket.clone();
    let outcome = staged.abort().await.expect("aborted");
    assert_eq!(outcome.issuance, StagedIssuanceEffect::NotEntered);
    assert_eq!(outcome.controller, PreparedControllerDisposal::Drained);
    assert_eq!(outcome.custody, StagedCustodyRelease::Released);
    assert!(outcome.unobserved_grants.is_empty());
    for ticket in [&offer.ticket, &alias] {
        assert_eq!(
            ticket.cell().take_if(|_| Ok(())).err(),
            Some(PreparedControllerRefusal::Retired)
        );
    }
    reopens(root).await;
}

/// A usable handle of the stage's auth owner that escaped into owned auth
/// work keeps the realm's custody: the abort says it is retained, never
/// released, and the realm reopens only after that handle is gone.
#[tokio::test]
async fn abort_reports_custody_retained_by_an_escaped_auth_owner() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let escaped = staged
        .parts()
        .unwrap()
        .machine
        .generated_auth_lease_handle();
    let outcome = staged.abort().await.expect("aborted");
    assert_eq!(outcome.custody, StagedCustodyRelease::Retained);
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "the escaped owner still holds the realm"
    );
    drop(escaped);
    reopens(root).await;
}

/// The native host is installed only on the stage's unshared machine with
/// no registered session: a stage whose machine registered a session refuses
/// to finish, typed, and its realm reopens once that registration is gone.
/// The grant command that finish settled before the refusal (abandoned by
/// its caller inside its selection write) keeps its exact outcome
/// observable once after the terminal error, and every later step refuses
/// as ended.
#[tokio::test]
async fn finish_refuses_a_staged_machine_with_a_registered_session() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let (acknowledged, release) = hold_publication(root);
    abandon_after_entry(&mut staged, catalog(), acknowledged).await;
    let machine = staged.parts().unwrap().machine.administrative_machine();
    let bindings = machine
        .prepare_bindings(SessionId::new())
        .await
        .expect("a session registers on the staged machine");
    drop(machine);
    release.send(()).unwrap();
    assert!(
        matches!(
            staged.finish().await,
            Err(StagedRealmError::Persistence(
                PersistenceError::RuntimeAuthority(_)
            ))
        ),
        "the native host is not installed over a registered session"
    );
    let published = selected_catalog(root)
        .await
        .expect("the abandoned command published its selection");
    let unobserved = staged.take_unobserved_grants();
    let [Ok(settled)] = unobserved.as_slice() else {
        panic!("the settled command's exact outcome survives the terminal error: {unobserved:?}");
    };
    assert_eq!(settled.issued, published);
    assert!(
        staged.take_unobserved_grants().is_empty(),
        "the outcome is observed once, never replayed"
    );
    assert!(matches!(
        staged.finish().await,
        Err(StagedRealmError::Ended)
    ));
    assert!(matches!(staged.abort().await, Err(StagedRealmError::Ended)));
    drop(bindings);
    reopens(root).await;
}

/// A finish waiter dropped while it joins the entered grant (parked inside
/// its selection write) leaves the stage and that exact command with the
/// stage. Finishing the same stage again rejoins it: the finish completes
/// with the command's exact outcome and the grant it published, and the
/// stage has ended.
#[tokio::test]
async fn a_dropped_finish_waiter_leaves_the_entered_grant_with_the_stage() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let (acknowledged, release) = hold_publication(root);
    abandon_after_entry(&mut staged, catalog(), acknowledged).await;
    {
        let finish = staged.finish();
        tokio::pin!(finish);
        tokio::select! {
            biased;
            finished = &mut finish => panic!(
                "finish completed while the entered command was parked: {:?}",
                finished.map(|open| open.issuance)
            ),
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    assert!(
        staged.entered_grant.is_some(),
        "the dropped finish waiter left the entered command with the stage"
    );
    assert!(staged.parts().is_ok(), "the stage has not ended");
    assert_eq!(selected_catalog(root).await, None, "nothing published yet");

    release.send(()).unwrap();
    let open = staged
        .finish()
        .await
        .expect("the same stage finishes, rejoining the command");
    assert_eq!(open.issuance, StagedIssuanceEffect::Committed);
    let published = selected_catalog(root)
        .await
        .expect("the command published its selection");
    let [Ok(settled)] = open.unobserved_grants.as_slice() else {
        panic!(
            "the rejoined finish reports the command's exact outcome: {:?}",
            open.unobserved_grants
        );
    };
    assert_eq!(settled.issued, published);
    assert_eq!(
        open.bundle
            .stock_governance()
            .and_then(|stock| stock.composed_agent_refs())
            .and_then(|refs| refs.catalog.clone()),
        Some(published),
        "the composition holds the grant that command published"
    );
    assert!(matches!(
        staged.finish().await,
        Err(StagedRealmError::Ended)
    ));
    assert!(matches!(staged.abort().await, Err(StagedRealmError::Ended)));
}

/// An abort waiter dropped while it joins the entered grant (parked inside
/// its selection write) leaves the stage, its controller already retired,
/// and that exact command with the stage, which still holds the realm.
/// Aborting the same stage again rejoins it: the abort completes with the
/// command's exact outcome, the controller's disposal and a completed
/// release, and the realm reopens at once.
#[tokio::test]
async fn a_dropped_abort_waiter_leaves_the_entered_grant_with_the_stage() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let offer = staged
        .prepare_controller(
            &controller_builder(root),
            &controller_request(),
            SessionId::new(),
        )
        .await
        .expect("prepared");
    let (acknowledged, release) = hold_publication(root);
    abandon_after_entry(&mut staged, catalog(), acknowledged).await;
    {
        let abort = staged.abort();
        tokio::pin!(abort);
        tokio::select! {
            biased;
            aborted = &mut abort => panic!(
                "abort completed while the entered command was parked: {aborted:?}"
            ),
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    assert!(
        staged.entered_grant.is_some(),
        "the dropped abort waiter left the entered command with the stage"
    );
    assert_eq!(
        offer.ticket.cell().take_if(|_| Ok(())).err(),
        Some(PreparedControllerRefusal::Retired),
        "the first abort attempt already retired every alias"
    );
    assert!(
        matches!(staged.finish().await, Err(StagedRealmError::Aborting)),
        "only abort completes an aborting stage"
    );
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "the stage still holds the realm"
    );

    release.send(()).unwrap();
    let outcome = staged
        .abort()
        .await
        .expect("the same stage aborts, rejoining the command");
    assert_eq!(outcome.issuance, StagedIssuanceEffect::Committed);
    assert_eq!(outcome.controller, PreparedControllerDisposal::Drained);
    assert_eq!(outcome.custody, StagedCustodyRelease::Released);
    let published = selected_catalog(root)
        .await
        .expect("the command published its selection");
    let [Ok(settled)] = outcome.unobserved_grants.as_slice() else {
        panic!(
            "the rejoined abort reports the command's exact outcome: {:?}",
            outcome.unobserved_grants
        );
    };
    assert_eq!(settled.issued, published);
    let mut again = stage(root)
        .await
        .expect("the realm reopens at once: the abort completed its release");
    again.abort().await.expect("aborted");
}

/// `release()` runs the claim's final destruction itself when it owns the
/// last reference, and reports nothing while that destruction is parked:
/// the realm is still held then. It reports `Released` only once the
/// destruction completed, and the realm reopens at once.
#[tokio::test]
async fn release_reports_released_only_after_its_final_claim_destruction() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let StagedParts { stores, machine } = staged.staged.take().unwrap();
    drop(stores);
    let (parked, complete) = machine
        .hold_custody_release_for_test()
        .expect("a governed stage holds execution custody");
    let releasing = std::thread::spawn(move || machine.release());
    let parked_on = parked
        .recv_timeout(Duration::from_secs(10))
        .expect("the final destruction is parked");
    assert_eq!(
        parked_on,
        releasing.thread().id(),
        "release() itself runs the final destruction"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !releasing.is_finished(),
        "no report while the final destruction is parked"
    );
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "the realm is still held while its final destruction is parked"
    );
    complete.send(()).unwrap();
    assert_eq!(releasing.join().unwrap(), StagedCustodyRelease::Released);
    let mut again = stage(root)
        .await
        .expect("the realm reopens at once: the reported release was complete");
    again.abort().await.expect("aborted");
}

/// While another holder shares the claim, `release()` reports `Retained`.
/// The holder then runs the final destruction on its own thread; while that
/// is parked the realm is still held (the report was true), and it reopens
/// at once after the destruction completes.
#[tokio::test]
async fn release_reports_retained_while_another_holder_runs_the_final_claim_destruction() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let StagedParts { stores, machine } = staged.staged.take().unwrap();
    drop(stores);
    let escaped = machine.generated_auth_lease_handle();
    let (parked, complete) = machine
        .hold_custody_release_for_test()
        .expect("a governed stage holds execution custody");
    assert_eq!(machine.release(), StagedCustodyRelease::Retained);
    let holder = std::thread::spawn(move || drop(escaped));
    let parked_on = parked
        .recv_timeout(Duration::from_secs(10))
        .expect("the holder's final destruction is parked");
    assert_eq!(
        parked_on,
        holder.thread().id(),
        "the last holder runs the final destruction"
    );
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "the realm is still held while the holder's final destruction is parked"
    );
    complete.send(()).unwrap();
    holder.join().unwrap();
    let mut again = stage(root)
        .await
        .expect("the realm reopens once the holder's final destruction completed");
    again.abort().await.expect("aborted");
}

/// Dropping an unfinished stage retires its prepared controller too.
#[tokio::test]
async fn dropping_an_unfinished_stage_retires_its_prepared_controller() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let offer = staged
        .prepare_controller(
            &controller_builder(root),
            &controller_request(),
            SessionId::new(),
        )
        .await
        .expect("prepared");
    drop(staged);
    assert_eq!(
        offer.ticket.cell().take_if(|_| Ok(())).err(),
        Some(PreparedControllerRefusal::Retired)
    );
    reopens(root).await;
}

/// The finished realm's session takes the controller prepared under the
/// stage's owner, through the bindings that clone that same owner. A build
/// cancelled after the take drops the retained graph and its share of the
/// owner: every alias is refused as consumed, and once the realm's own
/// handles are dropped the realm reopens.
#[tokio::test]
async fn a_build_cancelled_after_the_take_releases_the_realm_owner() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let session_id = SessionId::new();
    let offer = staged
        .prepare_controller(
            &controller_builder(root),
            &controller_request(),
            session_id.clone(),
        )
        .await
        .expect("prepared under the stage's auth owner");
    assert!(!offer.report.provider.is_empty());
    let open = staged.finish().await.expect("finished");
    let machine = open.bundle.runtime_adapter();
    let bindings = machine.prepare_bindings(session_id.clone()).await.unwrap();

    // The runtime-backed service gives its builds the realm as their default.
    let mut builder = controller_builder(root);
    builder.default_realm_id = Some(realm());
    builder.wait_for_mcp = true;
    let (entered_tx, entered_rx) = oneshot::channel();
    let mut request = controller_request();
    let build = request.build.get_or_insert_with(Default::default);
    offer.ticket.seat_build(build);
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
    build.external_tools = Some(Arc::new(HeldUpdates {
        entered: std::sync::Mutex::new(Some(entered_tx)),
    }));
    let task = tokio::spawn(async move {
        let (event_tx, _event_rx) = mpsc::channel(8);
        builder.build_agent(&request, event_tx).await.map(drop)
    });
    entered_rx
        .await
        .expect("the session's build took its controller and waits on its tools");
    assert_eq!(
        offer.ticket.cell().take_if(|_| Ok(())).err(),
        Some(PreparedControllerRefusal::Consumed)
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());

    drop(machine);
    drop(open);
    reopens(root).await;
}

/// A host signs in through the stage's own provider-auth pair, and the
/// controller prepared next resolves that credential under the same owner.
/// The stage forwards its existing pair (no new runtime or auth owner), and
/// a `HostAuthService` built from the factory on that pair signs the
/// operator in. Before the sign-in the managed binding cannot be prepared;
/// after it the controller is prepared with the signed-in account as its
/// credential owner and its actual route is granted under the stage's
/// custody. The service and the pair stay alive through the finish, which
/// they do not refuse: they share the custody claim, not the machine. The
/// installed machine keeps that same pair, the session's bindings clone its
/// owner with the signed-in credential live on it, and the session's build
/// takes the prepared controller through those bindings. The ended stage
/// refuses its pair, and the realm reopens once the last holder is gone.
#[tokio::test]
async fn a_sign_in_through_the_stage_pair_reaches_its_controller_and_session() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let pair = staged
        .provider_auth_runtime_authority()
        .expect("the stage forwards its own pair");
    let owner = pair.generated_auth_lease_handle();
    let (factory, persistence) = sign_in_factory(root);
    let config = sign_in_config();
    let service = HostAuthService::from_factory(&factory, pair.clone())
        .expect("the factory's persistence backs the service");
    let builder = crate::FactoryAgentBuilder::new(factory.clone(), config.clone());
    let session_id = SessionId::new();
    assert!(
        matches!(
            staged
                .prepare_controller(&builder, &sign_in_request(), session_id.clone())
                .await,
            Err(StagedRealmError::Controller(_))
        ),
        "the managed binding has no credential before the sign-in"
    );

    sign_in(&service, &pair, &persistence, &config).await;
    assert_eq!(
        service
            .status(&config, &sign_in_target())
            .await
            .expect("the service reads its owner")
            .phase,
        AuthStatusPhase::Valid
    );
    let offer = staged
        .prepare_controller(&builder, &sign_in_request(), session_id.clone())
        .await
        .expect("the controller resolves the signed-in credential under the stage's owner");
    assert_eq!(offer.report.provider, Provider::OpenAI.as_str());
    assert_eq!(
        offer.report.credential_owner,
        format!("account {REALM}/{SIGN_IN_ACCOUNT}")
    );
    let granted = staged
        .grant_stock_agent(StockGrantRequest {
            agent: DEFAULT_STOCK_AGENT.to_string(),
            target: StockGrantTarget::Controller {
                routes: BTreeSet::from([offer.report.key.clone()]),
            },
            expires_in_ms: None,
        })
        .await
        .expect("the actual route is granted under the stage's custody");

    let open = staged
        .finish()
        .await
        .expect("a live sign-in service and pair do not refuse the finish");
    assert_eq!(open.issuance, StagedIssuanceEffect::Committed);
    assert_eq!(
        open.bundle
            .stock_governance()
            .and_then(|stock| stock.composed_agent_refs())
            .and_then(|refs| refs.controller.clone()),
        Some(granted.issued),
        "the composition holds the granted route"
    );
    assert!(
        matches!(
            staged.provider_auth_runtime_authority(),
            Err(StagedRealmError::Ended)
        ),
        "the ended stage refuses its pair"
    );
    let machine = open.bundle.runtime_adapter();
    let installed = machine.provider_auth_runtime_authority();
    assert!(
        installed.generated_auth_lease_handle().same_owner(&owner),
        "the installed machine keeps the stage's auth owner"
    );
    assert!(
        std::ptr::addr_eq(
            Arc::as_ptr(&installed.oauth_flow_authority()),
            Arc::as_ptr(&pair.oauth_flow_authority())
        ),
        "and the flow authority of the same pair"
    );
    let bindings = machine.prepare_bindings(session_id).await.unwrap();
    assert!(
        bindings.auth_lease().same_owner(&owner),
        "the session's bindings clone the owner the sign-in used"
    );
    assert!(
        bindings
            .auth_lease()
            .snapshot(&LeaseKey::from_credential_identity(&sign_in_credential()))
            .credential_present,
        "the signed-in credential is live on the session's owner"
    );

    // The session's build takes the prepared controller through those
    // bindings (its same-owner check passes), then waits on its held tools.
    let mut consuming = crate::FactoryAgentBuilder::new(factory, config.clone());
    consuming.default_realm_id = Some(realm());
    consuming.wait_for_mcp = true;
    let (entered_tx, entered_rx) = oneshot::channel();
    let mut request = sign_in_request();
    let build = request.build.get_or_insert_with(Default::default);
    offer.ticket.seat_build(build);
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
    build.external_tools = Some(Arc::new(HeldUpdates {
        entered: std::sync::Mutex::new(Some(entered_tx)),
    }));
    let mut task = tokio::spawn(async move {
        let (event_tx, _event_rx) = mpsc::channel(8);
        consuming.build_agent(&request, event_tx).await.map(drop)
    });
    tokio::select! {
        biased;
        built = &mut task => panic!("the session's build ended before its held tools: {built:?}"),
        entered = entered_rx => entered.expect("the session's build waits on its held tools"),
    }
    assert_eq!(
        offer.ticket.cell().take_if(|_| Ok(())).err(),
        Some(PreparedControllerRefusal::Consumed)
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        service
            .status(&config, &sign_in_target())
            .await
            .expect("the service still reads its owner")
            .phase,
        AuthStatusPhase::Valid,
        "the installed realm keeps the sign-in on the same owner"
    );

    drop(service);
    drop(pair);
    drop(owner);
    drop(installed);
    drop(machine);
    drop(open);
    reopens(root).await;
}

/// Auth work holding the stage's pair keeps the realm's custody through an
/// abort. A sign-in service built on the pair, with a sign-in pending, does
/// not stop the abort, which waits for none of it and reports the custody
/// `Retained`. The stage refuses its pair once the abort has started and
/// after it ended, and the realm reopens only once that service is gone.
#[tokio::test]
async fn a_sign_in_service_that_survives_the_abort_retains_the_realm() {
    let tmp = governed_realm().await;
    let root = tmp.path();
    let mut staged = stage(root).await.expect("staged");
    let (factory, _persistence) = sign_in_factory(root);
    let service = HostAuthService::from_factory(
        &factory,
        staged
            .provider_auth_runtime_authority()
            .expect("the stage forwards its own pair"),
    )
    .expect("the factory's persistence backs the service");
    service
        .login_start(&sign_in_config(), &sign_in_target(), SIGN_IN_REDIRECT)
        .await
        .expect("a sign-in pending on the stage's flow owner");

    let (acknowledged, release) = hold_publication(root);
    abandon_after_entry(&mut staged, catalog(), acknowledged).await;
    {
        let abort = staged.abort();
        tokio::pin!(abort);
        tokio::select! {
            biased;
            aborted = &mut abort => panic!(
                "abort completed while the entered command was parked: {aborted:?}"
            ),
            () = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    assert!(
        matches!(
            staged.provider_auth_runtime_authority(),
            Err(StagedRealmError::Aborting)
        ),
        "an aborting stage refuses its pair"
    );
    release.send(()).unwrap();
    let outcome = staged.abort().await.expect("aborted");
    assert_eq!(
        outcome.custody,
        StagedCustodyRelease::Retained,
        "the service's pair still holds the realm's custody"
    );
    assert!(
        matches!(
            staged.provider_auth_runtime_authority(),
            Err(StagedRealmError::Ended)
        ),
        "an ended stage refuses its pair"
    );
    assert!(
        matches!(stage(root).await, Err(StagedRealmError::Persistence(_))),
        "the realm stays held while the service lives"
    );
    drop(service);
    reopens(root).await;
}
