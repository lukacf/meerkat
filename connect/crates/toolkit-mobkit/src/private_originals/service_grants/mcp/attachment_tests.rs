//! Source/grant preparation controls. No native attachment authority, server,
//! configuration write or actor entry is exercised by these fixtures.
use super::*;
use crate::original_ingress::tests::principal;
use meerkat_authorization_contracts::constraints::{AudienceRef, OperationRestrictionValues};
use meerkat_core::mcp_attachment_control::McpAttachmentFacts;
use meerkat_core::mcp_config::McpScope;

fn live_decision(config: &McpServerConfig) -> PrivateServiceMcpAccessDecision {
    let session = meerkat_core::SessionId::new();
    let mut decision = decision();
    decision.permission.connection = Id::new(&config.name).unwrap();
    decision.permission.resource = Id::new(&config.name).unwrap();
    decision.permission.lifetime = RoleLifetime {
        scope: LifetimeScope::Task { task: Id::new("approved-b").unwrap() },
        expires_at_ms: Some(u64::MAX),
    };
    decision.connection_revision = Revision::new(digest(&canonical_json(config).unwrap())).unwrap();
    decision.live_attachment = Some(PrivateServiceMcpLiveAttachment {
        destination: NativeWorkTarget {
            logical_owner: principal("agent"),
            logical_runtime: EvidenceId::new(meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()).unwrap(),
            context: EvidenceId::new(session.to_string()).unwrap(), context_generation: 1,
            audience: AudienceRef::Principal { principal: principal("user") },
        },
    });
    decision
}
fn control(route: &PrivateOriginalRoute, target: &NativeWorkTarget, config: &McpServerConfig)
    -> Arc<McpAttachmentControlRequest>
{
    McpAttachmentControlRequest::from_trusted_ingress(
        meerkat_core::SessionId::parse(target.context.as_str()).unwrap(), config.clone(),
        McpAttachmentFacts {
            requester: route.requester.clone(), actor: route.ingress_actor.clone(), realm: route.realm.clone(),
            destination: meerkat_runtime::meerkat_machine::scoped_destination_target(target),
        }, Arc::new(()),
    ).unwrap()
}

