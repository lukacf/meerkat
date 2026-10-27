#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::process::Command;

use meerkat_meta::{
    GovernanceValidationError, validate_governance_contracts, validate_governance_state,
    validate_traceability_statuses,
};

#[test]
fn governance_contracts_rejects_invalid_governance_state() {
    let err = validate_governance_state("spec", "governance_state: blocked")
        .expect_err("invalid governance state must fail");
    assert_eq!(
        err,
        GovernanceValidationError::InvalidGovernanceState {
            file: "spec".to_string(),
            found: "blocked".to_string(),
        }
    );
}

#[test]
fn governance_contracts_rejects_missing_governance_state() {
    let valid_traceability = "\
| REQ-ID | Phase | Implemented In | Runtime Caller | Evidence | Status |\n\
|--------|-------|----------------|----------------|----------|--------|\n\
| TYPE-001 | P0 | .rct/* | - | .rct/outputs/P0/ | MISSING |\n";
    let err = validate_governance_contracts(
        "project: test\n",
        "governance_state: realignment_in_progress\n",
        "governance_state: realignment_in_progress\n",
        valid_traceability,
    )
    .expect_err("missing governance_state must fail");
    assert_eq!(
        err,
        GovernanceValidationError::MissingGovernanceState {
            file: ".rct/spec.yaml".to_string(),
        }
    );
}

#[test]
fn governance_contracts_rejects_unknown_traceability_status() {
    let markdown = "\
| REQ-ID | Phase | Implemented In | Runtime Caller | Evidence | Status |\n\
|--------|-------|----------------|----------------|----------|--------|\n\
| CONTRACT-999 | P0 | - | - | .rct/outputs/P0/ | PENDING |\n";
    let err = validate_traceability_statuses(markdown)
        .expect_err("unknown status should fail validation");
    assert!(matches!(
        err,
        GovernanceValidationError::InvalidTraceabilityStatus { .. }
    ));
}

#[test]
fn governance_contracts_accepts_all_current_traceability_statuses() {
    let mut markdown = String::from(
        "\
| REQ-ID | Phase | Implemented In | Runtime Caller | Evidence | Status |\n\
|--------|-------|----------------|----------------|----------|--------|\n",
    );
    for (index, status) in [
        "TYPED",
        "WIRED",
        "VALIDATED",
        "PROVISIONAL",
        "MISSING",
        "DEFERRED",
        "STUBBED",
    ]
    .iter()
    .enumerate()
    {
        markdown.push_str(&format!(
            "| TYPE-{index:03} | P0 | .rct/* | - | .rct/outputs/P0/ | {status} |\n"
        ));
    }

    validate_traceability_statuses(&markdown).expect("all governance statuses should be accepted");
}

#[test]
fn governance_contracts_rejects_missing_traceability_evidence() {
    let markdown = "\
| REQ-ID | Phase | Implemented In | Runtime Caller | Evidence | Status |\n\
|--------|-------|----------------|----------------|----------|--------|\n\
| TYPE-001 | P0 | .rct/* | - |   | TYPED |\n";
    let err = validate_traceability_statuses(markdown)
        .expect_err("missing evidence should fail validation");
    assert!(matches!(
        err,
        GovernanceValidationError::MissingTraceabilityEvidence { .. }
    ));
}

#[test]
fn governance_contracts_rejects_placeholder_traceability_evidence() {
    let markdown = "\
| REQ-ID | Phase | Implemented In | Runtime Caller | Evidence | Status |\n\
|--------|-------|----------------|----------------|----------|--------|\n\
| TYPE-001 | P0 | .rct/* | - | - | TYPED |\n";
    let err = validate_traceability_statuses(markdown)
        .expect_err("placeholder evidence should fail validation");
    assert!(matches!(
        err,
        GovernanceValidationError::MissingTraceabilityEvidence { .. }
    ));
}

#[test]
fn governance_contracts_accepts_yaml_traceability_with_missing_rows_unimplemented() {
    let yaml = r#"
rows:
  - id: TYPE-001
    status: MISSING
    evidence: []
"#;

    validate_traceability_statuses(yaml).expect("yaml traceability should validate");
}

#[test]
fn governance_contracts_rejects_yaml_traceability_missing_typed_evidence() {
    let yaml = r#"
rows:
  - id: TYPE-001
    status: TYPED
    evidence: []
"#;

    let err = validate_traceability_statuses(yaml)
        .expect_err("typed yaml rows without evidence must fail validation");
    assert!(matches!(
        err,
        GovernanceValidationError::MissingTraceabilityEvidence { .. }
    ));
}

/// Unquoted scalars in a traceability row are text: numbers and YAML 1.1
/// words reach the status and evidence checks as written.
#[test]
fn traceability_yaml_scalars_are_text() {
    validate_traceability_statuses("rows:\n  - status: WIRED\n    evidence: [1, true, 1.10]\n")
        .expect("numeric and boolean-looking evidence entries are text");

    let err = validate_traceability_statuses("rows:\n  - status: yes\n    evidence: [x]\n")
        .expect_err("`yes` is the status text, not a boolean");
    assert_eq!(
        err,
        GovernanceValidationError::InvalidTraceabilityStatus {
            line: 1,
            status: "yes".to_string(),
        }
    );
}

