#![allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_imports,
    redundant_semicolons,
    clippy::redundant_clone
)]
//! Tests for identity-first continuity core types.

use meerkat_meta::identity_first::*;
use std::collections::BTreeMap;

// ---- Task 0.1 + 0.2: AgentIdentity + AgentRuntimeId ----

#[test]
fn identity_first_types_agent_identity_parse_valid() {
    let id = AgentIdentity::parse("triage:main").expect("should parse");
    assert_eq!(id.as_str(), "triage:main");
}

#[test]
fn identity_first_types_agent_identity_parse_empty_fails() {
    let err = AgentIdentity::parse("").expect_err("should fail");
    assert_eq!(err.input, "");
    assert!(err.reason.contains("empty"));
}

#[test]
fn identity_first_types_agent_identity_parse_slash_fails() {
    let err = AgentIdentity::parse("a/b").expect_err("should fail");
    assert_eq!(err.input, "a/b");
    assert!(err.reason.contains("slash"));
}

#[test]
fn identity_first_types_agent_identity_parse_whitespace_fails() {
    let err = AgentIdentity::parse("a b").expect_err("should fail");
    assert_eq!(err.input, "a b");
    assert!(err.reason.contains("whitespace"));
}

#[test]
fn identity_first_types_agent_identity_serde_roundtrip() {
    let id = AgentIdentity::parse("triage:main").expect("should parse");
    let json = serde_json::to_string(&id).expect("serialize");
    let back: AgentIdentity = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, back);
}

#[test]
fn identity_first_types_agent_runtime_id_parse_valid() {
    let id = AgentRuntimeId::parse("rt:abc123").expect("should parse");
    assert_eq!(id.as_str(), "rt:abc123");
}

#[test]
fn identity_first_types_agent_runtime_id_parse_empty_fails() {
    AgentRuntimeId::parse("").expect_err("should fail");
}

#[test]
fn identity_first_types_agent_runtime_id_serde_roundtrip() {
    let id = AgentRuntimeId::parse("rt:abc123").expect("should parse");
    let json = serde_json::to_string(&id).expect("serialize");
    let back: AgentRuntimeId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(id, back);
}

// ---- Task 0.3: AgentAddressability ----

#[test]
fn identity_first_types_addressability_default_is_addressable() {
    assert_eq!(
        AgentAddressability::default(),
        AgentAddressability::Addressable
    );
}

#[test]
fn identity_first_types_addressability_serde_roundtrip() {
    for variant in [
        AgentAddressability::Addressable,
        AgentAddressability::InternalOnly,
    ] {
        let json = serde_json::to_string(&variant).expect("serialize");
        let back: AgentAddressability = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, back);
    }
    // Check lowercase strings
    assert_eq!(
        serde_json::to_string(&AgentAddressability::Addressable).expect("ser"),
        "\"addressable\""
    );
    assert_eq!(
        serde_json::to_string(&AgentAddressability::InternalOnly).expect("ser"),
        "\"internal_only\""
    );
}

// ---- Task 0.4: DisplayName ----

#[test]
fn identity_first_types_display_name_non_empty() {
    DisplayName::parse("").expect_err("should fail");
    let dn = DisplayName::parse("Triage Agent").expect("should parse");
    assert_eq!(dn.as_str(), "Triage Agent");
}

#[test]
fn identity_first_types_display_name_serde_roundtrip() {
    let dn = DisplayName::parse("Triage Agent").expect("should parse");
    let json = serde_json::to_string(&dn).expect("serialize");
    let back: DisplayName = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(dn, back);
}

// ---- Task 0.5: ContinuityGeneration, CheckpointVersion, FencingToken ----

#[test]
fn identity_first_types_monotonic_u64_ord() {
    let a = ContinuityGeneration::new(1);
    let b = ContinuityGeneration::new(2);
    assert!(a < b);
    assert_eq!(a.get(), 1);
}

