//! Governance validation for runtime configuration and deployment policies.

use std::fmt;

use serde::Deserialize;

pub const STRICT_TRACEABILITY_STATUSES: &[&str] = &[
    "TYPED",
    "WIRED",
    "VALIDATED",
    "PROVISIONAL",
    "MISSING",
    "DEFERRED",
    "STUBBED",
];
const REQUIRED_GOVERNANCE_STATE: &str = "realignment_in_progress";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernanceValidationError {
    MissingGovernanceState { file: String },
    InvalidGovernanceState { file: String, found: String },
    NoTraceabilityRows,
    InvalidTraceabilityStatus { line: usize, status: String },
    MissingTraceabilityEvidence { line: usize },
    InvalidTraceabilityRow { line: usize },
}

impl fmt::Display for GovernanceValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGovernanceState { file } => {
                write!(f, "missing governance_state in {file}")
            }
            Self::InvalidGovernanceState { file, found } => write!(
                f,
                "invalid governance_state in {file}: expected {REQUIRED_GOVERNANCE_STATE}, found {found}"
            ),
            Self::NoTraceabilityRows => write!(f, "no traceability rows found"),
            Self::InvalidTraceabilityStatus { line, status } => {
                write!(f, "invalid traceability status at line {line}: {status}")
            }
            Self::MissingTraceabilityEvidence { line } => {
                write!(f, "missing traceability evidence/link at line {line}")
            }
            Self::InvalidTraceabilityRow { line } => {
                write!(f, "invalid traceability row format at line {line}")
            }
        }
    }
}

impl std::error::Error for GovernanceValidationError {}

#[derive(Debug, Deserialize)]
struct TraceabilityDocument {
    #[serde(default)]
    rows: Vec<TraceabilityRow>,
}

#[derive(Debug, Deserialize)]
struct TraceabilityRow {
    status: String,
    #[serde(default)]
    evidence: Vec<String>,
}

pub fn validate_governance_state(
    file_name: &str,
    content: &str,
) -> Result<(), GovernanceValidationError> {
    let line = content
        .lines()
        .find(|line| line.trim_start().starts_with("governance_state:"))
        .ok_or_else(|| GovernanceValidationError::MissingGovernanceState {
            file: file_name.to_string(),
        })?;

    let found = line
        .split_once(':')
        .map(|(_, value)| value.trim())
        .unwrap_or_default()
        .to_string();

    if found != REQUIRED_GOVERNANCE_STATE {
        return Err(GovernanceValidationError::InvalidGovernanceState {
            file: file_name.to_string(),
            found,
        });
    }

    Ok(())
}

pub fn validate_traceability_statuses(markdown: &str) -> Result<(), GovernanceValidationError> {
    if looks_like_markdown_table(markdown) {
        return validate_markdown_traceability_statuses(markdown);
    }

    validate_yaml_traceability_statuses(markdown)
}

fn validate_markdown_traceability_statuses(
    markdown: &str,
) -> Result<(), GovernanceValidationError> {
    let mut seen_rows = false;
    let mut status_column = None;
    let mut evidence_or_link_column = None;
    let mut header_line = None;

    for (idx, line) in markdown.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }

        let columns = trimmed
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();

        if columns.is_empty()
            || columns
                .iter()
                .all(|column| !column.is_empty() && column.chars().all(|ch| ch == '-' || ch == ':'))
        {
            continue;
        }

        if status_column.is_none() {
            header_line = Some(idx + 1);
            status_column = columns
                .iter()
                .position(|column| column.eq_ignore_ascii_case("Status"));
            evidence_or_link_column = columns
                .iter()
                .position(|column| is_evidence_or_link_column(column));
            continue;
        }

        let Some(status_column) = status_column else {
            continue;
        };
        let Some(evidence_or_link_column) = evidence_or_link_column else {
            return Err(GovernanceValidationError::InvalidTraceabilityRow {
                line: header_line.unwrap_or(idx + 1),
            });
        };
        if columns.len() <= status_column || columns.len() <= evidence_or_link_column {
            return Err(GovernanceValidationError::InvalidTraceabilityRow { line: idx + 1 });
        }

        seen_rows = true;
        let status = columns[status_column].trim_matches('`');
        if !STRICT_TRACEABILITY_STATUSES.contains(&status) {
            return Err(GovernanceValidationError::InvalidTraceabilityStatus {
                line: idx + 1,
                status: status.to_string(),
            });
        }
        let evidence = columns[evidence_or_link_column].trim_matches('`').trim();
        if status_requires_evidence(status) && is_missing_evidence(evidence) {
            return Err(GovernanceValidationError::MissingTraceabilityEvidence { line: idx + 1 });
        }
    }

    if !seen_rows {
        return Err(GovernanceValidationError::NoTraceabilityRows);
    }

    Ok(())
}

