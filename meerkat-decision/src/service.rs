//! The shared decision service.
//!
//! One evaluation is: validate the request against the configured limits,
//! reserve the caller's aggregate token allowance through the owner-issued
//! handle, run the selected backend under one total deadline, settle the
//! reservation exactly once from what the backend measured (on success and
//! on failure alike), validate every answer against the fixed interpretation
//! contract, and return typed judgments with route provenance. Thresholds
//! and dispositions are not decided here.

use std::sync::Arc;

use meerkat_core::AgentLlmClient;
use meerkat_core::time_compat::Duration;
use meerkat_core::{
    DecisionLimitsConfig, NestedUsageAccounting, NestedUsageMeasurement, NestedUsageReservation,
};

use crate::backend::{AttemptUsage, BackendUsage, Deadline, DecisionBackend, FailedEvaluation};
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

/// Which admitted LLM route this invocation may use.
///
/// The agent loop admits the event-isolated fork of its current client, so a
/// backend bound to the session follows hot-swaps and fallbacks. Host
/// invocations admit no session route; backends needing one report typed
/// unavailability rather than electing a route.
#[derive(Clone)]
pub enum RouteAdmission {
    /// No session route was admitted for this invocation.
    None,
    /// The caller's current admitted route, event-isolated.
    Session(Arc<dyn AgentLlmClient>),
    /// The caller has a route but it could not be isolated for a nested
    /// call; a backend bound to the session reports this typed, a backend
    /// that needs no route ignores it.
    Unavailable { message: String },
}

impl std::fmt::Debug for RouteAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("RouteAdmission::None"),
            Self::Unavailable { message } => f
                .debug_struct("RouteAdmission::Unavailable")
                .field("message", message)
                .finish(),
            Self::Session(client) => f
                .debug_struct("RouteAdmission::Session")
                .field("provider", &client.provider())
                .field("model", &client.model())
                .finish(),
        }
    }
}

/// Owner-issued invocation authority for one evaluation.
///
/// This is deliberately not deserializable: an agent cannot grant itself
/// budget participation, a route, or any other admission fact by supplying
/// JSON.
#[derive(Debug, Clone)]
pub struct DecisionAdmission {
    budget: BudgetAdmission,
    route: RouteAdmission,
}

impl DecisionAdmission {
    pub fn new(budget: BudgetAdmission) -> Self {
        Self {
            budget,
            route: RouteAdmission::None,
        }
    }

    /// Admission for a host invocation outside any agent budget or session.
    pub fn host_unbudgeted() -> Self {
        Self::new(BudgetAdmission::NotIssued)
    }

    /// Admit the caller's current event-isolated session route.
    #[must_use]
    pub fn with_session_route(mut self, route: Arc<dyn AgentLlmClient>) -> Self {
        self.route = RouteAdmission::Session(route);
        self
    }

    /// Record that the caller's route exists but could not be isolated.
    #[must_use]
    pub fn with_unavailable_route(mut self, message: impl Into<String>) -> Self {
        self.route = RouteAdmission::Unavailable {
            message: message.into(),
        };
        self
    }

    pub fn budget(&self) -> &BudgetAdmission {
        &self.budget
    }

