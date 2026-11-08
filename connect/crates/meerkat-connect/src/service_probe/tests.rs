//! Source controls for host selection and retirement, not provider acceptance.
use super::*;
use meerkat_authorization_contracts::{
    constraints::ResourceDomain,
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    resource::ResourceRef,
};
use meerkat_core::{PrincipalKind, TrustDomainId};
use serde_json::json;

pub(crate) fn caller() -> StockAuthenticatedCaller {
    let domain = TrustDomainId::new("protected-probe-control").unwrap();
    let principal =
        PrincipalRef::in_domain(PrincipalKind::Human, "viewer", domain.clone()).unwrap();
    let authority = PrincipalRef::in_domain(PrincipalKind::RuntimeHost, "host", domain).unwrap();
    StockAuthenticatedCaller::new(
        principal,
        HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: ResourceDomain {
                    authority,
                    namespace: "login".into(),
                },
                resource_id: "exact-login".into(),
            },
            revision: EvidenceId::new("first").unwrap(),
            digest: EvidenceDigest::of_bytes(b"control-login"),
        },
    )
    .unwrap()
}
pub(crate) fn binding(root: &std::path::Path) -> ProbeHostBinding {
    let storage = crate::RealmOptions {
        context_root: root.into(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve("private-probe")
    .unwrap();
    ProbeHostBinding {
        application_id: "application".into(),
        instance_id: "one".into(),
        origin: "https://host.invalid".into(),
        viewer_id: "viewer".into(),
        login_scope_id: "login".into(),
        expires_at_ms: now() + 60_000,
        realm: storage.locator,
        context_root: root.into(),
        user_config_root: root.join("home"),
        caller: caller(),
        ingress: PrincipalRef::in_domain(
            PrincipalKind::RuntimeHost,
            "ingress",
            TrustDomainId::new("protected-probe-control").unwrap(),
        )
        .unwrap(),
    }
}
pub(crate) fn selected(id: &str) -> RequiredServiceProbe {
    RequiredServiceProbe::outbound_mcp(
        id.into(),
        "github".into(),
        "enterprise".into(),
        "agent".into(),
        "repository_read".into(),
        json!({"repository":"owner/one"}),
        json!({"repository":"owner/one"}),
        None,
    )
    .unwrap()
}

#[test]
fn trusted_selection_refuses_duplicate_profiles_or_foreign_ingress_alias() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        ServiceProbeSelection::new(binding(dir.path()), vec![selected("one"), selected("one")])
            .is_err()
    );
    let mut same = binding(dir.path());
    same.ingress = same.caller.requester().clone();
    assert!(ServiceProbeSelection::new(same, vec![selected("one")]).is_err());
    let mut expired = binding(dir.path());
    expired.expires_at_ms = now();
    assert!(ServiceProbeSelection::new(expired, vec![selected("one")]).is_err());
    let mut relative = binding(dir.path());
    relative.context_root = "relative".into();
    assert!(ServiceProbeSelection::new(relative, vec![selected("one")]).is_err());
    let exact = ServiceProbeSelection::new(binding(dir.path()), vec![selected("one")]).unwrap();
    assert!(exact.lifetime.current(exact.binding.expires_at_ms));
    exact.control().retire();
    assert!(!exact.lifetime.current(exact.binding.expires_at_ms));
}

#[tokio::test]
async fn original_deadline_wakes_without_pipe_retirement() {
    let lifetime = ProbeLifetime::default();
    let expiry = now() + 20;
    tokio::time::timeout(std::time::Duration::from_secs(2), lifetime.closed(expiry))
        .await
        .unwrap();
    assert!(!lifetime.current(expiry));
    assert!(
        !lifetime.retired.load(Ordering::Acquire),
        "expiry is not a fabricated logout"
    );
}

#[tokio::test]
async fn actual_retirement_wakes_waiter_and_refuses_later_publication_join() {
    let lifetime = Arc::new(ProbeLifetime::default());
    lifetime.join(LocalAuthorizationPublication::new()).unwrap();
    let waiter = lifetime.closed(now() + 60_000);
    tokio::pin!(waiter);
    assert!(futures_util::poll!(waiter.as_mut()).is_pending());
    lifetime.retire();
    tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .unwrap();
    assert!(lifetime.join(LocalAuthorizationPublication::new()).is_err());
    lifetime.retire();
    assert!(!lifetime.current(now() + 60_000));
}

#[test]
fn dynamic_resource_contract_receives_exact_native_connection_and_result() {
    struct Repository {
        host: String,
        repository: String,
    }
    impl ProbeResultContract for Repository {
        fn accepts_target(&self, target: &ProbeResourceTarget<'_>) -> bool {
            target.installed_connection.name == "ghe"
                && matches!(&target.installed_connection.transport,meerkat_core::mcp_config::McpTransportConfig::Http(http) if http.url==self.host)
                && target.raw_operation == "repository_read"
                && target.resource == &json!({"repository":self.repository})
                && target.arguments == &json!({"repository":self.repository})
        }
        fn assess(&self, observation: &CommittedProbeResult<'_>) -> ResourceAssessment {
            if !self.accepts_target(&observation.target) {
                return ResourceAssessment::NotVerified;
            }
            match serde_json::from_str::<Value>(&observation.result.text_content()) {
                Ok(value) if value == json!({"repository":self.repository,"permission":"read"}) => {
                    ResourceAssessment::Verified
                }
                Ok(_) => ResourceAssessment::NotVerified,
                Err(_) => ResourceAssessment::Unknown,
            }
        }
    }
    let config: meerkat_core::McpServerConfig = serde_json::from_value(json!({"name":"ghe",
        "url":"https://ghe.invalid/mcp"}))
    .unwrap();
    let requirement = Repository {
        host: "https://ghe.invalid/mcp".into(),
        repository: "workspace/private".into(),
    };
    let resource = json!({"repository":"workspace/private"});
    let arguments = resource.clone();
    let target = || ProbeResourceTarget {
        profile_id: "read-private",
        resource: &resource,
        raw_operation: "repository_read",
        arguments: &arguments,
        installed_connection: &config,
    };
    let exact = ToolResult::new(
        "call".into(),
        json!({"repository":"workspace/private","permission":"read"}).to_string(),
        false,
    );
    assert_eq!(
        requirement.assess(&CommittedProbeResult {
            target: target(),
            result: &exact
        }),
        ResourceAssessment::Verified
    );
    let generic = ToolResult::new("call".into(), "success".into(), false);
    assert_eq!(
        requirement.assess(&CommittedProbeResult {
            target: target(),
            result: &generic
        }),
        ResourceAssessment::Unknown
    );
    let foreign = Repository {
        repository: "other/private".into(),
        ..requirement
    };
    assert!(!foreign.accepts_target(&target()));
    assert_eq!(
        foreign.assess(&CommittedProbeResult {
            target: target(),
            result: &exact
        }),
        ResourceAssessment::NotVerified
    );
}