#[test]
fn identity_first_types_monotonic_u64_serde_as_integer() {
    let generation = ContinuityGeneration::new(42);
    let json = serde_json::to_string(&generation).expect("serialize");
    assert_eq!(json, "42");
    let back: ContinuityGeneration = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(generation, back);

    let cv = CheckpointVersion::new(7);
    let json = serde_json::to_string(&cv).expect("serialize");
    assert_eq!(json, "7");

    let ft = FencingToken::new(99);
    assert_eq!(ft.to_string(), "99");
}

#[test]
fn identity_first_types_fencing_token_display() {
    let ft = FencingToken::new(1234);
    assert_eq!(ft.to_string(), "1234");
}

// ---- Task 0.6: CorrelationId, DispatchIdempotencyKey ----

#[test]
fn identity_first_types_string_newtypes_serde_roundtrip() {
    let cid = CorrelationId::new("req-123");
    let json = serde_json::to_string(&cid).expect("serialize");
    let back: CorrelationId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(cid, back);

    let dik = DispatchIdempotencyKey::new("idem-abc");
    let json = serde_json::to_string(&dik).expect("serialize");
    let back: DispatchIdempotencyKey = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(dik, back);
}

// ---- Task 0.7: InvalidIdentity error ----

#[test]
fn identity_first_types_invalid_identity_error() {
    let err = InvalidIdentity {
        input: "bad input".to_string(),
        reason: "contains whitespace".to_string(),
    };
    assert_eq!(err.input, "bad input");
    assert_eq!(err.reason, "contains whitespace");
    // Must impl std::error::Error
    let _: &dyn std::error::Error = &err;
    assert!(err.to_string().contains("bad input"));
}

// ---- Task 0.8: ContinuityRecord ----

#[test]
fn identity_first_types_continuity_record_serde_roundtrip() {
    let record = ContinuityRecord {
        identity: AgentIdentity::parse("triage:main").expect("parse"),
        agent_runtime_id: AgentRuntimeId::parse("rt:001").expect("parse"),
        session_id: meerkat_core::types::SessionId::new(),
        generation: ContinuityGeneration::new(0),
        checkpoint_version: CheckpointVersion::new(3),
    };
    let json = serde_json::to_string(&record).expect("serialize");
    let back: ContinuityRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(record, back);
}

// ---- Task 0.9: ContinuityResolveState ----

#[test]
fn identity_first_types_resolve_state_uninitialized_roundtrip() {
    let state = ContinuityResolveState::Uninitialized;
    let json = serde_json::to_string(&state).expect("serialize");
    let back: ContinuityResolveState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(state, back);
}

#[test]
fn identity_first_types_resolve_state_ready_roundtrip() {
    let record = ContinuityRecord {
        identity: AgentIdentity::parse("gate:main").expect("parse"),
        agent_runtime_id: AgentRuntimeId::parse("rt:002").expect("parse"),
        session_id: meerkat_core::types::SessionId::new(),
        generation: ContinuityGeneration::new(1),
        checkpoint_version: CheckpointVersion::new(5),
    };
    let state = ContinuityResolveState::Ready {
        record: record.clone(),
    };
    let json = serde_json::to_string(&state).expect("serialize");
    let back: ContinuityResolveState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(state, back);
}

#[test]
fn identity_first_types_resolve_state_broken_roundtrip() {
    let failure = ContinuityFailure {
        identity: AgentIdentity::parse("broken:one").expect("parse"),
        kind: ContinuityFailureKind::SnapshotMissing,
        record: None,
        detail: "snapshot file not found".to_string(),
    };
    let state = ContinuityResolveState::Broken {
        failure: failure.clone(),
    };
    let json = serde_json::to_string(&state).expect("serialize");
    let back: ContinuityResolveState = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(state, back);
}

// ---- Task 0.10: ContinuityFailure + ContinuityFailureKind ----

