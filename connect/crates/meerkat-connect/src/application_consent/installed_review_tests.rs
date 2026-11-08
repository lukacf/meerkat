//! Actual selected native lifetime and private connection, without a fabricated
//! Meta construction offer or a Console authentication/eligibility substitute.
use super::*;
use futures_util::FutureExt;
use meerkat_mobkit::rpc_gateway::GatewayHostError;
use meerkat_toolkit::host_lifetime::{
    NativeHostLifetime,
    control::{HostBootstrapSelection, HostLifetimeError, SelectedHostArtifact},
};
use std::{
    os::fd::IntoRawFd,
    panic::{AssertUnwindSafe, resume_unwind},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use toolkit_mobkit::{
    host_gateway::ToolkitGatewayHost,
    private_host::{InheritedHostPipes, PrivateHostConnection},
};

fn installed(root: &Path) -> HostBootstrapSelection {
    use meerkat_toolkit::host_lifetime::policy::*;
    let bundle = root.join("shared/policy/consent");
    fs::create_dir_all(&bundle).unwrap();
    for name in ["control", "activations"] {
        fs::create_dir_all(root.join("shared").join(name)).unwrap();
    }
    let write = |path: PathBuf, bytes: &[u8]| {
        fs::write(&path, bytes).unwrap();
        SelectedHostArtifact {
            path,
            sha256: hash(bytes),
        }
    };
    let source = write(bundle.join("policy.json"), br#"{"schema":1}"#);
    let tools = write(bundle.join("tools.json"), b"[]");
    let application = identity().application;
    let native = serde_json::from_value(serde_json::json!({
        "realm":meerkat_core::mob_realm_id("installed-consent").unwrap(),
        "grant_administrator":application, "remote_operator":null,
        "ingress_domain":{"authority":application,"namespace":"consent-fixture"},
        "application_tool_policy":{"name":"tools.json","sha256":tools.sha256},
        "members":[{"member":{"mob_id":"installed-consent","role":"worker","member":"a"},
            "logical_owner":application,"context_generation":1,
            "audience":{"audience":"principal","principal":application},
            "initial_surface":{"name":"tools.json","sha256":tools.sha256}}],"routes":[]
    }))
    .unwrap();
    let policy = CompiledLocalPolicy {
        schema: 7,
        deployment: Id::new("installed-consent").unwrap(),
        source_policy_sha256: source.sha256.clone(),
        components: vec![],
        channels: vec![],
        native_host: Some(native),
    };
    let local = write(
        bundle.join("toolkit-local-policy.json"),
        &policy.canonical_bytes().unwrap(),
    );
    let manifest = write(
        bundle.join("manifest.json"),
        &toolkit_package::canonical_json(&serde_json::json!({"artifacts":[
        {"path":"policy.json","sha256":source.sha256}, {"path":"tools.json","sha256":tools.sha256},
        {"path":"toolkit-local-policy.json","sha256":local.sha256}]}))
        .unwrap(),
    );
    let activation = write(
        root.join("shared/activations/1.json"),
        &toolkit_package::canonical_json(&serde_json::json!({
        "activation_id":"1","policy_bundle":{"path":"shared/policy/consent",
            "policy_digest":format!("policy-v1:sha256:{}",manifest.sha256)}}))
        .unwrap(),
    );
    std::os::unix::fs::symlink("shared/activations/1.json", root.join("current")).unwrap();
    HostBootstrapSelection {
        deployment_root: root.to_owned(),
        activation,
        policy_manifest: manifest,
        local_policy: local,
    }
}

struct InstalledPipe {
    owner: Arc<NativeHostLifetime>,
    connection: PrivateHostConnection,
    _commands: tokio::fs::File,
    _responses: tokio::io::BufReader<tokio::fs::File>,
}
impl InstalledPipe {
    async fn open(selection: &HostBootstrapSelection) -> Self {
        let (read, write) = nix::unistd::pipe().unwrap();
        let (responses, replies) = nix::unistd::pipe().unwrap();
        let pipes = InheritedHostPipes::adopt(read.into_raw_fd(), replies.into_raw_fd()).unwrap();
        let mut commands = tokio::fs::File::from_std(File::from(write));
        let mut responses =
            tokio::io::BufReader::new(tokio::fs::File::from_std(File::from(responses)));
        let mut request = serde_json::to_vec(&serde_json::json!({"version":1,"id":1,
            "command":{"kind":"bootstrap","selection":selection}}))
        .unwrap();
        request.push(b'\n');
        commands.write_all(&request).await.unwrap();
        commands.flush().await.unwrap();
        let (owner, connection) =
            PrivateHostConnection::bootstrap(pipes, NativeHostLifetime::bootstrap)
                .await
                .unwrap();
        // Keep the actual connection available for cleanup even if the response assertion fails.
        let result = AssertUnwindSafe(async {
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(10), responses.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap()["result"]["kind"],
                "bootstrap_accepted"
            );
        })
        .catch_unwind()
        .await;
        if let Err(error) = result {
            let _ = connection.finish().await;
            resume_unwind(error);
        }
        Self {
            owner,
            connection,
            _commands: commands,
            _responses: responses,
        }
    }
    fn host(&self) -> ToolkitGatewayHost {
        ToolkitGatewayHost::new(self.owner.clone(), self.connection.closed_observer())
    }
    async fn finish(self) {
        self.connection.finish().await.unwrap();
    }
}
fn compose(
    host: ToolkitGatewayHost,
    review: Option<ApplicationOperationReview>,
) -> Result<ToolkitGatewayHost, HostLifetimeError> {
    crate::application_installed::compose_operation_review(host, review)
}
fn selected(owner: Arc<ApplicationConsentOwner>) -> ApplicationOperationReview {
    ApplicationOperationReview::new(owner, Arc::new(NoModelReview), Duration::from_secs(30))
}

#[tokio::test]
async fn installed_review_binds_actual_owner_and_preserves_ordinary_none() {
    let directory = private_root();
    let root = directory.path().canonicalize().unwrap();
    let selection = installed(&root);
    let pipe = InstalledPipe::open(&selection).await;
    let caught = AssertUnwindSafe(async {
        let actual = ConsentApplicationIdentity::from_installed(&pipe.owner).unwrap();
        let issuer = ApplicationConsentOwner::create(&root, actual.clone()).unwrap();
        let (plain, ordinary_owner) =
            crate::application_installed::compose_operation_review_with_owner(pipe.host(), None)
                .unwrap();
        assert!(ordinary_owner.is_none());
        assert!(plain.selected_operation_review().unwrap().is_none());
        assert!(
            matches!(
                compose(plain, Some(selected(issuer.clone()))),
                Err(HostLifetimeError::WrongOwner)
            ),
            "an already offered ordinary selection cannot be upgraded"
        );
        for field in ["application", "deployment", "realm"] {
            let mut wrong = actual.clone();
            match field {
                "application" => wrong.application = human(),
                "deployment" => wrong.deployment = Id::new("wrong-deployment").unwrap(),
                _ => wrong.realm = RealmId::parse("wrong-realm").unwrap(),
            }
            let foreign = ApplicationConsentOwner::create(&root, wrong).unwrap();
            assert!(
                matches!(
                    compose(pipe.host(), Some(selected(foreign))),
                    Err(HostLifetimeError::WrongOwner)
                ),
                "{field}"
            );
        }
        let (host, workflow_owner) =
            crate::application_installed::compose_operation_review_with_owner(
                pipe.host(),
                Some(selected(issuer.clone())),
            )
            .unwrap();
        assert!(
            Arc::ptr_eq(workflow_owner.as_ref().unwrap(), &issuer),
            "the installed carrier returns its actual issuer"
        );
        let review = host.selected_operation_review().unwrap().unwrap();
        assert!(Arc::ptr_eq(
            &review,
            &host.selected_operation_review().unwrap().unwrap()
        ));
        assert!(
            review.consent_owner_loss_settlement().is_none(),
            "no Human route is invented"
        );
        assert!(issuer.store.load_all().unwrap().is_empty());
        let manifest = issuer.selection().artifact().path.clone();
        let bytes = fs::read(&manifest).unwrap();
        fs::remove_file(&manifest).unwrap();
        assert!(matches!(
            compose(pipe.host(), Some(selected(issuer.clone()))),
            Err(HostLifetimeError::Unavailable)
        ));
        assert!(matches!(
            host.selected_operation_review(),
            Err(GatewayHostError::Unavailable)
        ));
        create_file(&manifest, b"changed").unwrap();
        assert!(matches!(
            host.selected_operation_review(),
            Err(GatewayHostError::Unavailable)
        ));
        fs::write(&manifest, bytes).unwrap();
        assert!(Arc::ptr_eq(
            &review,
            &host.selected_operation_review().unwrap().unwrap()
        ));
        host
    })
    .catch_unwind()
    .await;
    pipe.finish().await;
    let host = match caught {
        Ok(host) => host,
        Err(error) => resume_unwind(error),
    };
    assert!(
        matches!(
            host.selected_operation_review(),
            Err(GatewayHostError::Denied)
        ),
        "actual private/native close fences further offers"
    );
}

#[tokio::test]
async fn installed_review_reopen_retains_exact_issuer_through_review_and_settlement() {
    let directory = private_root();
    let root = directory.path().canonicalize().unwrap();
    let selection = installed(&root);
    let pipe = InstalledPipe::open(&selection).await;
    let caught = AssertUnwindSafe(async {
        let identity = ConsentApplicationIdentity::from_installed(&pipe.owner).unwrap();
        let owner = ApplicationConsentOwner::create(&root, identity.clone()).unwrap();
        let issuer = owner.selection().clone();
        let ordinary = ordinary(&owner, &meerkat_core::SessionId::new());
        let host = compose(
            pipe.host(),
            Some(
                selected(owner.clone())
                    .with_human_consent(Arc::new(LocalHumanRoute), Duration::from_secs(30)),
            ),
        )
        .unwrap();
        let review = host.selected_operation_review().unwrap().unwrap();
        let settlement = review.consent_owner_loss_settlement().unwrap();
        drop(owner);
        drop(host);
        assert!(matches!(
            ApplicationConsentOwner::reopen(issuer.clone(), &identity),
            Err(ConsentOwnerError::OwnerHeld)
        ));
        drop(review);
        assert!(matches!(
            ApplicationConsentOwner::reopen(issuer.clone(), &identity),
            Err(ConsentOwnerError::OwnerHeld)
        ));
        drop(settlement);
        (identity, issuer, ordinary)
    })
    .catch_unwind()
    .await;
    pipe.finish().await;
    let (identity, issuer, ordinary) = match caught {
        Ok(value) => value,
        Err(error) => resume_unwind(error),
    };
    // A fresh actual native lifetime and private connection, using the same
    // installed selection. No session, machine or G recovery is claimed here.
    let restored = InstalledPipe::open(&selection).await;
    let caught = AssertUnwindSafe(async {
        assert_eq!(
            ConsentApplicationIdentity::from_installed(&restored.owner).unwrap(),
            identity
        );
        let reopened = ApplicationConsentOwner::reopen(issuer.clone(), &identity).unwrap();
        assert_eq!(reopened.selection(), &issuer);
        assert_eq!(reopened.store.load_all().unwrap(), vec![ordinary]);
        let host = compose(restored.host(), Some(selected(reopened))).unwrap();
        assert!(host.selected_operation_review().unwrap().is_some());
    })
    .catch_unwind()
    .await;
    restored.finish().await;
    if let Err(error) = caught {
        resume_unwind(error);
    }
}
