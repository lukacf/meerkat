//! An access decision whose native issuance was pending when its host was
//! lost, over the real installed pair: the next host's cold start settles it
//! from what the native grant owner recorded, and nothing is issued twice.
use super::*;
use crate::host_gateway::ToolkitGatewayHost;
use crate::private_originals::{
    PrivateOriginalError, PrivateServiceAccessDecision, PrivateServiceGrantObservation,
    PrivateServiceGrantState,
};
use meerkat_authorization::{
    clock::HostAuthorizationClock,
    grants::{FileGrantOwnerStore, LocalGrantAuthority},
    publication::LocalAuthorizationPublication,
};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy;

const ACCESS_SCHEMA: &str = "toolkit.authenticated-service-access-decision";

/// One composed host over the installed pair, with the store wrapper that
/// can hold its access decision.
struct OpenHost {
    host: ToolkitGatewayHost,
    store: Arc<PausedSourceStore>,
    // The private channel stays open while the host is used.
    _open: tokio::sync::watch::Sender<bool>,
}

async fn open_host(selection: &HostBootstrapSelection, scope: &StoreScope) -> OpenHost {
    let captured = Arc::new(OnceLock::<Arc<PausedSourceStore>>::new());
    let capture = captured.clone();
    let scope = scope.clone();
    let (owner, installed) = InstalledToolkitHost::open_with_source_store(
        selection.clone(),
        NativeOwnerStartup::Restore,
        None,
        Box::new(move |inner| {
            let store = Arc::new(PausedSourceStore::new(inner, scope));
            assert!(capture.set(store.clone()).is_ok());
            store
        }),
    )
    .await
    .unwrap();
    let (open, closed) = tokio::sync::watch::channel(false);
    OpenHost {
        host: installed.compose(Arc::new(owner), PrivateHostClosed(closed)),
        store: captured.get().unwrap().clone(),
        _open: open,
    }
}

/// The host is lost: its close begins on the loss and settles, and the
/// close's final release ends its native custody at once, without waiting
/// for the decision it admitted, which is still held.
async fn lose(host: &ToolkitGatewayHost) {
    host.begin_close(GatewayHostCloseReason::HostDisconnected)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), host.join_close())
        .await
        .unwrap()
        .expect("the lost host's close settles");
    host.release_native_custody()
        .await
        .expect("the lost host's final release follows its close");
}

