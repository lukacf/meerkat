//! Fixture tests for the structural effect-authority audit — the syn AST
//! port of `scripts/audit-effect-authority.sh`. Every `expect_audit_failure`
//! fixture from the retired script's `--self-test` is preserved here, plus
//! AST-shape evasion cases: a banned token inside a comment or string
//! literal must NOT flag, while the structurally equivalent code shape MUST.
#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use xtask::effect_authority::collect_effect_authority_findings;

const LIVE_WORKSPACE_RUNFILES: &str = "required";

const CORE_EXECUTOR_TRAIT_FIXTURE: &str = r"
trait CoreExecutor {
    fn boundary_handle(&self) {}
    fn interrupt_handle(&self) {}
    fn publication_handle(&self) {}
    fn machine_managed_post_stop_unregister(&self) -> bool { false }
    fn post_stop_cleanup_handle(&self) -> Option<()> { None }
    fn turn_finalization_boundary_handle(&self) {}
    fn apply(&mut self) {}
    fn checkpoint_committed_session_snapshot(&mut self) {}
    fn reconcile_committed_compaction_projections(&mut self) {}
    fn abort_uncommitted_compaction_projections(&mut self) {}
    fn publish_interaction_terminals(&mut self) {}
    fn cancel_after_boundary(&mut self) {}
    fn stop_runtime_executor(&mut self) {}
    fn cleanup_after_runtime_stop_terminalized(&mut self) {}
}
";

const MACHINE_MANAGED_EXECUTOR_FIXTURE: &str = r"
struct MachineManagedPostStopExecutor {
    inner: Executor,
}

impl CoreExecutor for MachineManagedPostStopExecutor {
    fn boundary_handle(&self) { self.inner.boundary_handle() }
    fn interrupt_handle(&self) { self.inner.interrupt_handle() }
    fn publication_handle(&self) { self.inner.publication_handle() }
    fn machine_managed_post_stop_unregister(&self) -> bool { false }
    fn post_stop_cleanup_handle(&self) -> Option<()> { None }
    fn turn_finalization_boundary_handle(&self) { self.inner.turn_finalization_boundary_handle() }
    fn apply(&mut self) { self.inner.apply() }
    fn checkpoint_committed_session_snapshot(&mut self) { self.inner.checkpoint_committed_session_snapshot() }
    fn reconcile_committed_compaction_projections(&mut self) { self.inner.reconcile_committed_compaction_projections() }
    fn abort_uncommitted_compaction_projections(&mut self) { self.inner.abort_uncommitted_compaction_projections() }
    fn publish_interaction_terminals(&mut self) { self.inner.publish_interaction_terminals() }
    fn cancel_after_boundary(&mut self) { self.inner.cancel_after_boundary() }
    fn stop_runtime_executor(&mut self) {
        self.inner.stop_runtime_executor();
        machine.lock_post_stop_cleanup_attachment();
    }
    fn cleanup_after_runtime_stop_terminalized(&mut self) {
        machine.unregister_terminalized_runtime_loop_if_current_with_guard();
    }
}

impl MeerkatMachine {
    fn ensure_session_with_executor_factory_inner(&self, executor: Executor) {
        let managed = executor.machine_managed_post_stop_unregister();
        let cleanup = executor.post_stop_cleanup_handle();
        let _ = (managed, cleanup);
        let _decorated = MachineManagedPostStopExecutor { inner: executor };
    }
}
";

fn write_file(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

fn findings_for(rel: &str, contents: &str) -> Vec<String> {
    let dir = tempdir().expect("tempdir");
    write_file(dir.path(), rel, contents);
    collect_effect_authority_findings(dir.path()).expect("collect findings")
}

fn core_executor_delegation_findings(trait_source: &str, decorator_source: &str) -> Vec<String> {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-core/src/lifecycle/core_executor.rs",
        trait_source,
    );
    write_file(
        dir.path(),
        "meerkat-runtime/src/meerkat_machine/session_management.rs",
        decorator_source,
    );
    collect_effect_authority_findings(dir.path()).expect("collect delegation findings")
}

