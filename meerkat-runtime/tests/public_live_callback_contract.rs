use meerkat_core::execution_scope::RunEffectScopeId;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::ops::OperationId;
use meerkat_runtime::live_request::{
    AdmittedLiveExecutionAuthority, AdmittedLiveExecutionRecord, LiveCallbackContinuationAdmission,
    LiveCallbackResultEvidence, LiveCallbackSuspensionRecord, LiveRequestChainPhase,
    LiveRequestChainTerminal, LiveRequestRunChainRecord, LiveRequestRunLink,
};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn completed_callback_witness_must_name_the_retained_batch_before_continuation() -> TestResult {
    let mut link = suspended_link();
    let callback = link.callback.as_mut().ok_or("callback")?;
    callback.results = LiveCallbackResultEvidence::Complete {
        session_id: callback.session_id.clone(),
        run_id: callback.run_id.clone(),
        batch_digest: callback.batch_digest,
        accepted_payload_digest: [20; 32],
    };
    let chain = LiveRequestRunChainRecord::new(
        OperationId(id(9)),
        admitted()?,
        vec![link],
        LiveRequestChainPhase::SuspendedCallbacks {},
    )?;
    let value = serde_json::to_value(chain)?;
    for (pointer, replacement) in [
        ("/runs/0/callback/results/session_id", json!(id(90))),
        ("/runs/0/callback/results/run_id", json!(id(90))),
        ("/runs/0/callback/results/batch_digest", json!(vec![90; 32])),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).ok_or("fixture field")? = replacement;
        assert!(serde_json::from_value::<LiveRequestRunChainRecord>(changed).is_err());
    }
    Ok(())
}

fn id(value: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(value)
}

fn admission_json() -> Value {
    json!({
        "source":{"session_id":id(1), "channel_id":"voice",
            "source":{"kind":"client_delegation","delegation":"d1"}},
        "input_id":id(2),
        "executor":{"session_id":id(1),"realm":"owner","runtime_epoch":id(3),"binding_generation":1},
        "grant":{"id":id(4),"issuer_realm":"issuer","generation":5},
        "ingress_generation_at_admission":6,
        "commit":{"revision":7,"digest":vec![8;32]}
    })
}

fn admitted() -> Result<AdmittedLiveExecutionRecord, serde_json::Error> {
    serde_json::from_value(admission_json())
}

fn running_link() -> LiveRequestRunLink {
    LiveRequestRunLink {
        input_id: InputId::from_uuid(id(2)),
        run_id: RunId::from_uuid(id(10)),
        scope_id: RunEffectScopeId::from_uuid(id(11)),
        callback: None,
    }
}

fn suspended_link() -> LiveRequestRunLink {
    let mut link = running_link();
    link.callback = Some(LiveCallbackSuspensionRecord {
        session_id: meerkat_core::SessionId(id(1)),
        run_id: link.run_id.clone(),
        expected_tool_use_ids: vec!["call-a".into(), "call-b".into()],
        batch_digest: [12; 32],
        results: LiveCallbackResultEvidence::Pending {},
        continuation: LiveCallbackContinuationAdmission::NotSubmitted {},
    });
    link
}

#[test]
fn admission_witness_content_survives_closed_channel_without_inventing_a_run() -> TestResult {
    let record = admitted()?;
    let image = (
        meerkat_core::live_adapter::LiveAdapterStatus::Closed,
        record.clone(),
    );
    let restored: (
        meerkat_core::live_adapter::LiveAdapterStatus,
        AdmittedLiveExecutionRecord,
    ) = serde_json::from_slice(&serde_json::to_vec(&image)?)?;
    assert_eq!(restored, image);
    assert!(serde_json::to_value(&record)?.get("run_id").is_none());
    let chain = LiveRequestRunChainRecord::new(
        OperationId(id(9)),
        record,
        Vec::new(),
        LiveRequestChainPhase::Queued {},
    )?;
    assert!(chain.runs().is_empty());
    let _: Option<AdmittedLiveExecutionAuthority> = None;
    Ok(())
}

#[test]
fn callback_suspension_is_not_request_terminal_and_restores_exact_owner() -> TestResult {
    let chain = LiveRequestRunChainRecord::new(
        OperationId(id(9)),
        admitted()?,
        vec![suspended_link()],
        LiveRequestChainPhase::SuspendedCallbacks {},
    )?;
    let value = serde_json::to_value(&chain)?;
    assert_eq!(value["phase"], json!({"kind":"suspended_callbacks"}));
    assert_eq!(
        serde_json::from_value::<LiveRequestRunChainRecord>(value)?,
        chain
    );
    assert_eq!(chain.runs()[0].run_id, RunId::from_uuid(id(10)));
    Ok(())
}

