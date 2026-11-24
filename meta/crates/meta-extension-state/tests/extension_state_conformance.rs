#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use meta_extension_state::*;
use std::{collections::BTreeSet, sync::Arc};

fn principal(name: &str) -> Principal {
    Principal::Agent(name.into())
}
fn caller(name: &str) -> HostAccessContext {
    HostAccessContext::new(
        "realm",
        principal(name),
        VerifiedLineage::root(),
        HostPolicy::ALLOW,
    )
    .unwrap()
}
fn content(value: u8) -> DocumentContent {
    DocumentContent {
        title: "book".into(),
        content_type: "application/json".into(),
        schema_version: 1,
        payload: vec![value],
    }
}
fn request(id: &str) -> RequestIdentity {
    RequestIdentity::new(id, id.as_bytes()).unwrap()
}
fn service(path: &std::path::Path) -> DocumentService {
    DocumentService::new(
        "realm",
        "test",
        Arc::new(SqliteExtensionDocumentStore::open(path).unwrap()),
    )
    .unwrap()
}
async fn create(
    service: &DocumentService,
    caller: &HostAccessContext,
    id: &str,
) -> MutationReceipt {
    service
        .mutate(
            caller,
            &request(id),
            Mutation::Create(NewDocument {
                content: content(1),
                owner: None,
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt
}
fn descendant(name: &str, edges: &[(&str, EdgeKind, Option<Role>)]) -> HostAccessContext {
    let mut child = principal(name);
    let mut links = Vec::new();
    for (parent, kind, ceiling) in edges {
        links.push(LineageLink {
            child: child.clone(),
            parent: principal(parent),
            kind: *kind,
            ceiling: *ceiling,
        });
        child = principal(parent);
    }
    HostAccessContext::new(
        "realm",
        principal(name),
        VerifiedLineage::derived(links).unwrap(),
        HostPolicy::ALLOW,
    )
    .unwrap()
}

#[tokio::test]
async fn private_default_and_filtered_listing_hide_existence_and_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = service(&dir.path().join("docs.sqlite"));
    let owner = caller("owner");
    let stranger = caller("stranger");
    let book = create(&store, &owner, "create").await;
    assert!(matches!(
        store.get(&stranger, &book.document_id).await,
        Err(Error::NotFound)
    ));
    assert!(
        store
            .list(&stranger, ListRequest::default())
            .await
            .unwrap()
            .documents
            .is_empty()
    );
    for mutation in [
        Mutation::Replace {
            id: book.document_id.clone(),
            expected_revision: Revision("fake".into()),
            content: content(2),
        },
        Mutation::SetAccess {
            id: book.document_id.clone(),
            expected_revision: Revision("fake".into()),
            access: DocumentAccess::default(),
        },
        Mutation::Delete {
            id: book.document_id.clone(),
            expected_revision: Revision("fake".into()),
        },
    ] {
        assert_eq!(
            store
                .mutate(&stranger, &request("attack"), mutation)
                .await
                .unwrap_err(),
            Error::NotFound
        );
    }
    let other_namespace = DocumentService::new(
        "realm",
        "other",
        Arc::new(SqliteExtensionDocumentStore::open(dir.path().join("docs.sqlite")).unwrap()),
    )
    .unwrap();
    assert!(matches!(
        other_namespace.get(&owner, &book.document_id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn fork_reach_spawn_grants_ceilings_and_subtree_denial_are_current() {
    let dir = tempfile::tempdir().unwrap();
    let store = service(&dir.path().join("docs.sqlite"));
    let owner = caller("owner");
    let book = create(&store, &owner, "create").await;
    let fork = descendant("fork", &[("owner", EdgeKind::Fork, Some(Role::Editor))]);
    let child = descendant("child", &[("owner", EdgeKind::Spawn, Some(Role::Editor))]);
    let grand = descendant(
        "grand",
        &[
            ("fork", EdgeKind::Fork, Some(Role::Reader)),
            ("owner", EdgeKind::Fork, Some(Role::Editor)),
        ],
    );
    assert!(store.get(&fork, &book.document_id).await.is_ok());
    assert!(matches!(
        store.get(&child, &book.document_id).await,
        Err(Error::NotFound)
    ));
    assert!(store.get(&grand, &book.document_id).await.is_ok());
    assert_eq!(
        store
            .mutate(
                &grand,
                &request("edit"),
                Mutation::Replace {
                    id: book.document_id.clone(),
                    expected_revision: book.revision.clone(),
                    content: content(2)
                }
            )
            .await
            .unwrap_err(),
        Error::NotFound
    );
    assert_eq!(
        store
            .mutate(
                &fork,
                &request("share"),
                Mutation::SetAccess {
                    id: book.document_id.clone(),
                    expected_revision: book.revision.clone(),
                    access: DocumentAccess::default()
                }
            )
            .await
            .unwrap_err(),
        Error::NotFound
    );
    let access = DocumentAccess {
        owner_reach: Reach::Descendants,
        grants: vec![Grant {
            audience: Audience::Agent {
                principal: principal("grand"),
                reach: Reach::SelfOnly,
            },
            role: Role::Editor,
        }],
        denies: vec![Deny {
            audience: Audience::Agent {
                principal: principal("fork"),
                reach: Reach::Descendants,
            },
            operation: DeniedOperation::Read,
        }],
    };
    let MutationOutcome {
        receipt: changed, ..
    } = store
        .mutate(
            &owner,
            &request("access"),
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: book.revision,
                access,
            },
        )
        .await
        .unwrap();
    assert!(store.get(&child, &book.document_id).await.is_ok());
    assert!(matches!(
        store.get(&grand, &book.document_id).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.get(&fork, &book.document_id).await,
        Err(Error::NotFound)
    ));
    let private = DocumentAccess {
        owner_reach: Reach::SelfOnly,
        ..Default::default()
    };
    store
        .mutate(
            &owner,
            &request("revoke"),
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: changed.revision,
                access: private,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.get(&child, &book.document_id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn compare_and_swap_and_receipts_survive_restart_and_concurrent_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let a = service(&path);
    let b = service(&path);
    let owner = caller("owner");
    let book = create(&a, &owner, "create").await;
    let mutation = |value| Mutation::Replace {
        id: book.document_id.clone(),
        expected_revision: book.revision.clone(),
        content: content(value),
    };
    let req_a = request("first");
    let req_b = request("second");
    let (left, right) = tokio::join!(
        a.mutate(&owner, &req_a, mutation(2)),
        b.mutate(&owner, &req_b, mutation(3))
    );
    assert!(matches!(
        (&left, &right),
        (Ok(_), Err(Error::Conflict)) | (Err(Error::Conflict), Ok(_))
    ));
    let (winning_request, committed) = match (left, right) {
        (Ok(r), _) => (req_a, r.receipt),
        (_, Ok(r)) => (req_b, r.receipt),
        _ => unreachable!(),
    };
    drop(a);
    drop(b);
    let reopened = service(&path);
    assert_eq!(
        reopened
            .lookup_receipt(&owner, &winning_request)
            .await
            .unwrap(),
        Some(committed.clone())
    );
    // Original action retry returns before re-evaluating its stale revision or
    // accepting a recalculated payload supplied by a concurrent evaluator.
    assert_eq!(
        reopened
            .mutate(&owner, &winning_request, mutation(99))
            .await
            .unwrap()
            .receipt,
        committed
    );
    let changed = RequestIdentity::new(
        winning_request_id(&winning_request, &reopened, &owner).await,
        b"different original action",
    )
    .unwrap();
    assert_eq!(
        reopened.lookup_receipt(&owner, &changed).await.unwrap_err(),
        Error::RequestIdReused
    );
}

async fn winning_request_id(
    request_identity: &RequestIdentity,
    store: &DocumentService,
    owner: &HostAccessContext,
) -> &'static str {
    if store
        .lookup_receipt(owner, &request("first"))
        .await
        .unwrap()
        == store.lookup_receipt(owner, request_identity).await.unwrap()
    {
        "first"
    } else {
        "second"
    }
}

#[tokio::test]
async fn duplicate_create_and_delete_are_exactly_once_and_revocation_hides_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let a = service(&path);
    let b = service(&path);
    let owner = caller("owner");
    let (left, right) = tokio::join!(create(&a, &owner, "same"), create(&b, &owner, "same"));
    assert_eq!(left, right);
    assert_eq!(
        a.list(&owner, ListRequest::default())
            .await
            .unwrap()
            .documents
            .len(),
        1
    );
    let MutationOutcome {
        receipt: granted, ..
    } = a
        .mutate(
            &owner,
            &request("grant"),
            Mutation::SetAccess {
                id: left.document_id.clone(),
                expected_revision: left.revision.clone(),
                access: DocumentAccess {
                    grants: vec![Grant {
                        audience: Audience::Agent {
                            principal: principal("editor"),
                            reach: Reach::Forks,
                        },
                        role: Role::Editor,
                    }],
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    let editor = caller("editor");
    let MutationOutcome {
        receipt: edited, ..
    } = a
        .mutate(
            &editor,
            &request("edit"),
            Mutation::Replace {
                id: left.document_id.clone(),
                expected_revision: granted.revision,
                content: content(2),
            },
        )
        .await
        .unwrap();
    let MutationOutcome {
        receipt: revoked, ..
    } = a
        .mutate(
            &owner,
            &request("revoke"),
            Mutation::SetAccess {
                id: left.document_id.clone(),
                expected_revision: edited.revision,
                access: DocumentAccess::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        a.lookup_receipt(&editor, &request("edit"))
            .await
            .unwrap_err(),
        Error::NotFound
    );
    let MutationOutcome {
        receipt: deleted, ..
    } = a
        .mutate(
            &owner,
            &request("delete"),
            Mutation::Delete {
                id: left.document_id.clone(),
                expected_revision: revoked.revision,
            },
        )
        .await
        .unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (metadata, payload): (String, Vec<u8>) = connection
        .query_row(
            "SELECT metadata,payload FROM extension_documents WHERE id=?1",
            [&left.document_id.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(!metadata.contains("book"));
    assert!(payload.is_empty());
    drop(a);
    drop(b);
    let reopened = service(&path);
    assert_eq!(
        reopened
            .lookup_receipt(&owner, &request("delete"))
            .await
            .unwrap(),
        Some(deleted)
    );
    assert!(matches!(
        reopened.get(&owner, &left.document_id).await,
        Err(Error::NotFound)
    ));
    let new = create(&reopened, &owner, "new").await;
    assert_ne!(new.document_id, left.document_id);
}

#[tokio::test]
async fn owner_transfer_requires_attested_target_and_worker_names_do_not_confer_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let store = service(&dir.path().join("docs.sqlite"));
    let owner = caller("owner");
    let book = create(&store, &owner, "create").await;
    let target = Owner::Agent(principal("new"));
    let transfer = || Mutation::Transfer {
        id: book.document_id.clone(),
        expected_revision: book.revision.clone(),
        owner: target.clone(),
        access: None,
    };
    assert_eq!(
        store
            .mutate(&owner, &request("transfer"), transfer())
            .await
            .unwrap_err(),
        Error::NotFound
    );
    let owner = owner.with_valid_owners(BTreeSet::from([target.clone()]));
    store
        .mutate(&owner, &request("transfer"), transfer())
        .await
        .unwrap();
    assert!(matches!(
        store.get(&owner, &book.document_id).await,
        Err(Error::NotFound)
    ));
    assert!(store.get(&caller("new"), &book.document_id).await.is_ok());
    let worker = |creation| {
        HostAccessContext::new(
            "realm",
            Principal::Worker {
                mob_id: "mob".into(),
                member_id: "same-name".into(),
                creation_id: creation,
            },
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .unwrap()
    };
    let old = worker("old".into());
    let own = create(&store, &old, "worker").await;
    assert!(matches!(
        store.get(&worker("new".into()), &own.document_id).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn policy_read_denial_cannot_create_and_mob_membership_is_not_management() {
    let dir = tempfile::tempdir().unwrap();
    let store = service(&dir.path().join("docs.sqlite"));
    let denied = HostAccessContext::new(
        "realm",
        principal("a"),
        VerifiedLineage::root(),
        HostPolicy {
            read: false,
            edit: true,
            manage: true,
        },
    )
    .unwrap();
    let new = |owner| {
        Mutation::Create(NewDocument {
            content: content(0),
            owner,
            access: DocumentAccess::default(),
        })
    };
    assert_eq!(
        store
            .mutate(&denied, &request("deny"), new(None))
            .await
            .unwrap_err(),
        Error::NotFound
    );
    let manager = caller("manager")
        .with_memberships(
            BTreeSet::from(["mob".into()]),
            BTreeSet::from(["mob".into()]),
            false,
        )
        .unwrap();
    let member = caller("member")
        .with_memberships(BTreeSet::from(["mob".into()]), BTreeSet::new(), false)
        .unwrap();
    let MutationOutcome { receipt: book, .. } = store
        .mutate(
            &manager,
            &request("mob"),
            new(Some(Owner::Mob("mob".into()))),
        )
        .await
        .unwrap();
    assert!(matches!(
        store.get(&member, &book.document_id).await,
        Err(Error::NotFound)
    ));
    assert!(store.get(&manager, &book.document_id).await.is_ok());
}

#[test]
fn lineage_rejects_cycles_discontinuity_and_wrong_current_principal() {
    let link = LineageLink {
        child: principal("a"),
        parent: principal("b"),
        kind: EdgeKind::Fork,
        ceiling: Some(Role::Editor),
    };
    let cycle = LineageLink {
        child: principal("b"),
        parent: principal("a"),
        ..link
    };
    assert!(VerifiedLineage::derived(vec![link.clone(), cycle]).is_err());
    let lineage = VerifiedLineage::derived(vec![link]).unwrap();
    assert!(HostAccessContext::new("realm", principal("c"), lineage, HostPolicy::ALLOW).is_err());
}

#[tokio::test]
async fn concurrent_identical_requests_report_one_commit_and_one_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let a = service(&path);
    let b = service(&path);
    let owner = caller("owner");
    let original = request("same-create");
    let mutation = || {
        Mutation::Create(NewDocument {
            content: content(7),
            owner: None,
            access: DocumentAccess::default(),
        })
    };
    assert!(a.lookup_receipt(&owner, &original).await.unwrap().is_none());
    assert!(b.lookup_receipt(&owner, &original).await.unwrap().is_none());
    let (left, right) = tokio::join!(
        a.mutate(&owner, &original, mutation()),
        b.mutate(&owner, &original, mutation())
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.receipt, right.receipt);
    assert_ne!(left.replayed, right.replayed);
    assert_eq!(
        a.get(&owner, &left.receipt.document_id)
            .await
            .unwrap()
            .content
            .payload,
        vec![7]
    );
}

#[tokio::test]
async fn acl_revocation_and_content_edit_share_one_atomic_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let a = service(&path);
    let b = service(&path);
    let owner = caller("owner");
    let editor = caller("editor");
    let book = create(&a, &owner, "create").await;
    let shared = a
        .mutate(
            &owner,
            &request("share"),
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: book.revision,
                access: DocumentAccess {
                    grants: vec![Grant {
                        audience: Audience::Agent {
                            principal: principal("editor"),
                            reach: Reach::Forks,
                        },
                        role: Role::Editor,
                    }],
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap()
        .receipt;
    let revoke = request("revoke");
    let edit = request("edit");
    let (revoked, edited) = tokio::join!(
        a.mutate(
            &owner,
            &revoke,
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: shared.revision.clone(),
                access: DocumentAccess::default(),
            }
        ),
        b.mutate(
            &editor,
            &edit,
            Mutation::Replace {
                id: book.document_id.clone(),
                expected_revision: shared.revision.clone(),
                content: content(2),
            }
        )
    );
    match (revoked, edited) {
        (Ok(_), Err(Error::NotFound)) => {
            assert!(matches!(
                a.get(&editor, &book.document_id).await,
                Err(Error::NotFound)
            ));
            assert_eq!(
                a.get(&owner, &book.document_id)
                    .await
                    .unwrap()
                    .content
                    .payload,
                vec![1]
            );
        }
        (Err(Error::Conflict), Ok(_)) => {
            assert_eq!(
                a.get(&owner, &book.document_id)
                    .await
                    .unwrap()
                    .content
                    .payload,
                vec![2]
            );
        }
        result => panic!("atomic order violated: {result:?}"),
    }
}

/// A host asks the store whether any live document of a realm, in any
/// namespace, names a mob: as its owner, in a grant or in a deny. A deleted
/// document, another realm and an unnamed mob do not count. A row that does
/// not decode fails the answer instead of being skipped; only a reference
/// already found answers before it.
#[tokio::test]
async fn mob_references_cover_every_namespace_owner_and_audience_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let store = Arc::new(SqliteExtensionDocumentStore::open(&path).unwrap());
    let in_namespace = |namespace: &str| {
        DocumentService::new(
            "realm",
            namespace,
            Arc::clone(&store) as Arc<dyn ExtensionDocumentStore>,
        )
        .unwrap()
    };
    let manager = caller("manager")
        .with_memberships(BTreeSet::new(), BTreeSet::from(["owned".into()]), false)
        .unwrap();
    assert!(!store.references_mob("realm", "owned").await.unwrap());

    let create = |owner: Option<Owner>, access: DocumentAccess| {
        Mutation::Create(NewDocument {
            content: content(1),
            owner,
            access,
        })
    };
    in_namespace("one")
        .mutate(
            &manager,
            &request("owned"),
            create(Some(Owner::Mob("owned".into())), DocumentAccess::default()),
        )
        .await
        .unwrap();
    let granted = in_namespace("two")
        .mutate(
            &manager,
            &request("granted"),
            create(
                None,
                DocumentAccess {
                    grants: vec![Grant {
                        audience: Audience::Mob("granted".into()),
                        role: Role::Reader,
                    }],
                    ..DocumentAccess::default()
                },
            ),
        )
        .await
        .unwrap()
        .receipt;
    in_namespace("three")
        .mutate(
            &manager,
            &request("denied"),
            create(
                None,
                DocumentAccess {
                    denies: vec![Deny {
                        audience: Audience::Mob("denied".into()),
                        operation: DeniedOperation::Read,
                    }],
                    ..DocumentAccess::default()
                },
            ),
        )
        .await
        .unwrap();
    for named in ["owned", "granted", "denied"] {
        assert!(
            store.references_mob("realm", named).await.unwrap(),
            "{named}"
        );
    }
    assert!(!store.references_mob("realm", "unnamed").await.unwrap());
    assert!(
        !store
            .references_mob("another-realm", "owned")
            .await
            .unwrap()
    );

    in_namespace("two")
        .mutate(
            &manager,
            &request("delete-granted"),
            Mutation::Delete {
                id: granted.document_id.clone(),
                expected_revision: granted.revision.clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        !store.references_mob("realm", "granted").await.unwrap(),
        "a deleted document names no mob"
    );

    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE extension_documents SET metadata = '{' WHERE namespace = 'three'",
            [],
        )
        .unwrap();
    assert!(matches!(
        store.references_mob("realm", "unnamed").await,
        Err(Error::Storage(_))
    ));
}

// Member-keyed principals: two mobs' identities of one name.
//
// A host keeps the documents of all its mobs in one realm, and two mobs may
// each have an application identity of the same name. The default mob's
// identity is stored as it always was (`{"kind":"agent","value":NAME}`); an
// identity of another mob carries its mob as a value of its own
// (`{"kind":"mob_agent","value":{"mob_id":MOB,"identity":NAME}}`). The rows
// build the second form from its stored bytes: the bytes are the contract a
// store keeps and an earlier build meets.

/// The stored form of the identity `identity` of the mob `mob_id`, a mob
/// other than the host's default mob.
fn mob_agent_json(mob_id: &str, identity: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "mob_agent",
        "value": { "mob_id": mob_id, "identity": identity },
    })
}
/// The stored form of the default mob's identity `identity`.
fn agent_json(identity: &str) -> serde_json::Value {
    serde_json::json!({ "kind": "agent", "value": identity })
}
/// The identity `identity` of the mob `mob_id`, read from its stored form.
fn mob_agent(mob_id: &str, identity: &str) -> Principal {
    serde_json::from_value(mob_agent_json(mob_id, identity))
        .expect("the stored form of another mob's identity decodes")
}
fn caller_as(principal: Principal) -> HostAccessContext {
    HostAccessContext::new(
        "realm",
        principal,
        VerifiedLineage::root(),
        HostPolicy::ALLOW,
    )
    .unwrap()
}
fn agent_audience(principal: Principal) -> Audience {
    Audience::Agent {
        principal,
        reach: Reach::SelfOnly,
    }
}
/// The `(principal, request id, document id)` of every stored receipt.
fn stored_receipts(path: &std::path::Path) -> Vec<(serde_json::Value, String, String)> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT principal, request_id, document_id FROM extension_receipts \
             ORDER BY principal, request_id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(|row| {
            let (principal, request, document) = row.unwrap();
            (serde_json::from_str(&principal).unwrap(), request, document)
        })
        .collect()
}
/// The stored owner of the document `id`.
fn stored_owner(path: &std::path::Path, id: &DocumentId) -> serde_json::Value {
    let metadata: String = rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT metadata FROM extension_documents WHERE id = ?1",
            [&id.0],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str::<serde_json::Value>(&metadata).unwrap()["owner"].clone()
}
/// Every table and index of the store, with its definition.
fn stored_schema(path: &std::path::Path) -> Vec<(String, String, Option<String>)> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}
async fn listed(service: &DocumentService, caller: &HostAccessContext) -> Vec<DocumentId> {
    service
        .list(caller, ListRequest::default())
        .await
        .unwrap()
        .documents
        .into_iter()
        .map(|document| document.id)
        .collect()
}

/// Two mobs' identities of one name are two principals: each owns, reads,
/// lists and manages its own documents, and a grant or a deny that names
/// one reaches only that one.
#[tokio::test]
async fn member_keyed_two_mobs_agents_of_one_name_keep_their_own_documents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let service = service(&path);
    let default_alice = caller("alice");
    let second_alice = caller_as(mob_agent("second", "alice"));

    let default_book = create(&service, &default_alice, "default-create").await;
    let second_book = create(&service, &second_alice, "second-create").await;
    assert_eq!(
        stored_owner(&path, &default_book.document_id),
        serde_json::json!({ "kind": "agent", "value": agent_json("alice") })
    );
    assert_eq!(
        stored_owner(&path, &second_book.document_id),
        serde_json::json!({ "kind": "agent", "value": mob_agent_json("second", "alice") })
    );

    // Each reads and lists its own document only.
    assert!(
        service
            .get(&default_alice, &default_book.document_id)
            .await
            .is_ok()
    );
    assert!(
        service
            .get(&second_alice, &second_book.document_id)
            .await
            .is_ok()
    );
    assert_eq!(
        service
            .get(&default_alice, &second_book.document_id)
            .await
            .unwrap_err(),
        Error::NotFound
    );
    assert_eq!(
        service
            .get(&second_alice, &default_book.document_id)
            .await
            .unwrap_err(),
        Error::NotFound
    );
    assert_eq!(
        listed(&service, &default_alice).await,
        vec![default_book.document_id.clone()]
    );
    assert_eq!(
        listed(&service, &second_alice).await,
        vec![second_book.document_id.clone()]
    );

    // Neither manages the other's document.
    assert_eq!(
        service
            .mutate(
                &second_alice,
                &request("second-deletes-default"),
                Mutation::Delete {
                    id: default_book.document_id.clone(),
                    expected_revision: default_book.revision.clone(),
                },
            )
            .await
            .unwrap_err(),
        Error::NotFound
    );
    assert_eq!(
        service
            .mutate(
                &default_alice,
                &request("default-shares-second"),
                Mutation::SetAccess {
                    id: second_book.document_id.clone(),
                    expected_revision: second_book.revision.clone(),
                    access: DocumentAccess::default(),
                },
            )
            .await
            .unwrap_err(),
        Error::NotFound
    );

    // A grant that names one of them reaches only that one, and a deny that
    // names one of them leaves the other's grant standing.
    let carol = caller("carol");
    let shared = service
        .mutate(
            &carol,
            &request("carol-create"),
            Mutation::Create(NewDocument {
                content: content(1),
                owner: None,
                access: DocumentAccess {
                    grants: vec![Grant {
                        audience: agent_audience(mob_agent("second", "alice")),
                        role: Role::Reader,
                    }],
                    ..DocumentAccess::default()
                },
            }),
        )
        .await
        .unwrap()
        .receipt;
    assert!(
        service
            .get(&second_alice, &shared.document_id)
            .await
            .is_ok()
    );
    assert_eq!(
        service
            .get(&default_alice, &shared.document_id)
            .await
            .unwrap_err(),
        Error::NotFound,
        "a grant to another mob's alice is no grant to the default mob's"
    );
    let regranted = service
        .mutate(
            &carol,
            &request("carol-regrant"),
            Mutation::SetAccess {
                id: shared.document_id.clone(),
                expected_revision: shared.revision.clone(),
                access: DocumentAccess {
                    grants: vec![
                        Grant {
                            audience: agent_audience(principal("alice")),
                            role: Role::Reader,
                        },
                        Grant {
                            audience: agent_audience(mob_agent("second", "alice")),
                            role: Role::Reader,
                        },
                    ],
                    denies: vec![Deny {
                        audience: agent_audience(mob_agent("second", "alice")),
                        operation: DeniedOperation::Read,
                    }],
                    ..DocumentAccess::default()
                },
            },
        )
        .await
        .unwrap()
        .receipt;
    assert_eq!(regranted.document_id, shared.document_id);
    assert!(
        service
            .get(&default_alice, &shared.document_id)
            .await
            .is_ok(),
        "a deny on another mob's alice does not deny the default mob's"
    );
    assert_eq!(
        service
            .get(&second_alice, &shared.document_id)
            .await
            .unwrap_err(),
        Error::NotFound
    );
}

/// A request id is one identity's: two mobs' identities of one name each
/// commit under the same request id, neither replays the other's receipt,
/// and the same id with another request is no reuse across them.
#[tokio::test]
async fn member_keyed_two_mobs_agents_of_one_name_keep_their_own_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let service = service(&path);
    let default_alice = caller("alice");
    let second_alice = caller_as(mob_agent("second", "alice"));
    let new = || {
        Mutation::Create(NewDocument {
            content: content(1),
            owner: None,
            access: DocumentAccess::default(),
        })
    };

    // The same request id and the same original request.
    let first = service
        .mutate(&default_alice, &request("shared-id"), new())
        .await
        .unwrap();
    let second = service
        .mutate(&second_alice, &request("shared-id"), new())
        .await
        .unwrap();
    assert!(!first.replayed);
    assert!(
        !second.replayed,
        "another mob's alice commits her own request, she replays nobody's"
    );
    assert_ne!(first.receipt.document_id, second.receipt.document_id);
    assert_eq!(
        service
            .lookup_receipt(&default_alice, &request("shared-id"))
            .await
            .unwrap(),
        Some(first.receipt.clone())
    );
    assert_eq!(
        service
            .lookup_receipt(&second_alice, &request("shared-id"))
            .await
            .unwrap(),
        Some(second.receipt.clone())
    );
    // Each replays her own.
    let replay = service
        .mutate(&second_alice, &request("shared-id"), new())
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, second.receipt);

    // The same request id with different original requests.
    let other = |original: &[u8]| RequestIdentity::new("other-id", original).unwrap();
    service
        .mutate(&default_alice, &other(b"the default mob's request"), new())
        .await
        .unwrap();
    let outcome = service
        .mutate(&second_alice, &other(b"another mob's request"), new())
        .await
        .expect("an id the default mob's alice used is not used by another mob's");
    assert!(!outcome.replayed);

    assert_eq!(
        stored_receipts(&path)
            .into_iter()
            .map(|(principal, request, _)| (principal, request))
            .collect::<Vec<_>>(),
        vec![
            (agent_json("alice"), "other-id".to_owned()),
            (agent_json("alice"), "shared-id".to_owned()),
            (mob_agent_json("second", "alice"), "other-id".to_owned()),
            (mob_agent_json("second", "alice"), "shared-id".to_owned()),
        ],
        "the receipt key carries the mob in the stored principal"
    );
}

/// The rows of a default mob's identity keep the bytes they always had, and
/// writing them moves no schema: a store that only the default mob's
/// identities wrote is the store an earlier build wrote.
#[tokio::test]
async fn member_keyed_a_default_mob_agents_rows_keep_their_stored_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let service = service(&path);
    let schema = stored_schema(&path);
    let book = create(&service, &caller("alice"), "create").await;

    assert_eq!(
        stored_owner(&path, &book.document_id),
        serde_json::json!({ "kind": "agent", "value": { "kind": "agent", "value": "alice" } })
    );
    let principal: String = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT principal FROM extension_receipts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(principal, r#"{"kind":"agent","value":"alice"}"#);
    assert_eq!(stored_schema(&path), schema);
    let tables: Vec<_> = schema
        .iter()
        .filter(|(kind, _, _)| kind == "table")
        .map(|(_, name, _)| name.as_str())
        .collect();
    assert_eq!(
        tables,
        [
            "extension_documents",
            "extension_mob_holders",
            "extension_receipts"
        ]
    );
    let receipts = schema
        .iter()
        .find(|(_, name, _)| name == "extension_receipts")
        .and_then(|(_, _, sql)| sql.clone())
        .unwrap();
    assert!(
        receipts.contains("PRIMARY KEY(realm, namespace, principal, request_id)"),
        "{receipts}"
    );
}

/// The first write of another mob's identity adds a row like any other
/// write: no table, column or index is added or changed.
#[tokio::test]
async fn member_keyed_another_mobs_agent_moves_no_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let service = service(&path);
    create(&service, &caller("alice"), "default-create").await;
    let schema = stored_schema(&path);
    let user_version = |path: &std::path::Path| -> i64 {
        rusqlite::Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    };
    let version = user_version(&path);

    create(
        &service,
        &caller_as(mob_agent("second", "alice")),
        "second-create",
    )
    .await;
    assert_eq!(stored_schema(&path), schema);
    assert_eq!(user_version(&path), version);
}

/// A document and a receipt stored before principals carried a mob are the
/// default mob's: its identity reads the document and replays the receipt,
/// and another mob's identity of the same name finds neither. The rows are
/// planted in the bytes an earlier build wrote
/// (`member_keyed_a_default_mob_agents_rows_keep_their_stored_bytes` pins
/// them), not written by this build.
#[tokio::test]
async fn member_keyed_a_document_and_a_receipt_stored_before_are_the_default_mobs() {
    use sha2::Digest as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let service = service(&path);
    let original = b"the original request";
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO extension_documents(realm,namespace,id,metadata,payload,deleted) \
                 VALUES ('realm','test','earlier-document',?1,?2,0)",
                rusqlite::params![
                    r#"{"id":"earlier-document","revision":"earlier-revision","content":{"title":"book","content_type":"application/json","schema_version":1,"payload":[]},"owner":{"kind":"agent","value":{"kind":"agent","value":"alice"}},"access":{"owner_reach":"forks","grants":[],"denies":[]},"created_at_ms":1,"updated_at_ms":1}"#,
                    vec![7_u8],
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO extension_receipts(realm,namespace,principal,request_id,fingerprint,\
                 receipt,document_id,operation) \
                 VALUES ('realm','test',?1,'earlier-request',?2,?3,'earlier-document','\"Manage\"')",
                rusqlite::params![
                    r#"{"kind":"agent","value":"alice"}"#,
                    sha2::Sha256::digest(original).to_vec(),
                    r#"{"document_id":"earlier-document","revision":"earlier-revision","deleted":false}"#,
                ],
            )
            .unwrap();
    }
    let id = DocumentId("earlier-document".into());
    let earlier_request = RequestIdentity::new("earlier-request", original).unwrap();
    let earlier_receipt = MutationReceipt {
        document_id: id.clone(),
        revision: Revision("earlier-revision".into()),
        deleted: false,
    };
    let new = || {
        Mutation::Create(NewDocument {
            content: content(1),
            owner: None,
            access: DocumentAccess::default(),
        })
    };

    let default_alice = caller("alice");
    assert_eq!(
        service
            .get(&default_alice, &id)
            .await
            .unwrap()
            .content
            .payload,
        vec![7]
    );
    assert_eq!(
        service
            .lookup_receipt(&default_alice, &earlier_request)
            .await
            .unwrap(),
        Some(earlier_receipt.clone())
    );
    let replay = service
        .mutate(&default_alice, &earlier_request, new())
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, earlier_receipt);

    let second_alice = caller_as(mob_agent("second", "alice"));
    assert_eq!(
        service.get(&second_alice, &id).await.unwrap_err(),
        Error::NotFound
    );
    assert_eq!(
        service
            .lookup_receipt(&second_alice, &earlier_request)
            .await
            .unwrap(),
        None
    );
    let fresh = service
        .mutate(&second_alice, &earlier_request, new())
        .await
        .unwrap();
    assert!(
        !fresh.replayed,
        "another mob's alice replays no earlier receipt"
    );
    assert_ne!(fresh.receipt.document_id, id);
}

/// The principal an earlier build decodes: the two forms it knows, and
/// nothing else.
#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[allow(dead_code)]
enum EarlierPrincipal {
    Agent(String),
    Worker {
        mob_id: String,
        member_id: String,
        creation_id: String,
    },
}
/// The owner an earlier build decodes.
#[derive(Debug, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[allow(dead_code)]
enum EarlierOwner {
    Agent(EarlierPrincipal),
    Mob(String),
    Realm,
}

/// What an earlier build reads in a store this build wrote. A document of
/// the default mob's identity decodes as it always did. A document that
/// names another mob's identity does not decode: the earlier build fails on
/// it, and never reads it as the default mob's identity of that name. Its
/// receipt lookup, keyed by the default mob's stored principal, finds the
/// default mob's receipt only.
#[tokio::test]
async fn member_keyed_an_earlier_build_refuses_another_mobs_agent_and_reads_the_default_mobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let service = service(&path);
    let default_book = create(&service, &caller("alice"), "shared-id").await;
    let second_book = create(
        &service,
        &caller_as(mob_agent("second", "alice")),
        "shared-id",
    )
    .await;

    let default_owner: EarlierOwner =
        serde_json::from_value(stored_owner(&path, &default_book.document_id))
            .expect("an earlier build decodes the default mob's owner");
    assert!(matches!(
        default_owner,
        EarlierOwner::Agent(EarlierPrincipal::Agent(identity)) if identity == "alice"
    ));
    let refused =
        serde_json::from_value::<EarlierOwner>(stored_owner(&path, &second_book.document_id))
            .expect_err("an earlier build does not decode another mob's identity");
    assert!(
        refused.to_string().contains("unknown variant `mob_agent`"),
        "{refused}"
    );

    // The earlier build's receipt key for alice is the default mob's bytes.
    let found: Vec<String> = {
        let connection = rusqlite::Connection::open(&path).unwrap();
        let mut statement = connection
            .prepare(
                "SELECT document_id FROM extension_receipts WHERE realm='realm' \
                 AND namespace='test' AND principal=?1 AND request_id='shared-id'",
            )
            .unwrap();
        statement
            .query_map([r#"{"kind":"agent","value":"alice"}"#], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    };
    assert_eq!(found, vec![default_book.document_id.0.clone()]);
}

/// A document names the mob of another mob's identity that owns it or that
/// a grant or a deny names, by the mob's id alone: a new mob under that id
/// would have an identity of the same name. So the mob reference check
/// counts it, until the document is deleted. A default mob's identity names
/// no mob.
#[tokio::test]
async fn member_keyed_mob_references_count_the_mob_of_another_mobs_agent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite");
    let store = Arc::new(SqliteExtensionDocumentStore::open(&path).unwrap());
    let service = DocumentService::new(
        "realm",
        "test",
        Arc::clone(&store) as Arc<dyn ExtensionDocumentStore>,
    )
    .unwrap();
    for mob_id in ["owning", "granted", "denied", "alice"] {
        assert!(
            !store.references_mob("realm", mob_id).await.unwrap(),
            "{mob_id}"
        );
    }

    let owned = create(&service, &caller_as(mob_agent("owning", "alice")), "owned").await;
    let shared = service
        .mutate(
            &caller("alice"),
            &request("shared"),
            Mutation::Create(NewDocument {
                content: content(1),
                owner: None,
                access: DocumentAccess {
                    grants: vec![Grant {
                        audience: agent_audience(mob_agent("granted", "bob")),
                        role: Role::Reader,
                    }],
                    denies: vec![Deny {
                        audience: agent_audience(mob_agent("denied", "eve")),
                        operation: DeniedOperation::Read,
                    }],
                    ..DocumentAccess::default()
                },
            }),
        )
        .await
        .unwrap()
        .receipt;
    for mob_id in ["owning", "granted", "denied"] {
        assert!(
            store.references_mob("realm", mob_id).await.unwrap(),
            "{mob_id}"
        );
    }
    assert!(
        !store.references_mob("realm", "alice").await.unwrap(),
        "the default mob's identity names no mob"
    );
    let document = service
        .get(&caller_as(mob_agent("owning", "alice")), &owned.document_id)
        .await
        .unwrap();
    assert!(document.names_mob("owning"));
    assert!(!document.names_mob("granted"));

    service
        .mutate(
            &caller("alice"),
            &request("delete-shared"),
            Mutation::Delete {
                id: shared.document_id,
                expected_revision: shared.revision,
            },
        )
        .await
        .unwrap();
    assert!(!store.references_mob("realm", "granted").await.unwrap());
    assert!(!store.references_mob("realm", "denied").await.unwrap());
    assert!(store.references_mob("realm", "owning").await.unwrap());
}

/// A caller that is another mob's identity, and an owner it was validated
/// for that is one, name that mob in the context's authority: a host that
/// orders changes of which mob holds an id checks it.
#[test]
fn member_keyed_a_context_of_another_mobs_agent_names_its_mob() {
    let caller = caller_as(mob_agent("second", "alice"))
        .with_valid_owners(BTreeSet::from([Owner::Agent(mob_agent("third", "bob"))]));
    assert_eq!(
        caller.authority_mob_ids(),
        BTreeSet::from(["second", "third"])
    );
    assert_eq!(
        self::caller("alice").authority_mob_ids(),
        BTreeSet::new(),
        "the default mob's identity names no mob"
    );
}

/// [`Principal::agent`] spells an identity from its mob as a record stores
/// it (none for the default mob), and `stored_form` reads what a caller of
/// the default mob names: an identity that names the default mob by its id
/// is the default mob's form, in a principal, an owner and every owner and
/// audience a mutation names. No other principal, owner or audience changes.
#[test]
fn member_keyed_the_default_mobs_identity_has_one_stored_form() {
    assert_eq!(Principal::agent(None, "alice"), principal("alice"));
    assert_eq!(
        Principal::agent(Some("second"), "alice"),
        mob_agent("second", "alice")
    );

    let by_id = || mob_agent("launch", "alice");
    let worker = || Principal::Worker {
        mob_id: "launch".into(),
        member_id: "alice".into(),
        creation_id: "creation".into(),
    };
    assert_eq!(by_id().stored_form("launch", "launch"), principal("alice"));
    assert_eq!(by_id().stored_form("second", "second"), by_id());
    assert_eq!(
        principal("alice").stored_form("launch", "launch"),
        principal("alice")
    );
    assert_eq!(worker().stored_form("launch", "launch"), worker());

    assert_eq!(
        Owner::Agent(by_id()).stored_form("launch", "launch"),
        Owner::Agent(principal("alice"))
    );
    assert_eq!(
        Owner::Mob("launch".into()).stored_form("launch", "launch"),
        Owner::Mob("launch".into())
    );
    assert_eq!(Owner::Realm.stored_form("launch", "launch"), Owner::Realm);

    let access = || DocumentAccess {
        owner_reach: Reach::Descendants,
        grants: vec![
            Grant {
                audience: Audience::Agent {
                    principal: by_id(),
                    reach: Reach::Descendants,
                },
                role: Role::Editor,
            },
            Grant {
                audience: Audience::Mob("launch".into()),
                role: Role::Reader,
            },
        ],
        denies: vec![
            Deny {
                audience: Audience::Agent {
                    principal: by_id(),
                    reach: Reach::SelfOnly,
                },
                operation: DeniedOperation::Edit,
            },
            Deny {
                audience: Audience::Realm,
                operation: DeniedOperation::Read,
            },
        ],
    };
    let folded = || DocumentAccess {
        owner_reach: Reach::Descendants,
        grants: vec![
            Grant {
                audience: Audience::Agent {
                    principal: principal("alice"),
                    reach: Reach::Descendants,
                },
                role: Role::Editor,
            },
            Grant {
                audience: Audience::Mob("launch".into()),
                role: Role::Reader,
            },
        ],
        denies: vec![
            Deny {
                audience: Audience::Agent {
                    principal: principal("alice"),
                    reach: Reach::SelfOnly,
                },
                operation: DeniedOperation::Edit,
            },
            Deny {
                audience: Audience::Realm,
                operation: DeniedOperation::Read,
            },
        ],
    };
    assert_eq!(access().stored_form("launch", "launch"), folded());
    assert_eq!(access().stored_form("second", "second"), access());

    let id = || DocumentId("document".into());
    let revision = || Revision("revision".into());
    match Mutation::Create(NewDocument {
        content: content(1),
        owner: Some(Owner::Agent(by_id())),
        access: access(),
    })
    .stored_form("launch", "launch")
    {
        Mutation::Create(new) => {
            assert_eq!(new.owner, Some(Owner::Agent(principal("alice"))));
            assert_eq!(new.access, folded());
            assert_eq!(new.content, content(1));
        }
        other => panic!("{other:?}"),
    }
    match (Mutation::SetAccess {
        id: id(),
        expected_revision: revision(),
        access: access(),
    })
    .stored_form("launch", "launch")
    {
        Mutation::SetAccess {
            id: kept,
            expected_revision,
            access,
        } => {
            assert_eq!((kept, expected_revision), (id(), revision()));
            assert_eq!(access, folded());
        }
        other => panic!("{other:?}"),
    }
    match (Mutation::Transfer {
        id: id(),
        expected_revision: revision(),
        owner: Owner::Agent(by_id()),
        access: Some(access()),
    })
    .stored_form("launch", "launch")
    {
        Mutation::Transfer {
            id: kept,
            expected_revision,
            owner,
            access,
        } => {
            assert_eq!((kept, expected_revision), (id(), revision()));
            assert_eq!(owner, Owner::Agent(principal("alice")));
            assert_eq!(access, Some(folded()));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        (Mutation::Replace {
            id: id(),
            expected_revision: revision(),
            content: content(2),
        })
        .stored_form("launch", "launch"),
        Mutation::Replace { content: kept, .. } if kept == content(2)
    ));
    assert!(matches!(
        (Mutation::Delete {
            id: id(),
            expected_revision: revision(),
        })
        .stored_form("launch", "launch"),
        Mutation::Delete { id: kept, .. } if kept == id()
    ));
}

/// The mob and the identity of another mob's identity are identifiers like
/// any other: an empty one is refused where a principal is validated.
#[test]
fn member_keyed_another_mobs_identity_is_validated_as_identifiers() {
    let context = |mob_id: &str, identity: &str| {
        HostAccessContext::new(
            "realm",
            mob_agent(mob_id, identity),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
    };
    assert!(context("second", "alice").is_ok());
    assert!(matches!(context("", "alice"), Err(Error::Invalid(_))));
    assert!(matches!(context("second", ""), Err(Error::Invalid(_))));
}

/// What an owner and an audience name by a mob's id alone: an owning or
/// audience mob, and the mob of another mob's identity. The default mob's
/// identity, a worker and the realm name none.
#[test]
fn member_keyed_owners_and_audiences_name_the_mob_of_another_mobs_identity() {
    let worker = Principal::Worker {
        mob_id: "worked".into(),
        member_id: "alice".into(),
        creation_id: "creation".into(),
    };
    assert_eq!(Owner::Mob("owning".into()).named_mob(), Some("owning"));
    assert_eq!(
        Owner::Agent(mob_agent("second", "alice")).named_mob(),
        Some("second")
    );
    assert_eq!(Owner::Agent(principal("alice")).named_mob(), None);
    assert_eq!(Owner::Agent(worker.clone()).named_mob(), None);
    assert_eq!(Owner::Realm.named_mob(), None);

    assert_eq!(Audience::Mob("granted".into()).named_mob(), Some("granted"));
    assert_eq!(
        agent_audience(mob_agent("second", "alice")).named_mob(),
        Some("second")
    );
    assert_eq!(agent_audience(principal("alice")).named_mob(), None);
    assert_eq!(agent_audience(worker).named_mob(), None);
    assert_eq!(Audience::Realm.named_mob(), None);
}

/// The order `Principal` derives is the declared one: the default mob's
/// identity, then another mob's identity, then a worker, whatever their
/// names. The default mob's identity and a worker compare with each other
/// as they did before the variant between them existed.
#[test]
fn member_keyed_principals_keep_their_declared_order() {
    let agent = principal("zed");
    let other_mobs = mob_agent("a", "a");
    let worker = Principal::Worker {
        mob_id: "a".into(),
        member_id: "a".into(),
        creation_id: "a".into(),
    };
    assert!(agent < other_mobs);
    assert!(other_mobs < worker);
    assert!(agent < worker);
}

/// What a caller of another mob names. An identity without a mob is an
/// identity of the caller's own mob; an identity with the default mob's id
/// is stored in the default mob's form; an identity with any other mob's id,
/// the caller's own included, is stored as named. A worker, a mob and the
/// realm are stored as named. The same holds for an owner and for every
/// owner and audience a mutation names.
#[test]
fn member_keyed_a_bare_name_is_stored_as_an_identity_of_the_callers_own_mob() {
    let stored = |principal: Principal| principal.stored_form("launch", "second");
    let worker = || Principal::Worker {
        mob_id: "second".into(),
        member_id: "bob".into(),
        creation_id: "creation".into(),
    };
    assert_eq!(stored(principal("bob")), mob_agent("second", "bob"));
    assert_eq!(stored(mob_agent("launch", "bob")), principal("bob"));
    assert_eq!(
        stored(mob_agent("second", "bob")),
        mob_agent("second", "bob")
    );
    assert_eq!(stored(mob_agent("third", "bob")), mob_agent("third", "bob"));
    assert_eq!(stored(worker()), worker());
    // A caller of the default mob names its own mob's identity bare too.
    assert_eq!(
        principal("bob").stored_form("launch", "launch"),
        principal("bob")
    );

    assert_eq!(
        Owner::Agent(principal("bob")).stored_form("launch", "second"),
        Owner::Agent(mob_agent("second", "bob"))
    );
    assert_eq!(
        Owner::Mob("second".into()).stored_form("launch", "second"),
        Owner::Mob("second".into())
    );

    // A grant and a deny each name an identity without a mob.
    let named = || DocumentAccess {
        owner_reach: Reach::SelfOnly,
        grants: vec![Grant {
            audience: agent_audience(principal("bob")),
            role: Role::Reader,
        }],
        denies: vec![Deny {
            audience: agent_audience(principal("eve")),
            operation: DeniedOperation::Read,
        }],
    };
    let read = || DocumentAccess {
        owner_reach: Reach::SelfOnly,
        grants: vec![Grant {
            audience: agent_audience(mob_agent("second", "bob")),
            role: Role::Reader,
        }],
        denies: vec![Deny {
            audience: agent_audience(mob_agent("second", "eve")),
            operation: DeniedOperation::Read,
        }],
    };
    assert_eq!(named().stored_form("launch", "second"), read());

    let id = || DocumentId("document".into());
    let revision = || Revision("revision".into());
    match Mutation::Create(NewDocument {
        content: content(1),
        owner: Some(Owner::Agent(principal("bob"))),
        access: named(),
    })
    .stored_form("launch", "second")
    {
        Mutation::Create(new) => {
            assert_eq!(new.owner, Some(Owner::Agent(mob_agent("second", "bob"))));
            assert_eq!(new.access, read());
        }
        other => panic!("{other:?}"),
    }
    match (Mutation::SetAccess {
        id: id(),
        expected_revision: revision(),
        access: named(),
    })
    .stored_form("launch", "second")
    {
        Mutation::SetAccess { access, .. } => assert_eq!(access, read()),
        other => panic!("{other:?}"),
    }
    match (Mutation::Transfer {
        id: id(),
        expected_revision: revision(),
        owner: Owner::Agent(principal("bob")),
        access: Some(named()),
    })
    .stored_form("launch", "second")
    {
        Mutation::Transfer { owner, access, .. } => {
            assert_eq!(owner, Owner::Agent(mob_agent("second", "bob")));
            assert_eq!(access, Some(read()));
        }
        other => panic!("{other:?}"),
    }
}