#[test]
fn identity_first_types_failure_kind_all_variants() {
    let kinds = [
        ContinuityFailureKind::SnapshotMissing,
        ContinuityFailureKind::SnapshotCorrupted,
        ContinuityFailureKind::GenerationMismatch,
        ContinuityFailureKind::StoreUnavailable,
        ContinuityFailureKind::ResumeRejected,
        ContinuityFailureKind::EmbodimentFailed,
        ContinuityFailureKind::CheckpointUnrecoverable,
    ];
    for kind in &kinds {
        let json = serde_json::to_string(kind).expect("serialize");
        let back: ContinuityFailureKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(kind, &back);
    }
}

#[test]
fn identity_first_types_failure_with_record_roundtrip() {
    let record = ContinuityRecord {
        identity: AgentIdentity::parse("agent:x").expect("parse"),
        agent_runtime_id: AgentRuntimeId::parse("rt:x").expect("parse"),
        session_id: meerkat_core::types::SessionId::new(),
        generation: ContinuityGeneration::new(2),
        checkpoint_version: CheckpointVersion::new(10),
    };
    let failure = ContinuityFailure {
        identity: AgentIdentity::parse("agent:x").expect("parse"),
        kind: ContinuityFailureKind::GenerationMismatch,
        record: Some(record),
        detail: "expected gen 3".to_string(),
    };
    let json = serde_json::to_string(&failure).expect("serialize");
    let back: ContinuityFailure = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(failure, back);
}

// ---- Task 0.11: LeaseGrant ----

#[test]
fn identity_first_types_lease_grant_ttl_as_ms() {
    let grant = LeaseGrant {
        identity: AgentIdentity::parse("agent:a").expect("parse"),
        fencing_token: FencingToken::new(1),
        ttl: std::time::Duration::from_secs(30),
    };
    let json = serde_json::to_value(&grant).expect("serialize");
    assert_eq!(json["ttl"], 30_000);

    let back: LeaseGrant = serde_json::from_value(json).expect("deserialize");
    assert_eq!(grant, back);
}

// ---- Task 0.12: LeaseAcquireResult + LeaseRenewResult ----

#[test]
fn identity_first_types_lease_acquire_result_roundtrip() {
    let grant = LeaseGrant {
        identity: AgentIdentity::parse("agent:a").expect("parse"),
        fencing_token: FencingToken::new(1),
        ttl: std::time::Duration::from_secs(5),
    };
    let acquired = LeaseAcquireResult::Acquired(grant.clone());
    let json = serde_json::to_string(&acquired).expect("serialize");
    let back: LeaseAcquireResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(acquired, back);

    let held = LeaseAcquireResult::AlreadyHeld {
        identity: AgentIdentity::parse("agent:a").expect("parse"),
        holder: "other-runtime".to_string(),
    };
    let json = serde_json::to_string(&held).expect("serialize");
    let back: LeaseAcquireResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(held, back);
}

#[test]
fn identity_first_types_lease_renew_result_roundtrip() {
    let grant = LeaseGrant {
        identity: AgentIdentity::parse("agent:b").expect("parse"),
        fencing_token: FencingToken::new(2),
        ttl: std::time::Duration::from_secs(10),
    };
    let renewed = LeaseRenewResult::Renewed(grant);
    let json = serde_json::to_string(&renewed).expect("serialize");
    let back: LeaseRenewResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(renewed, back);

    let lost = LeaseRenewResult::Lost {
        identity: AgentIdentity::parse("agent:b").expect("parse"),
    };
    let json = serde_json::to_string(&lost).expect("serialize");
    let back: LeaseRenewResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(lost, back);
}

// ---- Task 0.13: DurableAgentSpec ----

