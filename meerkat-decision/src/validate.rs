//! Request and answer validation against the fixed interpretation contract.

use indexmap::IndexMap;
use meerkat_core::DecisionLimitsConfig;

use crate::backend::{RawAnswer, RawDistribution, RawGradeDistribution};
use crate::contracts::{
    BackendKind, BinaryJudgment, ChoiceJudgment, DecisionRequest, GradeJudgment, GradeLevelIndex,
    Instructions, Judgment, NativeSignal, OptionId, Question, QuestionId, QuestionJudgment,
    QuestionKind, UnitInterval,
};
use crate::error::{AnswerValidationError, RequestValidationError};

/// Option id reserved for the abstention answer token; no supplied option may
/// use it, so abstention can never alias a real alternative.
pub const RESERVED_ABSTAIN_OPTION: &str = "abstain";

/// A request that passed structural and bound validation.
///
/// Only [`ValidatedRequest::validate`] constructs one, so every backend and
/// the answer validator can rely on unique ids, bounded sizes, and admissible
/// option and level sets without re-checking.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedRequest {
    request: DecisionRequest,
    order: IndexMap<QuestionId, usize>,
}

impl ValidatedRequest {
    pub fn validate(
        request: DecisionRequest,
        limits: &DecisionLimitsConfig,
    ) -> Result<Self, RequestValidationError> {
        if request.questions.is_empty() {
            return Err(RequestValidationError::NoQuestions);
        }
        if request.questions.len() > limits.max_questions {
            return Err(RequestValidationError::TooManyQuestions {
                count: request.questions.len(),
                max: limits.max_questions,
            });
        }
        let state_bytes = request.state.byte_len();
        if state_bytes > limits.max_state_bytes {
            return Err(RequestValidationError::StateTooLarge {
                bytes: state_bytes,
                max: limits.max_state_bytes,
            });
        }
        if let Some(task) = request.task.as_ref()
            && task.len() > limits.max_instruction_bytes
        {
            return Err(RequestValidationError::TaskTooLarge {
                bytes: task.len(),
                max: limits.max_instruction_bytes,
            });
        }

        let mut order = IndexMap::with_capacity(request.questions.len());
        for (index, question) in request.questions.iter().enumerate() {
            let id = question.id().clone();
            if order.insert(id.clone(), index).is_some() {
                return Err(RequestValidationError::DuplicateQuestionId { id });
            }
            check_text(&id, question.instructions(), limits)?;
            match question {
                Question::Binary { criteria, .. } => {
                    if let Some(criteria) = criteria {
                        check_text(&id, &criteria.yes, limits)?;
                        check_text(&id, &criteria.no, limits)?;
                    }
                }
                Question::ChooseOne { options, .. } => {
                    if options.len() < 2 {
                        return Err(RequestValidationError::TooFewOptions {
                            question: id,
                            count: options.len(),
                        });
                    }
                    if options.len() > limits.max_options_per_choice {
                        return Err(RequestValidationError::TooManyOptions {
                            question: id,
                            count: options.len(),
                            max: limits.max_options_per_choice,
                        });
                    }
                    let mut seen = std::collections::BTreeSet::new();
                    for option in options {
                        if option.id.as_str() == RESERVED_ABSTAIN_OPTION {
                            return Err(RequestValidationError::ReservedOption {
                                question: id,
                                option: option.id.to_string(),
                            });
                        }
                        if !seen.insert(option.id.as_str()) {
                            return Err(RequestValidationError::DuplicateOption {
                                question: id,
                                option: option.id.to_string(),
                            });
                        }
                        check_text(&id, &option.description, limits)?;
                    }
                }
                Question::Grade { levels, .. } => {
                    if levels.len() < 2 {
                        return Err(RequestValidationError::TooFewLevels {
                            question: id,
                            count: levels.len(),
                        });
                    }
                    if levels.len() > limits.max_grade_levels {
                        return Err(RequestValidationError::TooManyLevels {
                            question: id,
                            count: levels.len(),
                            max: limits.max_grade_levels,
                        });
                    }
                    for level in levels {
                        check_text(&id, &level.description, limits)?;
                    }
                }
            }
        }
        Ok(Self { request, order })
    }

