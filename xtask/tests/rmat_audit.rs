#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use xtask::rmat_audit::collect_findings;
use xtask::rmat_policy::AuditPolicy;

fn write_file(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

#[test]
fn catches_parallel_transition_table() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-core/src/state.rs",
        r"
pub enum LoopState { Completed }
impl LoopState {
    pub fn transition(&mut self, next: LoopState) { *self = next; }
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "NoParallelTransitionTable"
            && finding.key.path == "meerkat-core/src/state.rs"
    }));
}

#[test]
fn catches_guarded_apply() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-runtime/src/driver/ephemeral.rs",
        r"
struct Driver;
impl Driver {
    fn current_state(&self) -> u8 { 0 }
    fn apply(&mut self) {}
}
fn probe(driver: &mut Driver) {
    if driver.current_state() == 0 {
        driver.apply();
    }
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(
        findings
            .iter()
            .any(|finding| finding.key.rule == "NoGuardedApply")
    );
}

#[test]
fn catches_dead_authority_dead_code() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mob/src/runtime/actor.rs",
        r"
#[allow(dead_code)]
enum FutureInput {
    Pending,
}

#[allow(dead_code)]
fn future_shell_hook() {}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "NoDeadAuthorityWiring"
            && finding.key.path == "meerkat-mob/src/runtime/actor.rs"
            && finding.key.symbol == "FutureInput"
    }));
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "NoDeadAuthorityWiring"
            && finding.key.path == "meerkat-mob/src/runtime/actor.rs"
            && finding.key.symbol == "future_shell_hook"
    }));
}

#[test]
fn catches_protected_flag_write() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-runtime/src/driver/ephemeral.rs",
        r"
struct Driver { wake_requested: bool }
fn wrong(driver: &mut Driver) {
    driver.wake_requested = true;
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "NoShellSemanticFlagWrites"
            && finding.key.path == "meerkat-runtime/src/driver/ephemeral.rs"
    }));
}

#[test]
fn warns_on_lifecycle_suspicion() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-comms/src/runtime/comms_runtime.rs",
        r"
enum ReservationState { Reserved, Attached, Completed }
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(
        findings
            .iter()
            .any(|finding| finding.key.rule == "LifecycleSuspicionReport")
    );
}

#[test]
fn lifecycle_suspicion_can_be_suppressed() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-comms/src/runtime/comms_runtime.rs",
        r"
// RMAT-ALLOW(LifecycleSuspicionReport): internal reservation lifecycle
enum ReservationState { Reserved, Attached, Completed }
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    let finding = findings
        .iter()
        .find(|finding| finding.key.rule == "LifecycleSuspicionReport")
        .expect("suspicion finding");
    assert!(finding.suppressed);
}

#[test]
fn forbidden_shell_reads_catches_phase_call_in_mob_actor() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mob/src/runtime/actor.rs",
        r"
struct Orchestrator;
impl Orchestrator { fn phase(&self) -> u8 { 0 } fn apply(&mut self) {} }
struct Actor { orch: Orchestrator }
impl Actor {
    fn handle(&mut self) {
        let _ = self.orch.phase();
        self.orch.apply();
    }
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "ForbiddenShellAuthorityReads"
            && finding.key.path == "meerkat-mob/src/runtime/actor.rs"
            && finding.key.symbol.contains(".phase()")
            && !finding.suppressed
    }));
}

#[test]
fn forbidden_shell_reads_allows_lifecycle_authority_phase_in_mob_actor() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mob/src/runtime/actor.rs",
        r"
struct Authority;
impl Authority { fn phase(&self) -> u8 { 0 } }
struct Actor { lifecycle_authority: Authority }
impl Actor {
    fn handle(&self) -> u8 {
        self.lifecycle_authority.phase()
    }
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(
        !findings
            .iter()
            .any(|finding| finding.key.rule == "ForbiddenShellAuthorityReads"),
        "lifecycle_authority.phase() must be allowed; got {findings:#?}"
    );
}

#[test]
fn forbidden_shell_reads_catches_policy_field_in_ephemeral_driver() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-runtime/src/driver/ephemeral.rs",
        r"
struct Policy { apply_mode: u8, queue_mode: u8, consume_point: u8 }
fn route(policy: &Policy) -> u8 {
    match policy.apply_mode {
        0 => policy.queue_mode,
        _ => policy.consume_point,
    }
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    let forbidden: Vec<_> = findings
        .iter()
        .filter(|finding| finding.key.rule == "ForbiddenShellAuthorityReads")
        .collect();
    let has_symbol = |needle: &str| {
        forbidden
            .iter()
            .any(|finding| finding.key.symbol == needle && !finding.suppressed)
    };
    assert!(has_symbol("policy.apply_mode"), "{forbidden:#?}");
    assert!(has_symbol("policy.queue_mode"), "{forbidden:#?}");
    assert!(has_symbol("policy.consume_point"), "{forbidden:#?}");
}

#[test]
fn forbidden_shell_reads_catches_shadow_field_in_mcp_router() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mcp/src/router.rs",
        r"