fn expect_failure(name: &str, rel: &str, contents: &str) {
    let findings = findings_for(rel, contents);
    assert!(
        !findings.is_empty(),
        "{name}: fixture must fail the audit but passed"
    );
}

fn expect_clean(name: &str, rel: &str, contents: &str) {
    let findings = findings_for(rel, contents);
    assert!(
        findings.is_empty(),
        "{name}: fixture must pass the audit, got {findings:#?}"
    );
}

#[test]
fn live_workspace_effect_authority_audit_is_clean() {
    assert_eq!(LIVE_WORKSPACE_RUNFILES, "required");
    let root = xtask::public_contracts::repo_root().expect("repo root");
    let findings = collect_effect_authority_findings(&root).expect("collect findings");
    assert!(
        findings.is_empty(),
        "effect-authority audit must be clean for the committed workspace, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_complete_partition_is_clean() {
    let findings = core_executor_delegation_findings(
        CORE_EXECUTOR_TRAIT_FIXTURE,
        MACHINE_MANAGED_EXECUTOR_FIXTURE,
    );
    assert!(
        findings.is_empty(),
        "complete CoreExecutor decorator partition must pass, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_new_defaulted_trait_method() {
    let trait_source = CORE_EXECUTOR_TRAIT_FIXTURE.replace(
        "    fn cleanup_after_runtime_stop_terminalized(&mut self) {}\n",
        "    fn cleanup_after_runtime_stop_terminalized(&mut self) {}\n    fn newly_defaulted_projection(&mut self) {}\n",
    );
    let findings =
        core_executor_delegation_findings(&trait_source, MACHINE_MANAGED_EXECUTOR_FIXTURE);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("newly_defaulted_projection")),
        "new defaulted trait method must fail the exhaustive partition, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_omitted_forward() {
    let decorator_source = MACHINE_MANAGED_EXECUTOR_FIXTURE.replace(
        "    fn reconcile_committed_compaction_projections(&mut self) { self.inner.reconcile_committed_compaction_projections() }\n",
        "",
    );
    let findings =
        core_executor_delegation_findings(CORE_EXECUTOR_TRAIT_FIXTURE, &decorator_source);
    assert!(
        findings.iter().any(|finding| {
            finding.contains("missing")
                && finding.contains("reconcile_committed_compaction_projections")
        }),
        "omitted forwarding method must fail method-set equality, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_non_inner_forward() {
    let decorator_source = MACHINE_MANAGED_EXECUTOR_FIXTURE.replace(
        "self.inner.abort_uncommitted_compaction_projections()",
        "self.abort_uncommitted_compaction_projections()",
    );
    let findings =
        core_executor_delegation_findings(CORE_EXECUTOR_TRAIT_FIXTURE, &decorator_source);
    assert!(
        findings.iter().any(|finding| {
            finding.contains("abort_uncommitted_compaction_projections")
                && finding.contains("self.inner")
        }),
        "non-inner forwarding must fail the call-shape check, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_nested_side_effect_forward() {
    let decorator_source = MACHINE_MANAGED_EXECUTOR_FIXTURE.replace(
        "fn apply(&mut self) { self.inner.apply() }",
        "fn apply(&mut self) { { side_effect; self.inner.apply() } }",
    );
    let findings =
        core_executor_delegation_findings(CORE_EXECUTOR_TRAIT_FIXTURE, &decorator_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("pure CoreExecutor decorator method `apply`")),
        "nested side effects must not satisfy pure forwarding, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_missing_override_seams() {
    let decorator_source = MACHINE_MANAGED_EXECUTOR_FIXTURE
        .replace(
            "machine.lock_post_stop_cleanup_attachment()",
            "machine.unrelated_stop_hook()",
        )
        .replace(
            "machine.unregister_terminalized_runtime_loop_if_current_with_guard()",
            "machine.unrelated_cleanup_hook()",
        );
    let findings =
        core_executor_delegation_findings(CORE_EXECUTOR_TRAIT_FIXTURE, &decorator_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("exact post-stop attachment fence"))
            && findings
                .iter()
                .any(|finding| finding.contains("exact machine unregister seam")),
        "decorator overrides must retain both exact machine seams, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_changed_consumed_capabilities() {
    let decorator_source = MACHINE_MANAGED_EXECUTOR_FIXTURE
        .replace(
            "fn machine_managed_post_stop_unregister(&self) -> bool { false }",
            "fn machine_managed_post_stop_unregister(&self) -> bool { true }",
        )
        .replace(
            "fn post_stop_cleanup_handle(&self) -> Option<()> { None }",
            "fn post_stop_cleanup_handle(&self) -> Option<()> { Some(()) }",
        );
    let findings =
        core_executor_delegation_findings(CORE_EXECUTOR_TRAIT_FIXTURE, &decorator_source);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("must be explicitly false"))
            && findings
                .iter()
                .any(|finding| finding.contains("must be explicitly None")),
        "consumed capability values must remain pinned after decoration, got {findings:#?}"
    );
}

#[test]
fn core_executor_decorator_rejects_post_wrap_capability_capture() {
    let before = r"        let managed = executor.machine_managed_post_stop_unregister();
        let cleanup = executor.post_stop_cleanup_handle();
        let _ = (managed, cleanup);
        let _decorated = MachineManagedPostStopExecutor { inner: executor };
";
    let after = r"        let _decorated = MachineManagedPostStopExecutor { inner: executor };
        let managed = executor.machine_managed_post_stop_unregister();
        let cleanup = executor.post_stop_cleanup_handle();
        let _ = (managed, cleanup);
";
    let decorator_source = MACHINE_MANAGED_EXECUTOR_FIXTURE.replace(before, after);
    let findings =
        core_executor_delegation_findings(CORE_EXECUTOR_TRAIT_FIXTURE, &decorator_source);
    assert!(
        findings.iter().any(|finding| {
            finding.contains("machine_managed_post_stop_unregister") && finding.contains("captured")
        }) && findings.iter().any(|finding| {
            finding.contains("post_stop_cleanup_handle") && finding.contains("captured")
        }),
        "post-wrap capability capture must fail ordering checks, got {findings:#?}"
    );
}

// --- Fixtures ported from the retired script self-test ---

#[test]
fn peer_hard_cancel_fixture_fails() {
    expect_failure(
        "peer hard-cancel",
        "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs",
        r#"
fn bad(machine: &Machine) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad");
}
"#,
    );
}

