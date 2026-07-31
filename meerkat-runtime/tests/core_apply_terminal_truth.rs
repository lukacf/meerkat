//! Keep core apply terminal state behind one authority.
//!
//! `CoreApplyOutput` may carry receipts and snapshots alongside terminal
//! state, but the terminal fact itself must not be duplicated as both a legacy
//! `run_result` mirror and `CoreApplyTerminal`.

use std::fs;
use std::path::Path;

fn workspace_root() -> Result<&'static Path, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "meerkat-runtime crate should live below workspace root".to_string())
}

fn extract_braced_item<'a>(contents: &'a str, marker: &str) -> Result<&'a str, String> {
    let start = contents
        .find(marker)
        .ok_or_else(|| format!("missing marker `{marker}`"))?;
    let open = contents[start..]
        .find('{')
        .map(|offset| start + offset)
        .ok_or_else(|| format!("missing opening brace after `{marker}`"))?;

    let mut depth = 0usize;
    for (offset, ch) in contents[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&contents[start..=open + offset]);
                }
            }
            _ => {}
        }
    }

    Err(format!("unterminated braced item after `{marker}`"))
}

fn assert_terminal_intent_validation_precedes_markers(
    source: &str,
    owner: &str,
    markers: &[&str],
) -> Result<(), String> {
    let validation = source
        .find("primitive.peer_response_terminal_apply_intent_violation()")
        .ok_or_else(|| {
            format!(
                "{owner} must reject malformed terminal peer-response intent before applying it"
            )
        })?;
    for consumption in markers {
        let consumption = source
            .find(consumption)
            .ok_or_else(|| format!("{owner} missing `{consumption}`"))?;
        if validation >= consumption {
            return Err(format!(
                "{owner} must validate terminal peer-response intent before `{consumption}`"
            ));
        }
    }
    Ok(())
}

fn derive_attribute_before<'a>(contents: &'a str, marker: &str) -> Result<&'a str, String> {
    let start = contents
        .find(marker)
        .ok_or_else(|| format!("missing marker `{marker}`"))?;
    contents[..start]
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("#[derive"))
        .ok_or_else(|| format!("missing derive attribute before `{marker}`"))
}

