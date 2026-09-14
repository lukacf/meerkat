//! The scoped runtime owner is a prerequisite, not a serde-record substitute.

use meerkat_machine_schema::catalog::canonical_machine_schemas;
use meerkat_runtime::live_ledger::authority::dsl::{
    LiveEffectPhase, LiveRequestEvidenceKind, LiveRequestInput as Input,
    LiveRequestMachineAuthority as Authority, LiveRequestMachineMutator as Mutator,
    ScopedEffectKind, ToolMutationClass,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn generated_admission_observation_checks_every_binding_without_regranting() -> TestResult {
    let mut owner = admitted()?;
    let observation = || Input::ObserveAdmission {
        request_id: "request".into(),
        source: "source".into(),
        payload: "payload-digest".into(),
        input_id: "input".into(),
        admission_commit: "admission-commit".into(),
        grant_id: "grant".into(),
        generation: 1,
        executor: "executor-binding".into(),
        ingress_generation: 1,
    };
    for field in 0..9 {
        let mut invalid = observation();
        let Input::ObserveAdmission {
            request_id,
            source,
            payload,
            input_id,
            admission_commit,
            grant_id,
            generation,
            executor,
            ingress_generation,
        } = &mut invalid
        else {
            return Err("expected admission observation".into());
        };
        match field {
            0 => *request_id = "another-request".into(),
            1 => *source = "another-source".into(),
            2 => *payload = "another-payload".into(),
            3 => *input_id = "another-input".into(),
            4 => *admission_commit = "another-commit".into(),
            5 => *grant_id = "another-grant".into(),
            6 => *generation = 2,
            7 => *executor = "another-binding".into(),
            8 => *ingress_generation = 2,
            _ => unreachable!(),
        }
        assert!(
            Mutator::apply(&mut owner, invalid).is_err(),
            "field {field}"
        );
    }
    for revoke in [false, true] {
        if revoke {
            Mutator::apply(
                &mut owner,
                Input::Revoke {
                    grant_id: "grant".into(),
                    generation: 1,
                },
            )?;
        }
        let before = format!("{:?}", owner.state());
        let observed = Mutator::apply(&mut owner, observation())?;
        assert!(matches!(
            observed.effects(),
            [meerkat_runtime::live_ledger::authority::dsl::LiveRequestEffect::AdmissionObserved { .. }]
        ));
        assert_eq!(format!("{:?}", owner.state()), before);
    }
    for missing in [false, true] {
        let mut invalid = owner.state().clone();
        if missing {
            invalid.request_ingress_generations.clear();
        } else {
            invalid
                .request_ingress_generations
                .insert("request".into(), 0);
        }
        assert!(Authority::recover_from_state(invalid).is_err());
    }
    Ok(())
}

#[test]
fn scoped_execution_requires_the_canonical_live_request_owner() {
    let schemas = canonical_machine_schemas();
    assert!(
        schemas
            .iter()
            .any(|schema| schema.machine.as_str() == "LiveRequestMachine"),
        "missing generated LiveRequestMachine: stored scope content cannot authorize restoration or post-await effect start"
    );
}

fn admitted() -> Result<Authority, Box<dyn std::error::Error>> {
    let mut owner = Authority::new();
    for input in [
        Input::Activate {
            grant_id: "grant".into(),
            generation: 1,
            expires_at: 100,
            executor: "executor-binding".into(),
            record: "complete-grant-record".into(),
            profile_revision: "profile-revision".into(),
            evidence: [LiveRequestEvidenceKind::ApplicationSnapshot].into(),
            mutations: [ToolMutationClass::ReadOnly].into(),
            tools_restricted: true,
            tools: ["allowed_tool".into()].into(),
            max_requests: 2,
            max_concurrent_requests: 1,
            max_effects: 2,
            max_tokens: 1000,
            max_duration_ms: 100,
            now: 1,
        },
        Input::Reserve {
            content_complete: true,
            content_discontinuous: false,
            content_empty: false,
            content_fits: true,
            request_id: "request".into(),
            source: "source".into(),
            payload: "payload-digest".into(),
            evidence: LiveRequestEvidenceKind::ApplicationSnapshot,
            profile_revision: "profile-revision".into(),
            parent_scope: "parent".into(),
            grant_id: "grant".into(),
            generation: 1,
            executor: "executor-binding".into(),
            now: 2,
            credit_records: 1,
            credit_bytes: 1000,
            snapshot_ceiling: 1000,
        },
        Input::Admit {
            request_id: "request".into(),
            source_ingress_open: true,
            source: "source".into(),
            payload: "payload-digest".into(),
            input_id: "input".into(),
            admission_commit: "admission-commit".into(),
            ingress_generation: 1,
            credit_records: 1,
            credit_bytes: 1000,
            snapshot_ceiling: 1000,
            profile_revision: "profile-revision".into(),
            now: 3,
        },
    ] {
        Mutator::apply(&mut owner, input)?;
    }
    Ok(owner)
}

fn stage() -> Input {
    Input::Stage {
        request_id: "request".into(),
        input_id: "input".into(),
        admission_commit: "admission-commit".into(),
        run_id: "actual-run".into(),
        scope_id: "scope".into(),
        scope_record: "scope-record-digest".into(),
        executor: "executor-binding".into(),
        profile_revision: "profile-revision".into(),
        now: 4,
    }
}

fn staged() -> Result<Authority, Box<dyn std::error::Error>> {
    let mut owner = admitted()?;
    Mutator::apply(&mut owner, stage())?;
    Ok(owner)
}

#[test]
fn generated_scope_custody_is_run_keyed_and_nonreplaceable() -> TestResult {
    let mut owner = staged()?;
    assert_eq!(
        owner
            .state()
            .run_scopes
            .get("actual-run")
            .map(String::as_str),
        Some("scope")
    );
    assert_eq!(
        owner.state().scope_runs.get("scope").map(String::as_str),
        Some("actual-run")
    );
    assert_eq!(
        owner
            .state()
            .run_scope_records
            .get("actual-run")
            .map(String::as_str),
        Some("scope-record-digest")
    );
    assert!(!owner.state().run_scope_records.contains_key("request"));
    assert_eq!(
        owner
            .state()
            .run_inputs
            .get("actual-run")
            .map(String::as_str),
        Some("input")
    );
    assert_eq!(
        owner
            .state()
            .run_admission_commits
            .get("actual-run")
            .map(String::as_str),
        Some("admission-commit")
    );
    for replacement in [false, true] {
        let mut input = stage();
        let Input::Stage {
            run_id,
            scope_id,
            scope_record,
            ..
        } = &mut input
        else {
            return Err("expected stage".into());
        };
        if replacement {
            *run_id = "another-run".into();
        }
        *scope_id = "replacement-scope".into();
        *scope_record = "replacement-record".into();
        let before = format!("{:?}", owner.state());
        assert!(Mutator::apply(&mut owner, input).is_err());
        assert_eq!(format!("{:?}", owner.state()), before);
    }
    for field in 0..7 {
        let mut state = owner.state().clone();
        match field {
            0 => {
                let record = state
                    .run_scope_records
                    .remove("actual-run")
                    .ok_or("scope")?;
                state.run_scope_records.insert("request".into(), record);
            }
            1 => {
                let scope = state.run_scopes.remove("actual-run").ok_or("scope")?;
                state.run_scopes.insert("request".into(), scope);
            }
            2 => {
                state.scope_runs.insert("scope".into(), "request".into());
            }
            3 => {
                state.run_inputs.remove("actual-run");
            }
            4 => {
                state.run_admission_commits.remove("actual-run");
            }
            5 => {
                state
                    .run_inputs
                    .insert("actual-run".into(), "foreign-input".into());
            }
            6 => {
                state
                    .run_admission_commits
                    .insert("actual-run".into(), "foreign-commit".into());
            }
            _ => return Err("invalid corruption".into()),
        }
        assert!(
            Authority::recover_from_state(state).is_err(),
            "field {field}"
        );
    }
    Ok(())
}

#[test]
fn generated_stage_and_recovery_require_exact_admission_commit() -> TestResult {
    let mut owner = admitted()?;
    let mut wrong = stage();
    let Input::Stage {
        admission_commit, ..
    } = &mut wrong
    else {
        return Err("expected stage".into());
    };
    *admission_commit = "foreign-admission".into();
    assert!(Mutator::apply(&mut owner, wrong).is_err());
    assert!(owner.state().bound_requests.is_empty());
    assert_eq!(
        owner.state().request_admission_commits.get("request"),
        Some(&"admission-commit".to_owned())
    );
    for corruption in 0..3 {
        let mut state = owner.state().clone();
        match corruption {
            0 => state.request_admission_commits.clear(),
            1 => {
                state
                    .request_admission_commits
                    .insert("request".into(), String::new());
            }
            2 => {
                state
                    .request_admission_commits
                    .insert("foreign".into(), "foreign-admission".into());
            }
            _ => return Err("invalid corruption".into()),
        }
        assert!(
            Authority::recover_from_state(state).is_err(),
            "corruption {corruption}"
        );
    }
    Mutator::apply(&mut owner, stage())?;
    Ok(())
}

fn restore() -> Input {
    Input::RestoreScope {
        request_id: "request".into(),
        input_id: "input".into(),
        admission_commit: "admission-commit".into(),
        run_id: "actual-run".into(),
        scope_id: "scope".into(),
        scope_record: "scope-record-digest".into(),
        parent_scope: "parent".into(),
        executor: "executor-binding".into(),
        profile_revision: "profile-revision".into(),
        now: 5,
    }
}

fn claim() -> Input {
    Input::ClaimEffect {
        request_id: "request".into(),
        input_id: "input".into(),
        admission_commit: "admission-commit".into(),
        run_id: "actual-run".into(),
        scope_id: "scope".into(),
        scope_record: "scope-record-digest".into(),
        parent_scope: "parent".into(),
        executor: "executor-binding".into(),
        claim_id: "claim".into(),
        claim_record: "claim-record".into(),
        effect_id: "effect".into(),
        chain_id: "effect".into(),
        attempt: 0,
        target: "tool-and-arguments-digest".into(),
        kind: ScopedEffectKind::ToolDispatch,
        tool: "allowed_tool".into(),
        mutation: ToolMutationClass::ReadOnly,
        profile_revision: "profile-revision".into(),
        policy_revision: "synthetic-policy-observation".into(),
        policy_permits: true,
        credit_schema: meerkat_runtime::live_ledger::authority::dsl::LiveCompletionCreditSchema::V1,
        credit_records: 4,
        credit_bytes: 40,
        minimum_record_charge: 1,
        maximum_record_charge: 10,
        snapshot_ceiling: 128,
        available_records: 4,
        available_bytes: 168,
        now: 5,
    }
}

#[test]
fn generated_claim_enforces_stored_permission_even_when_ordinary_policy_permits() -> TestResult {
    for mutation in 0..6 {
        let mut owner = staged()?;
        let mut candidate = claim();
        let Input::ClaimEffect {
            tool,
            mutation: class,
            profile_revision,
            policy_permits,
            kind,
            admission_commit,
            ..
        } = &mut candidate
        else {
            return Err("expected claim".into());
        };
        match mutation {
            0 => *tool = "not_activated".into(),
            1 => *class = ToolMutationClass::Mutating,
            2 => *class = ToolMutationClass::Unknown,
            3 => *profile_revision = "edited-profile".into(),
            4 => *kind = ScopedEffectKind::ModelComputation,
            5 => *admission_commit = "foreign-admission".into(),
            _ => return Err("invalid case".into()),
        }
        assert!(*policy_permits);
        assert!(
            Mutator::apply(&mut owner, candidate).is_err(),
            "case {mutation}"
        );
        assert!(owner.state().claim_ids.is_empty());
        assert_eq!(owner.state().remaining_effects.get("request"), Some(&2));
        Mutator::apply(&mut owner, claim())?;
    }
    Ok(())
}

#[test]
fn generated_grant_limits_survive_completion_and_recovery() -> TestResult {
    let reserve = |suffix: &str| Input::Reserve {
        content_complete: true,
        content_discontinuous: false,
        content_empty: false,
        content_fits: true,
        request_id: format!("request-{suffix}"),
        source: format!("source-{suffix}"),
        payload: format!("payload-{suffix}"),
        evidence: LiveRequestEvidenceKind::ApplicationSnapshot,
        profile_revision: "profile-revision".into(),
        parent_scope: String::new(),
        grant_id: "grant".into(),
        generation: 1,
        executor: "executor-binding".into(),
        now: 5,
        credit_records: 1,
        credit_bytes: 1000,
        snapshot_ceiling: 1000,
    };
    let admit = |suffix: &str| Input::Admit {
        source_ingress_open: true,
        request_id: format!("request-{suffix}"),
        source: format!("source-{suffix}"),
        payload: format!("payload-{suffix}"),
        input_id: format!("input-{suffix}"),
        admission_commit: format!("admission-commit-{suffix}"),
        ingress_generation: 1,
        credit_records: 1,
        credit_bytes: 1000,
        snapshot_ceiling: 1000,
        profile_revision: "profile-revision".into(),
        now: 5,
    };
    let mut owner = staged()?;
    Mutator::apply(&mut owner, reserve("2"))?;
    assert!(Mutator::apply(&mut owner, admit("2")).is_err());
    Mutator::apply(
        &mut owner,
        Input::Complete {
            request_id: "request".into(),
            run_id: "actual-run".into(),
            input_id: "input".into(),
            ordinary_completion_digest: "a".repeat(64),
            completion_records: 1,
            completion_bytes: 10,
            completion_sequence: 1,
            completion_digest: "b".repeat(64),
        },
    )?;
    let mut owner = Authority::recover_from_state(owner.state().clone())?;
    for invalid_commit in ["", "admission-commit"] {
        let mut invalid = admit("2");
        let Input::Admit {
            admission_commit, ..
        } = &mut invalid
        else {
            return Err("expected admission".into());
        };
        *admission_commit = invalid_commit.into();
        assert!(Mutator::apply(&mut owner, invalid).is_err());
        assert_eq!(owner.state().admitted_requests.len(), 1);
    }
    Mutator::apply(&mut owner, admit("2"))?;
    let mut duplicate_commit = owner.state().clone();
    duplicate_commit
        .request_admission_commits
        .insert("request-2".into(), "admission-commit".into());
    assert!(Authority::recover_from_state(duplicate_commit).is_err());
    Mutator::apply(
        &mut owner,
        Input::Stage {
            request_id: "request-2".into(),
            input_id: "input-2".into(),
            admission_commit: "admission-commit-2".into(),
            run_id: "run-2".into(),
            scope_id: "scope-2".into(),
            scope_record: "scope-record-2".into(),
            executor: "executor-binding".into(),
            profile_revision: "profile-revision".into(),
            now: 5,
        },
    )?;
    Mutator::apply(
        &mut owner,
        Input::Complete {
            request_id: "request-2".into(),
            run_id: "run-2".into(),
            input_id: "input-2".into(),
            ordinary_completion_digest: "c".repeat(64),
            completion_records: 1,
            completion_bytes: 10,
            completion_sequence: 2,
            completion_digest: "d".repeat(64),
        },
    )?;
    Mutator::apply(&mut owner, reserve("3"))?;
    assert!(owner.state().grant_active_requests.is_empty());
    assert_eq!(owner.state().grant_admitted_requests.len(), 2);
    assert!(Mutator::apply(&mut owner, admit("3")).is_err());
    Ok(())
}

#[test]
fn generated_recovery_rejects_lost_permission_and_occupancy_fields() -> TestResult {
    for case in 0..4 {
        let mut state = staged()?.state().clone();
        match case {
            0 => state.grant_active_requests.clear(),
            1 => state.grant_admitted_requests.clear(),
            2 => state.request_grant_records.clear(),
            3 => state.grant_max_requests = 0,
            _ => return Err("invalid case".into()),
        }
        assert!(Authority::recover_from_state(state).is_err(), "case {case}");
    }
    Ok(())
}

#[test]
fn generated_scope_restore_preserves_admission_won_close_and_exact_bindings() -> TestResult {
    let mut owner = staged()?;
    Mutator::apply(&mut owner, Input::CloseIngress {})?;
    let mut recovered = Authority::recover_from_state(owner.state().clone())?;
    Mutator::apply(&mut recovered, restore())?;
    for field in 0..8 {
        let mut changed = restore();
        let Input::RestoreScope {
            request_id,
            input_id,
            run_id,
            scope_id,
            scope_record,
            parent_scope,
            executor,
            admission_commit,
            ..
        } = &mut changed
        else {
            return Err("wrong fixture variant".into());
        };
        [
            request_id,
            input_id,
            run_id,
            scope_id,
            scope_record,
            parent_scope,
            executor,
            admission_commit,
        ][field]
            .push_str("-different");
        assert!(
            Mutator::apply(&mut recovered, changed).is_err(),
            "field {field}"
        );
    }
    Mutator::apply(&mut recovered, claim())?;
    Ok(())
}

#[test]
fn generated_restore_rejects_missing_scope_fields_and_current_fences() -> TestResult {
    let owner = staged()?;
    let mut missing = owner.state().clone();
    missing.run_scope_records.clear();
    assert!(Authority::recover_from_state(missing).is_err());
    for fence in [
        Input::Cancel {
            request_id: "request".into(),
        },
        Input::Revoke {
            grant_id: "grant".into(),
            generation: 1,
        },
        Input::FenceExecutor {
            executor: "replacement".into(),
        },
    ] {
        let mut changed = Authority::recover_from_state(owner.state().clone())?;
        Mutator::apply(&mut changed, fence)?;
        assert!(Mutator::apply(&mut changed, restore()).is_err());
        assert!(Mutator::apply(&mut changed, claim()).is_err());
    }
    let mut expired = restore();
    if let Input::RestoreScope { now, .. } = &mut expired {
        *now = 100;
    }
    let mut owner = staged()?;
    assert!(Mutator::apply(&mut owner, expired).is_err());
    Ok(())
}

#[test]
fn generated_recovery_rejects_same_cardinality_identity_corruption() -> TestResult {
    let mut owner = staged()?;
    Mutator::apply(&mut owner, claim())?;
    for field in 0..9 {
        let mut state = owner.state().clone();
        match field {
            0 => {
                state
                    .source_requests
                    .insert("source".into(), "another-request".into());
            }
            1 => {
                state
                    .run_requests
                    .insert("actual-run".into(), "another-request".into());
            }
            2 => {
                state
                    .scope_runs
                    .insert("scope".into(), "another-run".into());
            }
            3 => {
                state
                    .claim_requests
                    .insert("claim".into(), "another-request".into());
            }
            4 => {
                state
                    .claim_runs
                    .insert("claim".into(), "another-run".into());
            }
            5 => {
                state.spent_effects.clear();
                state.spent_effects.insert("another-effect".into());
            }
            6 => {
                state.cancelled_requests.insert("another-request".into());
            }
            7 => {
                state.request_inputs.insert("request".into(), String::new());
            }
            8 => {
                state.request_phases.insert(
                    "request".into(),
                    meerkat_runtime::live_ledger::authority::dsl::LiveRequestPhase::Reserved,
                );
            }
            _ => unreachable!(),
        }
        assert!(
            Authority::recover_from_state(state).is_err(),
            "corrupted identity field {field} was accepted"
        );
    }
    Ok(())
}

#[test]
fn executor_binding_aba_cannot_reactivate_an_old_admission() -> TestResult {
    let mut owner = staged()?;
    Mutator::apply(
        &mut owner,
        Input::FenceExecutor {
            executor: "replacement".into(),
        },
    )?;
    Mutator::apply(
        &mut owner,
        Input::FenceExecutor {
            executor: "executor-binding".into(),
        },
    )?;
    assert!(Mutator::apply(&mut owner, restore()).is_err());
    assert!(Mutator::apply(&mut owner, claim()).is_err());
    Ok(())
}

#[tokio::test]
async fn generated_claim_rechecks_revocation_after_policy_await() -> TestResult {
    let owner = std::sync::Arc::new(std::sync::Mutex::new(staged()?));
    let (policy_entered, entered) = tokio::sync::oneshot::channel();
    let (release_policy, policy_result) = tokio::sync::oneshot::channel();
    let current = std::sync::Arc::clone(&owner);
    let claimant = tokio::spawn(async move {
        policy_entered
            .send(())
            .map_err(|()| "policy entry receiver lost")?;
        policy_result.await.map_err(|_| "policy result lost")?;
        let mut current = current.lock().map_err(|_| "owner poisoned")?;
        Ok::<_, &'static str>(Mutator::apply(&mut *current, claim()).is_err())
    });
    entered.await?;
    {
        let mut owner = owner.lock().map_err(|_| "owner poisoned")?;
        Mutator::apply(
            &mut *owner,
            Input::Revoke {
                grant_id: "grant".into(),
                generation: 1,
            },
        )?;
    }
    release_policy.send(()).map_err(|()| "claimant lost")?;
    assert!(claimant.await??);
    assert!(
        owner
            .lock()
            .map_err(|_| "owner poisoned")?
            .state()
            .claim_ids
            .is_empty()
    );
    Ok(())
}

#[test]
fn claimed_effect_survives_revoke_and_recovery_without_resend_permission() -> TestResult {
    let mut owner = staged()?;
    Mutator::apply(&mut owner, claim())?;
    Mutator::apply(
        &mut owner,
        Input::Revoke {
            grant_id: "grant".into(),
            generation: 1,
        },
    )?;
    let mut recovered = Authority::recover_from_state(owner.state().clone())?;
    assert!(Mutator::apply(&mut recovered, claim()).is_err());
    let feedback = Input::SettleEffect {
        claim_id: "claim".into(),
        request_id: "request".into(),
        run_id: "actual-run".into(),
        target: "tool-and-arguments-digest".into(),
        outcome: LiveEffectPhase::Unknown,
        completion_records: 1,
        completion_bytes: 10,
        completion_sequence: 1,
        completion_digest: "a".repeat(64),
        local_noninvocation_proven: false,
        token_accounting_status:
            meerkat_core::execution_scope::ScopedTokenAccountingStatus::NotApplicable,
        token_accounting_record: "not-applicable-accounting".into(),
        observed_tokens: 0,
    };
    Mutator::apply(&mut recovered, feedback.clone())?;
    assert!(Mutator::apply(&mut recovered, feedback).is_err());
    assert!(recovered.state().spent_effects.contains("effect"));
    assert_eq!(
        recovered.state().claim_phases.get("claim"),
        Some(&LiveEffectPhase::Unknown)
    );
    assert_eq!(
        recovered.state().claim_credit_spent_records.get("claim"),
        Some(&1)
    );
    Ok(())
}

fn model_claim(ordinal: u64) -> Result<Input, Box<dyn std::error::Error>> {
    let mut input = claim();
    let Input::ClaimEffect {
        claim_id,
        effect_id,
        chain_id,
        attempt,
        kind,
        tool,
        ..
    } = &mut input
    else {
        return Err("expected claim fixture".into());
    };
    *claim_id = format!("model-claim-{ordinal}");
    *effect_id = format!("model-effect-{ordinal}");
    *chain_id = "logical-model".into();
    *attempt = ordinal;
    *kind = ScopedEffectKind::ModelComputation;
    tool.clear();
    Ok(input)
}

fn model_quote() -> Input {
    Input::ResolveModelAttempt {
        request_id: "request".into(),
        run_id: "actual-run".into(),
        scope_id: "scope".into(),
        chain_id: "logical-model".into(),
        now: 5,
    }
}

fn model_feedback(outcome: LiveEffectPhase, sequence: u64) -> Input {
    Input::SettleEffect {
        claim_id: "model-claim-0".into(),
        request_id: "request".into(),
        run_id: "actual-run".into(),
        target: "tool-and-arguments-digest".into(),
        outcome,
        completion_records: 1,
        completion_bytes: 10,
        completion_sequence: sequence,
        completion_digest: format!("{sequence:064x}"),
        local_noninvocation_proven: outcome == LiveEffectPhase::NotStarted,
        token_accounting_status: if outcome == LiveEffectPhase::NotStarted {
            meerkat_core::execution_scope::ScopedTokenAccountingStatus::NotApplicable
        } else {
            meerkat_core::execution_scope::ScopedTokenAccountingStatus::Unmeasured
        },
        token_accounting_record: "unmeasured-accounting".into(),
        observed_tokens: 0,
    }
}

fn accounting_feedback(
    outcome: LiveEffectPhase,
    sequence: u64,
    known: Option<u64>,
) -> Result<Input, Box<dyn std::error::Error>> {
    let mut input = model_feedback(outcome, sequence);
    let Input::SettleEffect {
        token_accounting_status,
        token_accounting_record,
        observed_tokens,
        ..
    } = &mut input
    else {
        return Err("expected model feedback".into());
    };
    if let Some(tokens) = known {
        *token_accounting_status =
            meerkat_core::execution_scope::ScopedTokenAccountingStatus::Disputed;
        *token_accounting_record = format!("disputed-reported-counter:{tokens}");
        *observed_tokens = tokens;
    }
    Ok(input)
}

#[test]
fn generated_observed_token_limit_preserves_completion_and_all_other_fences() -> TestResult {
    use meerkat_runtime::live_ledger::authority::dsl::LiveRequestEffect;

    for tokens in [0, 999, 1000, u64::MAX] {
        let mut owner = staged()?;
        Mutator::apply(&mut owner, model_claim(0)?)?;
        Mutator::apply(
            &mut owner,
            accounting_feedback(LiveEffectPhase::Succeeded, 1, Some(tokens))?,
        )?;
        assert_eq!(
            owner.state().request_known_tokens.get("request"),
            Some(&tokens)
        );
        assert_eq!(
            owner.state().claim_phases.get("model-claim-0"),
            Some(&LiveEffectPhase::Succeeded)
        );
        let mut recovered = Authority::recover_from_state(owner.state().clone())?;
        let quoted = Mutator::apply(&mut recovered, model_quote())?;
        if tokens < 1000 {
            assert!(matches!(
                quoted.effects(),
                [LiveRequestEffect::ModelAttemptResolved { attempt: 1, .. }]
            ));
        } else {
            assert!(matches!(quoted.effects(), [
                LiveRequestEffect::ModelTokenBudgetExhausted { used, limit: 1000 }
            ] if *used == tokens));
            assert!(Mutator::apply(&mut recovered, model_claim(1)?).is_err());
        }
        for fence in [
            Input::Cancel {
                request_id: "request".into(),
            },
            Input::Revoke {
                grant_id: "grant".into(),
                generation: 1,
            },
            Input::FenceExecutor {
                executor: "replacement".into(),
            },
        ] {
            let mut fenced = Authority::recover_from_state(owner.state().clone())?;
            Mutator::apply(&mut fenced, fence)?;
            assert!(Mutator::apply(&mut fenced, model_quote()).is_err());
            assert!(Mutator::apply(&mut fenced, model_claim(1)?).is_err());
        }
        let mut expired_quote = model_quote();
        let Input::ResolveModelAttempt { now, .. } = &mut expired_quote else {
            return Err("expected quotation".into());
        };
        *now = 100;
        assert!(Mutator::apply(&mut recovered, expired_quote).is_err());
        let mut expired_claim = model_claim(1)?;
        let Input::ClaimEffect { now, .. } = &mut expired_claim else {
            return Err("expected claim".into());
        };
        *now = 100;
        assert!(Mutator::apply(&mut recovered, expired_claim).is_err());
    }
    Ok(())
}

#[test]
fn generated_late_accounting_correction_only_advances_known_delta() -> TestResult {
    for corrected in [None, Some(400), Some(700), Some(u64::MAX)] {
        let mut owner = staged()?;
        Mutator::apply(&mut owner, model_claim(0)?)?;
        Mutator::apply(
            &mut owner,
            accounting_feedback(LiveEffectPhase::Unknown, 1, Some(600))?,
        )?;
        let mut recovered = Authority::recover_from_state(owner.state().clone())?;
        Mutator::apply(
            &mut recovered,
            accounting_feedback(LiveEffectPhase::Succeeded, 2, corrected)?,
        )?;
        let expected = corrected.unwrap_or(0).max(600);
        assert_eq!(
            recovered.state().request_known_tokens.get("request"),
            Some(&expected)
        );
        assert_eq!(
            recovered.state().claim_known_tokens.get("model-claim-0"),
            Some(&expected)
        );
        assert_eq!(
            recovered.state().claim_retry_eligible.get("model-claim-0"),
            Some(&false)
        );
        Authority::recover_from_state(recovered.state().clone())?;
    }
    let mut owner = staged()?;
    Mutator::apply(&mut owner, model_claim(0)?)?;
    Mutator::apply(
        &mut owner,
        accounting_feedback(LiveEffectPhase::Succeeded, 1, Some(900))?,
    )?;
    Mutator::apply(&mut owner, model_claim(1)?)?;
    let mut last = accounting_feedback(LiveEffectPhase::Succeeded, 2, Some(u64::MAX))?;
    let Input::SettleEffect { claim_id, .. } = &mut last else {
        return Err("expected feedback".into());
    };
    *claim_id = "model-claim-1".into();
    Mutator::apply(&mut owner, last)?;
    assert_eq!(
        owner.state().request_known_tokens.get("request"),
        Some(&u64::MAX)
    );
    Authority::recover_from_state(owner.state().clone())?;
    Ok(())
}

#[test]
fn generated_model_lineage_allows_only_exact_conclusive_successors() -> TestResult {
    for phase in [
        LiveEffectPhase::Succeeded,
        LiveEffectPhase::Failed,
        LiveEffectPhase::Unknown,
        LiveEffectPhase::Cancelled,
        LiveEffectPhase::NotStarted,
    ] {
        let mut owner = staged()?;
        Mutator::apply(&mut owner, model_quote())?;
        Mutator::apply(&mut owner, model_claim(0)?)?;
        assert!(Mutator::apply(&mut owner, model_quote()).is_err());
        assert!(Mutator::apply(&mut owner, model_claim(1)?).is_err());
        Mutator::apply(&mut owner, model_feedback(phase, 1))?;
        let mut recovered = Authority::recover_from_state(owner.state().clone())?;
        assert!(Mutator::apply(&mut recovered, model_claim(0)?).is_err());
        assert!(Mutator::apply(&mut recovered, model_claim(2)?).is_err());
        let quote = Mutator::apply(&mut recovered, model_quote());
        if matches!(phase, LiveEffectPhase::Succeeded | LiveEffectPhase::Failed) {
            assert!(matches!(quote?.effects(),
                [meerkat_runtime::live_ledger::authority::dsl::LiveRequestEffect::ModelAttemptResolved { chain_id, attempt: 1 }]
                if chain_id == "logical-model"));
            Mutator::apply(&mut recovered, model_claim(1)?)?;
        } else {
            assert!(quote.is_err());
            assert!(Mutator::apply(&mut recovered, model_claim(1)?).is_err());
            if phase == LiveEffectPhase::Unknown {
                Mutator::apply(
                    &mut recovered,
                    model_feedback(LiveEffectPhase::Succeeded, 2),
                )?;
                let mut corrected = Authority::recover_from_state(recovered.state().clone())?;
                assert!(Mutator::apply(&mut corrected, model_quote()).is_err());
                assert!(Mutator::apply(&mut corrected, model_claim(1)?).is_err());
                assert_eq!(
                    corrected.state().claim_retry_eligible.get("model-claim-0"),
                    Some(&false)
                );
            }
        }
    }
    Ok(())
}

#[test]
fn generated_model_lineage_recovery_rejects_gaps_duplicates_and_foreign_joins() -> TestResult {
    let mut owner = staged()?;
    Mutator::apply(&mut owner, model_claim(0)?)?;
    Mutator::apply(&mut owner, model_feedback(LiveEffectPhase::Succeeded, 1))?;
    Mutator::apply(&mut owner, model_claim(1)?)?;
    for ordinal in [0, 2, u64::MAX] {
        let mut state = owner.state().clone();
        state.claim_attempts.insert("model-claim-1".into(), ordinal);
        assert!(Authority::recover_from_state(state).is_err());
    }
    let mut state = owner.state().clone();
    state
        .claim_runs
        .insert("model-claim-1".into(), "foreign-run".into());
    assert!(Authority::recover_from_state(state).is_err());
    let mut state = owner.state().clone();
    state
        .chain_latest_claims
        .insert("logical-model".into(), "model-claim-0".into());
    assert!(Authority::recover_from_state(state).is_err());
    let mut state = owner.state().clone();
    state
        .claim_retry_eligible
        .insert("model-claim-0".into(), false);
    assert!(Authority::recover_from_state(state).is_err());
    Ok(())
}
