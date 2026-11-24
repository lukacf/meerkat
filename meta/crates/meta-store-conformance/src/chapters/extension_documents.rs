//! `ExtensionDocumentStore` profile for the mob id contract a host relies on
//! when a mob takes or keeps an id: whether live documents name the id
//! ([`mob_references`]), and which mob holds it ([`mob_id_holders`]).
//!
//! Documents name a mob by its id alone, as their owner (`Owner::Mob`) or in
//! an audience grant or deny (`Audience::Mob`), or through an identity of the
//! mob that owns a document or that a grant or a deny names
//! (`Principal::MobAgent`), so a new mob under the id of a destroyed one, and
//! its identities of the same names, would inherit what they grant. A host
//! refuses a create
//! while documents name the id, records the holder of every mob that
//! activates under an id, and on a restore compares it: a restored mob of
//! another holder is quarantined. A store that keeps the trait's default
//! holder methods answers an error, which the host treats as a quarantine, so
//! [`mob_id_holders`] fails on it.

use meerkat_store_conformance::ConformanceFailure;
use meta_extension_state::{
    Audience, DeniedOperation, Deny, DocumentAccess, DocumentContent, DocumentService, Grant,
    HostAccessContext, HostPolicy, MobIdHolder, Mutation, NewDocument, Owner, Principal, Reach,
    RequestIdentity, Role, VerifiedLineage,
};
use std::collections::BTreeSet;

use crate::factory::ExtensionDocumentStoreFactory;
use crate::steps::Steps;

/// Which mob holds a mob id: unrecorded until a host records it, the last
/// recorded holder after, per realm and mob id, durably across a reopen.
///
/// A holder record the store cannot decode must fail the answer, never read
/// as no holder (the host would adopt the id). This profile cannot plant such
/// a record in a store generically, so it does not check that; for the
/// built-in SQLite store, Meta's integration test
/// `extension_an_unreadable_holder_quarantines_without_failing_the_restore`
/// does.
pub async fn mob_id_holders(
    factory: &dyn ExtensionDocumentStoreFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter("extension_documents::mob_id_holders");
    let store = factory.open().await?;

    let step = "unrecorded_id_has_no_holder";
    let holder = steps.wrap(step, store.mob_id_holder("realm", "mob").await)?;
    steps.ensure(step, holder.is_none(), format!("found {holder:?}"))?;

    let step = "recorded_holder_is_answered";
    steps.wrap(
        step,
        store
            .record_mob_id_holder("realm", "mob", &MobIdHolder::Host)
            .await,
    )?;
    let holder = steps.wrap(step, store.mob_id_holder("realm", "mob").await)?;
    steps.ensure(
        step,
        holder == Some(MobIdHolder::Host),
        format!("found {holder:?}"),
    )?;

    let step = "a_new_record_replaces_the_holder";
    let session = MobIdHolder::OwnerSession("owner-session".into());
    steps.wrap(
        step,
        store.record_mob_id_holder("realm", "mob", &session).await,
    )?;
    let holder = steps.wrap(step, store.mob_id_holder("realm", "mob").await)?;
    steps.ensure(
        step,
        holder.as_ref() == Some(&session),
        format!("found {holder:?}"),
    )?;

    let step = "holders_are_per_realm_and_mob_id";
    for (realm, mob_id) in [("another-realm", "mob"), ("realm", "another-mob")] {
        let holder = steps.wrap(step, store.mob_id_holder(realm, mob_id).await)?;
        steps.ensure(
            step,
            holder.is_none(),
            format!("{realm}/{mob_id} found {holder:?}"),
        )?;
    }

    let step = "holders_survive_a_reopen";
    drop(store);
    let reopened = factory.open().await?;
    let holder = steps.wrap(step, reopened.mob_id_holder("realm", "mob").await)?;
    steps.ensure(
        step,
        holder.as_ref() == Some(&session),
        format!("found {holder:?}"),
    )?;
    Ok(())
}