#[test]
fn one_request_retains_multiple_actual_runs_linked_by_admitted_callback_input() -> TestResult {
    let mut first = suspended_link();
    let callback = first.callback.as_mut().ok_or("callback")?;
    callback.results = LiveCallbackResultEvidence::Complete {
        session_id: callback.session_id.clone(),
        run_id: callback.run_id.clone(),
        batch_digest: callback.batch_digest,
        accepted_payload_digest: [13; 32],
    };
    callback.continuation = LiveCallbackContinuationAdmission::Admitted {
        input_id: InputId::from_uuid(id(14)),
        commit: serde_json::from_value(json!({"revision":15,"digest":vec![16;32]}))?,
    };
    let second = LiveRequestRunLink {
        input_id: InputId::from_uuid(id(14)),
        run_id: RunId::from_uuid(id(17)),
        scope_id: RunEffectScopeId::from_uuid(id(18)),
        callback: None,
    };
    let chain = LiveRequestRunChainRecord::new(
        OperationId(id(9)),
        admitted()?,
        vec![first, second],
        LiveRequestChainPhase::Final {
            outcome: LiveRequestChainTerminal::Succeeded,
        },
    )?;
    assert_eq!(chain.runs().len(), 2);
    assert_eq!(chain.request_id(), &OperationId(id(9)));
    assert_eq!(
        serde_json::from_value::<LiveRequestRunChainRecord>(serde_json::to_value(&chain)?)?,
        chain
    );
    Ok(())
}

#[test]
fn wrong_session_run_duplicate_calls_and_early_continuation_refuse() -> TestResult {
    let valid = LiveRequestRunChainRecord::new(
        OperationId(id(9)),
        admitted()?,
        vec![suspended_link()],
        LiveRequestChainPhase::SuspendedCallbacks {},
    )?;
    let encoded = serde_json::to_value(valid)?;
    for (pointer, value) in [
        ("/runs/0/callback/session_id", json!(id(99))),
        ("/runs/0/callback/run_id", json!(id(99))),
        (
            "/runs/0/callback/expected_tool_use_ids",
            json!(["same", "same"]),
        ),
        ("/runs/0/callback/expected_tool_use_ids", json!([])),
        (
            "/runs/0/callback/continuation",
            json!({"kind":"unconfirmed"}),
        ),
    ] {
        let mut altered = encoded.clone();
        *altered.pointer_mut(pointer).ok_or("fixture pointer")? = value;
        assert!(
            serde_json::from_value::<LiveRequestRunChainRecord>(altered).is_err(),
            "{pointer}"
        );
    }
    Ok(())
}

#[test]
fn runless_refusal_and_callback_phase_cannot_synthesize_success_or_unrelated_next_run() -> TestResult
{
    assert!(
        LiveRequestRunChainRecord::new(
            OperationId(id(9)),
            admitted()?,
            Vec::new(),
            LiveRequestChainPhase::Final {
                outcome: LiveRequestChainTerminal::Succeeded
            },
        )
        .is_err()
    );
    assert!(
        LiveRequestRunChainRecord::new(
            OperationId(id(9)),
            admitted()?,
            vec![suspended_link()],
            LiveRequestChainPhase::Final {
                outcome: LiveRequestChainTerminal::Succeeded
            },
        )
        .is_err()
    );
    let mut next = running_link();
    next.input_id = InputId::from_uuid(id(98));
    next.run_id = RunId::from_uuid(id(99));
    next.scope_id = RunEffectScopeId::from_uuid(id(100));
    assert!(
        LiveRequestRunChainRecord::new(
            OperationId(id(9)),
            admitted()?,
            vec![suspended_link(), next],
            LiveRequestChainPhase::Running {},
        )
        .is_err()
    );
    let cancelled = LiveRequestRunChainRecord::new(
        OperationId(id(9)),
        admitted()?,
        Vec::new(),
        LiveRequestChainPhase::Final {
            outcome: LiveRequestChainTerminal::Cancelled,
        },
    )?;
    assert!(cancelled.runs().is_empty());
    Ok(())
}

#[test]
fn admission_and_phase_carriers_reject_cross_owner_and_added_authority_fields() -> TestResult {
    let mut value = admission_json();
    value["executor"]["session_id"] = json!(id(99));
    assert!(serde_json::from_value::<AdmittedLiveExecutionRecord>(value).is_err());
    for phase in ["queued", "running", "suspended_callbacks", "held"] {
        assert!(
            serde_json::from_value::<LiveRequestChainPhase>(
                json!({"kind":phase,"permission":"unrestricted"})
            )
            .is_err()
        );
    }
    for field in [
        "scope_override",
        "current_run",
        "ingress_open",
        "grant_override",
    ] {
        let mut value = admission_json();
        value[field] = json!(true);
        assert!(serde_json::from_value::<AdmittedLiveExecutionRecord>(value).is_err());
    }
    Ok(())
}