    pub fn request(&self) -> &DecisionRequest {
        &self.request
    }

    pub fn questions(&self) -> &[Question] {
        &self.request.questions
    }

    pub fn question(&self, id: &QuestionId) -> Option<&Question> {
        self.order
            .get(id)
            .and_then(|index| self.request.questions.get(*index))
    }

    /// Approximate token footprint of the request document, used only to
    /// size the caller-budget reservation before egress.
    pub fn estimated_input_tokens(&self) -> u64 {
        let question_bytes: usize = self
            .request
            .questions
            .iter()
            .map(|question| serde_json::to_string(question).map_or(0, |text| text.len()))
            .sum();
        let task_bytes = self.request.task.as_ref().map_or(0, String::len);
        let total = self.request.state.byte_len() + question_bytes + task_bytes;
        (total as u64).div_ceil(4)
    }
}

fn check_text(
    question: &QuestionId,
    text: &Instructions,
    limits: &DecisionLimitsConfig,
) -> Result<(), RequestValidationError> {
    if !text.has_admissible_shape() {
        return Err(RequestValidationError::InstructionShape {
            question: question.clone(),
        });
    }
    if text.is_empty() {
        return Err(RequestValidationError::EmptyInstructions {
            question: question.clone(),
        });
    }
    let bytes = text.byte_len();
    if bytes > limits.max_instruction_bytes {
        return Err(RequestValidationError::InstructionTooLarge {
            question: question.clone(),
            bytes,
            max: limits.max_instruction_bytes,
        });
    }
    Ok(())
}

/// Validate every raw backend answer against the request and produce the
/// typed judgments in request order.
///
/// Every question must receive exactly one answer of its own kind; extra or
/// unknown answers, out-of-set options, out-of-range levels, and non-finite
/// numerics are typed faults. Nothing is defaulted.
pub fn validate_answers(
    request: &ValidatedRequest,
    backend: BackendKind,
    answers: Vec<(String, RawAnswer)>,
) -> Result<IndexMap<QuestionId, QuestionJudgment>, AnswerValidationError> {
    let mut by_id: IndexMap<QuestionId, RawAnswer> = IndexMap::with_capacity(answers.len());
    for (raw_id, answer) in answers {
        let id = QuestionId::new(raw_id.clone())
            .map_err(|_| AnswerValidationError::UnknownQuestion { id: raw_id.clone() })?;
        if request.question(&id).is_none() {
            return Err(AnswerValidationError::UnknownQuestion { id: raw_id });
        }
        by_id.insert(id, answer);
    }

    let mut judgments = IndexMap::with_capacity(request.questions().len());
    for question in request.questions() {
        let id = question.id().clone();
        let raw = by_id
            .shift_remove(&id)
            .ok_or_else(|| AnswerValidationError::MissingAnswer {
                question: id.clone(),
            })?;
        let judgment = interpret(question, raw, backend)?;
        judgments.insert(id, judgment);
    }
    Ok(judgments)
}

