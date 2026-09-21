//! The shared decision service.
//!
//! One evaluation is: validate the request against the configured limits,
//! reserve the caller's aggregate token allowance through the owner-issued
//! handle, run the selected backend under one total deadline, validate every
//! answer against the fixed interpretation contract, settle the reservation
//! exactly once from what was measured, and return typed judgments with route
//! provenance. Thresholds and dispositions are not decided here.

use std::sync::Arc;

use meerkat_core::time_compat::Duration;
use meerkat_core::{
    DecisionLimitsConfig, NestedUsageAccounting, NestedUsageMeasurement, NestedUsageReservation,
};

use crate::backend::{BackendResponse, BackendUsage, Deadline, DecisionBackend};
use crate::contracts::{
    BudgetParticipation, DECISION_CONTRACT_VERSION, DecisionAccounting, DecisionRequest,
    DecisionResult,
};
use crate::error::DecisionError;
use crate::validate::{ValidatedRequest, validate_answers};

#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// How this invocation participates in the caller's aggregate token budget.
///
/// Issued by the owner of the budget (the agent loop through its dispatch
/// context, or a host that constructed the service). Request payloads cannot
/// carry it.
#[derive(Debug, Clone)]
pub enum BudgetAdmission {
    /// The owning agent issued nested-usage accounting; reserve before egress
    /// and settle exactly once afterwards.
    Nested(NestedUsageAccounting),
    /// The invoking context issued no accounting. Nothing is charged and the
    /// result says so; nothing is fabricated.
    NotIssued,
}

/// Owner-issued invocation authority for one evaluation.
///
/// This is deliberately not deserializable: an agent cannot grant itself
/// budget participation or any other admission fact by supplying JSON.
#[derive(Debug, Clone)]
pub struct DecisionAdmission {
    budget: BudgetAdmission,
}

impl DecisionAdmission {
    pub fn new(budget: BudgetAdmission) -> Self {
        Self { budget }
    }

    /// Admission for a host invocation outside any agent budget.
    pub fn host_unbudgeted() -> Self {
        Self {
            budget: BudgetAdmission::NotIssued,
        }
    }

    pub fn budget(&self) -> &BudgetAdmission {
        &self.budget
    }
}

/// Provider-neutral batched decision evaluation over one configured backend.
pub struct DecisionService {
    backend: Arc<dyn DecisionBackend>,
    limits: DecisionLimitsConfig,
}

impl std::fmt::Debug for DecisionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionService")
            .field("backend", &self.backend.kind())
            .field("limits", &self.limits)
            .finish()
    }
}

impl DecisionService {
    pub fn new(backend: Arc<dyn DecisionBackend>, limits: DecisionLimitsConfig) -> Self {
        Self { backend, limits }
    }

    pub fn limits(&self) -> &DecisionLimitsConfig {
        &self.limits
    }

    pub fn backend(&self) -> &Arc<dyn DecisionBackend> {
        &self.backend
    }

    /// Evaluate one batched request.
    pub async fn evaluate(
        &self,
        admission: &DecisionAdmission,
        request: DecisionRequest,
    ) -> Result<DecisionResult, DecisionError> {
        let validated = ValidatedRequest::validate(request, &self.limits)?;

        // Reserve before egress so the aggregate axis can refuse the call
        // instead of discovering an overspend after the tokens are gone.
        let reservation = match admission.budget() {
            BudgetAdmission::Nested(accounting) => {
                let estimate = validated
                    .estimated_input_tokens()
                    .saturating_add(u64::from(self.limits.max_output_tokens));
                Some((accounting, accounting.reserve(estimate)?))
            }
            BudgetAdmission::NotIssued => None,
        };

        let deadline = Deadline::after(Duration::from_millis(self.limits.deadline_ms));
        let evaluation = tokio::time::timeout(
            deadline.remaining(),
            self.backend
                .evaluate(&validated, deadline, self.limits.max_attempts),
        )
        .await;
        let response = match evaluation {
            Ok(Ok(response)) => response,
            // No response means no tokens were provably spent; the reservation
            // drops and releases its estimate.
            Ok(Err(failure)) => return Err(failure.into()),
            Err(_elapsed) => {
                return Err(DecisionError::DeadlineExceeded {
                    deadline_ms: self.limits.deadline_ms,
                });
            }
        };

        // The backend answered, so its tokens were spent whether or not the
        // answers pass interpretation. Settle first; then judge.
        let (accounting, budget) = settle(reservation, &response);
        let judgments = validate_answers(&validated, self.backend.kind(), response.answers)?;

        Ok(DecisionResult {
            contract: DECISION_CONTRACT_VERSION,
            route: response.route,
            judgments,
            accounting,
            budget,
            attempts: response.attempts,
        })
    }
}

