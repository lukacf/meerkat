//! Body-free observations retained by the actual private source owner.
//! Native policy callbacks perform no I/O, formatting, waiting or callbacks.
//! The existing gateway close join drains a bounded snapshot outside them.

use meerkat_core::{OperationAuthorizationError, OperationRefusalKind};
use std::{
    io::Write,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    NativeInvocationMandate,
    PreparedMandateOwner,
    PreparedAccountOwner,
    Mandate,
    MandateCollectorSource,
    MandateCurrentInvocation,
    MandateHelperSource,
    MandateSourceRoute,
    MandateSelectedSource,
    MandateGrantBounds,
    Account,
    AccountCollectorSource,
    AccountCollectorOperation,
    AccountMember,
    AccountCurrentInvocation,
    AccountWorkflowCall,
    AccountMapping,
    AccountDeadline,
    AccountWorkflowAllowance,
    AccountDispatcher,
    AccountTarget,
    AccountProjection,
    GrantTarget,
    GrantLineage,
    GrantHelperDeadline,
    GrantRender,
}

impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::NativeInvocationMandate => "native.invocation_mandate",
            Self::PreparedMandateOwner => "prepared.mandate_owner",
            Self::PreparedAccountOwner => "prepared.account_owner",
            Self::Mandate => "mandate.result",
            Self::MandateCollectorSource => "mandate.collector_source",
            Self::MandateCurrentInvocation => "mandate.current_invocation",
            Self::MandateHelperSource => "mandate.helper_source",
            Self::MandateSourceRoute => "mandate.source_route",
            Self::MandateSelectedSource => "mandate.selected_source",
            Self::MandateGrantBounds => "mandate.grant_bounds",
            Self::Account => "account.result",
            Self::AccountCollectorSource => "account.collector_source",
            Self::AccountCollectorOperation => "account.collector_operation",
            Self::AccountMember => "account.member",
            Self::AccountCurrentInvocation => "account.current_invocation",
            Self::AccountWorkflowCall => "account.workflow_call",
            Self::AccountMapping => "account.mapping",
            Self::AccountDeadline => "account.deadline",
            Self::AccountWorkflowAllowance => "account.workflow_allowance",
            Self::AccountDispatcher => "account.dispatcher",
            Self::AccountTarget => "account.target",
            Self::AccountProjection => "account.projection",
            Self::GrantTarget => "grant.target",
            Self::GrantLineage => "grant.lineage",
            Self::GrantHelperDeadline => "grant.helper_deadline",
            Self::GrantRender => "grant.render",
        }
    }
}

const CAPACITY: usize = 128;

#[derive(Clone, Copy)]
pub(super) enum Field {
    Mandate,
    Account,
}

#[derive(Clone, Copy)]
enum Observation {
    Failure(Stage, OperationAuthorizationError),
    Material(Field, Option<usize>),
    NativeMandate {
        bytes: Option<usize>,
        blank: Option<bool>,
    },
    OperationFailure(&'static str, OperationAuthorizationError),
}
struct Rows {
    values: [Option<Observation>; CAPACITY],
    len: usize,
}
impl Default for Rows {
    fn default() -> Self {
        Self {
            values: [None; CAPACITY],
            len: 0,
        }
    }
}

/// One bounded diagnostic buffer, with no authority, native handles or text.
/// Contention/full/poisoned observations are dropped without changing policy.
#[derive(Default)]
pub(crate) struct ReviewObservations {
    rows: Mutex<Rows>,
    lost: AtomicBool,
}
impl ReviewObservations {
    fn record(&self, observation: Observation) {
        let Ok(mut rows) = self.rows.try_lock() else {
            self.lost.store(true, Ordering::Relaxed);
            return;
        };
        if rows.len == CAPACITY {
            self.lost.store(true, Ordering::Relaxed);
            return;
        }
        let index = rows.len;
        rows.values[index] = Some(observation);
        rows.len += 1;
    }

    /// Transfer one snapshot before any writer is touched. A later callback
    /// stays in the same retained buffer for another actual lifecycle drain.
    pub(crate) fn drain(&self) -> ReviewObservationBatch {
        match self.rows.try_lock() {
            Ok(mut rows) => ReviewObservationBatch {
                rows: std::mem::take(&mut *rows),
                lost: self.lost.swap(false, Ordering::Relaxed),
            },
            Err(_) => {
                self.lost.store(true, Ordering::Relaxed);
                ReviewObservationBatch {
                    rows: Rows::default(),
                    lost: true,
                }
            }
        }
    }

