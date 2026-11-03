#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use xtask::runtime_authority_bypass::collect_runtime_authority_bypass_findings;

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
    collect_runtime_authority_bypass_findings(dir.path()).expect("collect findings")
}

fn findings_for_files(files: &[(&str, &str)]) -> Vec<String> {
    let dir = tempdir().expect("tempdir");
    for (rel, contents) in files {
        write_file(dir.path(), rel, contents);
    }
    collect_runtime_authority_bypass_findings(dir.path()).expect("collect findings")
}

fn expect_failure(name: &str, contents: &str) {
    let findings = findings_for("crates/meerkat-runtime/src/driver/ephemeral.rs", contents);
    assert!(
        !findings.is_empty(),
        "{name}: fixture must fail the audit but passed"
    );
}

fn expect_clean(name: &str, contents: &str) {
    let findings = findings_for("crates/meerkat-runtime/src/driver/ephemeral.rs", contents);
    assert!(
        findings.is_empty(),
        "{name}: fixture must pass the audit, got {findings:#?}"
    );
}

#[test]
fn live_workspace_runtime_authority_bypass_audit_is_clean() {
    let root = xtask::public_contracts::repo_root().expect("repo root");
    let findings = collect_runtime_authority_bypass_findings(&root).expect("collect findings");
    assert!(
        findings.is_empty(),
        "runtime-authority-bypass audit must be clean for the committed workspace, got {findings:#?}"
    );
}

#[test]
fn planted_raw_enqueue_violation_fails() {
    expect_failure(
        "raw enqueue",
        r"
impl Driver {
    fn bad(&mut self, input_id: InputId, input: Input) {
        self.queue.enqueue(input_id, input);
    }
}
",
    );
}

#[test]
fn planted_raw_dequeue_violation_fails() {
    expect_failure(
        "raw dequeue",
        r"
impl Driver {
    fn bad(&mut self) {
        let _ = self.steer_queue.dequeue();
    }
}
",
    );
}

#[test]
fn planted_public_raw_dequeue_api_fails() {
    expect_failure(
        "public raw dequeue API",
        r"
impl Driver {
    pub fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        self.queue.dequeue().map(|queued| (queued.input_id, queued.input))
    }
}
",
    );
}

#[test]
fn planted_raw_stage_violation_fails() {
    expect_failure(
        "raw stage",
        r"
impl Driver {
    fn bad(&mut self, input_ids: &[InputId], run_id: &RunId) {
        let _ = self.machine_realize_stage_batch(input_ids, run_id);
    }
}
",
    );
}

#[test]
fn planted_public_queue_module_violation_fails() {
    let findings = findings_for_files(&[("crates/meerkat-runtime/src/lib.rs", "pub mod queue;")]);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("public raw InputQueue module")),
        "public queue module fixture must fail the audit, got {findings:#?}"
    );
}

#[test]
fn planted_public_input_queue_reexport_violation_fails() {
    let findings = findings_for_files(&[(
        "crates/meerkat-runtime/src/lib.rs",
        "pub(crate) mod queue;\npub use queue::InputQueue;",
    )]);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("public raw InputQueue re-export")),
        "public InputQueue re-export fixture must fail the audit, got {findings:#?}"
    );
}

#[test]
fn planted_generated_capability_mint_outside_runtime_bridge_fails() {
    let findings = findings_for_files(&[(
        "crates/meerkat-runtime/src/completion.rs",
        r"
fn bad() {
    let _ = generated_command_capabilities::AuthorizedRuntimeLoopBatch::mint_from_generated_command_plan();
}
",
    )]);
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("generated capability mint outside runtime bridge")),
        "minting generated command capabilities outside the bridge must fail, got {findings:#?}"
    );
}

#[test]
fn comments_and_strings_are_not_violations() {
    expect_clean(
        "comments and strings",
        r#"
fn docs_only() {
    // self.queue.enqueue(input_id, input);
    let _ = "self.machine_realize_stage_batch(input_ids, run_id)";
}
"#,
    );
}