#[test]
fn legacy_interrupt_current_run_definition_fails() {
    expect_failure(
        "legacy interrupt_current_run definition",
        "meerkat-runtime/src/meerkat_machine/session_management.rs",
        r#"
impl Machine {
    pub async fn interrupt_current_run(&self, session_id: &SessionId) {
        let _ = self.hard_cancel_current_run(session_id, "bad").await;
    }
}
"#,
    );
}

#[test]
fn root_user_interrupt_module_mount_fails() {
    expect_failure(
        "root user_interrupt module",
        "meerkat-runtime/src/meerkat_machine/mod.rs",
        r#"
#[path = "../user_interrupt.rs"]
pub(crate) mod user_interrupt;
"#,
    );
}

#[test]
fn split_interrupt_command_construction_fails() {
    expect_failure(
        "split InterruptCurrentRun command",
        "meerkat-runtime/src/driver/sneaky.rs",
        r"
fn bad(session_id: SessionId, reason: String) {
    let _ = MeerkatMachineCommand::InterruptCurrentRun
    {
        session_id,
        reason,
    };
}
",
    );
}

#[test]
fn interrupt_command_variant_definition_fails() {
    expect_failure(
        "InterruptCurrentRun command variant",
        "meerkat-runtime/src/meerkat_machine_types.rs",
        r"
pub(crate) enum MeerkatMachineCommand {
    RegisterSession {
        session_id: SessionId,
    },
    InterruptCurrentRun {
        session_id: SessionId,
        reason: String,
    },
    CancelAfterBoundary {
        session_id: SessionId,
    },
}
",
    );
}