/// The YAML rules for traceability documents, as Meerkat Core's SKILL.md
/// frontmatter parser applies them: YAML 1.2 booleans, `<<` an ordinary key,
/// a duplicated key and an unrecognized tag refused, and the parser's default
/// budget (nesting depth 64, nodes, events, alias expansion).
fn traceability_yaml_options() -> serde_saphyr::Options {
    let mut options = serde_saphyr::Options::default();
    options.strict_booleans = true;
    options.merge_keys = serde_saphyr::MergeKeyPolicy::AsOrdinary;
    options.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::Error;
    options.reject_unsupported_tags = true;
    options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::PassThrough;
    options.with_snippet = false;
    options
}

/// A `...` document end marker line. A traceability document is one YAML
/// document, and serde-saphyr 2.0.0 stops at the marker and ignores what
/// follows it, even malformed text: upstream issue #228, fixed after 2.0.0 by
/// <https://github.com/bourumir-wyngs/serde-saphyr/pull/230>. Remove this
/// guard at the bump to a release with that fix, and decide then whether a
/// marker with nothing after it stays refused (the traceability_yaml tests
/// pin both). Lines break where YAML breaks them: at LF, CRLF and a lone CR.
fn has_document_end_marker(yaml: &str) -> bool {
    yaml.split(['\n', '\r']).any(|line| {
        line.strip_prefix("...")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
    })
}

fn validate_yaml_traceability_statuses(yaml: &str) -> Result<(), GovernanceValidationError> {
    if has_document_end_marker(yaml) {
        return Err(GovernanceValidationError::InvalidTraceabilityRow { line: 1 });
    }
    let document: TraceabilityDocument =
        serde_saphyr::from_str_with_options(yaml, traceability_yaml_options())
            .map_err(|_| GovernanceValidationError::InvalidTraceabilityRow { line: 1 })?;

    if document.rows.is_empty() {
        return Err(GovernanceValidationError::NoTraceabilityRows);
    }

    for (index, row) in document.rows.iter().enumerate() {
        if !STRICT_TRACEABILITY_STATUSES.contains(&row.status.as_str()) {
            return Err(GovernanceValidationError::InvalidTraceabilityStatus {
                line: index + 1,
                status: row.status.clone(),
            });
        }
        if status_requires_evidence(&row.status)
            && row.evidence.iter().all(|entry| is_missing_evidence(entry))
        {
            return Err(GovernanceValidationError::MissingTraceabilityEvidence { line: index + 1 });
        }
    }

    Ok(())
}

fn looks_like_markdown_table(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.trim_start().starts_with('|'))
}

fn is_evidence_or_link_column(column: &str) -> bool {
    let normalized = column
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.contains("evidence") || normalized.contains("link")
}

fn is_missing_evidence(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }

    let normalized = value.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "-" | "--" | "n/a" | "na" | "none" | "null" | "todo" | "tbd" | "placeholder"
    )
}

fn status_requires_evidence(status: &str) -> bool {
    !matches!(status, "MISSING" | "DEFERRED" | "STUBBED")
}

pub fn validate_governance_contracts(
    spec_yaml: &str,
    plan_yaml: &str,
    checklist_yaml: &str,
    traceability_markdown: &str,
) -> Result<(), GovernanceValidationError> {
    validate_governance_state(".rct/spec.yaml", spec_yaml)?;
    validate_governance_state(".rct/plan.yaml", plan_yaml)?;
    validate_governance_state(".rct/checklist.yaml", checklist_yaml)?;
    validate_traceability_statuses(traceability_markdown)?;
    Ok(())
}

/// Deprecated alias — use [`validate_governance_contracts`] instead.
#[deprecated(since = "0.4.11", note = "renamed to validate_governance_contracts")]
pub fn validate_phase0_governance_contracts(
    spec_yaml: &str,
    plan_yaml: &str,
    checklist_yaml: &str,
    traceability_markdown: &str,
) -> Result<(), GovernanceValidationError> {
    validate_governance_contracts(spec_yaml, plan_yaml, checklist_yaml, traceability_markdown)
}
