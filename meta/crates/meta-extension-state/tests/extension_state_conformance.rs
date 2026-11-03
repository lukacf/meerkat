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