#[test]
fn direct_runtime_effect_constructor_fails() {
    expect_failure(
        "direct RuntimeEffect constructor",
        "meerkat-runtime/src/runtime_loop.rs",
        r#"
fn bad() {
    let _ = RuntimeEffect::cancel_after_boundary("bad");
}
"#,
    );
}

#[test]
fn direct_runtime_loop_executor_stop_fails() {
    expect_failure(
        "direct runtime-loop executor stop",
        "meerkat-runtime/src/runtime_loop.rs",
        r#"
async fn bad(executor: &mut dyn CoreExecutor) {
    let _ = executor
        .stop_runtime_executor("bad".to_string())
        .await;
}
"#,
    );
}

#[test]
fn warn_only_cancel_after_boundary_fails() {
    expect_failure(
        "warn-only cancel-after-boundary effect",
        "meerkat-runtime/src/control_plane.rs",
        r#"
async fn bad(executor: &mut dyn CoreExecutor) -> Result<bool, Error> {
    if let Err(err) = executor.cancel_after_boundary("bad".to_string()).await {
        tracing::warn!(error = %err, "failed to apply runtime executor effect");
    }
    Ok(false)
}
"#,
    );
}

#[test]
fn dropped_interrupt_yielding_effect_send_fails() {
    expect_failure(
        "dropped interrupt-yielding effect send",
        "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs",
        r"
fn bad(tx: Sender, projected_effect: ProjectedRuntimeEffect) {
    let _ = tx.try_send(projected_effect.into_effect());
}
",
    );
}

#[test]
fn trace_only_boundary_cancel_failure_fails() {
    expect_failure(
        "trace-only boundary cancel failure",
        "meerkat-runtime/src/meerkat_machine/dispatch_control.rs",
        r#"
fn bad() {
    tracing::trace!("out-of-band Ingest boundary cancel was not applied");
}
"#,
    );
}

#[test]
fn runtime_effect_from_fact_call_fails() {
    expect_failure(
        "RuntimeEffect::from_fact",
        "meerkat-runtime/src/runtime_loop.rs",
        r"
fn bad(fact: RuntimeEffectFact) {
    let _ = RuntimeEffect::from_fact(fact);
}
",
    );
}

#[test]
fn visible_runtime_effect_fact_fails() {
    expect_failure(
        "visible RuntimeEffectFact/from_fact",
        "meerkat-runtime/src/effect.rs",
        r"
pub(crate) enum RuntimeEffectFact {
    CancelAfterBoundary { reason: String },
}

impl RuntimeEffect {
    pub(crate) fn from_fact(fact: RuntimeEffectFact) -> Self {
        todo!()
    }
}
",
    );
}

#[test]
fn runtime_shell_fact_literal_fails() {
    expect_failure(
        "runtime-shell fact literal",
        "meerkat-runtime/src/runtime_loop.rs",
        r"
fn bad(reason: String) {
    let _ = RuntimeEffectFact::CancelAfterBoundary { reason };
}
",
    );
}

#[test]
fn generated_runtime_effect_fact_shape_fails() {
    expect_failure(
        "generated runtime-effect fact",
        "meerkat-runtime/src/runtime_loop.rs",
        r"
fn bad(reason: String) {
    let _ = MeerkatMachineEffect::RuntimeEffectFact {
        kind: RuntimeEffectKind::CancelAfterBoundary,
        reason,
    };
}
",
    );
}

#[test]
fn public_hard_cancel_authority_fails() {
    expect_failure(
        "public hard-cancel authority",
        "meerkat-runtime/src/user_interrupt.rs",
        r"
impl Machine {
    pub async fn hard_cancel_current_run(&self) {
        let authority = UserInterruptAuthority::new();
        self.hard_cancel_current_run_authorized(authority).await;
    }
}
",
    );
}

#[test]
fn visible_user_interrupt_authority_constructor_fails() {
    expect_failure(
        "visible UserInterruptAuthority constructor",
        "meerkat-runtime/src/user_interrupt.rs",
        r"
struct UserInterruptAuthority(());

impl UserInterruptAuthority {
    pub(super) fn new() -> Self {
        Self(())
    }
}
",
    );
}