#[test]
fn identity_first_types_durable_agent_spec_roundtrip() {
    let spec = DurableAgentSpec {
        identity: AgentIdentity::parse("triage:main").expect("parse"),
        profile: meerkat_mob::ProfileName::from("lead"),
        addressability: AgentAddressability::InternalOnly,
        display_name: Some(DisplayName::parse("Triage").expect("parse")),
        labels: {
            let mut m = BTreeMap::new();
            m.insert("env".to_string(), "prod".to_string());
            m
        },
        context: Some(serde_json::json!({"key": "value"})),
        additional_instructions: vec!["Be helpful.".to_string()],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
    let json = serde_json::to_string(&spec).expect("serialize");
    let back: DurableAgentSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(spec, back);
}

#[test]
fn identity_first_types_durable_agent_spec_addressability_defaults() {
    // When addressability is omitted from JSON, it should default to Addressable
    let json = r#"{"identity":"x:y","profile":"worker"}"#;
    let spec: DurableAgentSpec = serde_json::from_str(json).expect("deserialize");
    assert_eq!(spec.addressability, AgentAddressability::Addressable);
}

// ---- Task 0.14: DispatchInput + DispatchOrigin ----

#[test]
fn identity_first_types_dispatch_origin_snake_case() {
    let origins = [
        (DispatchOrigin::Connector, "\"connector\""),
        (DispatchOrigin::Scheduler, "\"scheduler\""),
        (DispatchOrigin::Policy, "\"policy\""),
        (DispatchOrigin::Flow, "\"flow\""),
        (DispatchOrigin::System, "\"system\""),
    ];
    for (variant, expected_json) in &origins {
        let json = serde_json::to_string(variant).expect("serialize");
        assert_eq!(&json, expected_json);
        let back: DispatchOrigin = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, &back);
    }
}

#[test]
fn identity_first_types_dispatch_input_roundtrip() {
    let input = DispatchInput {
        content: meerkat_core::ContentInput::Text("hello".to_string()),
        origin: DispatchOrigin::Connector,
        correlation_id: Some(CorrelationId::new("corr-1")),
        idempotency_key: Some(DispatchIdempotencyKey::new("idem-1")),
    };
    let json = serde_json::to_string(&input).expect("serialize");
    let back: DispatchInput = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(input, back);
}

#[test]
fn identity_first_types_dispatch_input_multimodal_roundtrip() {
    use meerkat_core::types::{ContentBlock, ImageData};

    let input = DispatchInput {
        content: meerkat_core::ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: "Look at this image:".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "iVBORw0KGgo=".to_string(),
                },
            },
        ]),
        origin: DispatchOrigin::Connector,
        correlation_id: Some(CorrelationId::new("corr-multi")),
        idempotency_key: None,
    };
    let json = serde_json::to_string(&input).expect("serialize");
    let back: DispatchInput = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(input, back);
}

#[test]
fn identity_first_types_dispatch_input_optional_fields() {
    let input = DispatchInput {
        content: meerkat_core::ContentInput::Text("hello".to_string()),
        origin: DispatchOrigin::System,
        correlation_id: None,
        idempotency_key: None,
    };
    let json = serde_json::to_string(&input).expect("serialize");
    let back: DispatchInput = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(input, back);
}

// ---- Task 0.15: ManagedPeerEdge ----

#[test]
fn identity_first_types_managed_peer_edge_canonical_ordering() {
    let a = AgentIdentity::parse("beta:main").expect("parse");
    let b = AgentIdentity::parse("alpha:main").expect("parse");
    let edge = ManagedPeerEdge::new(a, b).expect("should succeed");
    // a should be "alpha:main" (sorted)
    assert_eq!(edge.a().as_str(), "alpha:main");
    assert_eq!(edge.b().as_str(), "beta:main");
}

#[test]
fn identity_first_types_managed_peer_edge_self_edge_rejected() {
    let a = AgentIdentity::parse("same:agent").expect("parse");
    let b = AgentIdentity::parse("same:agent").expect("parse");
    let err = ManagedPeerEdge::new(a, b).expect_err("self-edge should fail");
    assert_eq!(err, ManagedPeerEdgeError::SelfEdge);
}

#[test]
fn identity_first_types_managed_peer_edge_serde_roundtrip() {
    let edge = ManagedPeerEdge::new(
        AgentIdentity::parse("a:main").expect("parse"),
        AgentIdentity::parse("b:main").expect("parse"),
    )
    .expect("should succeed");
    let json = serde_json::to_string(&edge).expect("serialize");
    let back: ManagedPeerEdge = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(edge, back);
}