    pub(super) fn operation_failure(
        &self,
        stage: &'static str,
        error: &OperationAuthorizationError,
    ) {
        if !matches!(
            stage,
            "work.collector"
                | "work.member"
                | "work.current_invocation"
                | "work.deadline"
                | "operation.member"
                | "operation.current_invocation"
                | "operation.workflow_call"
                | "operation.deadline"
                | "operation.workflow_allowance"
        ) {
            return;
        }
        if matches!(error, OperationAuthorizationError::Unavailable)
            || matches!(error, OperationAuthorizationError::Refused(refusal) if refusal.kind() == OperationRefusalKind::Denied)
        {
            self.record(Observation::OperationFailure(stage, *error));
        }
    }
}

/// No Drop logger. Losing a snapshot or failing to write it changes no result
/// and supplies no claim that all callbacks or native work have settled.
pub(crate) struct ReviewObservationBatch {
    rows: Rows,
    lost: bool,
}
impl ReviewObservationBatch {
    pub(crate) fn write_to(self, output: &mut dyn Write) {
        for observation in self.rows.values[..self.rows.len].iter().flatten() {
            match *observation {
                Observation::Failure(stage, error) => write_failure(output, stage, &error),
                Observation::Material(field, bytes) => {
                    let field = match field {
                        Field::Mandate => "mandate",
                        Field::Account => "account",
                    };
                    let bytes = bytes
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".into());
                    let _ = writeln!(
                        output,
                        "{{\"event\":\"toolkit_installed_review_owner_material\",\"field\":\"{field}\",\"bytes\":{bytes}}}"
                    );
                }
                Observation::NativeMandate { bytes, blank } => {
                    let bytes = bytes
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".into());
                    let blank = blank
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".into());
                    let stage = Stage::NativeInvocationMandate.label();
                    let _ = writeln!(
                        output,
                        "{{\"event\":\"toolkit_installed_review_native_material\",\"stage\":\"{stage}\",\"bytes\":{bytes},\"blank\":{blank}}}"
                    );
                }
                Observation::OperationFailure(stage, error) => {
                    super::write_installed_operation_failure(output, stage, &error)
                }
            }
        }
        if self.lost {
            let _ = writeln!(
                output,
                "{{\"event\":\"toolkit_installed_review_observations_lost\",\"stage\":\"gateway.close_join\"}}"
            );
        }
    }
}

pub(crate) fn observe<T>(
    observations: &ReviewObservations,
    stage: Stage,
    result: Result<T, OperationAuthorizationError>,
) -> Result<T, OperationAuthorizationError> {
    if let Err(error) = &result {
        failure(observations, stage, error);
    }
    result
}
pub(super) fn passive_step<T>(
    observations: &ReviewObservations,
    passive: bool,
    stage: Stage,
    result: Result<T, OperationAuthorizationError>,
) -> Result<T, OperationAuthorizationError> {
    if passive {
        observe(observations, stage, result)
    } else {
        result
    }
}
pub(super) fn failure(
    observations: &ReviewObservations,
    stage: Stage,
    error: &OperationAuthorizationError,
) {
    observations.record(Observation::Failure(stage, *error));
}
pub(super) fn material(
    observations: &ReviewObservations,
    field: Field,
    result: Result<Option<String>, OperationAuthorizationError>,
) -> Result<Option<String>, OperationAuthorizationError> {
    match &result {
        Ok(value) => observations.record(Observation::Material(
            field,
            value.as_ref().map(String::len),
        )),
        Err(error) => failure(
            observations,
            match field {
                Field::Mandate => Stage::Mandate,
                Field::Account => Stage::Account,
            },
            error,
        ),
    }
    result
}
pub(crate) fn native_mandate(
    observations: &ReviewObservations,
    result: Result<Option<String>, OperationAuthorizationError>,
) -> Result<Option<String>, OperationAuthorizationError> {
    match &result {
        Ok(value) => observations.record(Observation::NativeMandate {
            bytes: value.as_ref().map(String::len),
            blank: value.as_ref().map(|value| value.trim().is_empty()),
        }),
        Err(error) => failure(observations, Stage::NativeInvocationMandate, error),
    }
    result
}

fn write_failure(output: &mut dyn Write, stage: Stage, error: &OperationAuthorizationError) {
    let cause = match error {
        OperationAuthorizationError::Unavailable => "unavailable",
        OperationAuthorizationError::ObservationUnavailable(_) => "observation_unavailable",
        OperationAuthorizationError::Refused(refusal) => match refusal.kind() {
            OperationRefusalKind::Denied => "denied",
            OperationRefusalKind::MalformedFacts => "malformed_facts",
            OperationRefusalKind::ReprepareRequired => "reprepare_required",
        },
    };
    let stage = stage.label();
    let _ = writeln!(
        output,
        "{{\"event\":\"toolkit_installed_review_failure\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}"
    );
}

// Render controls explicitly drain after the callback result already exists.
#[cfg(test)]
fn material_to(
    output: &mut dyn Write,
    field: Field,
    result: Result<Option<String>, OperationAuthorizationError>,
) -> Result<Option<String>, OperationAuthorizationError> {
    let observations = ReviewObservations::default();
    let result = material(&observations, field, result);
    observations.drain().write_to(output);
    result
}
#[cfg(test)]
fn native_mandate_to(
    output: &mut dyn Write,
    result: Result<Option<String>, OperationAuthorizationError>,
) -> Result<Option<String>, OperationAuthorizationError> {
    let observations = ReviewObservations::default();
    let result = native_mandate(&observations, result);
    observations.drain().write_to(output);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::OperationRefused;

    fn rows(output: &[u8]) -> Vec<serde_json::Value> {
        std::str::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn review_buffer_bounds_overflow_contention_and_repeated_drain() {
        let observations = ReviewObservations::default();
        for _ in 0..CAPACITY + 1 {
            assert_eq!(
                native_mandate(&observations, Ok(Some("private".into()))).unwrap(),
                Some("private".into())
            );
        }
        let mut output = Vec::new();
        observations.drain().write_to(&mut output);
        let emitted = rows(&output);
        assert_eq!(emitted.len(), CAPACITY + 1);
        assert_eq!(
            emitted.last().unwrap(),
            &serde_json::json!({"event": "toolkit_installed_review_observations_lost", "stage": "gateway.close_join"})
        );
        output.clear();
        observations.drain().write_to(&mut output);
        assert!(output.is_empty(), "a snapshot is drained once");
        let held = observations.rows.lock().unwrap();
        // A blocking lock here would deadlock this same-thread callback.
        assert_eq!(
            observe(
                &observations,
                Stage::GrantLineage,
                Err::<(), _>(OperationAuthorizationError::Unavailable)
            ),
            Err(OperationAuthorizationError::Unavailable)
        );
        drop(held);
        observations.drain().write_to(&mut output);
        assert_eq!(
            rows(&output),
            vec![
                serde_json::json!({"event": "toolkit_installed_review_observations_lost", "stage": "gateway.close_join"})
            ]
        );
        let independent = ReviewObservations::default();
        output.clear();
        independent.drain().write_to(&mut output);
        assert!(output.is_empty(), "observations are not process-global");
    }

    #[test]
    fn review_drain_releases_buffer_before_output_and_retains_later_observations() {
        let observations = ReviewObservations::default();
        assert_eq!(native_mandate(&observations, Ok(None)), Ok(None));
        struct DuringWrite<'a> {
            observations: &'a ReviewObservations,
            wrote: bool,
            bytes: Vec<u8>,
        }
        impl Write for DuringWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.wrote {
                    self.wrote = true;
                    assert_eq!(
                        material(self.observations, Field::Account, Ok(None)),
                        Ok(None)
                    );
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let snapshot = observations.drain();
        let mut writer = DuringWrite {
            observations: &observations,
            wrote: false,
            bytes: Vec::new(),
        };
        snapshot.write_to(&mut writer);
        assert_eq!(rows(&writer.bytes).len(), 1);
        let mut later = Vec::new();
        observations.drain().write_to(&mut later);
        assert_eq!(
            rows(&later),
            vec![
                serde_json::json!({"event": "toolkit_installed_review_owner_material", "field": "account", "bytes": null})
            ]
        );
    }

    #[test]
    fn review_buffer_preserves_closed_operation_labels_and_passive_selection() {
        let observations = ReviewObservations::default();
        observations.operation_failure(
            "request-secret/authority-secret",
            &OperationAuthorizationError::Unavailable,
        );
        observations.operation_failure(
            "operation.member",
            &OperationRefused::new(OperationRefusalKind::MalformedFacts).into(),
        );
        assert_eq!(
            passive_step(
                &observations,
                false,
                Stage::AccountMember,
                Err::<(), _>(OperationAuthorizationError::Unavailable)
            ),
            Err(OperationAuthorizationError::Unavailable)
        );
        let mut output = Vec::new();
        observations.drain().write_to(&mut output);
        assert!(output.is_empty());
        observations.operation_failure(
            "operation.member",
            &OperationAuthorizationError::Unavailable,
        );
        assert_eq!(
            passive_step(
                &observations,
                true,
                Stage::AccountMember,
                Err::<(), _>(OperationAuthorizationError::Unavailable)
            ),
            Err(OperationAuthorizationError::Unavailable)
        );
        observations.drain().write_to(&mut output);
        assert_eq!(
            rows(&output),
            vec![
                serde_json::json!({"event": "toolkit_installed_operation_unavailable", "stage": "operation.member", "cause": "Unavailable"}),
                serde_json::json!({"event": "toolkit_installed_review_failure", "stage": "account.member", "cause": "unavailable"}),
            ]
        );
    }

    #[test]
    fn native_mandate_diagnostic_preserves_absent_blank_and_oversize_material() {
        for value in [
            None,
            Some(String::new()),
            Some(" \t\n".into()),
            Some("private-mandate-\u{00e5}".into()),
            Some("x".repeat(4097)),
        ] {
            let pointer = value.as_ref().map(|value| value.as_ptr());
            let mut output = Vec::new();
            let actual = native_mandate_to(&mut output, Ok(value.clone())).unwrap();
            assert_eq!(actual, value);
            let row: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(
                row,
                serde_json::json!({"event": "toolkit_installed_review_native_material",
                "stage": "native.invocation_mandate", "bytes": value.as_ref().map(String::len),
                "blank": value.as_ref().map(|value| value.trim().is_empty())})
            );
            assert!(
                !String::from_utf8(output)
                    .unwrap()
                    .contains("private-mandate")
            );
            // The observer returns the same allocation, not reconstructed text.
            let actual = native_mandate_to(&mut Vec::new(), Ok(value)).unwrap();
            assert_eq!(actual.as_ref().map(|value| value.as_ptr()), pointer);
        }
    }

    #[test]
    fn native_mandate_diagnostic_writer_failure_preserves_every_result() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        for value in [
            None,
            Some("private-mandate".to_owned()),
            Some("x".repeat(4097)),
        ] {
            let pointer = value.as_ref().map(|value| value.as_ptr());
            let actual = native_mandate_to(&mut Broken, Ok(value)).unwrap();
            assert_eq!(actual.as_ref().map(|value| value.as_ptr()), pointer);
        }
        for (error, cause) in [
            (OperationAuthorizationError::Unavailable, "unavailable"),
            (
                OperationAuthorizationError::ObservationUnavailable(
                    meerkat_core::authorization::OperationObservationError,
                ),
                "observation_unavailable",
            ),
            (
                OperationRefused::new(OperationRefusalKind::Denied).into(),
                "denied",
            ),
            (
                OperationRefused::new(OperationRefusalKind::MalformedFacts).into(),
                "malformed_facts",
            ),
            (
                OperationRefused::new(OperationRefusalKind::ReprepareRequired).into(),
                "reprepare_required",
            ),
        ] {
            assert_eq!(native_mandate_to(&mut Broken, Err(error)), Err(error));
            let mut output = Vec::new();
            assert_eq!(native_mandate_to(&mut output, Err(error)), Err(error));
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
                serde_json::json!({"event": "toolkit_installed_review_failure",
                    "stage": "native.invocation_mandate", "cause": cause})
            );
        }
    }