#[test]
fn public_hard_cancel_live_handle_fails() {
    expect_failure(
        "public hard-cancel live-handle",
        "meerkat-runtime/src/user_interrupt.rs",
        r#"
impl Machine {
    pub async fn hard_cancel_current_run(&self) {
        let handle = self.interrupt_handle_for(&session_id).await.unwrap();
        handle.hard_cancel_current_run("bad".to_string()).await.unwrap();
    }

    pub(crate) async fn hard_cancel_current_run_authorized(&self) {
        let handle = self.interrupt_handle_for(&session_id).await.unwrap();
        handle.hard_cancel_current_run("allowed".to_string()).await.unwrap();
    }

    async fn interrupt_handle_for(&self) {}
}
"#,
    );
}

#[test]
fn recursive_rpc_interrupt_handle_fails() {
    expect_failure(
        "recursive RPC interrupt-handle",
        "meerkat-rpc/src/session_executor.rs",
        r"
impl CoreExecutorInterruptHandle for SessionRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self) {
        let _ = self.runtime.interrupt(&self.session_id).await;
    }
}
",
    );
}

#[test]
fn bridge_hard_cancel_handler_fails() {
    expect_failure(
        "bridge hard-cancel handler",
        "meerkat-runtime/src/comms_drain.rs",
        r"
async fn bad(adapter: Adapter, session_id: SessionId, command: BridgeCommand) {
    match command {
        BridgeCommand::HardCancelMember(payload) => {
            let _ = adapter
                .hard_cancel_current_run(session_id, payload.reason)
                .await;
        }
        _ => {}
    }
}
",
    );
}

#[test]
fn comms_drain_hard_cancel_fails() {
    expect_failure(
        "comms-drain hard-cancel",
        "meerkat-runtime/src/comms_drain.rs",
        r#"
async fn bad(machine: Machine, session_id: SessionId) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad").await;
}
"#,
    );
}

#[test]
fn local_bridge_hard_cancel_fails() {
    expect_failure(
        "local bridge hard-cancel",
        "meerkat-mob/src/runtime/local_bridge.rs",
        r#"
async fn bad(machine: Machine, session_id: SessionId) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad").await;
}
"#,
    );
}

#[test]
fn public_surface_interrupt_fails() {
    expect_failure(
        "public surface interrupt",
        "meerkat-rest/src/lib.rs",
        r"
async fn public_interrupt(service: Service, session_id: SessionId) {
    let _ = service.interrupt(&session_id).await;
}
",
    );
}

#[test]
fn multiline_public_surface_interrupt_fails() {
    expect_failure(
        "multiline public surface interrupt",
        "meerkat-rest/src/lib.rs",
        r"
async fn public_interrupt(service: Service, session_id: SessionId) {
    let _ = service
        .interrupt(&session_id)
        .await;
}
",
    );
}

#[test]
fn public_interrupt_current_run_fails() {
    expect_failure(
        "public interrupt_current_run",
        "meerkat-rpc/src/realtime_ws.rs",
        r"
async fn public_interrupt(adapter: Adapter, session_id: SessionId) {
    let _ = adapter.interrupt_current_run(&session_id).await;
}
",
    );
}

#[test]
fn example_interrupt_bypass_fails() {
    expect_failure(
        "example interrupt bypass",
        "examples/999-runtime-backed/src/main.rs",
        r"
async fn bad(service: Service, session_id: SessionId) {
    let _ = service.interrupt(&session_id).await;
}
",
    );
}

// --- Rules the script enforced outside its self-test fixtures ---

#[test]
fn tombstone_names_fail_anywhere_including_docs() {
    let first = format!("{}{}", "RunControl", "Command");
    let second = format!("{}{}", "CoreExecutor", "Control");
    expect_failure(
        "tombstone in docs",
        "docs/architecture/old.md",
        &format!("The {first} surface is documented here.\n"),
    );
    expect_failure(
        "tombstone in code comment",
        "meerkat-runtime/src/lib.rs",
        &format!("// {second} used to live here\n"),
    );
}