/// Traceability YAML the parser refuses as a malformed row: a `<<` merge (an
/// ordinary key, so the row has no status), a second document, and a
/// duplicated row field.
#[test]
fn traceability_yaml_refuses_merges_documents_and_duplicate_fields() {
    for (case, yaml) in [
        (
            "merge key",
            "base: &b {status: TYPED, evidence: [x]}\nrows:\n  - <<: *b\n",
        ),
        (
            "second document",
            "rows:\n  - status: TYPED\n    evidence: [x]\n---\nrows: []\n",
        ),
        (
            "duplicate status",
            "rows:\n  - status: TYPED\n    status: WIRED\n    evidence: [x]\n",
        ),
    ] {
        let err = validate_traceability_statuses(yaml).expect_err(case);
        assert_eq!(
            err,
            GovernanceValidationError::InvalidTraceabilityRow { line: 1 },
            "{case}"
        );
    }
}

/// Traceability YAML refused as a malformed row since the parser moved to
/// serde-saphyr. serde_yaml accepted eight of these: a custom tag (it dropped
/// the tag and kept the text), a key given twice in a row or at the top level
/// (it kept the last), a null evidence entry (stored as "~", which passed as
/// evidence), a `...` document end marker, nesting deeper than 64 levels, and
/// 101 aliases of one anchor (the parser budget refuses 100 or more aliases
/// that outnumber their anchors more than ten to one).
/// It refused two differently: a null status as the status "~", and an empty
/// evidence entry as missing evidence. It refused a `...` after a lone CR
/// (YAML breaks lines there) as a malformed row too; the guard must see it.
#[test]
fn traceability_yaml_refuses_tags_duplicates_nulls_end_markers_and_budget() {
    let deep = format!(
        "rows:\n  - status: TYPED\n    evidence: [x]\n    deep: {}{}\n",
        "[".repeat(70),
        "]".repeat(70)
    );
    let cases = [
        (
            "tagged status",
            "rows:\n  - status: !custom TYPED\n    evidence: [x]\n".to_string(),
        ),
        (
            "tagged evidence",
            "rows:\n  - status: TYPED\n    evidence: [!custom x]\n".to_string(),
        ),
        (
            "duplicate key in a row",
            "rows:\n  - id: A\n    id: B\n    status: TYPED\n    evidence: [x]\n".to_string(),
        ),
        (
            "duplicate top-level key",
            "owner: a\nowner: b\nrows:\n  - status: TYPED\n    evidence: [x]\n".to_string(),
        ),
        (
            "null evidence",
            "rows:\n  - status: TYPED\n    evidence: [~]\n".to_string(),
        ),
        (
            "empty evidence entry",
            "rows:\n  - status: TYPED\n    evidence:\n      -\n".to_string(),
        ),
        (
            "null status",
            "rows:\n  - status: ~\n    evidence: [x]\n".to_string(),
        ),
        (
            "document end marker",
            "rows:\n  - status: TYPED\n    evidence: [x]\n... # end\n".to_string(),
        ),
        (
            "document end marker after a lone CR",
            "rows:\n  - status: TYPED\n    evidence: [x]\r...\r]]] not yaml\n".to_string(),
        ),
        ("nesting past the budget", deep),
        ("101 aliases of one anchor", aliased_evidence(101)),
    ];
    let mismatched: Vec<(&str, Result<(), GovernanceValidationError>)> = cases
        .iter()
        .map(|(case, yaml)| (*case, validate_traceability_statuses(yaml)))
        .filter(|(_, result)| {
            *result != Err(GovernanceValidationError::InvalidTraceabilityRow { line: 1 })
        })
        .collect();
    assert!(
        mismatched.is_empty(),
        "not refused as a malformed row: {mismatched:?}"
    );
}

/// 99 aliases of one anchor stay within the parser budget's alias ratio.
#[test]
fn traceability_yaml_accepts_aliases_within_the_budget_ratio() {
    validate_traceability_statuses(&aliased_evidence(99))
        .expect("99 aliases of one anchor are accepted");
}

/// One traceability row whose evidence is an anchored `x` and `aliases`
/// aliases of it.
fn aliased_evidence(aliases: usize) -> String {
    format!(
        "rows:\n  - status: TYPED\n    evidence: [&a x{}]\n",
        ", *a".repeat(aliases)
    )
}

/// Text after a `...` document end marker is refused, malformed or not.
/// serde_yaml refused it; serde-saphyr 2.0.0 alone would ignore it, so the
/// marker itself is refused.
#[test]
fn traceability_yaml_refuses_text_after_a_document_end_marker() {
    let err = validate_traceability_statuses(
        "rows:\n  - status: TYPED\n    evidence: [x]\n...\n]]] not yaml\n",
    )
    .expect_err("text after the marker is not ignored");
    assert_eq!(
        err,
        GovernanceValidationError::InvalidTraceabilityRow { line: 1 }
    );
}

#[test]
fn governance_contracts_accepts_markdown_traceability_after_leading_prose() {
    let markdown = r#"
# Traceability

Intro text before the table is allowed.

| REQ-ID | Phase | Implemented In | Runtime Caller | Evidence | Status |
|--------|-------|----------------|----------------|----------|--------|
| TYPE-001 | P0 | .rct/* | - | .rct/phase-0-evidence.txt | TYPED |
"#;

    validate_traceability_statuses(markdown)
        .expect("markdown traceability with leading prose should validate");
}

/// RCT documents are local working state this repository does not track,
/// so the binary has no default root: without an explicit `--root` it
/// refuses with its usage instead of reading whatever directory it runs in.
#[test]
fn governance_check_requires_an_explicit_root() {
    let output = Command::new(env!("CARGO_BIN_EXE_governance_check"))
        .output()
        .expect("governance_check should run");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--root <DIR>"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