fn settle(
    reservation: Option<(&NestedUsageAccounting, NestedUsageReservation)>,
    response: &BackendResponse,
) -> (DecisionAccounting, BudgetParticipation) {
    let (accounting, measurement) = match &response.usage {
        BackendUsage::Provider(usage) => {
            let accounting = DecisionAccounting::Measured {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            };
            // Normalization has one owner: the shared TurnUsage contract. A
            // usage without provider accounting is unmeasured for budget
            // purposes; the raw counters are still reported as-is above.
            let measurement = match meerkat_core::types::TurnUsage::try_from_usage(usage.clone()) {
                Ok(turn_usage) => NestedUsageMeasurement::ProviderTurn(turn_usage),
                Err(_) => NestedUsageMeasurement::Unmeasured,
            };
            (accounting, measurement)
        }
        BackendUsage::Reported {
            input_tokens,
            output_tokens,
        } => (
            DecisionAccounting::Measured {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
            },
            NestedUsageMeasurement::BackendReported {
                total_tokens: input_tokens.saturating_add(*output_tokens),
            },
        ),
        BackendUsage::Unmeasured => (
            DecisionAccounting::Unmeasured,
            NestedUsageMeasurement::Unmeasured,
        ),
    };
    let budget = match reservation {
        Some((handle, reservation)) => handle.settle(reservation, measurement).into(),
        None => BudgetParticipation::NotIssued,
    };
    (accounting, budget)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use meerkat_core::{Budget, BudgetLimits};

    use super::*;
    use crate::backend::RawAnswer;
    use crate::contracts::{BackendKind, BinaryAnswer, BinaryJudgment, Judgment, RouteProvenance};
    use crate::error::{AnswerValidationError, BackendFailure};
    use crate::validate::tests::sample_request;

    struct ScriptedBackend {
        responses: Mutex<Vec<Result<BackendResponse, BackendFailure>>>,
        delay: Option<Duration>,
    }

    impl ScriptedBackend {
        fn once(response: Result<BackendResponse, BackendFailure>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(vec![response]),
                delay: None,
            })
        }
    }

    #[async_trait]
    impl DecisionBackend for ScriptedBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::SessionLlm
        }

        async fn evaluate(
            &self,
            _request: &ValidatedRequest,
            _deadline: Deadline,
            _max_attempts: u32,
        ) -> Result<BackendResponse, BackendFailure> {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn valid_response(usage: BackendUsage) -> BackendResponse {
        BackendResponse {
            answers: vec![
                (
                    "is_urgent".into(),
                    RawAnswer::BinaryCategorical(BinaryAnswer::Yes),
                ),
                (
                    "department".into(),
                    RawAnswer::ChoiceSelected {
                        option: "billing".into(),
                        distribution: None,
                    },
                ),
                ("frustration".into(), RawAnswer::GradeLevel { index: 1 }),
            ],
            route: RouteProvenance::SessionLlm {
                provider: meerkat_core::Provider::Other,
                model: "fake".into(),
            },
            usage,
            attempts: 1,
        }
    }

    fn provider_usage_with_accounting() -> BackendUsage {
        let turn = meerkat_core::types::TurnUsage::host_declared(
            meerkat_core::Provider::Other,
            "fake",
            meerkat_core::types::Usage {
                input_tokens: 120,
                output_tokens: 30,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                provider_accounting: None,
            },
        );
        BackendUsage::Provider(turn.into_inner())
    }

    #[tokio::test]
    async fn evaluate_charges_the_owner_budget_exactly_once_from_measured_usage() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(provider_usage_with_accounting()))),
            DecisionLimitsConfig::default(),
        );
        let admission =
            DecisionAdmission::new(BudgetAdmission::Nested(budget.nested_usage_accounting()));

        let result = service
            .evaluate(&admission, sample_request())
            .await
            .unwrap();

        assert_eq!(result.budget, BudgetParticipation::Charged { tokens: 150 });
        assert_eq!(
            result.accounting,
            DecisionAccounting::Measured {
                input_tokens: 120,
                output_tokens: 30
            }
        );
        assert_eq!(budget.token_usage(), Some((150, 10_000)));
        assert!(matches!(
            result.judgments["is_urgent"].judgment,
            Judgment::Binary(BinaryJudgment::Categorical {
                answer: BinaryAnswer::Yes
            })
        ));
        assert_eq!(result.route.backend(), BackendKind::SessionLlm);
    }

    #[tokio::test]
    async fn evaluate_refuses_before_egress_when_budget_cannot_admit_the_estimate() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(100));
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(provider_usage_with_accounting()))),
            DecisionLimitsConfig::default(),
        );
        let admission =
            DecisionAdmission::new(BudgetAdmission::Nested(budget.nested_usage_accounting()));

        let error = service
            .evaluate(&admission, sample_request())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DecisionError::BudgetRefused { limit: 100, .. }
        ));
        assert_eq!(budget.token_usage(), Some((0, 100)));
    }

    #[tokio::test]
    async fn unmeasured_usage_releases_the_reservation_and_marks_the_result() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let raw_without_accounting = BackendUsage::Provider(meerkat_core::types::Usage {
            input_tokens: 5,
            output_tokens: 5,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            provider_accounting: None,
        });
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(raw_without_accounting))),
            DecisionLimitsConfig::default(),
        );
        let admission =
            DecisionAdmission::new(BudgetAdmission::Nested(budget.nested_usage_accounting()));

        let result = service
            .evaluate(&admission, sample_request())
            .await
            .unwrap();
        assert_eq!(result.budget, BudgetParticipation::Unmeasured);
        assert_eq!(
            result.accounting,
            DecisionAccounting::Measured {
                input_tokens: 5,
                output_tokens: 5
            }
        );
        assert_eq!(budget.token_usage(), Some((0, 10_000)));
    }

    #[tokio::test]
    async fn backend_reported_usage_charges_totals_and_not_issued_admission_charges_nothing() {
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(BackendUsage::Reported {
                input_tokens: 300,
                output_tokens: 20,
            }))),
            DecisionLimitsConfig::default(),
        );
        let result = service
            .evaluate(&DecisionAdmission::host_unbudgeted(), sample_request())
            .await
            .unwrap();
        assert_eq!(result.budget, BudgetParticipation::NotIssued);
        assert_eq!(
            result.accounting,
            DecisionAccounting::Measured {
                input_tokens: 300,
                output_tokens: 20
            }
        );
    }

    #[tokio::test]
    async fn invalid_answers_fail_closed_but_still_settle_spent_tokens() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let mut response = valid_response(provider_usage_with_accounting());
        response.answers[1].1 = RawAnswer::ChoiceSelected {
            option: "sales".into(),
            distribution: None,
        };
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(response)),
            DecisionLimitsConfig::default(),
        );
        let admission =
            DecisionAdmission::new(BudgetAdmission::Nested(budget.nested_usage_accounting()));

        let error = service
            .evaluate(&admission, sample_request())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DecisionError::InvalidAnswer(AnswerValidationError::OptionNotSupplied { .. })
        ));
        assert_eq!(budget.token_usage(), Some((150, 10_000)));
    }

    #[tokio::test]
    async fn backend_failure_releases_the_reservation() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let service = DecisionService::new(
            ScriptedBackend::once(Err(BackendFailure::Unauthorized)),
            DecisionLimitsConfig::default(),
        );
        let admission =
            DecisionAdmission::new(BudgetAdmission::Nested(budget.nested_usage_accounting()));

        let error = service
            .evaluate(&admission, sample_request())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            DecisionError::BackendFailure(BackendFailure::Unauthorized)
        );
        assert_eq!(budget.token_usage(), Some((0, 10_000)));
    }

    #[tokio::test]
    async fn deadline_bounds_the_whole_evaluation() {
        let backend = Arc::new(ScriptedBackend {
            responses: Mutex::new(vec![Ok(valid_response(BackendUsage::Unmeasured))]),
            delay: Some(Duration::from_millis(200)),
        });
        let limits = DecisionLimitsConfig {
            deadline_ms: 20,
            ..DecisionLimitsConfig::default()
        };
        let service = DecisionService::new(backend, limits);
        let error = service
            .evaluate(&DecisionAdmission::host_unbudgeted(), sample_request())
            .await
            .unwrap_err();
        assert_eq!(error, DecisionError::DeadlineExceeded { deadline_ms: 20 });
    }

    #[tokio::test]
    async fn invalid_requests_never_reach_the_backend() {
        let service = DecisionService::new(
            Arc::new(ScriptedBackend {
                responses: Mutex::new(Vec::new()),
                delay: None,
            }),
            DecisionLimitsConfig::default(),
        );
        let mut request = sample_request();
        request.questions.clear();
        let error = service
            .evaluate(&DecisionAdmission::host_unbudgeted(), request)
            .await
            .unwrap_err();
        assert!(matches!(error, DecisionError::InvalidRequest(_)));
    }
}
