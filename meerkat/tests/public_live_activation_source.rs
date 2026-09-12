#![cfg(all(not(target_arch = "wasm32"), feature = "live"))]

use std::collections::BTreeMap;

use meerkat::live_activation::{
    FileLiveExecutionGrantSource, LiveActivationDocumentLocator, LiveActivationLookup,
    LiveActivationSelection, LiveActivationSourceError, select_live_activation,
};
use meerkat_core::connection::RealmChain;
use meerkat_core::live_execution::activation::{
    LiveActivationDeclaration, LiveActivationDocument, LiveActivationEntry, LiveActivationId,
    LiveExecutorSelector, LiveProfileRevision,
};
use meerkat_core::{Config, RealmId};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Document = LiveActivationDocument<()>;
type Declaration = LiveActivationDeclaration<()>;

fn declaration() -> Result<Declaration, serde_json::Error> {
    serde_json::from_value(json!({
        "issuer_realm": "owner", "profile_id": "voice", "profile_revision": vec![1; 32],
        "requesting_realms": ["child", "owner"],
        "executor": {"kind": "session", "session_id": "00000000-0000-0000-0000-000000000001"},
        "allowed_evidence": ["application_snapshot"],
        "permission": {
            "allowed_mutations": ["read_only"], "tools": {"kind": "unrestricted"},
            "limits": {
                "max_requests": 100, "max_concurrent_requests": 1,
                "max_effects_per_request": 10, "max_tokens_per_request": 1000,
                "max_duration_ms": 30000
            }
        },
        "generation": 1, "revoke_policy": "cancel_pending_and_request_running_cancellation"
    }))
}

fn chain() -> Result<RealmChain, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    config.realm.insert("owner".into(), Default::default());
    config.realm.insert(
        "child".into(),
        serde_json::from_value(json!({"parent": "owner"}))?,
    );
    Ok(RealmChain::resolve(&config, &RealmId::parse("child")?)?)
}

fn documents(
    declaration: Declaration,
) -> Result<BTreeMap<RealmId, Document>, Box<dyn std::error::Error>> {
    Ok(BTreeMap::from([
        (
            RealmId::parse("owner")?,
            Document {
                activations: BTreeMap::from([(
                    LiveActivationId::parse("activation")?,
                    LiveActivationEntry::Set(Box::new(declaration)),
                )]),
            },
        ),
        (RealmId::parse("child")?, Document::default()),
    ]))
}

fn lookup<'a>(declaration: &'a Declaration, realm: &'a RealmId) -> LiveActivationLookup<'a, ()> {
    LiveActivationLookup {
        profile_id: &declaration.profile_id,
        profile_revision: declaration.profile_revision,
        requesting_realm: realm,
        executor: &declaration.executor,
        now: chrono::DateTime::UNIX_EPOCH,
    }
}

#[test]
fn absent_disabled_inherit_and_set_preserve_whole_entry_owner() -> TestResult {
    let declaration = declaration()?;
    let child = RealmId::parse("child")?;
    let lookup = lookup(&declaration, &child);
    let mut documents = documents(declaration.clone())?;
    let inherited = select_live_activation(&chain()?, &documents, &lookup)?;
    let LiveActivationSelection::Selected {
        declaration: inherited,
        ..
    } = inherited
    else {
        return Err("expected inherited declaration".into());
    };
    assert_eq!(*inherited, declaration);
    let id = LiveActivationId::parse("activation")?;
    for (entry, is_disabled) in [
        (LiveActivationEntry::Inherit, false),
        (LiveActivationEntry::Disable, true),
    ] {
        documents
            .get_mut(&child)
            .ok_or("child document")?
            .activations
            .insert(id.clone(), entry);
        assert_eq!(
            matches!(
                select_live_activation(&chain()?, &documents, &lookup)?,
                LiveActivationSelection::Disabled
            ),
            is_disabled
        );
    }
    let mut replacement = declaration.clone();
    replacement.issuer_realm = child.clone();
    replacement.allowed_evidence.clear();
    documents
        .get_mut(&child)
        .ok_or("child document")?
        .activations
        .insert(id, LiveActivationEntry::Set(Box::new(replacement.clone())));
    let LiveActivationSelection::Selected {
        declaration: selected,
        ..
    } = select_live_activation(&chain()?, &documents, &lookup)?
    else {
        return Err("expected replacement".into());
    };
    assert_eq!(
        *selected, replacement,
        "parent permission must not be unioned"
    );
    for document in documents.values_mut() {
        document.activations.clear();
    }
    assert_eq!(
        select_live_activation(&chain()?, &documents, &lookup)?,
        LiveActivationSelection::Disabled
    );
    Ok(())
}