#[test]
fn identity_first_types_managed_peer_edge_deser_rejects_self_edge() {
    // Craft JSON with a == b — must be rejected by deserialization
    let json = r#"{"a":"x:main","b":"x:main"}"#;
    let result: Result<ManagedPeerEdge, _> = serde_json::from_str(json);
    assert!(result.is_err(), "deserializing a self-edge must fail");
}

#[test]
fn identity_first_types_managed_peer_edge_deser_reorders_non_canonical() {
    // b < a in JSON — deserialization should canonicalize
    let json = r#"{"a":"z:main","b":"a:main"}"#;
    let edge: ManagedPeerEdge = serde_json::from_str(json).expect("deserialize");
    assert_eq!(edge.a().as_str(), "a:main");
    assert_eq!(edge.b().as_str(), "z:main");
}

// ---- Task 0.16: NotAddressable error ----

#[test]
fn identity_first_types_not_addressable_error() {
    let err = NotAddressable {
        identity: AgentIdentity::parse("internal:agent").expect("parse"),
        addressability: AgentAddressability::InternalOnly,
    };
    let _: &dyn std::error::Error = &err;
    assert!(err.to_string().contains("internal:agent"));
    assert!(err.to_string().contains("not addressable"));
}

// ---- Task 0.17: IdentityStatus + DurabilityPolicy ----

#[test]
fn identity_first_types_identity_status_full_roundtrip() {
    let status = IdentityStatus {
        identity: AgentIdentity::parse("triage:main").expect("parse"),
        state: IdentityLifecycleState::Active,
        agent_runtime_id: Some(AgentRuntimeId::parse("rt:001").expect("parse")),
        session_id: Some(meerkat_core::types::SessionId::new()),
        member_id: Some("mk--triage-main".to_string()),
        profile: Some(meerkat_mob::ProfileName::from("lead")),
        runtime_mode: None,
        addressability: AgentAddressability::Addressable,
        display_name: Some(DisplayName::parse("Triage Agent").expect("parse")),
        labels: {
            let mut m = BTreeMap::new();
            m.insert("team".to_string(), "core".to_string());
            m
        },
        generation: Some(meerkat_mob::Generation::new(2)),
        intent_managed: false,
        convergence: None,
        wake_refusal: Some(WakeRefusal {
            code: -32005,
            session_id: meerkat_core::types::SessionId::new(),
            remedy: WakeRefusalRemedy::OperatorReview,
        }),
        replaced_session: None,
        customizer_tools_pending: None,
        session_repair_required: Some(
            meerkat_meta::identity_first::SessionRepairRequired::audited_endpoint_divergence(
                meerkat_core::types::SessionId::new(),
                Some(&meerkat_meta::identity_first::SessionRepairScope {
                    state_root: Some("/srv/state".into()),
                    realm: Some("meta".to_string()),
                    runtime_store: None,
                }),
                "live transcript does not preserve the graph-proved audited endpoint",
            ),
        ),
    };
    let json = serde_json::to_string(&status).expect("serialize");
    let back: IdentityStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(status, back);
    // The hold rides meerkat's stable wire token and the exact commands.
    let val: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    assert_eq!(
        val["session_repair_required"]["hold"],
        "audited_endpoint_divergence"
    );
    let session = val["session_id"].as_str().expect("session id");
    let held = val["session_repair_required"]["session_id"]
        .as_str()
        .expect("held session id");
    assert!(
        val["session_repair_required"]["diagnose_command"]
            .as_str()
            .unwrap()
            .ends_with(&format!("session repair-wholeblob {held} --json")),
        "{val}"
    );
    assert!(
        val["session_repair_required"]["apply_command"]
            .as_str()
            .unwrap()
            .starts_with("rkat --state-root /srv/state --realm meta session repair-wholeblob"),
        "{val}"
    );
    let _ = session;
    // Core's mob member id of the binding rides as an optional string.
    assert_eq!(val["member_id"], "mk--triage-main");
    // The generation is Core's; the refused wake rides with its remedy.
    assert_eq!(val["generation"], 2);
    assert_eq!(val["wake_refusal"]["remedy"], "operator_review");
}