use std::collections::HashMap;
struct Router { removal_timeouts: HashMap<String, u64> }
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "ForbiddenShellAuthorityReads"
            && finding.key.path == "meerkat-mcp/src/router.rs"
            && finding.key.symbol == "Router::removal_timeouts"
            && !finding.suppressed
    }));
}

#[test]
fn forbidden_shell_reads_catches_shadow_counters_in_mob_actor_struct() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mob/src/runtime/actor.rs",
        r"
struct MobActor {
    tracked_flows: std::collections::BTreeMap<String, String>,
    pending_spawn_ids: Vec<String>,
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    let has_symbol = |needle: &str| {
        findings.iter().any(|finding| {
            finding.key.rule == "ForbiddenShellAuthorityReads" && finding.key.symbol == needle
        })
    };
    assert!(has_symbol("MobActor::tracked_flows"), "{findings:#?}");
    assert!(has_symbol("MobActor::pending_spawn_ids"), "{findings:#?}");
}

#[test]
fn forbidden_shell_reads_ignores_struct_literal_projection_of_allowed_snapshot() {
    // A shell file may populate a legitimate snapshot type that happens to
    // *have* a field named `tracked_flows` — the field declaration lives on
    // the snapshot type in another file. AST-scoped FieldDeclared rules must
    // not flag struct-literal initializers as if they were struct decls.
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mob/src/runtime/actor.rs",
        r"
struct Snapshot;
fn projection() -> Snapshot {
    Snapshot { tracked_flows: Default::default() }
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    assert!(
        !findings
            .iter()
            .any(|finding| finding.key.rule == "ForbiddenShellAuthorityReads"),
        "struct-literal field names must not be flagged as declarations; got {findings:#?}"
    );
}

#[test]
fn forbidden_shell_reads_production_rmat_allow_does_not_suppress() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-mcp/src/router.rs",
        r"
use std::collections::HashMap;
struct Router {
    // RMAT-ALLOW(ForbiddenShellAuthorityReads): production-style fixture must remain unsuppressed
    removal_timeouts: HashMap<String, u64>,
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    let finding = findings
        .iter()
        .find(|finding| {
            finding.key.rule == "ForbiddenShellAuthorityReads"
                && finding.key.symbol == "Router::removal_timeouts"
        })
        .expect("forbidden finding");
    assert!(
        !finding.suppressed,
        "ForbiddenShellAuthorityReads suppressions must not mask production violations",
    );
}

#[test]
fn forbidden_shell_reads_fixture_rmat_allow_is_test_only() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "rmat-test-fixtures/meerkat-mcp/src/router.rs",
        r"
use std::collections::HashMap;
struct Router {
    // RMAT-ALLOW(ForbiddenShellAuthorityReads): fixture-only canary
    removal_timeouts: HashMap<String, u64>,
}
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    let finding = findings
        .iter()
        .find(|finding| {
            finding.key.rule == "ForbiddenShellAuthorityReads"
                && finding.key.symbol == "Router::removal_timeouts"
        })
        .expect("forbidden finding");
    assert!(
        finding.suppressed,
        "ForbiddenShellAuthorityReads suppressions are accepted only under explicit RMAT fixture paths",
    );
}

#[test]
fn legacy_rmat_read_seam_script_is_removed() {
    // Regression guard: the regex-based governance shim must not reappear.
    // Compile-time AST rules (ForbiddenShellAuthorityReads) are the single
    // authority for shell/authority read-seam enforcement.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask parent is repo root");
    let script = repo_root.join("scripts").join("rmat-read-seam-lint.sh");
    assert!(
        !script.exists(),
        "scripts/rmat-read-seam-lint.sh must be deleted — see docs/architecture/RMAT.md"
    );
    let makefile = std::fs::read_to_string(repo_root.join("Makefile")).expect("read Makefile");
    assert!(
        !makefile.contains("rmat-read-seam-lint"),
        "Makefile must not reference the deleted rmat-read-seam-lint target"
    );
}

#[test]
fn suppression_must_be_local_not_file_wide() {
    let dir = tempdir().expect("tempdir");
    write_file(
        dir.path(),
        "meerkat-comms/src/runtime/comms_runtime.rs",
        r"
// RMAT-ALLOW(LifecycleSuspicionReport): only for ReservationState
enum ReservationState { Reserved, Attached, Completed }

fn unrelated() {}

enum AnotherState { Pending, Running, Completed }
",
    );

    let findings = collect_findings(dir.path(), &AuditPolicy::load()).expect("findings");
    let reservation = findings
        .iter()
        .find(|finding| {
            finding.key.rule == "LifecycleSuspicionReport"
                && finding.key.symbol == "ReservationState"
        })
        .expect("reservation suspicion");
    assert!(reservation.suppressed);

    let another = findings
        .iter()
        .find(|finding| {
            finding.key.rule == "LifecycleSuspicionReport" && finding.key.symbol == "AnotherState"
        })
        .expect("another suspicion");
    assert!(!another.suppressed);
}
