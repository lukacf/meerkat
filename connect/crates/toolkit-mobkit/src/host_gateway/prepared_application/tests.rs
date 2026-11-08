//! Inert selected-artifact/lifetime controls. No native agent, model, grant,
//! credential, WorkGraph or provider operation is initialized or invoked.
use super::*;
use async_trait::async_trait;
use meerkat_mobkit::{
    identity_first::{
        RosterError,
        contracts::{ProjectedRosterProvider, RosterProvider},
    },
    rpc_gateway::{GatewayHostComposition, GatewayHostError},
};
use meerkat_toolkit::host_lifetime::{
    control::{
        HostBootstrapSelection, HostCloseReason, PrivateHostLifetimeControl, SelectedHostArtifact,
    },
    policy::{CompiledLocalPolicy, CompiledMemberClass, CompiledNativeHost, CompiledNativeMember},
};
use std::{collections::VecDeque, path::PathBuf, sync::Mutex};
use toolkit_package::{Id, canonical_json};

const BYTES: &[u8] = br#"{"selected":"application"}"#;
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn artifact(path: PathBuf, bytes: &[u8]) -> SelectedHostArtifact {
    std::fs::write(&path, bytes).unwrap();
    SelectedHostArtifact {
        path,
        sha256: digest(bytes),
    }
}
fn definition() -> MobDefinition {
    serde_json::from_value(serde_json::json!({"id":"application","profiles":{
        "helper":{"model":"gpt-5.5"},"ordinary":{"model":"gpt-5.5"}
    }}))
    .unwrap()
}
fn row(identity: &str, label: &str) -> DurableAgentSpec {
    serde_json::from_value(serde_json::json!({"identity":identity,
        "profile":if identity == "helper" { "helper" } else { "ordinary" },
        "labels":{"current":label}}))
    .unwrap()
}
fn context() -> RosterContext {
    RosterContext {
        mob_definition: None,
        previous_identities: vec![],
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    selection: HostBootstrapSelection,
    owner: Arc<NativeHostLifetime>,
    channel: tokio::sync::watch::Sender<bool>,
    host: ToolkitGatewayHost,
}
impl Fixture {
    fn new() -> Self {
        Self::with_native_host(None)
    }
    fn with_native_host(native_host: Option<CompiledNativeHost>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let bundle = root.join("shared/policy/bundle.1");
        for path in [
            &bundle,
            &root.join("shared/activations"),
            &root.join("shared/control"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let source = artifact(bundle.join("policy.json"), b"{}");
        let policy = CompiledLocalPolicy {
            schema: 7,
            deployment: Id::new("application").unwrap(),
            source_policy_sha256: source.sha256.clone(),
            components: vec![],
            channels: vec![],
            native_host,
        };
        let local_policy = artifact(
            bundle.join("toolkit-local-policy.json"),
            &policy.canonical_bytes().unwrap(),
        );
        let selected = artifact(bundle.join("prepared.json"), BYTES);
        let policy_manifest = artifact(
            bundle.join("manifest.json"),
            &canonical_json(&serde_json::json!({"artifacts":[
                {"path":"policy.json","sha256":source.sha256},
                {"path":"toolkit-local-policy.json","sha256":local_policy.sha256},
                {"path":"prepared.json","sha256":selected.sha256}
            ]}))
            .unwrap(),
        );
        let activation = artifact(
            root.join("shared/activations/1.json"),
            &canonical_json(&serde_json::json!({
                "activation_id":"1", "policy_bundle":{"path":"shared/policy/bundle.1",
                "policy_digest":format!("policy-v1:sha256:{}",policy_manifest.sha256)}
            }))
            .unwrap(),
        );
        std::os::unix::fs::symlink("shared/activations/1.json", root.join("current")).unwrap();
        let selection = HostBootstrapSelection {
            deployment_root: root,
            activation,
            policy_manifest,
            local_policy,
        };
        let owner = Arc::new(NativeHostLifetime::bootstrap(selection.clone()).unwrap());
        let (channel, closed) = tokio::sync::watch::channel(false);
        let mut host = ToolkitGatewayHost::new(owner.clone(), PrivateHostClosed(closed));
        // This fixture exercises transport/current selection only. It has no
        // governed composition, and cannot execute helper or ordinary work.
        let reference = CompiledArtifactReference {
            name: "prepared.json".into(),
            sha256: digest(BYTES),
        };
        let bytes = crate::installed_host::read_bundle_artifact(&selection, &reference).unwrap();
        host.setup_application = Some(
            PreparedSetupApplication::from_verified_artifact(
                reference,
                bytes,
                definition(),
                Arc::new(SelectedHelper),
            )
            .unwrap(),
        );
        Self {
            _directory: directory,
            selection,
            owner,
            channel,
            host,
        }
    }
    fn request(&self) -> PreparedApplicationRequest {
        PreparedApplicationRequest {
            bundle: BYTES.to_vec(),
            declared_digest: digest(BYTES),
        }
    }
}

struct SelectedHelper;
impl RosterProjection for SelectedHelper {
    fn project(
        &self,
        _context: &RosterContext,
        roster: Vec<DurableAgentSpec>,
    ) -> Result<Vec<DurableAgentSpec>, String> {
        if roster
            .iter()
            .filter(|row| row.identity.as_str() == "helper")
            .count()
            != 1
        {
            return Err("selected helper is missing or ambiguous".into());
        }
        Ok(roster)
    }
}

#[tokio::test]
async fn exact_carrier_bytes_and_selected_digest_are_both_required() {
    let fixture = Fixture::new();
    let mut request = fixture.request();
    request.declared_digest = "f".repeat(64);
    assert!(matches!(
        fixture.host.prepared_application(Some(&request)).await,
        Err(GatewayHostError::Denied)
    ));
    assert!(fixture.host.prepared_application_selected.get().is_none());
    request = fixture.request();
    request.bundle.extend_from_slice(b" ");
    assert!(matches!(
        fixture.host.prepared_application(Some(&request)).await,
        Err(GatewayHostError::Denied)
    ));
    request.declared_digest = digest(&request.bundle);
    assert!(matches!(
        fixture.host.prepared_application(Some(&request)).await,
        Err(GatewayHostError::Denied)
    ));
    assert!(fixture.host.prepared_application_selected.get().is_none());
    let selected = fixture
        .host
        .prepared_application(Some(&fixture.request()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.definition, definition());
    assert_eq!(selected.selection_id, digest(BYTES));
    assert!(selected.roster_projection.is_some());
}

struct CurrentRoster(Mutex<VecDeque<Vec<DurableAgentSpec>>>);
#[async_trait]
impl RosterProvider for CurrentRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.0.lock().unwrap().pop_front().unwrap())
    }
}
#[tokio::test]
async fn actual_native_wrapper_projects_every_current_provider_result() {
    let fixture = Fixture::new();
    let selected = fixture
        .host
        .prepared_application(Some(&fixture.request()))
        .await
        .unwrap()
        .unwrap();
    let first = vec![row("ordinary-a", "first"), row("helper", "selected")];
    let second = vec![row("helper", "selected"), row("ordinary-b", "new")];
    let removed = vec![row("ordinary-b", "new")];
    let provider = ProjectedRosterProvider::new(
        Arc::new(CurrentRoster(Mutex::new(
            [first.clone(), second.clone(), removed].into(),
        ))),
        selected.roster_projection.unwrap(),
    );
    assert_eq!(provider.roster(&context()).await.unwrap(), first);
    assert_eq!(provider.roster(&context()).await.unwrap(), second);
    assert!(matches!(
        provider.roster(&context()).await,
        Err(RosterError::Refused(_))
    ));
}

#[tokio::test]
async fn retained_projection_refuses_closed_or_replaced_selected_owner() {
    for reason in ["activation", "private", "private_drop", "replacement"] {
        let fixture = Fixture::new();
        let selected = fixture
            .host
            .prepared_application(Some(&fixture.request()))
            .await
            .unwrap()
            .unwrap();
        let projection = selected.roster_projection.unwrap();
        assert!(
            projection
                .project(&context(), vec![row("helper", "current")])
                .is_ok()
        );
        let request = fixture.request();
        match reason {
            "activation" => {
                fixture
                    .owner
                    .begin_close_activation(HostCloseReason::Shutdown)
                    .unwrap();
            }
            "private" => {
                fixture.channel.send(true).unwrap();
            }
            "private_drop" => {
                drop(fixture.channel);
            }
            "replacement" => {
                std::fs::write(
                    &fixture.selection.local_policy.path,
                    b"changed selected policy",
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            projection
                .project(&context(), vec![row("helper", "current")])
                .is_err(),
            "{reason}"
        );
        assert!(
            fixture
                .host
                .prepared_application(Some(&request))
                .await
                .is_err(),
            "{reason}"
        );
    }
}

#[tokio::test]
async fn required_carrier_absence_is_refused_by_the_early_host_hook() {
    let fixture = Fixture::new();
    assert!(matches!(
        fixture.host.prepared_application(None).await,
        Err(GatewayHostError::Denied)
    ));
    assert!(fixture.host.prepared_application_selected.get().is_none());
    assert!(fixture.host.grant_configuration.lock().unwrap().is_some());
    // This direct hook control starts no storage. Native init ordering is
    // established separately from its actual select_prepared_application call.
}

#[tokio::test]
async fn earlier_selection_does_not_authenticate_a_later_absent_or_changed_carrier() {
    let fixture = Fixture::new();
    let request = fixture.request();
    assert!(
        fixture
            .host
            .prepared_application(Some(&request))
            .await
            .unwrap()
            .is_some()
    );
    assert!(fixture.host.prepared_application_selected.get().is_some());
    assert!(matches!(
        fixture.host.prepared_application(None).await,
        Err(GatewayHostError::Denied)
    ));
    let mut changed = request.clone();
    changed.bundle.extend_from_slice(b" ");
    changed.declared_digest = digest(&changed.bundle);
    assert!(matches!(
        fixture.host.prepared_application(Some(&changed)).await,
        Err(GatewayHostError::Denied)
    ));
    assert!(
        fixture
            .host
            .prepared_application(Some(&request))
            .await
            .unwrap()
            .is_some()
    );
    assert!(fixture.host.grant_configuration.lock().unwrap().is_some());
}

#[tokio::test]
async fn an_ordinary_host_keeps_absence_without_selecting_an_application() {
    let mut fixture = Fixture::new();
    fixture.host.setup_application = None;
    assert!(
        fixture
            .host
            .prepared_application(None)
            .await
            .unwrap()
            .is_none()
    );
    // The native selector rejects Some -> None; the ordinary Toolkit host
    // never supplies an unauthenticated prepared selection or parses fallback.
    assert!(
        fixture
            .host
            .prepared_application(Some(&fixture.request()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(fixture.host.prepared_application_selected.get().is_none());
    assert!(fixture.host.grant_configuration.lock().unwrap().is_some());
}

#[test]
fn later_composition_still_requires_an_earlier_selected_carrier() {
    let fixture = Fixture::new();
    assert!(matches!(
        fixture.host.native_grant_work_configuration(),
        Err(GatewayHostError::Denied)
    ));
    assert!(fixture.host.grant_configuration.lock().unwrap().is_some());
}

#[test]
fn inconsistent_retained_artifact_is_rejected_before_carrier_selection() {
    let reference = CompiledArtifactReference {
        name: "prepared.json".into(),
        sha256: digest(BYTES),
    };
    assert!(
        PreparedSetupApplication::from_verified_artifact(
            reference,
            b"other bytes".to_vec(),
            definition(),
            Arc::new(SelectedHelper),
        )
        .is_err()
    );
}

struct PassiveCustomizer;
#[async_trait]
impl meerkat_mobkit::identity_first::AgentCustomizer for PassiveCustomizer {
    async fn customize_build(
        &self,
        _: &meerkat_mobkit::identity_first::AgentBuildContext,
        _: &DurableAgentSpec,
        _: &mut meerkat_mobkit::identity_first::AgentBuildDraft,
    ) -> Result<(), meerkat_mobkit::identity_first::CustomizerError> {
        Ok(())
    }
}

#[test]
fn gateway_customizer_preserves_absence_and_legacy_callback_and_refuses_closed_host() {
    for close in ["private", "activation", "replacement"] {
        let fixture = Fixture::new();
        let inner: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> =
            Arc::new(PassiveCustomizer);
        assert!(
            fixture
                .host
                .compose_agent_customizer(None)
                .unwrap()
                .is_none()
        );
        let composed = fixture
            .host
            .compose_agent_customizer(Some(inner.clone()))
            .unwrap()
            .unwrap();
        assert!(
            Arc::ptr_eq(&inner, &composed),
            "an ordinary host must retain the actual callback"
        );
        match close {
            "private" => {
                fixture.channel.send(true).unwrap();
            }
            "activation" => {
                fixture
                    .owner
                    .begin_close_activation(HostCloseReason::Shutdown)
                    .unwrap();
            }
            "replacement" => {
                std::fs::write(
                    &fixture.selection.local_policy.path,
                    b"changed selected policy",
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            fixture.host.compose_agent_customizer(None).is_err(),
            "{close}"
        );
        assert!(
            fixture.host.compose_agent_customizer(Some(inner)).is_err(),
            "{close}"
        );
    }
}

#[test]
fn declared_member_classes_require_sdk_customizer_before_native_bootstrap() {
    use meerkat_authorization_contracts::constraints::{AudienceRef, ResourceDomain};
    use meerkat_core::{MobMemberBinding, PrincipalKind, PrincipalRef, TrustDomainId};

    let principal = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "application",
        TrustDomainId::new("application").unwrap(),
    )
    .unwrap();
    let reference = CompiledArtifactReference {
        name: "prepared.json".into(),
        sha256: digest(BYTES),
    };
    let mut native = CompiledNativeHost {
        authentication_domain: None,
        event_domain: None,
        realm: meerkat_core::mob_realm_id("application").unwrap(),
        grant_administrator: principal.clone(),
        remote_operator: None,
        service_parent_administration: None,
        ingress_domain: ResourceDomain {
            authority: principal.clone(),
            namespace: "events".into(),
        },
        application_tool_policy: reference.clone(),
        setup: None,
        member_classes: vec![],
        members: vec![CompiledNativeMember {
            member: MobMemberBinding {
                mob_id: "application".into(),
                role: "ordinary".into(),
                member: "ordinary".into(),
            },
            logical_owner: principal.clone(),
            context_generation: 1,
            audience: AudienceRef::Principal { principal },
            initial_surface: reference,
            setup_capability: None,
        }],
        routes: vec![],
    };
    // An actual selected native host with no classes keeps optional absence.
    let ordinary = Fixture::with_native_host(Some(native.clone()));
    assert!(
        ordinary
            .host
            .compose_agent_customizer(None)
            .unwrap()
            .is_none()
    );
    native.member_classes.push(CompiledMemberClass {
        id: Id::new("dynamic").unwrap(),
        purpose: Default::default(),
        profile: "ordinary".into(),
        factory: Id::new("application:dynamic").unwrap(),
        callback_account: None,
        callbacks: vec![],
        grants: vec![],
    });
    let declared = Fixture::with_native_host(Some(native));
    assert!(matches!(
        declared.host.compose_agent_customizer(None),
        Err(GatewayHostError::AgentCustomizerRequired)
    ));
    // The hook neither consumes native configuration nor opens a runtime.
    assert!(declared.host.grant_configuration.lock().unwrap().is_some());
    assert!(declared.host.machine.get().is_none());
    let inner: Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer> =
        Arc::new(PassiveCustomizer);
    assert!(
        declared
            .host
            .compose_agent_customizer(Some(inner))
            .unwrap()
            .is_some()
    );
    declared.channel.send(true).unwrap();
    assert!(matches!(
        declared.host.compose_agent_customizer(None),
        Err(GatewayHostError::Denied)
    ));
}
