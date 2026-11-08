//! The private host's close and the native custody bound to its actual
//! lifetime: every close joins the work the host admitted before it, and no
//! join of the close ends that custody. The process's close releases it once
//! every join has returned; a host loss guard releases it at once.
use super::*;
use crate::original_ingress::ToolkitOriginalOwner;
use crate::original_ingress::tests::{
    cancellation_store_faults::CancelStore, initialized, principal, private_event,
    private_originals_with_store,
};
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    evidence::EvidenceId,
    work_association::{OriginalWorkRef, QualifiedIngressNamespace},
};
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
use meerkat_toolkit::host_lifetime::NativeHostLifetime;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use toolkit_store::{StoreError, StoreKey, StoreScope, ToolkitStore};

/// Stand-in until the lifetime's final custody release exists: it releases
/// nothing. The change that adds the release removes it.
trait FinalCustodyRelease {
    async fn release_native_custody(&self) -> Result<(), HostLifetimeError>;
}
impl FinalCustodyRelease for NativeHostLifetime {
    async fn release_native_custody(&self) -> Result<(), HostLifetimeError> {
        Ok(())
    }
}

/// A real selected lifetime serving the private host, with its grant owner
/// and originals store bound as its native custody, and one retained
/// original whose cancellation the store can hold in its write.
struct CustodyHost {
    _directory: tempfile::TempDir,
    source: Arc<ToolkitOriginalOwner>,
    store: Arc<CancelStore>,
    owner: Arc<NativeHostLifetime>,
    connection: PrivateHostConnection,
    commands: tokio::net::unix::pipe::Sender,
    responses: BufReader<tokio::net::unix::pipe::Receiver>,
    original: PrivateOriginalRef,
    namespace: QualifiedIngressNamespace,
    work: OriginalWorkRef,
}

async fn custody_host() -> CustodyHost {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let selected = native_selection(directory.path());
    let source = initialized(&directory.path().join("grants"));
    let store = CancelStore::new(&directory.path().join("originals"));
    let originals = private_originals_with_store(source.clone(), store.clone()).await;
    let original = originals
        .retain_event(private_event("close-custody"))
        .await
        .unwrap();
    let namespace = QualifiedIngressNamespace {
        realm: meerkat_core::RealmId::parse("test-realm").unwrap(),
        ingress: source.ingress_domain().clone(),
        occurrence_scope: EvidenceId::new("close-custody").unwrap(),
    };
    let work = OriginalWorkRef {
        authority: principal("source"),
        work: original.original.clone(),
    };
    let (pipes, command_file, response_file) = pipes();
    let mut commands = tokio::net::unix::pipe::Sender::from_file(command_file).unwrap();
    let mut responses =
        BufReader::new(tokio::net::unix::pipe::Receiver::from_file(response_file).unwrap());
    send(
        &mut commands,
        1,
        Command::Bootstrap {
            selection: selected,
        },
    )
    .await;
    let grants = source.grants();
    let sqlite = store.sqlite().clone();
    let (owner, _, connection) =
        PrivateHostConnection::bootstrap_with_originals(pipes, move |selection| async move {
            let mut owner = NativeHostLifetime::bootstrap(selection)?;
            owner.bind_native_grant_custody(&grants)?;
            owner.bind_native_store_custody(&sqlite)?;
            Ok((owner, originals))
        })
        .await
        .unwrap();
    let bootstrap = next_reply(&mut responses).await;
    assert_eq!(bootstrap["result"]["kind"], "bootstrap_accepted");
    CustodyHost {
        _directory: directory,
        source,
        store,
        owner,
        connection,
        commands,
        responses,
        original,
        namespace,
        work,
    }
}

async fn next_reply(
    responses: &mut BufReader<tokio::net::unix::pipe::Receiver>,
) -> serde_json::Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), responses.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

fn scope() -> StoreScope {
    StoreScope {
        realm: "test-realm".into(),
        owner: "originals".into(),
    }
}

/// Both custodies ended: the bound store refuses a read through the host's
/// own handle, and the bound grant owner refuses a lookup.
async fn assert_released(source: &ToolkitOriginalOwner, store: &CancelStore) {
    let absent = StoreKey {
        realm: "test-realm".into(),
        owner: "originals".into(),
        id: "absent".into(),
    };
    assert!(
        matches!(
            store.sqlite().get(&absent).await,
            Err(StoreError::OwnerUnavailable)
        ),
        "the final release ended the store's custody"
    );
    assert!(
        matches!(
            source
                .grants()
                .issued_reference(&EvidenceId::new("absent").unwrap()),
            Err(GrantRefusal::Unavailable)
        ),
        "the final release ended the grant owner's custody"
    );
}

