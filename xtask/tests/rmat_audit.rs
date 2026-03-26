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
        "meerkat-mob/src/runtime/mob_lifecycle_authority.rs",
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
            && finding.key.path == "meerkat-mob/src/runtime/mob_lifecycle_authority.rs"
            && finding.key.symbol == "FutureInput"
    }));
    assert!(findings.iter().any(|finding| {
        finding.key.rule == "NoDeadAuthorityWiring"
            && finding.key.path == "meerkat-mob/src/runtime/mob_lifecycle_authority.rs"
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