/// Whether any live document of a realm, in any namespace, names a mob: as
/// its owner, in a grant or in a deny, or through an identity of the mob
/// that owns the document or that a grant or a deny names. A deleted
/// document, another realm, an unnamed mob and the default mob's identity do
/// not count, and every answer follows the store's own writes at once.
pub async fn mob_references(
    factory: &dyn ExtensionDocumentStoreFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter("extension_documents::mob_references");
    let store = factory.open().await?;
    let manager = steps.wrap(
        "caller",
        HostAccessContext::new(
            "realm",
            Principal::Agent("manager".into()),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        )
        .and_then(|caller| {
            caller.with_memberships(BTreeSet::new(), BTreeSet::from(["owned".into()]), false)
        }),
    )?;
    let in_namespace = |namespace: &str| {
        steps.wrap(
            "service",
            DocumentService::new("realm", namespace, std::sync::Arc::clone(&store)),
        )
    };
    let create = |owner: Option<Owner>, access: DocumentAccess| {
        Mutation::Create(NewDocument {
            content: DocumentContent {
                title: "named".into(),
                content_type: "application/test".into(),
                schema_version: 1,
                payload: vec![1],
            },
            owner,
            access,
        })
    };
    let request = |id: &str| RequestIdentity::new(id, id.as_bytes());

    let step = "an_empty_realm_names_no_mob";
    let named = steps.wrap(step, store.references_mob("realm", "owned").await)?;
    steps.ensure(step, !named, "an empty realm names a mob")?;

    let step = "owner_grant_and_deny_name_a_mob";
    steps.wrap(
        step,
        in_namespace("one")?
            .mutate(
                &manager,
                &steps.wrap(step, request("owned"))?,
                create(Some(Owner::Mob("owned".into())), DocumentAccess::default()),
            )
            .await,
    )?;
    let granted = steps
        .wrap(
            step,
            in_namespace("two")?
                .mutate(
                    &manager,
                    &steps.wrap(step, request("granted"))?,
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
                .await,
        )?
        .receipt;
    steps.wrap(
        step,
        in_namespace("three")?
            .mutate(
                &manager,
                &steps.wrap(step, request("denied"))?,
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
            .await,
    )?;
    for mob_id in ["owned", "granted", "denied"] {
        let named = steps.wrap(step, store.references_mob("realm", mob_id).await)?;
        steps.ensure(step, named, format!("no document names {mob_id}"))?;
    }

    let step = "another_mobs_identity_names_its_mob";
    // An identity of another mob names its mob by the id alone, like a mob
    // owner or a mob audience: a new mob under that id would have an
    // identity of the same name.
    let identity = |mob_id: &str| Principal::agent(Some(mob_id), "alice");
    let identity_audience = |mob_id: &str| Audience::Agent {
        principal: identity(mob_id),
        reach: Reach::SelfOnly,
    };
    let identity_owner = steps.wrap(
        step,
        HostAccessContext::new(
            "realm",
            identity("identity-owning"),
            VerifiedLineage::root(),
            HostPolicy::ALLOW,
        ),
    )?;
    steps.wrap(
        step,
        in_namespace("four")?
            .mutate(
                &identity_owner,
                &steps.wrap(step, request("identity-owned"))?,
                create(
                    None,
                    DocumentAccess {
                        grants: vec![Grant {
                            audience: identity_audience("identity-granted"),
                            role: Role::Reader,
                        }],
                        denies: vec![Deny {
                            audience: identity_audience("identity-denied"),
                            operation: DeniedOperation::Read,
                        }],
                        ..DocumentAccess::default()
                    },
                ),
            )
            .await,
    )?;
    for mob_id in ["identity-owning", "identity-granted", "identity-denied"] {
        let named = steps.wrap(step, store.references_mob("realm", mob_id).await)?;
        steps.ensure(
            step,
            named,
            format!("no document names {mob_id} through its identity"),
        )?;
    }
    // The default mob's identity carries no mob and names none.
    let named = steps.wrap(step, store.references_mob("realm", "manager").await)?;
    steps.ensure(step, !named, "the default mob's identity names a mob")?;

    let step = "other_mobs_and_realms_are_not_named";
    for (realm, mob_id) in [("realm", "unnamed"), ("another-realm", "owned")] {
        let named = steps.wrap(step, store.references_mob(realm, mob_id).await)?;
        steps.ensure(step, !named, format!("{realm} names {mob_id}"))?;
    }

    let step = "a_deleted_document_names_no_mob";
    steps.wrap(
        step,
        in_namespace("two")?
            .mutate(
                &manager,
                &steps.wrap(step, request("delete-granted"))?,
                Mutation::Delete {
                    id: granted.document_id,
                    expected_revision: granted.revision,
                },
            )
            .await,
    )?;
    let named = steps.wrap(step, store.references_mob("realm", "granted").await)?;
    steps.ensure(step, !named, "a deleted document still names its mob")?;
    Ok(())
}