#[test]
fn identity_first_types_lifecycle_state_all_variants() {
    let states = [
        IdentityLifecycleState::Dormant,
        IdentityLifecycleState::Active,
        IdentityLifecycleState::Broken,
    ];
    for state in &states {
        let json = serde_json::to_string(state).expect("serialize");
        let back: IdentityLifecycleState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(state, &back);
    }
}

#[test]
fn identity_first_types_durability_policy_buffered_export() {
    let policy = DurabilityPolicy::BufferedExport {
        max_loss_window_ms: 5000,
    };
    let json = serde_json::to_string(&policy).expect("serialize");
    let back: DurabilityPolicy = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(policy, back);

    let val: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    assert_eq!(val["kind"], "buffered_export");
    assert_eq!(val["max_loss_window_ms"], 5000);
}

// ---- Task 0.18: AgentBuildContext + AgentBuildDraft + ExternalToolDef ----

#[test]
fn identity_first_types_agent_build_context_roundtrip() {
    let edge = ManagedPeerEdge::new(
        AgentIdentity::parse("a:main").expect("parse"),
        AgentIdentity::parse("b:main").expect("parse"),
    )
    .expect("edge");

    let ctx = AgentBuildContext {
        identity: AgentIdentity::parse("a:main").expect("parse"),
        active_peers: vec![
            AgentIdentity::parse("a:main").expect("parse"),
            AgentIdentity::parse("b:main").expect("parse"),
        ],
        managed_edges: vec![edge],
        runtime_services: Default::default(),
    };
    let json = serde_json::to_string(&ctx).expect("serialize");
    let back: AgentBuildContext = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(ctx, back);
}

#[test]
fn identity_first_types_agent_build_draft_roundtrip() {
    let draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: Some("claude-sonnet-4-6".to_string()),
        system_prompt: Some("You are a helpful agent.".to_string()),
        additional_instructions: vec!["Be concise.".to_string()],
        labels: {
            let mut m = BTreeMap::new();
            m.insert("role".to_string(), "triage".to_string());
            m
        },
        app_context: Some(serde_json::json!({"version": 2})),
        external_tools: vec![ExternalToolDef {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
            artifact: None,
        }],
        local_external_tools: Default::default(),
        provider_params: None,
    };
    let json = serde_json::to_string(&draft).expect("serialize");
    let back: AgentBuildDraft = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(draft, back);
}

#[test]
fn identity_first_types_compaction_curator_is_strictly_host_local() {
    struct HostCurator;

    #[async_trait::async_trait]
    impl meerkat_core::CompactionCurator for HostCurator {
        async fn curate_summary(
            &self,
            _window: meerkat_core::CompactionWindow<'_>,
        ) -> Result<meerkat_core::CuratedCompactionSummary, meerkat_core::CompactionCuratorError>
        {
            meerkat_core::CuratedCompactionSummary::new("host summary")
        }
    }

    let draft = AgentBuildDraft {
        compaction_curator: CompactionCuratorOverlay::new(std::sync::Arc::new(HostCurator)),
        model: None,
        system_prompt: None,
        additional_instructions: Vec::new(),
        labels: BTreeMap::new(),
        app_context: None,
        external_tools: Vec::new(),
        local_external_tools: Default::default(),
        provider_params: None,
    };

    let json = serde_json::to_value(&draft).expect("serialize host-local draft");
    assert!(
        json.get("compaction_curator").is_none(),
        "executable host behavior must never enter the draft wire shape"
    );
    let decoded: AgentBuildDraft = serde_json::from_value(json).expect("deserialize draft");
    assert!(
        !decoded.compaction_curator.is_some(),
        "a JSON round-trip cannot mint executable host behavior"
    );
}