fn interpret(
    question: &Question,
    raw: RawAnswer,
    backend: BackendKind,
) -> Result<QuestionJudgment, AnswerValidationError> {
    let id = question.id();
    let mismatch = |actual: QuestionKind| AnswerValidationError::KindMismatch {
        question: id.clone(),
        expected: question.kind(),
        actual,
    };
    match (question, raw) {
        (Question::Binary { .. }, RawAnswer::BinaryCategorical(answer)) => Ok(QuestionJudgment {
            judgment: Judgment::Binary(BinaryJudgment::Categorical { answer }),
            native_signals: Vec::new(),
        }),
        (Question::Binary { .. }, RawAnswer::BinaryProbability { yes }) => {
            let yes = unit(id, "yes", yes)?;
            Ok(QuestionJudgment {
                judgment: Judgment::Binary(BinaryJudgment::NativeProbability { yes }),
                native_signals: Vec::new(),
            })
        }
        (
            Question::ChooseOne { options, .. },
            RawAnswer::ChoiceSelected {
                option,
                distribution,
            },
        ) => {
            let selected = options
                .iter()
                .find(|candidate| candidate.id.as_str() == option)
                .map(|candidate| candidate.id.clone())
                .ok_or_else(|| AnswerValidationError::OptionNotSupplied {
                    question: id.clone(),
                    option: option.clone(),
                })?;
            let native_signals = match distribution {
                Some(distribution) => vec![choice_distribution(
                    id,
                    options.iter().map(|option| &option.id),
                    distribution,
                    backend,
                )?],
                None => Vec::new(),
            };
            Ok(QuestionJudgment {
                judgment: Judgment::Choice(ChoiceJudgment::Selected { option: selected }),
                native_signals,
            })
        }
        (Question::ChooseOne { .. }, RawAnswer::ChoiceAbstain) => Ok(QuestionJudgment {
            judgment: Judgment::Choice(ChoiceJudgment::Abstain),
            native_signals: Vec::new(),
        }),
        (Question::Grade { levels, .. }, RawAnswer::GradeLevel { index }) => {
            let levels_len = u32::try_from(levels.len()).unwrap_or(u32::MAX);
            if index >= levels_len {
                return Err(AnswerValidationError::LevelOutOfRange {
                    question: id.clone(),
                    level: index,
                    levels: levels_len,
                });
            }
            Ok(QuestionJudgment {
                judgment: Judgment::Grade(GradeJudgment::Level {
                    index: GradeLevelIndex::new(index),
                }),
                native_signals: Vec::new(),
            })
        }
        (Question::Grade { .. }, RawAnswer::GradeAbstain) => Ok(QuestionJudgment {
            judgment: Judgment::Grade(GradeJudgment::Abstain),
            native_signals: Vec::new(),
        }),
        (
            Question::Grade { levels, .. },
            RawAnswer::GradeWeighted {
                position,
                distribution,
            },
        ) => {
            let max_position = levels.len().saturating_sub(1) as f64;
            if !position.is_finite() || position < 0.0 || position > max_position {
                return Err(AnswerValidationError::InvalidNumeric {
                    question: id.clone(),
                    field: "position".to_string(),
                });
            }
            let native_signals = match distribution {
                Some(distribution) => {
                    vec![grade_distribution(id, levels.len(), distribution, backend)?]
                }
                None => Vec::new(),
            };
            Ok(QuestionJudgment {
                judgment: Judgment::Grade(GradeJudgment::NativeWeighted { position }),
                native_signals,
            })
        }
        (_, RawAnswer::BinaryCategorical(_) | RawAnswer::BinaryProbability { .. }) => {
            Err(mismatch(QuestionKind::Binary))
        }
        (_, RawAnswer::ChoiceSelected { .. } | RawAnswer::ChoiceAbstain) => {
            Err(mismatch(QuestionKind::ChooseOne))
        }
        (
            _,
            RawAnswer::GradeLevel { .. }
            | RawAnswer::GradeAbstain
            | RawAnswer::GradeWeighted { .. },
        ) => Err(mismatch(QuestionKind::Grade)),
    }
}

fn unit(id: &QuestionId, field: &str, value: f64) -> Result<UnitInterval, AnswerValidationError> {
    UnitInterval::new(value).map_err(|_| AnswerValidationError::InvalidNumeric {
        question: id.clone(),
        field: field.to_string(),
    })
}

fn choice_distribution<'a>(
    id: &QuestionId,
    options: impl Iterator<Item = &'a OptionId>,
    distribution: RawDistribution,
    backend: BackendKind,
) -> Result<NativeSignal, AnswerValidationError> {
    let supplied: Vec<&OptionId> = options.collect();
    let mut probabilities = IndexMap::with_capacity(distribution.probabilities.len());
    for (option, probability) in distribution.probabilities {
        let option_id = supplied
            .iter()
            .find(|candidate| candidate.as_str() == option)
            .map(|candidate| (*candidate).clone())
            .ok_or_else(|| AnswerValidationError::DistributionOptionNotSupplied {
                question: id.clone(),
                option: option.clone(),
            })?;
        probabilities.insert(option_id, unit(id, "probabilities", probability)?);
    }
    Ok(NativeSignal::ChoiceDistribution {
        backend,
        probabilities,
        confidence: unit(id, "confidence", distribution.confidence)?,
    })
}