#[test]
fn core_apply_terminal_truth_has_one_authority() -> Result<(), String> {
    let root = workspace_root()?;
    let core_executor =
        fs::read_to_string(root.join("meerkat-core/src/lifecycle/core_executor.rs"))
            .map_err(|err| format!("read core executor source: {err}"))?;
    let runtime_loop = fs::read_to_string(root.join("meerkat-runtime/src/runtime_loop.rs"))
        .map_err(|err| format!("read runtime loop source: {err}"))?;
    let runtime_driver =
        fs::read_to_string(root.join("meerkat-runtime/src/meerkat_machine/driver.rs"))
            .map_err(|err| format!("read runtime driver source: {err}"))?;
    let completion_source = fs::read_to_string(root.join("meerkat-runtime/src/completion.rs"))
        .map_err(|err| format!("read completion source: {err}"))?;
    let persistent_driver =
        fs::read_to_string(root.join("meerkat-runtime/src/driver/persistent.rs"))
            .map_err(|err| format!("read persistent driver source: {err}"))?;
    let ephemeral_driver = fs::read_to_string(root.join("meerkat-runtime/src/driver/ephemeral.rs"))
        .map_err(|err| format!("read ephemeral driver source: {err}"))?;
    let meerkat_machine_schema =
        fs::read_to_string(root.join("meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs"))
            .map_err(|err| format!("read MeerkatMachine schema source: {err}"))?;
    let meerkat_machine_model =
        fs::read_to_string(root.join("specs/machines/meerkat_machine/model.tla"))
            .map_err(|err| format!("read MeerkatMachine TLA source: {err}"))?;
    let meerkat_machine_contract =
        fs::read_to_string(root.join("specs/machines/meerkat_machine/contract.md"))
            .map_err(|err| format!("read MeerkatMachine contract source: {err}"))?;
    let accept_source = fs::read_to_string(root.join("meerkat-runtime/src/accept.rs"))
        .map_err(|err| format!("read accept source: {err}"))?;

    let output_struct = extract_braced_item(&core_executor, "pub struct CoreApplyOutput")?;
    assert!(
        output_struct.contains("pub terminal: Option<CoreApplyTerminal>"),
        "CoreApplyOutput should expose CoreApplyTerminal as the canonical terminal authority"
    );
    assert!(
        !output_struct.contains("pub run_result:"),
        "CoreApplyOutput must not duplicate terminal truth with a run_result mirror"
    );

    let terminal_publisher = extract_braced_item(
        &runtime_loop,
        "async fn publish_authorized_runtime_terminal_batch",
    )?;
    assert!(
        !terminal_publisher.contains("run_result:"),
        "runtime completion resolution must branch from CoreApplyTerminal only"
    );
    assert!(
        !terminal_publisher.contains("if let Some(result) = run_result"),
        "runtime completion resolution must not keep a separate run_result branch"
    );

    let completion_authority = extract_braced_item(
        &runtime_driver,
        "pub(crate) struct RuntimeCompletionResultAuthority",
    )?;
    let completion_authority_derive = derive_attribute_before(
        &runtime_driver,
        "pub(crate) struct RuntimeCompletionResultAuthority",
    )?;
    assert!(
        runtime_driver.contains(
            "#[must_use = \"runtime completion authority must be consumed by waiter resolution\"]"
        ),
        "runtime completion authority must stay must-use so generated proof is not dropped silently"
    );
    assert!(
        !completion_authority.contains("Clone") && !completion_authority_derive.contains("Clone"),
        "runtime completion authority must not be Clone; waiter fanout consumes one token and clones only derived cleanup observations"
    );
    let completion_attempt = extract_braced_item(
        &runtime_driver,
        "pub(crate) struct RuntimeCompletionResultAttempt",
    )?;
    let completion_realized = extract_braced_item(
        &runtime_driver,
        "pub(crate) struct RuntimeCompletionResultRealized",
    )?;
    assert!(
        runtime_driver.contains(
            "#[must_use = \"attempted runtime completion closure must be realized, failed, or abandoned\"]"
        ) && runtime_driver.contains(
            "#[must_use = \"realized runtime completion closure must mint a completion cleanup observation\"]"
        ) && completion_attempt.contains("authority: RuntimeCompletionResultAuthority")
            && completion_realized.contains("authority: RuntimeCompletionResultAuthority")
            && completion_authority
                .contains("generated_plan: generated_kernel_command_capabilities::CommandPlanKind")
            && !runtime_driver.contains(
                "generated_kernel_command_capabilities::RuntimeCompletionResultAuthority::mint_from_generated_command_plan()"
            )
            && runtime_driver.contains(
                "CommandPlanKind::AuthorizedRuntimeCompletionResultClosure"
            )
            && completion_source.contains("authority.begin_surface_resolution()")
            && completion_source.contains("Self::cleanup_from_realized_attempt(attempt)")
            && completion_source.contains("attempt.fail()")
            && completion_source.contains("attempt.abandon()")
            && !completion_source.contains("CompletionCleanupObservation::from_authority"),
        "completion waiter delivery must consume generated authority through Attempted -> Realized/Failed/Abandoned closure phases"
    );
    assert!(
        !terminal_publisher.contains("authority.clone()"),
        "runtime completion waiter fanout must not clone the generated authority token"
    );
    assert!(
        runtime_loop.contains("machine_authorize_runtime_loop_batch(&d)")
            && runtime_loop.contains("dequeue_batch_exact(&batch)")
            && runtime_loop.contains("prepare_runtime_loop_batch_start(")
            && !runtime_loop.contains("filter_map(|id| d.dequeue_by_id(id))"),
        "runtime loop batch execution must use authorized batch tokens and fail closed on projection mismatch"
    );
    let exact_dequeue =
        extract_braced_item(&ephemeral_driver, "pub(crate) fn dequeue_batch_exact")?;
    assert!(
        exact_dequeue.contains("match batch.source()")
            && exact_dequeue.contains("RuntimeLoopBatchSource::Queue")
            && exact_dequeue.contains("RuntimeLoopBatchSource::Steer")
            && exact_dequeue.contains("dequeue_exact_prefix(batch.input_ids())")
            && !exact_dequeue.contains("dequeue_by_id"),
        "runtime batch dequeue must enforce exact source/prefix conformance instead of draining by id from either queue"
    );
    assert!(
        runtime_driver.contains("input_runtime_boundary")
            && runtime_driver.contains("input_runtime_execution_kind")
            && runtime_driver.contains("input_peer_response_terminal_apply_intent")
            && runtime_driver.contains("input_is_prompt_for_batch")
            && !runtime_driver.contains("fn machine_validate_stage_drain_snapshot")
            && !runtime_driver.contains("machine_validate_stage_drain_snapshot("),
        "runtime batch grouping must use machine-owned grouping witnesses without retaining a shell stage-drain validator"
    );
    let batch_authorizer = extract_braced_item(
        &runtime_driver,
        "pub(crate) fn machine_authorize_runtime_loop_batch",
    )?;
    assert!(
        !runtime_driver.contains("pub(crate) fn machine_select_runtime_loop_batch")
            && batch_authorizer
                .contains("AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state")
            && batch_authorizer.contains("authority.state()")
            && !batch_authorizer.contains("runtime_semantics(")
            && !batch_authorizer.contains("driver_ingress()"),
        "runtime-loop batch authorization must use the generated command-plan selector over machine state, not a handwritten shell selector"
    );
    assert!(
        !ephemeral_driver.contains("pub fn dequeue_by_id")
            && !ephemeral_driver.contains("pub fn dequeue_next")
            && !ephemeral_driver.contains("pub fn stage_input")
            && !ephemeral_driver.contains("pub fn stage_batch")
            && !persistent_driver.contains("pub fn dequeue_by_id")
            && !persistent_driver.contains("pub fn dequeue_next")
            && !persistent_driver.contains("pub fn stage_input")
            && !persistent_driver.contains("pub fn stage_batch")
            && !ephemeral_driver.contains("pub fn contract_stage_current_run_input"),
        "raw driver dequeue/stage APIs must not be externally callable bypasses"
    );
    let runtime_batch_authority = extract_braced_item(
        &runtime_driver,
        "pub(crate) struct AuthorizedRuntimeLoopBatch",
    )?;
    let runtime_batch_authority_derive = derive_attribute_before(
        &runtime_driver,
        "pub(crate) struct AuthorizedRuntimeLoopBatch",
    )?;
    let stage_authority =
        extract_braced_item(&runtime_driver, "pub(crate) struct AuthorizedStageForRun")?;
    let stage_authority_derive =
        derive_attribute_before(&runtime_driver, "pub(crate) struct AuthorizedStageForRun")?;
    assert!(
        runtime_driver.contains(
            "#[must_use = \"runtime loop batch authority must be consumed by stage authorization\"]"
        ) && !runtime_batch_authority.contains("Clone")
            && !runtime_batch_authority_derive.contains("Clone"),
        "runtime loop batch authority must be must-use and non-Clone"
    );
    assert!(
        runtime_driver.contains(
            "#[must_use = \"stage-for-run authority must be consumed by machine_realize_stage_batch\"]"
        ) && !stage_authority.contains("Clone")
            && !stage_authority_derive.contains("Clone"),
        "stage-for-run authority must be must-use and non-Clone"
    );
    let prepare_batch_start = extract_braced_item(
        &runtime_driver,
        "pub(crate) async fn prepare_runtime_loop_batch_start",
    )?;
    let live_boundary_stage = extract_braced_item(
        &runtime_driver,
        "pub(crate) async fn machine_realize_live_boundary_context_injected",
    )?;
    assert!(
        !runtime_batch_authority
            .contains("generated_stage: generated_command_capabilities::AuthorizedStageForRun")
            && !runtime_driver.contains("pub(crate) fn into_stage_for_run")
            && prepare_batch_start.contains("machine_authorize_stage_for_run(")
            && prepare_batch_start.contains("machine_begin_run(&mut driver")
            && live_boundary_stage.contains("machine_authorize_stage_for_run(")
            && runtime_driver.contains("AuthorizedStageForRun::authorize_stage_for_run_from_state"),
        "stage-for-run authority must be carried from generated state plans instead of minted by handwritten runtime bridge code"
    );
    let run_commit_authority = extract_braced_item(
        &runtime_driver,
        "pub(crate) struct AuthorizedRuntimeLoopRunCommit",
    )?;
    let run_commit_authority_derive = derive_attribute_before(
        &runtime_driver,
        "pub(crate) struct AuthorizedRuntimeLoopRunCommit",
    )?;
    let runtime_loop_commit = extract_braced_item(
        &runtime_driver,
        "pub(crate) async fn commit_runtime_loop_run",
    )?;
    assert!(
        runtime_driver.contains(
            "#[must_use = \"runtime-loop run commit authority must be consumed by commit realization\"]"
        ) && !run_commit_authority.contains("Clone")
            && !run_commit_authority_derive.contains("Clone")
            && run_commit_authority.contains("run_id: RunId")
            && run_commit_authority.contains("consumed_input_ids: Vec<InputId>")
            && run_commit_authority.contains("commit_input_id: InputId")
            && run_commit_authority.contains("receipt: meerkat_core::lifecycle::RunBoundaryReceipt")
            && run_commit_authority
                .contains("generated_plan: generated_kernel_command_capabilities::CommandPlanKind")
            && run_commit_authority.contains("owner_session_id:")
            && run_commit_authority.contains("owner_agent_runtime_id:")
            && run_commit_authority.contains("commit_outcome: AuthorizedRuntimeLoopRunCommitOutcome")
            && run_commit_authority.contains(
                "effect_closure_obligations: Vec<RuntimeLoopRunCommitEffectObligation>"
            )
            && run_commit_authority.contains("return_projection: RuntimeLifecycleProjection")
            && runtime_driver.contains("struct RuntimeLoopRunCommitEffectObligation")
            && runtime_driver.contains("enum RuntimeLoopRunCommitEffect")
            && runtime_driver.contains("\"RuntimeLoopRunCommitEffect\"")
            && runtime_loop_commit.contains("effect_closure_obligations()")
            && runtime_loop_commit.contains("RuntimeLoopRunCommitEffect::Completed")
            && runtime_driver.contains("fn preview_authorized_runtime_loop_run_commit(")
            && runtime_driver.contains("MeerkatMachineInput::RunCompleted")
            && runtime_driver.contains("MeerkatMachineInput::Commit")
            && runtime_loop_commit.contains("AuthorizedRuntimeLoopRunCommit::authorize(")
            && runtime_loop_commit.contains("CommandPlanKind::AuthorizedRuntimeLoopRunCommit")
            && runtime_loop_commit.contains("commit_authority.commit_outcome().outcome()")
            && runtime_loop_commit.contains("&return_projection != commit_authority.return_projection()")
            && !runtime_loop_commit.contains("commit_authority.into_parts()"),
        "runtime-loop run commit must consume a generated-shaped authority binding run id, terminal inputs, owner, outcome, receipt, and return projection"
    );
    let shared_stage_realizer = extract_braced_item(
        &runtime_driver,
        "pub(crate) fn machine_realize_authorized_stage_batch",
    )?;
    assert!(
        shared_stage_realizer.contains("authority: AuthorizedStageForRun")
            && shared_stage_realizer.contains("machine_realize_authorized_stage_batch(authority)")
            && !shared_stage_realizer.contains("machine_realize_stage_batch(&input_ids"),
        "shared stage realization must consume AuthorizedStageForRun instead of raw ids"
    );
    let concrete_authorized_stage = extract_braced_item(
        &ephemeral_driver,
        "pub(crate) fn machine_realize_authorized_stage_batch",
    )?;
    assert!(
        concrete_authorized_stage
            .contains("authority: crate::meerkat_machine::driver::AuthorizedStageForRun")
            && concrete_authorized_stage.contains("authority.into_parts()")
            && concrete_authorized_stage
                .contains("self.machine_realize_stage_batch(&input_ids, &run_id)"),
        "concrete staging must be reachable through an explicit AuthorizedStageForRun wrapper"
    );
    let live_boundary_realizer = extract_braced_item(
        &ephemeral_driver,
        "pub(crate) fn machine_realize_live_boundary_context_injected",
    )?;
    assert!(
        live_boundary_realizer
            .contains("stage_authority: crate::meerkat_machine::driver::AuthorizedStageForRun")
            && live_boundary_realizer
                .contains("self.machine_realize_authorized_stage_batch(stage_authority)")
            && !live_boundary_realizer.contains("self.machine_realize_stage_batch(input_ids"),
        "live-boundary staging must consume explicit stage authority instead of raw ids"
    );
    let stage_for_run_transition =
        extract_braced_item(&meerkat_machine_schema, "transition StageForRun")?;
    assert!(
        stage_for_run_transition.contains("guard \"input_queued\"")
            && stage_for_run_transition.contains("guard \"input_lane_bound\"")
            && stage_for_run_transition.contains("guard \"input_sequence_bound\"")
            && stage_for_run_transition.contains("guard \"input_recovery_lane_bound\"")
            && stage_for_run_transition.contains("guard \"input_not_run_associated\"")
            && stage_for_run_transition.contains("guard \"current_run_matches\"")
            && stage_for_run_transition
                .contains("self.input_attempt_counts.increment(input_id, 1)"),
        "StageForRun must own queued/lane/sequence/run-association/current-run predicates and fold attempt increment into staging"
    );
    let stage_start = meerkat_machine_model
        .find("StageForRunIdle(input_id, run_id) ==")
        .ok_or_else(|| "generated TLA missing StageForRunIdle operator".to_string())?;
    let stage_end = meerkat_machine_model[stage_start..]
        .find("StageForRunAttached(input_id, run_id) ==")
        .map(|offset| stage_start + offset)
        .ok_or_else(|| "generated TLA missing StageForRunAttached operator".to_string())?;
    let stage_for_run_model = &meerkat_machine_model[stage_start..stage_end];
    assert!(
        stage_for_run_model.contains("current_run_id # None")
            && stage_for_run_model.contains("current_run_id[\"value\"] ELSE None) = run_id"),
        "generated StageForRun TLA must bind staging to the active machine-owned current_run_id"
    );
    let command_plan_start = meerkat_machine_contract
        .find("## Command Plans")
        .ok_or_else(|| "generated contract missing Command Plans section".to_string())?;
    let command_plan_end = meerkat_machine_contract[command_plan_start..]
        .find("## Invariants")
        .map(|offset| command_plan_start + offset)
        .ok_or_else(|| {
            "generated contract missing Invariants section after Command Plans".to_string()
        })?;
    let command_plans = &meerkat_machine_contract[command_plan_start..command_plan_end];
    assert!(
        command_plans.contains("### `AuthorizedAcceptedInputMaterialization`")
            && command_plans.contains("### `AuthorizeRuntimeLoopBatch`")
            && command_plans.contains("### `AuthorizedStageForRun`")
            && command_plans.contains("### `AuthorizedRuntimeLoopRunCommit`")
            && command_plans.contains("### `AuthorizedRuntimeCompletionResultClosure`")
            && command_plans.contains("- Authority: `AuthorizedRuntimeLoopBatch`")
            && command_plans.contains("- Authority: `AuthorizedRuntimeLoopRunCommit`")
            && command_plans.contains("- Authority: `RuntimeCompletionResultAuthority`")
            && command_plans.contains(
                "- Command Effects: `TurnRunCompleted`, `TurnRunFailed`, `TurnRunCancelled`"
            )
            && command_plans.contains(
                "`TurnRunCompleted` via `AuthorizedRuntimeLoopRunCommit` (RuntimeLoopRunCommitEffect) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`"
            )
            && command_plans.contains(
                "- Command Effects: `RuntimeCompletionResultResolved`"
            )
            && command_plans.contains(
                "`RuntimeCompletionResultResolved` via `RuntimeCompletionResultAuthority` (LocalSurfaceResultAlignment) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`"
            )
            && command_plans.contains("`StageForRunIdle`: `input_queued`, `input_lane_bound`, `input_sequence_bound`, `input_recovery_lane_bound`, `input_not_run_associated`, `current_run_matches`"),
        "generated contract must expose queue-to-run, run-commit, and completion-result closure command plans with expanded guards and effects"
    );
    let stage_realizer = extract_braced_item(&ephemeral_driver, "fn machine_realize_stage_batch")?;
    assert!(
        !stage_realizer.contains("IncrementAttemptCount"),
        "runtime staging must not split StageForRun from the attempt-count update"
    );

    let ingress_capability = extract_braced_item(
        &accept_source,
        "pub(crate) struct RuntimeIngressExecutionCapability",
    )?;
    let resolved_admission = extract_braced_item(&accept_source, "pub struct ResolvedAdmission")?;
    let ingress_capability_derive = derive_attribute_before(
        &accept_source,
        "pub(crate) struct RuntimeIngressExecutionCapability",
    )?;
    let resolved_admission_derive =
        derive_attribute_before(&accept_source, "pub struct ResolvedAdmission")?;
    assert!(
        accept_source.contains(
            "#[must_use = \"runtime ingress execution capability must be consumed by accept_resolved_input\"]"
        ),
        "runtime ingress execution capability must stay must-use so admission proof is not dropped silently"
    );
    assert!(
        !ingress_capability.contains("Clone") && !ingress_capability_derive.contains("Clone"),
        "runtime ingress execution capability must not be Clone"
    );
    assert!(
        !accept_source.contains("pub(crate) fn from_admission_resolved_effect"),
        "runtime ingress capability constructor must remain private to the accept module"
    );
    assert!(
        !resolved_admission.contains("Clone") && !resolved_admission_derive.contains("Clone"),
        "ResolvedAdmission must not be Clone because it carries a one-shot ingress execution capability"
    );

    let persistent_accept = extract_braced_item(
        &persistent_driver,
        "pub(crate) async fn accept_resolved_input",
    )?;
    let bounded_preview = persistent_accept
        .find("preview_accept_resolved_input_bounded(&input, &resolved)")
        .ok_or_else(|| {
            "persistent accept must preview the resolved admission without cloning full authority"
                .to_string()
        })?;
    let resolved_flags = persistent_accept
        .find("let flags = resolved.coarse_flags();")
        .ok_or_else(|| "persistent accept must derive flags from resolved authority".to_string())?;
    let committed_accept = persistent_accept
        .find("let mut outcome = match self.inner.accept_resolved_input(input, resolved).await")
        .ok_or_else(|| {
            "persistent accept must commit through the authority-revalidating inner accept"
                .to_string()
        })?;
    let completion_signal = persistent_accept
        .find(".machine_apply_accept_with_completion_signal")
        .ok_or_else(|| {
            "persistent accept must apply completion signal after committed admission".to_string()
        })?;
    let delta_persist = persistent_accept
        .find(".persist_input_states_atomically(&self.runtime_id, &records)")
        .ok_or_else(|| "persistent accept must persist the exact changed-row delta".to_string())?;
    assert!(
        bounded_preview < resolved_flags
            && resolved_flags < committed_accept
            && committed_accept < completion_signal
            && completion_signal < delta_persist
            && !persistent_accept.contains("clone_with_isolated_dsl_authority"),
        "persistent accept must preview before committing through inner authority, signaling, and persisting only the changed-row delta"
    );

    let persistent_preview = extract_braced_item(
        &persistent_driver,
        "pub(crate) async fn preview_accept_resolved_input",
    )?;
    assert!(
        persistent_preview.contains("preview_accept_resolved_input_bounded(&input, resolved)")
            && !persistent_preview.contains("clone_with_isolated_dsl_authority")
            && !persistent_preview.contains(".accept_resolved_input(input"),
        "persistent preview must remain bounded and side-effect free"
    );
    Ok(())
}