/// The gateway customizer round-trips the draft as JSON
/// (`serde_json::to_value(&*draft)` out, `serde_json::from_value` back), so a
/// provider-params field that does not survive that hop is silently dropped
/// on every SDK-backed build.
#[test]
fn identity_first_types_agent_build_draft_provider_params_roundtrip() {
    use meerkat_core::lifecycle::run_primitive::{
        OpenAiPromptCacheOptions, OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
    };
    use meerkat_core::model_profile::capabilities::{OpenAiPromptCacheMode, OpenAiPromptCacheTtl};

    let draft = AgentBuildDraft {
        compaction_curator: Default::default(),
        model: None,
        system_prompt: None,
        additional_instructions: vec![],
        labels: BTreeMap::new(),
        app_context: None,
        external_tools: vec![],
        local_external_tools: Default::default(),
        provider_params: Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                prompt_cache_key: Some("tenant-a:stable-prefix".to_string()),
                prompt_cache_options: Some(OpenAiPromptCacheOptions {
                    mode: Some(OpenAiPromptCacheMode::Implicit),
                    ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
                }),
                ..Default::default()
            })),
            ..Default::default()
        }),
    };

    let json = serde_json::to_string(&draft).expect("serialize");
    let back: AgentBuildDraft = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(draft, back);
}

/// Backward compatibility: every profile, persisted draft and wire payload
/// written before `provider_params` existed must still deserialize.
#[test]
fn identity_first_types_agent_build_draft_without_provider_params_deserializes() {
    let legacy = serde_json::json!({
        "model": "claude-sonnet-4-6",
        "system_prompt": null,
        "additional_instructions": [],
        "labels": {},
        "app_context": null,
        "external_tools": []
    });

    let draft: AgentBuildDraft = serde_json::from_value(legacy).expect("legacy draft");
    assert_eq!(draft.provider_params, None);

    // And the field stays off the wire when unset, so an SDK that echoes the
    // draft back sees exactly the payload shape it saw before.
    let json = serde_json::to_value(&draft).expect("serialize");
    assert!(json.get("provider_params").is_none());
}

/// Meerkat's `ProviderParamsOverride` is `deny_unknown_fields`: reusing the
/// typed shape means a mistyped or unknown knob rejects the draft at ingress
/// instead of being ferried as untyped JSON and dropped at the LLM edge.
#[test]
fn identity_first_types_agent_build_draft_rejects_unknown_provider_params_key() {
    let unknown_knob = serde_json::json!({
        "model": null,
        "system_prompt": null,
        "app_context": null,
        "provider_params": { "prompt_cache_ttl": "30m" }
    });
    assert!(
        serde_json::from_value::<AgentBuildDraft>(unknown_knob).is_err(),
        "an unknown provider-params knob must fail closed"
    );

    let unknown_tag_knob = serde_json::json!({
        "model": null,
        "system_prompt": null,
        "app_context": null,
        "provider_params": {
            "provider_tag": { "provider": "open_ai", "prompt_cache_keys": "x" }
        }
    });
    assert!(
        serde_json::from_value::<AgentBuildDraft>(unknown_tag_knob).is_err(),
        "an unknown provider-tag knob must fail closed"
    );

    let mistyped = serde_json::json!({
        "model": null,
        "system_prompt": null,
        "app_context": null,
        "provider_params": { "temperature": "warm" }
    });
    assert!(
        serde_json::from_value::<AgentBuildDraft>(mistyped).is_err(),
        "a mistyped provider-params knob must fail closed"
    );
}

#[test]
fn identity_first_types_external_tool_def_roundtrip() {
    let tool = ExternalToolDef {
        name: "calculator".to_string(),
        description: "Compute arithmetic".to_string(),
        input_schema: serde_json::json!({"type": "object"}),
        artifact: None,
    };
    let json = serde_json::to_string(&tool).expect("serialize");
    let back: ExternalToolDef = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(tool, back);
}

// ---- Task 0.19: SessionSnapshot ----