#[test]
fn user_interrupt_authority_minting_outside_module_fails() {
    expect_failure(
        "authority minted outside user_interrupt",
        "meerkat-runtime/src/runtime_loop.rs",
        r"
fn bad() {
    let authority = UserInterruptAuthority::new();
}
",
    );
}

#[test]
fn peer_admission_file_reaching_interrupt_authority_fails() {
    expect_failure(
        "peer-admission interrupt reach",
        "meerkat-runtime/src/meerkat_machine/peer_admission.rs",
        r"
async fn bad(runtime: Runtime, session_id: SessionId) {
    let _ = runtime.interrupt(&session_id).await;
}
",
    );
}

// --- AST-shape evasion cases: comments/strings must not satisfy the
// detectors; structural variants must. ---

#[test]
fn comment_and_string_tokens_do_not_false_positive() {
    expect_clean(
        "comment/string mentions are not violations",
        "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs",
        r#"
//! Discussing hard_cancel_current_run in docs is fine.
// A comment mentioning interrupt_handle_for and
// MeerkatMachineCommand::InterruptCurrentRun must not flag.
fn fine() {
    let doc = "RuntimeEffect::cancel_after_boundary and interrupt_current_run(";
    let other = r"let _ = tx.try_send(projected_effect.into_effect());";
    let _ = (doc, other);
}
"#,
    );
}

#[test]
fn surface_comment_and_string_do_not_false_positive() {
    expect_clean(
        "surface comment/string mentions are not violations",
        "meerkat-rest/src/lib.rs",
        r#"
// service.interrupt(&session_id) in a comment must not flag
fn fine() {
    let doc = "call service.interrupt(&session_id) to bypass (don't!)";
    let _ = doc;
}
"#,
    );
}

#[test]
fn cfg_test_scoped_bridge_hard_cancel_is_allowed() {
    expect_clean(
        "cfg(test) bridge hard-cancel is out of production scope",
        "meerkat-mob/src/runtime/actor.rs",
        r"
fn production() {}

#[cfg(test)]
mod tests {
    fn fixture(command: BridgeCommand) {
        match command {
            BridgeCommand::HardCancelMember(payload) => drop(payload),
            _ => {}
        }
    }
}
",
    );
}

#[test]
fn core_executor_interrupt_handle_impl_is_allowed_on_surfaces() {
    expect_clean(
        "CoreExecutorInterruptHandle impl is the sanctioned adapter seam",
        "meerkat-cli/src/main.rs",
        r"
struct Handle {
    service: Service,
    session_id: SessionId,
}

impl CoreExecutorInterruptHandle for Handle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .interrupt(&self.session_id)
            .await
    }
}
",
    );
}

#[test]
fn runtime_owned_interrupt_path_is_allowed_on_surfaces() {
    expect_clean(
        "SessionRuntime::interrupt is the machine-routed path",
        "meerkat-rpc/src/handlers/turn.rs",
        r"
async fn handle(runtime: &SessionRuntime, session_id: SessionId) {
    match runtime.interrupt(&session_id).await {
        Ok(()) => {}
        Err(_) => {}
    }
}
",
    );
}

#[test]
fn interrupt_command_match_pattern_is_allowed() {
    expect_clean(
        "destructuring the variant in a match pattern is not construction",
        "meerkat-runtime/src/driver/handler.rs",
        r"
fn classify(command: &MeerkatMachineCommand) -> bool {
    matches!(command, MeerkatMachineCommand::InterruptCurrentRun { .. })
}
",
    );
}

#[test]
fn structural_variant_split_across_lines_still_fails() {
    expect_failure(
        "multi-line UFCS direct interrupt call",
        "meerkat-rest/src/handlers.rs",
        r"
async fn bad(service: &Service, session_id: SessionId) {
    let _ = Service::interrupt_current_run(
        service,
        &session_id,
    )
    .await;
}
",
    );
}

#[test]
fn allowlisted_example_stays_clean() {
    expect_clean(
        "standalone demo example is explicitly classified",
        "examples/034-codemob-mcp/src/tools/consult.rs",
        r"
async fn force_state(service: Service, session_id: SessionId) {
    let _ = service.interrupt(&session_id).await;
}
",
    );
}