#[test]
fn authorized_stage_wrapper_is_clean() {
    expect_clean(
        "authorized stage wrapper",
        r"
impl Driver {
    fn machine_realize_authorized_stage_batch(&mut self, authority: AuthorizedStageForRun) {
        let (input_ids, run_id, _source) = authority.into_parts();
        let _ = self.machine_realize_stage_batch(&input_ids, &run_id);
    }
}
",
    );
}

#[test]
fn planted_mob_only_session_call_outside_the_mob_runtime_fails() {
    let findings = findings_for(
        "crates/meerkat-rpc/src/forwarding.rs",
        r"
async fn forward(service: &Service, id: &SessionId, record: Record) {
    let _ = service.record_mob_instruction_target(id, record).await;
}
",
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("record_mob_instruction_target")),
        "a call outside the mob runtime's session service must fail, got {findings:#?}"
    );
}

#[test]
fn the_mob_session_service_and_test_code_may_call_the_mob_only_entry_points() {
    let findings = findings_for_files(&[
        (
            "crates/meerkat-mob/src/runtime/session_service.rs",
            r"
async fn record(service: &Service, id: &SessionId, record: Record) {
    let _ = service.record_mob_instruction_target(id, record).await;
}
",
        ),
        (
            "crates/meerkat-rpc/src/lib.rs",
            r#"
#[cfg(test)]
mod tests {
    async fn release(service: &Service, id: &SessionId) {
        let _ = service.release_mob_instruction_target(id, "mob").await;
    }
}
"#,
        ),
    ]);
    assert!(
        findings.is_empty(),
        "the mob runtime's session service and test code must pass, got {findings:#?}"
    );
}

#[test]
fn a_path_form_mob_only_session_call_fails() {
    let findings = findings_for(
        "crates/meerkat-rpc/src/forwarding.rs",
        r"
async fn forward(service: &Service, id: &SessionId, record: Record) {
    let _ = PersistentSessionService::record_mob_instruction_target(service, id, record).await;
}

fn release_entry() {
    let release = PersistentSessionService::release_mob_instruction_target;
    drop(release);
}
",
    );
    for method in [
        "record_mob_instruction_target",
        "release_mob_instruction_target",
    ] {
        assert!(
            findings.iter().any(|finding| finding.contains(method)),
            "a path naming `{method}` outside the mob runtime's session service must fail, \
             got {findings:#?}"
        );
    }
}

#[test]
fn a_mob_only_session_call_inside_a_macro_body_fails() {
    let findings = findings_for(
        "meta/crates/meerkat-meta/src/forwarding.rs",
        r#"
async fn race(service: &Service, id: &SessionId) {
    tokio::select! {
        _ = service.release_mob_instruction_target(id, "mob") => {}
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
    }
}
"#,
    );
    assert!(
        findings
            .iter()
            .any(|finding| finding.contains("release_mob_instruction_target")),
        "a call inside a macro body must fail, got {findings:#?}"
    );
}

#[test]
fn a_mob_only_session_call_in_an_example_fails() {
    let call = r"
async fn main_flow(service: &Service, id: &SessionId, record: Record) {
    let _ = service.record_mob_instruction_target(id, record).await;
}
";
    let examples = [
        "examples/099-instructions-rs/src/main.rs",
        "crates/meerkat/examples/instructions.rs",
    ];
    let findings = findings_for_files(&[(examples[0], call), (examples[1], call)]);
    for example in examples {
        assert!(
            findings.iter().any(|finding| finding.starts_with(example)),
            "a call in {example} must fail, got {findings:#?}"
        );
    }
}

#[test]
fn mob_only_names_in_comments_and_strings_pass() {
    let findings = findings_for(
        "crates/meerkat-rpc/src/forwarding.rs",
        r#"
// record_mob_instruction_target belongs to the mob runtime alone.
/// So does release_mob_instruction_target.
fn describe() -> &'static str {
    tracing::info!("record_mob_instruction_target is sealed");
    /* release_mob_instruction_target(id) */
    "release_mob_instruction_target"
}
"#,
    );
    assert!(
        findings.is_empty(),
        "a name in a comment or a string names no entry point, got {findings:#?}"
    );
}