#[test]
fn identity_first_types_session_snapshot_base64_roundtrip() {
    let data = vec![0x00, 0x01, 0xFF, 0xFE, 0xAB, 0xCD];
    let snapshot = SessionSnapshot { data: data.clone() };
    let json = serde_json::to_string(&snapshot).expect("serialize");

    // Wire format: {"data": "<base64 string>"} per spec TYPE-22
    let val: serde_json::Value = serde_json::from_str(&json).expect("parse");
    assert!(
        val.is_object(),
        "snapshot should serialize as {{\"data\": \"...\"}}"
    );
    assert!(
        val.get("data").and_then(|d| d.as_str()).is_some(),
        "snapshot.data should be a base64 string"
    );

    let back: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(snapshot, back);
    assert_eq!(back.data, data);
}

// ---- Task 0.20: RosterContext + TopologyContext ----

#[test]
fn identity_first_types_roster_context_roundtrip() {
    let ctx = RosterContext {
        mob_definition: None,
        previous_identities: vec![AgentIdentity::parse("old:agent").expect("parse")],
    };
    let json = serde_json::to_string(&ctx).expect("serialize");
    let back: RosterContext = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(ctx, back);
}

#[test]
fn identity_first_types_topology_context_roundtrip() {
    let ctx = TopologyContext {
        roster: vec![DurableAgentSpec {
            identity: AgentIdentity::parse("agent:a").expect("parse"),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: vec![],
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }],
    };
    let json = serde_json::to_string(&ctx).expect("serialize");
    let back: TopologyContext = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(ctx, back);
}

// ---- Task 0.21 + 0.22: Error types ----

#[test]
fn identity_first_types_continuity_store_error_variants() {
    let errors: Vec<Box<dyn std::error::Error>> = vec![
        Box::new(ContinuityStoreError::StaleFencingToken {
            identity: AgentIdentity::parse("a:b").expect("parse"),
            presented: FencingToken::new(1),
            current: FencingToken::new(2),
        }),
        Box::new(ContinuityStoreError::StaleCheckpointVersion {
            identity: AgentIdentity::parse("a:b").expect("parse"),
            presented: CheckpointVersion::new(1),
            current: CheckpointVersion::new(2),
        }),
        Box::new(ContinuityStoreError::StaleContinuityGeneration {
            identity: AgentIdentity::parse("a:b").expect("parse"),
            presented: ContinuityGeneration::new(1),
            current: ContinuityGeneration::new(2),
        }),
        Box::new(ContinuityStoreError::NotFound {
            identity: AgentIdentity::parse("a:b").expect("parse"),
        }),
        Box::new(ContinuityStoreError::Io("disk full".to_string())),
        Box::new(ContinuityStoreError::Corruption("bad checksum".to_string())),
    ];
    for err in &errors {
        // All implement Display + Error
        assert!(!err.to_string().is_empty());
    }
}

#[test]
fn identity_first_types_lease_error_variants() {
    let e1 = LeaseError::ProviderUnavailable("timeout".to_string());
    let e2 = LeaseError::Io("broken pipe".to_string());
    let _: &dyn std::error::Error = &e1;
    let _: &dyn std::error::Error = &e2;
    assert!(e1.to_string().contains("timeout"));
}

#[test]
fn identity_first_types_roster_error_variants() {
    let e = RosterError::ProviderUnavailable("down".to_string());
    let _: &dyn std::error::Error = &e;
    assert!(e.to_string().contains("down"));
}

#[test]
fn identity_first_types_topology_error_variants() {
    let e1 = TopologyError::InvalidEdge("self-loop".to_string());
    let e2 = TopologyError::ProviderUnavailable("unreachable".to_string());
    let _: &dyn std::error::Error = &e1;
    let _: &dyn std::error::Error = &e2;
}

#[test]
fn identity_first_types_customizer_error_variants() {
    let e1 = CustomizerError::BuildFailed("missing model".to_string());
    let e2 = CustomizerError::Io("file not found".to_string());
    let _: &dyn std::error::Error = &e1;
    let _: &dyn std::error::Error = &e2;
}