/// The serve shutdown, here on parent loss, joins the cancellation the host
/// admitted before it: held in its store write across the shutdown, the
/// cancellation commits through the bound store and grant owner, which are
/// both still in custody after the serve drained. Only the final release
/// ends that custody.
#[tokio::test]
async fn serve_shutdown_keeps_native_custody_for_admitted_cancellation() {
    let CustodyHost {
        _directory,
        source,
        store,
        owner,
        connection,
        mut commands,
        responses,
        original,
        namespace,
        work,
    } = custody_host().await;
    store.hold_next_cancellation();
    send(
        &mut commands,
        2,
        Command::CancelOriginalOperation { original },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), store.wait_for_held_cancellation())
        .await
        .unwrap();
    drop(commands);
    drop(responses);
    tokio::time::timeout(Duration::from_secs(2), connection.wait_closed())
        .await
        .unwrap();
    let mut finishing = tokio::spawn(connection.finish());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut finishing)
            .await
            .is_err(),
        "the serve shutdown joins the admitted cancellation"
    );
    store.release_cancellation();
    tokio::time::timeout(Duration::from_secs(5), finishing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let rows = store.sqlite().list_records(&scope(), None, 10).await;
    assert!(
        matches!(
            &rows,
            Ok(rows) if rows.len() == 1 && rows[0].data.body["operation_cancelled"] == true
        ),
        "the admitted cancellation committed, and the store is still in custody once the serve drained"
    );
    assert!(
        matches!(
            source.original_disposition(&namespace, &work),
            Ok(OriginalDisposition::OperationCancelled { .. })
        ),
        "the grant owner observes the admitted cancellation"
    );
    owner.release_native_custody().await.unwrap();
    assert_released(&source, &store).await;
    store.join_cancellation().await;
}

/// The parent's close, BeginCloseActivation then JoinClose, settles the host
/// without ending its native custody: a cancellation the host admitted
/// before that close, held in its store write, commits after the join and
/// its reply reports the cancellation. Only the final release, after every
/// join of the host, ends the custody.
#[tokio::test]
async fn parent_join_close_keeps_native_custody_for_admitted_cancellation() {
    let CustodyHost {
        _directory,
        source,
        store,
        owner,
        connection,
        mut commands,
        mut responses,
        original,
        ..
    } = custody_host().await;
    store.hold_next_cancellation();
    send(
        &mut commands,
        2,
        Command::CancelOriginalOperation { original },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), store.wait_for_held_cancellation())
        .await
        .unwrap();
    send(
        &mut commands,
        3,
        Command::BeginCloseActivation {
            reason: HostCloseReason::Shutdown,
        },
    )
    .await;
    let begun = next_reply(&mut responses).await;
    assert_eq!(begun["id"], 3);
    assert_eq!(begun["result"]["kind"], "close");
    let handle: HostCloseHandle =
        serde_json::from_value(begun["result"]["observation"]["handle"].clone()).unwrap();
    send(&mut commands, 4, Command::JoinClose { handle }).await;
    let joined = next_reply(&mut responses).await;
    assert_eq!(joined["id"], 4);
    assert_eq!(joined["result"]["observation"]["state"], "settled");
    store.release_cancellation();
    let cancelled = next_reply(&mut responses).await;
    assert_eq!(cancelled["id"], 2);
    assert_eq!(
        cancelled["result"]["kind"], "original_operation_cancelled",
        "the cancellation admitted before the parent's close is not refused after its join: {cancelled}"
    );
    drop(commands);
    drop(responses);
    tokio::time::timeout(Duration::from_secs(5), connection.finish())
        .await
        .unwrap()
        .unwrap();
    owner.release_native_custody().await.unwrap();
    assert_released(&source, &store).await;
    store.join_cancellation().await;
}

/// On a host loss the custody release stays immediate: the loss guard's
/// detached close ends the native custody as soon as the close settles,
/// without waiting for admitted work, which then fails closed.
#[tokio::test]
async fn host_loss_guard_releases_native_custody_once_the_close_settles() {
    let probe = ProbeOwner::new(false);
    let owner = Arc::new(probe.clone());
    let (closed, _observed) = watch::channel(false);
    drop(HostLossGuard {
        owner: owner.clone(),
        closed: PrivateHostClosePublisher {
            closed,
            publication: None,
        },
        armed: true,
    });
    assert!(probe.lost.load(Ordering::Acquire));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        probe.released.load(Ordering::Acquire),
        0,
        "nothing is released before the close settles"
    );
    probe.settle();
    let released = tokio::time::timeout(Duration::from_secs(2), async {
        while probe.released.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        released.is_ok(),
        "the loss guard's close ends the native custody once it settles"
    );
}