#[test]
fn exact_target_realm_revision_expiry_and_issuer_are_required() -> TestResult {
    let declaration = declaration()?;
    let child = RealmId::parse("child")?;
    let mut lookup = lookup(&declaration, &child);
    let docs = documents(declaration.clone())?;
    lookup.profile_revision = serde_json::from_value::<LiveProfileRevision>(json!(vec![2; 32]))?;
    assert!(matches!(
        select_live_activation(&chain()?, &docs, &lookup),
        Err(LiveActivationSourceError::ProfileRevisionMismatch)
    ));
    lookup.profile_revision = declaration.profile_revision;
    let other = LiveExecutorSelector::Session {
        session_id: meerkat_core::SessionId::new(),
    };
    lookup.executor = &other;
    assert_eq!(
        select_live_activation(&chain()?, &docs, &lookup)?,
        LiveActivationSelection::Disabled
    );
    lookup.executor = &declaration.executor;
    let mut altered = declaration.clone();
    altered.requesting_realms.remove(&child);
    assert_eq!(
        select_live_activation(&chain()?, &documents(altered)?, &lookup)?,
        LiveActivationSelection::Disabled
    );
    let mut altered = declaration.clone();
    altered.expires_at = Some(lookup.now);
    assert!(matches!(
        select_live_activation(&chain()?, &documents(altered)?, &lookup),
        Err(LiveActivationSourceError::Expired)
    ));
    let mut altered = declaration.clone();
    altered.issuer_realm = child.clone();
    assert!(matches!(
        select_live_activation(&chain()?, &documents(altered)?, &lookup),
        Err(LiveActivationSourceError::IssuerMismatch { .. })
    ));
    Ok(())
}

#[test]
fn ambiguous_exact_activations_never_choose_by_map_order() -> TestResult {
    let declaration = declaration()?;
    let child = RealmId::parse("child")?;
    let lookup = lookup(&declaration, &child);
    let mut docs = documents(declaration.clone())?;
    docs.get_mut(&RealmId::parse("owner")?)
        .ok_or("owner")?
        .activations
        .insert(
            LiveActivationId::parse("second")?,
            LiveActivationEntry::Set(Box::new(declaration.clone())),
        );
    assert!(matches!(
        select_live_activation(&chain()?, &docs, &lookup),
        Err(LiveActivationSourceError::AmbiguousActivation)
    ));
    Ok(())
}

#[tokio::test]
async fn native_source_rereads_trusted_document_and_fails_closed_on_bad_content() -> TestResult {
    let directory = tempfile::tempdir()?;
    let owner_directory = directory.path().join("owner");
    let child_directory = directory.path().join("child");
    tokio::fs::create_dir(&owner_directory).await?;
    tokio::fs::create_dir(&child_directory).await?;
    let owner = RealmId::parse("owner")?;
    let child = RealmId::parse("child")?;
    let owner_locator = LiveActivationDocumentLocator::beside_config_document(
        &owner_directory.join("config.toml"),
    )?;
    let child_locator = LiveActivationDocumentLocator::beside_config_document(
        &child_directory.join("config.toml"),
    )?;
    let source = FileLiveExecutionGrantSource::new(BTreeMap::from([
        (owner.clone(), owner_locator.clone()),
        (child.clone(), child_locator.clone()),
    ]));
    let declaration = declaration()?;
    let lookup = lookup(&declaration, &child);
    let docs = source.load::<()>(&chain()?).await?;
    assert_eq!(
        select_live_activation(&chain()?, &docs, &lookup)?,
        LiveActivationSelection::Disabled
    );
    let docs = documents(declaration.clone())?;
    tokio::fs::write(
        owner_locator.path(),
        toml::to_string(docs.get(&owner).ok_or("owner")?)?,
    )
    .await?;
    assert!(matches!(
        select_live_activation(&chain()?, &source.load::<()>(&chain()?).await?, &lookup)?,
        LiveActivationSelection::Selected { .. }
    ));
    tokio::fs::write(
        child_locator.path(),
        "[activations.activation]\nmode = \"disable\"\n",
    )
    .await?;
    assert_eq!(
        select_live_activation(&chain()?, &source.load::<()>(&chain()?).await?, &lookup)?,
        LiveActivationSelection::Disabled
    );
    tokio::fs::write(child_locator.path(), "raw-secret-must-not-appear").await?;
    let error = source
        .load::<()>(&chain()?)
        .await
        .err()
        .ok_or("expected invalid document")?;
    assert!(matches!(
        error,
        LiveActivationSourceError::InvalidDocument { .. }
    ));
    assert!(!error.to_string().contains("raw-secret-must-not-appear"));
    assert!(
        LiveActivationDocumentLocator::beside_config_document(std::path::Path::new("config.toml"))
            .is_err()
    );
    Ok(())
}