#[tokio::test]
async fn explicit_live_effect_matches_exact_native_control_and_never_persistence() {
    for connector in [true, false] {
        let dir = directory();
        let path = dir.path().join("grants");
        let owner = initialized(&path);
        let mut config = config();
        if !connector { config.connector = None; }
        let selected = if connector { selection(&config) }
            else { ServiceMcpSelection::no_connector(&config).unwrap() };
        let decision = live_decision(&config);
        let adapter = selected_adapter_for_decision(owner.clone(), &dir.path().join("originals"),
            &selected, &decision).await;
        let observed = apply(&adapter, decision.clone(), selected.clone()).await.unwrap();
        assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
        let target = &decision.live_attachment.as_ref().unwrap().destination;
        let route = &adapter.routes.get().unwrap()[0];
        let exact = control(route, target, &config);
        let binding = exact.effect_binding(McpAttachmentEffect::Apply);
        let original = original(&adapter, "approved-b");
        let before = std::fs::read(&path).unwrap();
        let row_before = adapter.store.get(&adapter.access_key(&observed.grant).unwrap()).await.unwrap().unwrap();
        adapter.observe_reviewed_mcp_attachment(&observed.grant, &original, target, &exact, &binding).await.unwrap();
        for scope in [McpScope::User, McpScope::Project] {
            assert!(matches!(adapter.observe_reviewed_mcp_attachment(&observed.grant, &original,
                target, &exact, &exact.effect_binding(McpAttachmentEffect::Persist(scope))).await,
                Err(PrivateOriginalError::Denied)));
        }
        for changed in ["command", "account", "destination", "generation", "requester", "binding"] {
            let mut changed_config = config.clone();
            let mut changed_target = target.clone();
            let mut changed_route = route.clone();
            match changed {
                "command" => {
                    changed_config = McpServerConfig::stdio("mail", "other-command", vec![], Default::default());
                    changed_config.connector = config.connector.clone();
                },
                "account" => {
                    if let Some(binding) = &mut changed_config.connector { binding.account = "other-account".into(); }
                    else { changed_config.connector = super::config().connector; }
                }
                "destination" => {
                    let session = meerkat_core::SessionId::new();
                    changed_target.context = EvidenceId::new(session.to_string()).unwrap();
                    changed_target.logical_runtime = EvidenceId::new(meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()).unwrap();
                }
                "generation" => changed_target.context_generation += 1,
                "requester" => changed_route.requester = principal("other-requester"),
                "binding" => {},
                _ => unreachable!(),
            }
            let other = control(&changed_route, &changed_target, &changed_config);
            let other_binding = if changed == "binding" { binding.clone() }
                else { other.effect_binding(McpAttachmentEffect::Apply) };
            assert!(matches!(adapter.observe_reviewed_mcp_attachment(&observed.grant, &original,
                &changed_target, &other, &other_binding).await, Err(PrivateOriginalError::Denied)), "{changed}");
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(adapter.store.get(&adapter.access_key(&observed.grant).unwrap()).await.unwrap().unwrap(), row_before);
    }
}

#[tokio::test]
async fn tool_only_parent_and_legacy_decision_cannot_supply_live_effect() {
    let dir = directory();
    let owner = initialized(&dir.path().join("grants"));
    let config = config();
    let selected = selection(&config);
    let adapter = selected_adapter(owner.clone(), &dir.path().join("originals"), &selected).await;
    let mut tool_only = live_decision(&config);
    let target = tool_only.live_attachment.take().unwrap().destination;
    let observed = apply(&adapter, tool_only.clone(), selected.clone()).await.unwrap();
    assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
    let exact = control(&adapter.routes.get().unwrap()[0], &target, &config);
    let effect = exact.effect_binding(McpAttachmentEffect::Apply);
    assert!(matches!(adapter.observe_reviewed_mcp_attachment(&observed.grant,
        &original(&adapter, "approved-b"), &target, &exact, &effect).await, Err(PrivateOriginalError::Denied)));
    let stored = adapter.store.get(&adapter.access_key(&observed.grant).unwrap()).await.unwrap().unwrap();
    assert!(stored.data.body["decision"].get("live_attachment").is_none());
    // Old v3 serialization and its digest remain exactly the absent-field form.
    let decoded = adapter.decode_access(&stored, &observed.grant).unwrap();
    assert_eq!(canonical_json(&decoded).unwrap(), canonical_json(&stored.data.body).unwrap());
    let mut explicit = tool_only.clone();
    explicit.decision = Id::new("new-live-decision").unwrap();
    explicit.live_attachment = Some(PrivateServiceMcpLiveAttachment { destination: target.clone() });
    let values = selected.0.attachment_operation(&adapter.routes.get().unwrap()[0], &explicit).unwrap().unwrap();
    let lineage = adapter.service_grant_lineage(&observed.grant, &original(&adapter, "approved-b")).await.unwrap();
    let resolved = owner.grants().resolve_lineage(&lineage, &adapter.routes.get().unwrap()[0].logical_executor, None).unwrap();
    assert!(resolved.restrictions().check_bounds(OperationRestrictionValues {
        action: &values.action, resource_domain: &values.resource_domain,
        processor: &values.processor, audience: &values.audience, now_ms: owner.clock().now().unwrap().unix_ms,
    }).is_err());
    drop(resolved);
    assert_eq!(apply(&adapter, explicit, selected).await.unwrap().state, PrivateServiceGrantState::GrantUnavailable);

    let dir = directory();
    let owner = initialized(&dir.path().join("grants"));
    let adapter = super::super::super::tests::adapter(owner, &dir.path().join("originals")).await;
    let legacy = adapter.apply_service_access_decision(super::super::super::tests::decision()).await.unwrap();
    let exact = control(&adapter.routes.get().unwrap()[0], &target, &config);
    assert!(matches!(adapter.observe_reviewed_mcp_attachment(&legacy.grant,
        &original(&adapter, "approved-b"), &target, &exact, &exact.effect_binding(McpAttachmentEffect::Apply)).await,
        Err(PrivateOriginalError::Denied)));
}

#[tokio::test]
async fn revoked_live_leaf_is_not_recovered_from_its_retained_decision() {
    let dir = directory();
    let owner = initialized(&dir.path().join("grants"));
    let config = config();
    let selected = selection(&config);
    let decision = live_decision(&config);
    let adapter = selected_adapter_for_decision(owner.clone(), &dir.path().join("originals"), &selected, &decision).await;
    let observed = apply(&adapter, decision.clone(), selected).await.unwrap();
    let target = &decision.live_attachment.as_ref().unwrap().destination;
    let exact = control(&adapter.routes.get().unwrap()[0], target, &config);
    let effect = exact.effect_binding(McpAttachmentEffect::Apply);
    adapter.observe_reviewed_mcp_attachment(&observed.grant, &original(&adapter, "approved-b"), target, &exact, &effect).await.unwrap();
    let (retained, _) = bind(&adapter, &observed.grant, "approved-b").await;
    let native = crate::original_ingress::tests::machine(&owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    owner.cancel_original(&retained, &app(), &mut custody).unwrap();
    assert!(matches!(adapter.observe_reviewed_mcp_attachment(&observed.grant,
        &original(&adapter, "approved-b"), target, &exact, &effect).await, Err(PrivateOriginalError::Unavailable)));
}

#[tokio::test]
async fn catalog_scope_requires_explicit_parent_and_uses_exact_original_session() {
    for parent_catalog in [false, true] {
        let dir = directory();
        let owner = initialized(&dir.path().join("grants"));
        let config = config();
        let selected = selection(&config);
        let mut requested = live_decision(&config);
        requested.catalog = Some(PrivateServiceMcpCatalog::OriginalSession);
        let mut parent = requested.clone();
        if !parent_catalog { parent.catalog = None; }
        let adapter = selected_adapter_for_decision(owner.clone(), &dir.path().join("originals"), &selected, &parent).await;
        let observed = apply(&adapter, requested.clone(), selected.clone()).await.unwrap();
        if !parent_catalog {
            assert_eq!(observed.state, PrivateServiceGrantState::GrantUnavailable);
            continue;
        }
        assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
        let route = &adapter.routes.get().unwrap()[0];
        let lineage = adapter.service_grant_lineage(&observed.grant, &original(&adapter, "approved-b")).await.unwrap();
        let resolved = owner.grants().resolve_lineage(&lineage, &route.logical_executor, None).unwrap();
        let session = meerkat_core::SessionId::parse(requested.live_attachment.as_ref().unwrap().destination.context.as_str()).unwrap();
        for selected_session in [session.clone(), meerkat_core::SessionId::new()] {
            let values = application_operations::catalog_read(&route.grant_caller, &route.logical_executor,
                &route.grant_audience, &selected_session);
            assert_eq!(resolved.restrictions().check_bounds(OperationRestrictionValues {
                action: &values.action, resource_domain: &values.resource_domain,
                processor: &values.processor, audience: &values.audience,
                now_ms: owner.clock().now().unwrap().unix_ms,
            }).is_ok(), selected_session == session);
        }
    }
    let legacy = decision();
    assert!(serde_json::to_value(&legacy).unwrap().get("catalog").is_none());
}