    #[test]
    fn owner_material_diagnostic_preserves_value_and_only_reports_utf8_size() {
        for field in [Field::Mandate, Field::Account] {
            for value in [
                None,
                Some(String::new()),
                Some("private-account-\u{00e5}".into()),
            ] {
                let expected_bytes = value.as_ref().map(String::len);
                let mut output = Vec::new();
                assert_eq!(
                    material_to(&mut output, field, Ok(value.clone())).unwrap(),
                    value
                );
                let row: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert_eq!(row["event"], "toolkit_installed_review_owner_material");
                assert_eq!(
                    row["field"],
                    match field {
                        Field::Mandate => "mandate",
                        Field::Account => "account",
                    }
                );
                assert_eq!(row["bytes"], serde_json::json!(expected_bytes));
                assert_eq!(row.as_object().unwrap().len(), 3);
                assert!(
                    !String::from_utf8(output)
                        .unwrap()
                        .contains("private-account")
                );
            }
        }
    }

    #[test]
    fn diagnostic_writer_failure_preserves_success_and_each_refusal() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let value = Some("private-mandate".to_owned());
        assert_eq!(
            material_to(&mut Broken, Field::Mandate, Ok(value.clone())).unwrap(),
            value
        );
        for error in [
            OperationAuthorizationError::Unavailable,
            OperationAuthorizationError::ObservationUnavailable(
                meerkat_core::authorization::OperationObservationError,
            ),
            OperationRefused::new(OperationRefusalKind::Denied).into(),
            OperationRefused::new(OperationRefusalKind::MalformedFacts).into(),
            OperationRefused::new(OperationRefusalKind::ReprepareRequired).into(),
        ] {
            assert_eq!(
                material_to(&mut Broken, Field::Account, Err(error)),
                Err(error)
            );
        }
    }

    #[test]
    fn review_failure_diagnostic_keeps_fixed_stage_and_cause_only() {
        let mut output = Vec::new();
        write_failure(
            &mut output,
            Stage::GrantLineage,
            &OperationRefused::new(OperationRefusalKind::ReprepareRequired).into(),
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
            serde_json::json!({"event": "toolkit_installed_review_failure", "stage": "grant.lineage", "cause": "reprepare_required"})
        );
    }
}