    pub fn route(&self) -> &RouteAdmission {
        &self.route
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

/// Reservation paired with the handle that minted it.
struct HeldReservation<'a> {
    handle: &'a NestedUsageAccounting,
    reservation: NestedUsageReservation,
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
        let held = match admission.budget() {
            BudgetAdmission::Nested(handle) => {
                let estimate = validated
                    .estimated_input_tokens()
                    .saturating_add(u64::from(self.limits.max_output_tokens));
                Some(HeldReservation {
                    handle,
                    reservation: handle.reserve(estimate)?,
                })
            }
            BudgetAdmission::NotIssued => None,
        };

        let deadline = Deadline::after(Duration::from_millis(self.limits.deadline_ms));
        let evaluation = tokio::time::timeout(
            deadline.remaining(),
            self.backend
                .evaluate(admission, &validated, deadline, self.limits.max_attempts),
        )
        .await;
        let response = match evaluation {
            Ok(Ok(response)) => response,
            Ok(Err(FailedEvaluation {
                failure,
                usage,
                attempts,
            })) => {
                // The backend may have spent tokens before failing; settle
                // from what it measured, never from an assumed zero.
                let (accounting, budget) = settle(held, &usage);
                return Err(DecisionError::BackendFailure {
                    failure,
                    accounting,
                    budget,
                    attempts,
                });
            }
            Err(_elapsed) => {
                // The in-flight call was dropped, so nothing was measured.
                // Absence is reported as unmeasured, not invented as zero or
                // as the estimate.
                let (_, budget) = settle(held, &BackendUsage::Unmeasured);
                return Err(DecisionError::DeadlineExceeded {
                    deadline_ms: self.limits.deadline_ms,
                    budget,
                });
            }
        };

        // The backend answered, so its tokens were spent whether or not the
        // answers pass interpretation. Settle first; then judge.
        let (accounting, budget) = settle(held, &response.usage);
        let judgments = match validate_answers(&validated, self.backend.kind(), response.answers) {
            Ok(judgments) => judgments,
            Err(error) => {
                return Err(DecisionError::InvalidAnswer {
                    error,
                    accounting,
                    budget,
                });
            }
        };

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
    held: Option<HeldReservation<'_>>,
    usage: &BackendUsage,
) -> (DecisionAccounting, BudgetParticipation) {
    let (accounting, measurement) = match usage {
        BackendUsage::Provider(attempts) if attempts.is_empty() => (
            DecisionAccounting::Unmeasured,
            NestedUsageMeasurement::Unmeasured,
        ),
        BackendUsage::Provider(attempts) => {
            // Normalization has one owner: the shared TurnUsage contract.
            // Every attempt must have completed with provider accounting
            // evidence for the evaluation to count as measured; an attempt
            // that spent tokens without reporting them, or a counter without
            // evidence, is not promoted to a number, because that number
            // could be a fabricated zero.
            let turns = attempts
                .iter()
                .map(|attempt| match attempt {
                    AttemptUsage::Measured(usage) => {
                        meerkat_core::types::TurnUsage::try_from_usage(usage.clone())
                            .map_err(|_| ())
                    }
                    AttemptUsage::Unmeasured => Err(()),
                })
                .collect::<Result<Vec<_>, ()>>();
            match turns {
                Ok(turns) => {
                    let (input_tokens, output_tokens) =
                        turns.iter().fold((0u64, 0u64), |acc, turn| {
                            let usage = turn.as_usage();
                            (
                                acc.0.saturating_add(usage.input_tokens),
                                acc.1.saturating_add(usage.output_tokens),
                            )
                        });
                    (
                        DecisionAccounting::Measured {
                            input_tokens,
                            output_tokens,
                        },
                        NestedUsageMeasurement::ProviderTurns(turns),
                    )
                }
                Err(()) => (
                    DecisionAccounting::Unmeasured,
                    NestedUsageMeasurement::Unmeasured,
                ),
            }
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
    let budget = match held {
        Some(HeldReservation {
            handle,
            reservation,
        }) => handle.settle(reservation, measurement).into(),
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
    use crate::backend::{BackendResponse, RawAnswer};
    use crate::contracts::{BackendKind, BinaryAnswer, BinaryJudgment, Judgment, RouteProvenance};
    use crate::error::{AnswerValidationError, BackendFailure};
    use crate::validate::tests::sample_request;

    struct ScriptedBackend {
        responses: Mutex<Vec<Result<BackendResponse, FailedEvaluation>>>,
        delay: Option<Duration>,
    }

    impl ScriptedBackend {
        fn once(response: Result<BackendResponse, FailedEvaluation>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(vec![response]),
                delay: None,
            })
        }
    }

    #[async_trait]
    impl DecisionBackend for ScriptedBackend {
        fn kind(&self) -> BackendKind {
            BackendKind::Llm
        }

        async fn evaluate(
            &self,
            _admission: &DecisionAdmission,
            _request: &ValidatedRequest,
            _deadline: Deadline,
            _max_attempts: u32,
        ) -> Result<BackendResponse, FailedEvaluation> {
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
            route: RouteProvenance::Llm {
                provider: meerkat_core::Provider::Other,
                model: "fake".into(),
            },
            usage,
            attempts: 1,
        }
    }

    fn accounted_usage(input: u64, output: u64) -> meerkat_core::types::Usage {
        meerkat_core::types::TurnUsage::host_declared(
            meerkat_core::Provider::Other,
            "fake",
            meerkat_core::types::Usage {
                input_tokens: input,
                output_tokens: output,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                provider_accounting: None,
            },
        )
        .into_inner()
    }

    fn provider_usage_with_accounting() -> BackendUsage {
        BackendUsage::Provider(vec![AttemptUsage::Measured(accounted_usage(120, 30))])
    }

    fn nested_admission(budget: &Budget) -> DecisionAdmission {
        DecisionAdmission::new(BudgetAdmission::Nested(budget.nested_usage_accounting()))
    }

    #[tokio::test]
    async fn evaluate_charges_the_owner_budget_exactly_once_from_measured_usage() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(provider_usage_with_accounting()))),
            DecisionLimitsConfig::default(),
        );

        let result = service
            .evaluate(&nested_admission(&budget), sample_request())
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
        assert_eq!(result.route.backend(), BackendKind::Llm);
    }

    #[tokio::test]
    async fn every_provider_attempt_is_charged_and_reported() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let usage = BackendUsage::Provider(vec![
            AttemptUsage::Measured(accounted_usage(40, 12)),
            AttemptUsage::Measured(accounted_usage(45, 10)),
        ]);
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(usage))),
            DecisionLimitsConfig::default(),
        );
        let result = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap();
        assert_eq!(
            result.accounting,
            DecisionAccounting::Measured {
                input_tokens: 85,
                output_tokens: 22
            }
        );
        assert_eq!(result.budget, BudgetParticipation::Charged { tokens: 107 });
        assert_eq!(budget.token_usage(), Some((107, 10_000)));
    }

    #[tokio::test]
    async fn evaluate_refuses_before_egress_when_budget_cannot_admit_the_estimate() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(100));
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(provider_usage_with_accounting()))),
            DecisionLimitsConfig::default(),
        );

        let error = service
            .evaluate(&nested_admission(&budget), sample_request())
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
        let raw_without_accounting =
            BackendUsage::Provider(vec![AttemptUsage::Measured(meerkat_core::types::Usage {
                input_tokens: 5,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                provider_accounting: None,
            })]);
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(raw_without_accounting))),
            DecisionLimitsConfig::default(),
        );

        let result = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap();
        assert_eq!(result.budget, BudgetParticipation::Unmeasured);
        // Raw counters without provider accounting evidence are not promoted
        // to a measurement: the report and the budget tell the same story.
        assert_eq!(result.accounting, DecisionAccounting::Unmeasured);
        assert_eq!(budget.token_usage(), Some((0, 10_000)));
    }

    #[tokio::test]
    async fn one_unaccounted_attempt_makes_the_whole_evaluation_unmeasured() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let mixed = BackendUsage::Provider(vec![
            AttemptUsage::Measured(accounted_usage(40, 12)),
            AttemptUsage::Measured(meerkat_core::types::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                provider_accounting: None,
            }),
        ]);
        let service = DecisionService::new(
            ScriptedBackend::once(Ok(valid_response(mixed))),
            DecisionLimitsConfig::default(),
        );
        let result = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap();
        assert_eq!(result.accounting, DecisionAccounting::Unmeasured);
        assert_eq!(result.budget, BudgetParticipation::Unmeasured);
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

        let error = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DecisionError::InvalidAnswer {
                error: AnswerValidationError::OptionNotSupplied { .. },
                budget: BudgetParticipation::Charged { tokens: 150 },
                ..
            }
        ));
        assert_eq!(budget.token_usage(), Some((150, 10_000)));
    }

    #[tokio::test]
    async fn backend_failure_after_provider_calls_still_charges_spent_tokens() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let service = DecisionService::new(
            ScriptedBackend::once(Err(FailedEvaluation {
                failure: BackendFailure::InvalidResponse {
                    message: "still not json".into(),
                },
                usage: BackendUsage::Provider(vec![
                    AttemptUsage::Measured(accounted_usage(40, 12)),
                    AttemptUsage::Measured(accounted_usage(40, 12)),
                ]),
                attempts: 2,
            })),
            DecisionLimitsConfig::default(),
        );

        let error = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DecisionError::BackendFailure {
                failure: BackendFailure::InvalidResponse { .. },
                accounting: DecisionAccounting::Measured {
                    input_tokens: 80,
                    output_tokens: 24
                },
                budget: BudgetParticipation::Charged { tokens: 104 },
                attempts: 2,
            }
        ));
        assert_eq!(budget.token_usage(), Some((104, 10_000)));
    }

    #[tokio::test]
    async fn an_attempt_that_spent_without_reporting_makes_the_failure_unmeasured() {
        // First attempt measured, repair attempt dropped mid-flight: the
        // earlier counters are not promoted to a total that omits the second
        // call's unknown spend.
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let service = DecisionService::new(
            ScriptedBackend::once(Err(FailedEvaluation {
                failure: BackendFailure::Timeout,
                usage: BackendUsage::Provider(vec![
                    AttemptUsage::Measured(accounted_usage(40, 12)),
                    AttemptUsage::Unmeasured,
                ]),
                attempts: 2,
            })),
            DecisionLimitsConfig::default(),
        );
        let error = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            DecisionError::BackendFailure {
                failure: BackendFailure::Timeout,
                accounting: DecisionAccounting::Unmeasured,
                budget: BudgetParticipation::Unmeasured,
                attempts: 2,
            }
        ));
        assert_eq!(budget.token_usage(), Some((0, 10_000)));
    }

    #[tokio::test]
    async fn backend_failure_before_any_call_releases_the_reservation_with_a_marker() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
        let service = DecisionService::new(
            ScriptedBackend::once(Err(BackendFailure::Unauthorized.into())),
            DecisionLimitsConfig::default(),
        );

        let error = service
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            DecisionError::BackendFailure {
                failure: BackendFailure::Unauthorized,
                accounting: DecisionAccounting::Unmeasured,
                budget: BudgetParticipation::Unmeasured,
                attempts: 0,
            }
        );
        assert_eq!(budget.token_usage(), Some((0, 10_000)));
    }

    #[tokio::test]
    async fn deadline_bounds_the_whole_evaluation_and_marks_the_budget() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(10_000));
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
            .evaluate(&nested_admission(&budget), sample_request())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            DecisionError::DeadlineExceeded {
                deadline_ms: 20,
                budget: BudgetParticipation::Unmeasured,
            }
        );
        assert_eq!(budget.token_usage(), Some((0, 10_000)));
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