#[test]
fn runtime_loop_terminal_snapshot_failures_are_fail_closed() -> Result<(), String> {
    let root = workspace_root()?;
    let runtime_loop = fs::read_to_string(root.join("meerkat-runtime/src/runtime_loop.rs"))
        .map_err(|err| format!("read runtime loop source: {err}"))?;

    assert!(
        !runtime_loop.contains("let _ = crate::meerkat_machine::fail_runtime_loop_run")
            && !runtime_loop.contains("let _ = fail_runtime_loop_run"),
        "runtime loop must not ignore failed terminal snapshot writes"
    );
    Ok(())
}

#[test]
fn terminal_notices_flow_through_canonical_typed_appends() -> Result<(), String> {
    let root = workspace_root()?;
    let runtime_backed = fs::read_to_string(root.join("meerkat/src/surface/runtime_backed.rs"))
        .map_err(|err| format!("read runtime-backed surface source: {err}"))?;
    let mcp_runtime_ingress =
        fs::read_to_string(root.join("meerkat-mcp-server/src/runtime_ingress.rs"))
            .map_err(|err| format!("read MCP runtime ingress source: {err}"))?;

    let runtime_backed_apply = extract_braced_item(&runtime_backed, "async fn apply")?;
    assert_terminal_intent_validation_precedes_markers(
        runtime_backed_apply,
        "runtime-backed apply",
        &["start_turn_request_from_primitive(&primitive)"],
    )?;
    assert!(
        runtime_backed_apply.contains("start_turn_request_from_primitive(&primitive)")
            && runtime_backed_apply.contains(".apply_runtime_turn("),
        "runtime-backed apply must build one admitted turn request before applying the reaction turn"
    );

    let runtime_backed_request =
        extract_braced_item(&runtime_backed, "fn start_turn_request_from_primitive")?;
    assert!(
        runtime_backed_request.contains(".with_typed_turn_appends(primitive.typed_turn_appends())"),
        "runtime-backed turn request must carry the primitive's ordinary typed appends"
    );

    let mcp_runtime_apply =
        extract_braced_item(&mcp_runtime_ingress, "async fn apply_runtime_turn")?;
    assert_terminal_intent_validation_precedes_markers(
        mcp_runtime_apply,
        "MCP runtime ingress apply",
        &["primitive.typed_turn_appends()"],
    )?;
    assert!(
        mcp_runtime_apply.contains(".with_typed_turn_appends(typed_turn_appends)"),
        "MCP runtime ingress must carry ordinary typed appends into the admitted turn"
    );
    Ok(())
}