/// The issuance state the access decision's row records.
async fn row_state(store: &PausedSourceStore, scope: &StoreScope) -> (String, String) {
    let rows: Vec<_> = store
        .list_records(scope, None, 64)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.data.schema == ACCESS_SCHEMA)
        .collect();
    assert_eq!(rows.len(), 1, "one access decision row");
    (
        rows[0].key.id.clone(),
        rows[0].data.body["issuance"]["state"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

fn answer(applied: &Result<PrivateServiceGrantObservation, PrivateOriginalError>) -> String {
    match applied {
        Ok(observation) => format!("{:?}", observation.state),
        Err(error) => format!("{error:?}"),
    }
}

/// Where the lost host's decision stood when the host was lost.
#[derive(Clone, Copy, PartialEq)]
enum Lost {
    /// Its pending row committed; it had not entered native issuance.
    BeforeIssuance,
    /// The native owner recorded its grant; its row had not.
    AfterIssuance,
}

struct LostDecision {
    _directory: tempfile::TempDir,
    scope: StoreScope,
    grants_path: PathBuf,
    decision: PrivateServiceAccessDecision,
}

/// Admit an access decision on a first host, hold it at `lost`, lose the
/// host, open the next host on the same pair, then let the lost host's
/// decision finish. Returns the next host and the lost host's answer.
async fn lose_host_with_pending_access(
    lost: Lost,
) -> (
    LostDecision,
    OpenHost,
    Result<PrivateServiceGrantObservation, PrivateOriginalError>,
) {
    let (directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let selected = local.native_host.as_ref().unwrap();
    let state = state_directory(&selection, selected, NativeOwnerStartup::Restore).unwrap();
    let scope = StoreScope {
        realm: selected.realm.to_string(),
        owner: local.deployment.to_string(),
    };
    let first = open_host(&selection, &scope).await;
    let mut decision = crate::private_originals::service_grants::tests::decision();
    decision.route = toolkit_package::Id::new("console").unwrap();
    match lost {
        Lost::BeforeIssuance => first.store.arm_access(),
        Lost::AfterIssuance => first.store.arm_access_swap(),
    }
    let pending = first.host.apply_service_access_decision(decision.clone());
    tokio::pin!(pending);
    tokio::select! {
        _ = first.store.entered.notified() => {}
        _ = &mut pending => panic!("the lost host's decision is held"),
    }
    lose(&first.host).await;
    let next = open_host(&selection, &scope).await;
    // The lost host's decision now finishes: its native owner and its store
    // are no longer in its custody, so it records nothing more.
    first.store.release.add_permits(1);
    let lost_answer = tokio::time::timeout(Duration::from_secs(15), &mut pending)
        .await
        .unwrap();
    (
        LostDecision {
            _directory: directory,
            scope,
            grants_path: state.join("grants.jsonl"),
            decision,
        },
        next,
        lost_answer,
    )
}

/// The native owner's access grants, read after the host's final release.
fn native_access_grants(grants_path: &Path) -> Vec<String> {
    let grants = LocalGrantAuthority::restore(
        Arc::new(FileGrantOwnerStore::new(
            grants_path,
            meerkat_authorization::grants::GrantLogFence::NotAdmissionRead,
        )),
        LocalAuthorizationPublication::new(),
        Arc::new(HostAuthorizationClock),
    )
    .unwrap();
    grants
        .unrevoked_grants(|id| id.as_str().starts_with("access-"))
        .unwrap()
        .into_iter()
        .map(|grant| grant.grant_id.as_str().to_owned())
        .collect()
}

async fn close(host: &ToolkitGatewayHost) {
    host.begin_close(GatewayHostCloseReason::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(15), host.join_close())
        .await
        .unwrap()
        .unwrap();
    host.release_native_custody().await.unwrap();
}

/// A host lost before its decision entered native issuance: the native owner
/// recorded no grant, and the lost host can no longer issue one. The next
/// host's cold start ends the decision as not issued, so a fresh approval of
/// the same decision is accepted and issues its grant once.
#[tokio::test]
async fn next_host_ends_an_unissued_pending_access_decision_for_a_fresh_approval() {
    let (lost, next, lost_answer) = lose_host_with_pending_access(Lost::BeforeIssuance).await;
    assert!(
        matches!(&lost_answer, Ok(observation)
            if observation.state == PrivateServiceGrantState::PendingReconciliation),
        "the lost host's decision stays pending: {}",
        answer(&lost_answer)
    );
    let (key, settled) = row_state(&next.store, &lost.scope).await;
    let mut fresh = lost.decision.clone();
    fresh.evidence.revision = EvidenceId::new("2").unwrap();
    fresh.evidence.sha256 = "b".repeat(64);
    let applied = next.host.apply_service_access_decision(fresh.clone()).await;
    assert!(
        settled == "not_issued"
            && matches!(&applied, Ok(observation)
                if observation.state == PrivateServiceGrantState::GrantIssued),
        "the next host left the lost host's decision {settled} and answered a fresh approval \
         of it {}",
        answer(&applied)
    );
    assert_eq!(row_state(&next.store, &lost.scope).await.1, "issued");
    // An exact retry of the fresh approval observes its grant.
    let replay = next.host.apply_service_access_decision(fresh).await;
    assert!(
        matches!(&replay, Ok(observation)
            if observation.state == PrivateServiceGrantState::GrantIssued),
        "{}",
        answer(&replay)
    );
    close(&next.host).await;
    assert_eq!(
        native_access_grants(&lost.grants_path),
        vec![format!("access-{key}")],
        "the native owner issued the decision's grant once"
    );
}

/// Two fresh approvals of the same request race to replace a decision the
/// next host ended as not issued: one replacement wins and issues the grant
/// once, and the other loses the compare-and-swap of the ended row and is
/// answered as a conflict.
#[tokio::test]
async fn concurrent_approvals_of_a_not_issued_decision_issue_once() {
    let (lost, next, lost_answer) = lose_host_with_pending_access(Lost::BeforeIssuance).await;
    assert!(
        matches!(&lost_answer, Ok(observation)
            if observation.state == PrivateServiceGrantState::PendingReconciliation),
        "the lost host's decision stays pending: {}",
        answer(&lost_answer)
    );
    let (key, settled) = row_state(&next.store, &lost.scope).await;
    assert_eq!(settled, "not_issued");
    let mut fresh = lost.decision.clone();
    fresh.evidence.revision = EvidenceId::new("2").unwrap();
    fresh.evidence.sha256 = "b".repeat(64);
    // The first approval's replacement is held before it reaches the store.
    next.store.arm_access_swap();
    let first = next.host.apply_service_access_decision(fresh.clone());
    tokio::pin!(first);
    tokio::select! {
        _ = next.store.entered.notified() => {}
        _ = &mut first => panic!("the first approval's replacement is held"),
    }
    let second = tokio::time::timeout(
        Duration::from_secs(15),
        next.host.apply_service_access_decision(fresh),
    )
    .await
    .unwrap();
    next.store.release.add_permits(1);
    let first = tokio::time::timeout(Duration::from_secs(15), &mut first)
        .await
        .unwrap();
    assert!(
        matches!(&second, Ok(observation)
            if observation.state == PrivateServiceGrantState::GrantIssued)
            && matches!(&first, Err(PrivateOriginalError::Conflict)),
        "the second approval was answered {} and the held first one {}",
        answer(&second),
        answer(&first)
    );
    assert_eq!(row_state(&next.store, &lost.scope).await.1, "issued");
    close(&next.host).await;
    assert_eq!(
        native_access_grants(&lost.grants_path),
        vec![format!("access-{key}")],
        "the native owner issued the decision's grant once"
    );
}

/// A host lost after the native owner recorded its decision's grant, before
/// the decision's row recorded it: the next host's cold start settles the
/// row as issued from the native record, and the decision observes its grant
/// without a second issuance.
#[tokio::test]
async fn next_host_settles_a_natively_issued_pending_access_decision() {
    let (lost, next, lost_answer) = lose_host_with_pending_access(Lost::AfterIssuance).await;
    assert!(
        matches!(&lost_answer, Ok(observation)
            if observation.state == PrivateServiceGrantState::PendingReconciliation),
        "the lost host could not record its grant: {}",
        answer(&lost_answer)
    );
    let (key, settled) = row_state(&next.store, &lost.scope).await;
    let journal = fs::read(&lost.grants_path).unwrap();
    let applied = next
        .host
        .apply_service_access_decision(lost.decision.clone())
        .await;
    assert!(
        settled == "issued"
            && matches!(&applied, Ok(observation)
                if observation.state == PrivateServiceGrantState::GrantIssued),
        "the next host left the lost host's decision {settled} and answered it {}",
        answer(&applied)
    );
    assert_eq!(
        fs::read(&lost.grants_path).unwrap(),
        journal,
        "observing the settled decision issues nothing"
    );
    close(&next.host).await;
    assert_eq!(
        native_access_grants(&lost.grants_path),
        vec![format!("access-{key}")],
        "the native owner holds the lost host's one grant"
    );
}

/// The pair opened through the prepared installation's opener, with the
/// store wrapper that can hold its access decision.
async fn open_prepared(
    selection: &HostBootstrapSelection,
    local: &CompiledLocalPolicy,
    scope: &StoreScope,
) -> (PreparedInstalledHost, Arc<PausedSourceStore>) {
    let selected = local.native_host.as_ref().unwrap();
    let policy = PreparedInstalledToolPolicy::from_canonical(
        &read_bundle_artifact(selection, &selected.application_tool_policy).unwrap(),
    )
    .unwrap();
    let captured = Arc::new(OnceLock::<Arc<PausedSourceStore>>::new());
    let capture = captured.clone();
    let scope = scope.clone();
    let prepared = PreparedInstallationOwner::open(
        selection.deployment_root.clone(),
        PreparedLocalPolicy {
            deployment: local.deployment.clone(),
            components: local.components.clone(),
            channels: local.channels.clone(),
            native_host: selected.preparation(),
        },
        operator_setup::OperatorOwnerStartup::Restore,
    )
    .unwrap()
    .complete_with_source_store(
        policy,
        Box::new(move |inner| {
            let store = Arc::new(PausedSourceStore::new(inner, scope));
            assert!(capture.set(store.clone()).is_ok());
            store
        }),
    )
    .await
    .unwrap();
    let store = captured.get().unwrap().clone();
    (prepared, store)
}

/// The prepared installation's opener settles a decision a lost host left
/// pending, as the installed host's opener does: the native owner recorded
/// the lost host's grant and its row did not, so the prepared pair's cold
/// start records the row as issued, and nothing is issued again.
#[tokio::test]
async fn prepared_installation_settles_a_lost_host_pending_access_decision() {
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let selected = local.native_host.as_ref().unwrap();
    let grants_path = state_directory(&selection, selected, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let scope = StoreScope {
        realm: selected.realm.to_string(),
        owner: local.deployment.to_string(),
    };
    let (key, prepared, store) = {
        let first = open_host(&selection, &scope).await;
        let mut decision = crate::private_originals::service_grants::tests::decision();
        decision.route = toolkit_package::Id::new("console").unwrap();
        first.store.arm_access_swap();
        let pending = first.host.apply_service_access_decision(decision);
        tokio::pin!(pending);
        tokio::select! {
            _ = first.store.entered.notified() => {}
            _ = &mut pending => panic!("the lost host's decision is held"),
        }
        lose(&first.host).await;
        let journal = fs::read(&grants_path).unwrap();
        // The prepared installation opens the pair while the lost host's
        // decision is still held.
        let (prepared, store) = open_prepared(&selection, &local, &scope).await;
        let (key, settled) = row_state(&store, &scope).await;
        assert_eq!(
            fs::read(&grants_path).unwrap(),
            journal,
            "the prepared installation's cold start issues nothing"
        );
        first.store.release.add_permits(1);
        let lost_answer = tokio::time::timeout(Duration::from_secs(15), &mut pending)
            .await
            .unwrap();
        assert!(
            settled == "issued"
                && matches!(&lost_answer, Ok(observation)
                    if observation.state == PrivateServiceGrantState::PendingReconciliation),
            "the prepared installation left the lost host's decision {settled}, and the lost \
             host answered {}",
            answer(&lost_answer)
        );
        (key, prepared, store)
    };
    assert_eq!(
        row_state(&store, &scope).await.1,
        "issued",
        "the lost host records nothing after the prepared installation settled its decision"
    );
    drop(store);
    drop(prepared);
    assert_eq!(
        native_access_grants(&grants_path),
        vec![format!("access-{key}")],
        "the native owner holds the lost host's one grant"
    );
}