fn grade_distribution(
    id: &QuestionId,
    levels: usize,
    distribution: RawGradeDistribution,
    backend: BackendKind,
) -> Result<NativeSignal, AnswerValidationError> {
    let levels_len = u32::try_from(levels).unwrap_or(u32::MAX);
    let mut probabilities = vec![None; levels];
    for (level, probability) in distribution.probabilities {
        if level >= levels_len {
            return Err(AnswerValidationError::LevelOutOfRange {
                question: id.clone(),
                level,
                levels: levels_len,
            });
        }
        probabilities[level as usize] = Some(unit(id, "probabilities", probability)?);
    }
    let probabilities = probabilities
        .into_iter()
        .map(|probability| {
            probability.ok_or_else(|| AnswerValidationError::InvalidNumeric {
                question: id.clone(),
                field: "probabilities".to_string(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NativeSignal::GradeDistribution {
        backend,
        probabilities,
        confidence: unit(id, "confidence", distribution.confidence)?,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;
    use crate::contracts::{BinaryAnswer, ChoiceOption, DecisionState, GradeLevel};

    pub(crate) fn sample_request() -> DecisionRequest {
        DecisionRequest {
            task: Some("Triage a support message".into()),
            state: DecisionState::text("Help! My payouts have been failing for 3 days."),
            questions: vec![
                Question::Binary {
                    id: QuestionId::new("is_urgent").unwrap(),
                    instructions: Instructions::text("Does this convey urgency?"),
                    criteria: None,
                },
                Question::ChooseOne {
                    id: QuestionId::new("department").unwrap(),
                    instructions: Instructions::text("Which team should handle this?"),
                    options: vec![
                        ChoiceOption {
                            id: OptionId::new("billing").unwrap(),
                            description: Instructions::text("Payments, invoicing, refunds"),
                        },
                        ChoiceOption {
                            id: OptionId::new("technical").unwrap(),
                            description: Instructions::text("Bugs, outages, integrations"),
                        },
                    ],
                },
                Question::Grade {
                    id: QuestionId::new("frustration").unwrap(),
                    instructions: Instructions::text("How frustrated is the customer?"),
                    levels: vec![
                        GradeLevel {
                            description: Instructions::text("Calm"),
                        },
                        GradeLevel {
                            description: Instructions::text("Frustrated"),
                        },
                        GradeLevel {
                            description: Instructions::text("Very angry"),
                        },
                    ],
                },
            ],
        }
    }

    pub(crate) fn validated() -> ValidatedRequest {
        ValidatedRequest::validate(sample_request(), &DecisionLimitsConfig::default()).unwrap()
    }

    #[test]
    fn validation_rejects_empty_duplicate_and_oversized_requests() {
        let limits = DecisionLimitsConfig::default();
        let mut request = sample_request();
        request.questions.clear();
        assert_eq!(
            ValidatedRequest::validate(request, &limits).unwrap_err(),
            RequestValidationError::NoQuestions
        );

        let mut request = sample_request();
        let duplicate = request.questions[0].clone();
        request.questions.push(duplicate);
        assert!(matches!(
            ValidatedRequest::validate(request, &limits).unwrap_err(),
            RequestValidationError::DuplicateQuestionId { .. }
        ));

        let mut request = sample_request();
        request.state = DecisionState::text("x".repeat(limits.max_state_bytes + 1));
        assert!(matches!(
            ValidatedRequest::validate(request, &limits).unwrap_err(),
            RequestValidationError::StateTooLarge { .. }
        ));

        let mut request = sample_request();
        if let Question::ChooseOne { options, .. } = &mut request.questions[1] {
            options.truncate(1);
        }
        assert!(matches!(
            ValidatedRequest::validate(request, &limits).unwrap_err(),
            RequestValidationError::TooFewOptions { .. }
        ));

        let mut request = sample_request();
        if let Question::ChooseOne { options, .. } = &mut request.questions[1] {
            options[0].id = OptionId::new("abstain").unwrap();
        }
        assert!(matches!(
            ValidatedRequest::validate(request, &limits).unwrap_err(),
            RequestValidationError::ReservedOption { .. }
        ));

        let mut request = sample_request();
        if let Question::Grade { levels, .. } = &mut request.questions[2] {
            levels.truncate(1);
        }
        assert!(matches!(
            ValidatedRequest::validate(request, &limits).unwrap_err(),
            RequestValidationError::TooFewLevels { .. }
        ));
    }

    #[test]
    fn answers_are_interpreted_in_request_order_with_native_signals() {
        let request = validated();
        let judgments = validate_answers(
            &request,
            BackendKind::Jev,
            vec![
                (
                    "frustration".into(),
                    RawAnswer::GradeWeighted {
                        position: 1.05,
                        distribution: Some(RawGradeDistribution {
                            probabilities: vec![(0, 0.0), (1, 0.95), (2, 0.05)],
                            confidence: 0.92,
                        }),
                    },
                ),
                (
                    "is_urgent".into(),
                    RawAnswer::BinaryProbability { yes: 0.95 },
                ),
                (
                    "department".into(),
                    RawAnswer::ChoiceSelected {
                        option: "billing".into(),
                        distribution: Some(RawDistribution {
                            probabilities: vec![
                                ("billing".into(), 0.88),
                                ("technical".into(), 0.12),
                            ],
                            confidence: 0.81,
                        }),
                    },
                ),
            ],
        )
        .unwrap();
        let ids: Vec<&str> = judgments.keys().map(QuestionId::as_str).collect();
        assert_eq!(ids, ["is_urgent", "department", "frustration"]);
        assert!(matches!(
            judgments["department"].judgment,
            Judgment::Choice(ChoiceJudgment::Selected { ref option }) if option.as_str() == "billing"
        ));
        assert_eq!(judgments["department"].native_signals.len(), 1);
        assert!(matches!(
            judgments["frustration"].judgment,
            Judgment::Grade(GradeJudgment::NativeWeighted { position }) if (position - 1.05).abs() < 1e-9
        ));
    }

    #[test]
    fn out_of_set_options_unknown_ids_and_missing_answers_fail_closed() {
        let request = validated();
        let mut answers = vec![
            (
                "is_urgent".to_string(),
                RawAnswer::BinaryCategorical(BinaryAnswer::Yes),
            ),
            (
                "department".to_string(),
                RawAnswer::ChoiceSelected {
                    option: "sales".into(),
                    distribution: None,
                },
            ),
            (
                "frustration".to_string(),
                RawAnswer::GradeLevel { index: 1 },
            ),
        ];
        assert!(matches!(
            validate_answers(&request, BackendKind::SessionLlm, answers.clone()).unwrap_err(),
            AnswerValidationError::OptionNotSupplied { ref option, .. } if option == "sales"
        ));

        answers[1].1 = RawAnswer::ChoiceAbstain;
        answers.push(("extra".to_string(), RawAnswer::GradeAbstain));
        assert!(matches!(
            validate_answers(&request, BackendKind::SessionLlm, answers.clone()).unwrap_err(),
            AnswerValidationError::UnknownQuestion { ref id } if id == "extra"
        ));

        answers.pop();
        answers.pop();
        assert!(matches!(
            validate_answers(&request, BackendKind::SessionLlm, answers.clone()).unwrap_err(),
            AnswerValidationError::MissingAnswer { ref question } if question.as_str() == "frustration"
        ));

        answers.push((
            "frustration".to_string(),
            RawAnswer::GradeLevel { index: 3 },
        ));
        assert!(matches!(
            validate_answers(&request, BackendKind::SessionLlm, answers.clone()).unwrap_err(),
            AnswerValidationError::LevelOutOfRange {
                level: 3,
                levels: 3,
                ..
            }
        ));

        answers.pop();
        answers.push((
            "frustration".to_string(),
            RawAnswer::BinaryCategorical(BinaryAnswer::No),
        ));
        assert!(matches!(
            validate_answers(&request, BackendKind::SessionLlm, answers).unwrap_err(),
            AnswerValidationError::KindMismatch {
                expected: QuestionKind::Grade,
                actual: QuestionKind::Binary,
                ..
            }
        ));
    }
}