#[test]
fn tests_scoped_interrupt_current_run_is_allowed() {
    expect_clean(
        "runtime test files may exercise interrupt_current_run",
        "meerkat-runtime/tests/interrupt.rs",
        r"
async fn drive(machine: &Machine, session_id: SessionId) {
    let _ = machine.interrupt_current_run(&session_id).await;
}
",
    );
}

// --- W2-F bridge-classifier gate (xtask bridge-classifier) ---

mod bridge_classifier {
    use super::{tempdir, write_file};
    use xtask::bridge_classifier::collect_bridge_classifier_findings;

    fn findings_for(rel: &str, contents: &str) -> Vec<String> {
        let dir = tempdir().expect("tempdir");
        write_file(dir.path(), rel, contents);
        collect_bridge_classifier_findings(dir.path()).expect("collect findings")
    }

    #[test]
    fn live_workspace_bridge_classifier_gate_is_clean() {
        let root = xtask::public_contracts::repo_root().expect("repo root");
        let findings = collect_bridge_classifier_findings(&root).expect("collect findings");
        assert!(
            findings.is_empty(),
            "W2-F bridge-classifier gate must be clean for the committed workspace, got {findings:#?}"
        );
    }

    #[test]
    fn match_on_response_status_fails() {
        let findings = findings_for(
            "meerkat-mob/src/runtime/supervisor_bridge.rs",
            r"
fn bad(reply: Reply) -> bool {
    match ResponseStatus::from(reply) {
        _ => false,
    }
}

fn arm_bad(status: ResponseStatus) -> bool {
    match status {
        ResponseStatus::Completed => true,
        _ => false,
    }
}
",
        );
        assert!(
            findings.len() >= 2,
            "ResponseStatus-naming scrutinee and terminal-variant arm must both flag, got {findings:#?}"
        );
    }

    #[test]
    fn multiline_match_and_terminal_variant_fail() {
        let findings = findings_for(
            "meerkat-contracts/src/wire/supervisor_bridge.rs",
            r"
fn bad(reply: &Reply) -> bool {
    match reply
        .response_status_view()
        .map(ResponseStatus::from)
    {
        _ => false,
    }
}

fn also_bad() -> ResponseStatus {
    ResponseStatus::Completed
}
",
        );
        assert!(
            findings.len() >= 2,
            "multi-line match scrutinee and terminal variant must both flag, got {findings:#?}"
        );
    }

    #[test]
    fn matches_macro_terminal_variant_fails() {
        let findings = findings_for(
            "meerkat-mob/src/runtime/local_bridge.rs",
            r"
fn bad(status: ResponseStatus) -> bool {
    matches!(status, ResponseStatus::Completed)
}
",
        );
        assert!(
            !findings.is_empty(),
            "terminal-variant interpretation inside matches! must flag"
        );
        let clean = findings_for(
            "meerkat-mob/src/runtime/local_bridge.rs",
            r#"
fn fine() {
    tracing::debug!("docs mention ResponseStatus::Completed in a string");
}
"#,
        );
        assert!(
            clean.is_empty(),
            "string-literal mention inside a macro must not flag, got {clean:#?}"
        );
    }

    #[test]
    fn comments_strings_and_cfg_test_do_not_false_positive() {
        let findings = findings_for(
            "meerkat-mob/src/runtime/local_bridge.rs",
            r#"
//! Doc text naming ResponseStatus::Completed must not flag.
use meerkat_core::interaction::ResponseStatus;

fn fine(status: ResponseStatus) -> Wire {
    // match status { ResponseStatus::Completed => ... } in a comment is fine
    let doc = "match status ResponseStatus::Failed";
    transport(status, doc)
}

#[cfg(test)]
mod tests {
    fn fixture() -> ResponseStatus {
        match probe() {
            _ => ResponseStatus::Completed,
        }
    }
}
"#,
        );
        assert!(
            findings.is_empty(),
            "type transport, comments, strings, and cfg(test) must not flag, got {findings:#?}"
        );
    }
}
