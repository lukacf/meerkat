#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use meerkat::{ConfigDelta, SdkConfigStore};
use serde_json::json;

#[tokio::test]
async fn test_sdk_config_ephemeral() {
    let store = SdkConfigStore::new();
    let baseline = store.get().await.expect("get config");

    let patched = store
        .patch(ConfigDelta(json!({
            "agent": { "model": "ephemeral-model" }
        })))
        .await
        .expect("patch config");

    assert_eq!(patched.agent.model, "ephemeral-model");

    let fresh_store = SdkConfigStore::new();
    let fresh = fresh_store.get().await.expect("get config");

    assert_eq!(
        fresh.agent.model, baseline.agent.model,
        "ephemeral override should not persist across stores"
    );
}

/// The documents a realm document store composes with, by realm: none, or a
/// realm's own.
#[derive(Default)]
struct Documents(std::collections::BTreeMap<String, meerkat::Config>);

#[async_trait::async_trait]
impl meerkat_core::RealmConfigSource for Documents {
    async fn config_for_realm(
        &self,
        realm: &meerkat_core::connection::RealmId,
    ) -> Result<Option<meerkat::Config>, meerkat::ConfigError> {
        Ok(self.0.get(realm.as_str()).cloned())
    }
}

/// The store for the user document (the `global` realm's own) keeps what
/// other realms compose of it. A realm without a section of its own takes
/// the section the user document states for it, so removing a
/// `[realm.<id>]` section whose parent is `global` from a user document that
/// states `[governance]` would drop the governance that realm composes: the
/// write is refused, naming the section and the realm, and nothing is
/// written. The realm's own document on this state root does not make the
/// edit ordinary config: another state root sharing the user document may
/// hold no document for the realm, and the realm composes the user's
/// section there. With `[realm.global]` stated, a realm without a section
/// still composes `global`, so the same edit is then ordinary config.
#[tokio::test]
async fn a_user_document_write_cannot_drop_a_stand_in_section_a_realm_composes_through() {
    const USER: &str = "[realm.team]\nparent = \"global\"\n\n\
                        [[governance.review.rules]]\n\
                        target = { kind = \"mcp_server\", server = \"mail\" }\n\
                        tier = \"r2\"\n";
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, USER).unwrap();
    let store = |documents: Documents| {
        SdkConfigStore::with_realm_document(
            &path,
            meerkat_core::connection::RealmId::global(),
            std::sync::Arc::new(documents),
        )
    };
    let remove_team = || ConfigDelta(json!({"realm": {"team": null}}));

    let refused = store(Documents::default())
        .patch(remove_team())
        .await
        .expect_err("the realm without a document would drop the user's governance");
    let message = refused.to_string();
    assert!(
        refused.administered_section() == Some(meerkat_core::AdministeredSection::Governance)
            && message.contains("removing [realm.team]")
            && message.contains("realm 'team'"),
        "{message}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), USER);

    let own = || {
        let mut own = meerkat::Config::default();
        own.realm.insert(
            "team".to_string(),
            meerkat_core::RealmConfigSection::default(),
        );
        Documents(std::collections::BTreeMap::from([(
            "team".to_string(),
            own,
        )]))
    };
    let refused = store(own())
        .patch(remove_team())
        .await
        .expect_err("on a state root without the realm's document it composes the user's section");
    let message = refused.to_string();
    assert!(
        refused.administered_section() == Some(meerkat_core::AdministeredSection::Governance)
            && message.contains("removing [realm.team]")
            && message.contains("realm 'team' (on a state root without its own document)"),
        "{message}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), USER);

    let with_global = format!("[realm.global]\n\n{USER}");
    for documents in [Documents::default(), own()] {
        std::fs::write(&path, &with_global).unwrap();
        store(documents).patch(remove_team()).await.expect(
            "a realm without a section still composes the user's governance through global",
        );
    }
}

/// A file-backed SDK store writes through its document's admission fence:
/// while an admission holds the fence, a full replace and a patch refuse
/// typed after their bounded wait, with nothing written, and the next write
/// after the hold ends lands.
#[cfg(unix)]
#[tokio::test]
async fn document_writes_refuse_admission_in_progress_with_nothing_written() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "max_tokens = 100\n").unwrap();
    let store = SdkConfigStore::with_path(path.clone());
    let fence = store.inner().admission_fence().expect("a document fence");
    fence.ensure_exists().unwrap();
    let mut held = meerkat_core::AdmissionHold::try_acquire(vec![fence]).unwrap();
    let (set, patch) = tokio::join!(
        store.set(meerkat::Config::default()),
        store.patch(ConfigDelta(json!({ "max_tokens": 300 }))),
    );
    assert!(
        matches!(set, Err(meerkat::ConfigError::AdmissionInProgress)),
        "{set:?}"
    );
    assert!(
        matches!(patch, Err(meerkat::ConfigError::AdmissionInProgress)),
        "{patch:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "max_tokens = 100\n"
    );
    held.release();
    let patched = store
        .patch(ConfigDelta(json!({ "max_tokens": 300 })))
        .await
        .unwrap();
    assert_eq!(patched.max_tokens, Some(300));
}
