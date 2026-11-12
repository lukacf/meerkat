//! The `rkat` exit status of a failed command.
//!
//! A failed command exits with the CLI code of the public [`ErrorCode`] its
//! typed error carries ([`ErrorCode::cli_exit_code`]), the same code the
//! JSON-RPC surface reports for that error. The code is read from the typed
//! owner in the error chain, never from display text. The first error in the
//! chain whose type has a public code projection decides: a mob-family error
//! answers with its owner's `wire_error_code()`, a runtime driver error with
//! its native admission, teardown or hosting class, a session or agent
//! error with the session transport mapping, and a refused help-session
//! resume with its own code. A typed error whose owner
//! assigns no public code, and a failure with no typed owner (an argument,
//! configuration or lookup check made by the CLI itself), exits
//! [`EXIT_ERROR`](crate::EXIT_ERROR).

use meerkat_contracts::ErrorCode;
use meerkat_contracts::error::{agent_transport_error_code, session_transport_error_code};

/// What a typed owner in the error chain says about its public code.
enum OwnerCode {
    /// The owner's public code.
    Public(ErrorCode),
    /// The owner is typed but assigns no public code; its verdict is final.
    Uncoded,
}

impl From<Option<ErrorCode>> for OwnerCode {
    fn from(code: Option<ErrorCode>) -> Self {
        code.map_or(Self::Uncoded, Self::Public)
    }
}

/// The public error code of a failed command, from the first typed owner in
/// `error`'s chain.
pub(crate) fn failure_error_code(error: &anyhow::Error) -> Option<ErrorCode> {
    match error.chain().find_map(owner_code)? {
        OwnerCode::Public(code) => Some(code),
        OwnerCode::Uncoded => None,
    }
}

/// The process exit status of a public error code.
pub(crate) fn code_exit_status(code: ErrorCode) -> u8 {
    u8::try_from(code.cli_exit_code()).unwrap_or(crate::EXIT_ERROR)
}

/// The process exit status of a failed command.
pub(crate) fn failure_exit_status(error: &anyhow::Error) -> u8 {
    failure_error_code(error).map_or(crate::EXIT_ERROR, code_exit_status)
}

/// The typed data of a refused reasoning effort level, when `owner` is the
/// typed owner of one: `{"kind": "reasoning_effort_refused", "retryable",
/// "reasoning_effort_refusal"}`, the object JSON-RPC answers in
/// `error.data`. A session error carries it itself; an agent error and the
/// runtime's refusal of a live reconfiguration are read through the same
/// session projection, so the shape has one owner.
fn reasoning_effort_refusal_details(
    owner: &(dyn std::error::Error + 'static),
) -> Option<serde_json::Value> {
    use meerkat_core::{AgentError, SessionError};
    let refused = |error: &SessionError| {
        error.structured_data().filter(|data| {
            data.get("kind").and_then(serde_json::Value::as_str)
                == Some(SessionError::REASONING_EFFORT_REFUSED_KIND)
        })
    };
    if let Some(error) = owner.downcast_ref::<SessionError>() {
        return refused(error);
    }
    if let Some(AgentError::ReasoningEffortRefused(refusal)) = owner.downcast_ref::<AgentError>() {
        return refused(&SessionError::Agent(AgentError::ReasoningEffortRefused(
            refusal.clone(),
        )));
    }
    if let Some(error) = owner.downcast_ref::<meerkat_runtime::RuntimeDriverError>() {
        return error
            .reasoning_effort_refused_session_error()
            .as_ref()
            .and_then(refused);
    }
    None
}

/// The failure document of a refused reasoning effort level: `{"error":
/// {"code": "INVALID_PARAMS", "category", "message", "details"}}`, the
/// `WireError` envelope with the typed refusal as its details and the text
/// stderr shows as its message. `None` for every other failure. The first
/// typed owner in `error`'s chain decides, as it decides the exit status.
pub(crate) fn reasoning_effort_refusal_document(
    error: &anyhow::Error,
) -> Option<serde_json::Value> {
    let owner = error.chain().find(|cause| owner_code(*cause).is_some())?;
    let details = reasoning_effort_refusal_details(owner)?;
    let mut wire = meerkat_contracts::WireError::new(ErrorCode::InvalidParams, error.to_string());
    wire.details = Some(details);
    Some(serde_json::json!({ "error": wire }))
}

/// Write the failure document of a refused reasoning effort level as one
/// JSON document on stdout; nothing for any other failure.
pub(crate) fn write_reasoning_effort_refusal_document(error: &anyhow::Error) {
    if let Some(document) = reasoning_effort_refusal_document(error) {
        match serde_json::to_string_pretty(&document) {
            Ok(json) => println!("{json}"),
            Err(render) => eprintln!("Warning: failed to render the failure document: {render}"),
        }
    }
}

/// The typed error of a turn that ended because the session's reasoning
/// effort level was refused before a provider request, when `error` is the
/// typed turn error of such a turn. The command then fails exactly as a
/// refused build does: the same typed owner, exit status and failure
/// document, whichever checkpoint refused.
pub(crate) fn reasoning_effort_refused_turn(
    error: &meerkat_core::TurnErrorMetadata,
) -> Option<meerkat_core::SessionError> {
    match error.reason.as_ref()? {
        meerkat_core::event::AgentErrorReason::ReasoningEffortRefused { refusal } => {
            Some(meerkat_core::SessionError::Agent(
                meerkat_core::AgentError::ReasoningEffortRefused(Box::new(refusal.clone())),
            ))
        }
        _ => None,
    }
}

/// Report a failed command on stderr and return its exit status. A mob
/// error that carries a typed console detail also prints one `detail:` line
/// rendered from the typed fields, never from a display string.
pub(crate) fn report_failure(error: &anyhow::Error) -> std::process::ExitCode {
    if failure_error_code(error) == Some(ErrorCode::BudgetExhausted) {
        eprintln!("Budget exhausted: {error}");
    } else {
        eprintln!("Error: {error}");
    }
    #[cfg(feature = "mob")]
    if let Some(detail) = deciding_mob_wire_detail(error)
        && let Ok(json) = detail.detail_value()
    {
        eprintln!("detail: {json}");
    }
    std::process::ExitCode::from(failure_exit_status(error))
}

/// `error` displayed as `render` writes it, kept typed in the chain so the
/// exit status still reads its owner's code.
pub(crate) fn typed_failure<E>(error: E, render: impl FnOnce(&E) -> String) -> anyhow::Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    let message = render(&error);
    anyhow::Error::new(error).context(message)
}

/// The typed not-found error for an exact session id the realm does not
/// hold, displayed as `Session not found: <id>`.
pub(crate) fn session_not_found(id: &meerkat_core::SessionId) -> anyhow::Error {
    typed_failure(
        meerkat_core::SessionError::NotFound { id: id.clone() },
        |_| format!("Session not found: {id}"),
    )
}

/// `Some` when `cause` is a typed owner; `None` when the chain continues.
fn owner_code(cause: &(dyn std::error::Error + 'static)) -> Option<OwnerCode> {
    #[cfg(feature = "mob")]
    if let Some(code) = mob_owner_code(cause) {
        return Some(code);
    }
    if let Some(error) = cause.downcast_ref::<meerkat_runtime::RuntimeDriverError>() {
        return Some(runtime_driver_error_code(error).into());
    }
    if let Some(error) = cause.downcast_ref::<meerkat_core::SessionError>() {
        return Some(OwnerCode::Public(session_transport_error_code(error)));
    }
    if let Some(error) = cause.downcast_ref::<meerkat_core::AgentError>() {
        return Some(OwnerCode::Public(agent_transport_error_code(error)));
    }
    if let Some(refusal) = cause.downcast_ref::<meerkat::help::HelpSessionNotResumable>() {
        return Some(OwnerCode::Public(refusal.error_code()));
    }
    if let Some(code) = config_write_admission_code(cause) {
        return Some(OwnerCode::Public(code));
    }
    // A fenced operation's own refusal (realm delete): nothing started.
    if let Some(refusal) = cause.downcast_ref::<meerkat_core::AdmissionRefusal>() {
        return Some(OwnerCode::Public(match refusal {
            meerkat_core::AdmissionRefusal::InProgress => ErrorCode::AdmissionInProgress,
            meerkat_core::AdmissionRefusal::Unavailable(_) => ErrorCode::AdmissionFenceUnavailable,
        }));
    }
    #[cfg(feature = "local-authorization")]
    if let Some(code) = stock_admission_code(cause) {
        return Some(OwnerCode::Public(code));
    }
    None
}

/// The public code of a stock grant command (`rkat realm govern`, `grant`,
/// `revoke-grant`) the realm's admission fence refused before it started.
/// Other stock refusals carry no public code.
#[cfg(feature = "local-authorization")]
fn stock_admission_code(cause: &(dyn std::error::Error + 'static)) -> Option<ErrorCode> {
    use meerkat::governed_realm::stock::StockGovernanceError;
    use meerkat::governed_realm::stock::issuance::StockGrantError;
    let governance = match cause.downcast_ref::<StockGrantError>() {
        Some(StockGrantError::AdmissionInProgress) => return Some(ErrorCode::AdmissionInProgress),
        Some(StockGrantError::AdmissionFenceUnavailable(_)) => {
            return Some(ErrorCode::AdmissionFenceUnavailable);
        }
        // Transparent: the refusal's own error is not in the chain.
        Some(StockGrantError::Refused(error)) => Some(error),
        Some(_) => return None,
        None => cause.downcast_ref::<StockGovernanceError>(),
    };
    match governance? {
        StockGovernanceError::AdmissionInProgress => Some(ErrorCode::AdmissionInProgress),
        StockGovernanceError::AdmissionFenceUnavailable(_) => {
            Some(ErrorCode::AdmissionFenceUnavailable)
        }
        _ => None,
    }
}

/// The public code of a config write an admission fence refused before it
/// started: a held fence or one that could not be used. Other config write
/// failures carry no public code.
fn config_write_admission_code(cause: &(dyn std::error::Error + 'static)) -> Option<ErrorCode> {
    let runtime = if let Some(error) = cause.downcast_ref::<meerkat_core::ConfigRuntimeError>() {
        Some(error)
    } else if let Some(meerkat_core::McpRealmPersistError::Config(error)) =
        cause.downcast_ref::<meerkat_core::McpRealmPersistError>()
    {
        Some(error)
    } else {
        None
    };
    match runtime {
        Some(meerkat_core::ConfigRuntimeError::AdmissionInProgress) => {
            return Some(ErrorCode::AdmissionInProgress);
        }
        Some(meerkat_core::ConfigRuntimeError::AdmissionFenceUnavailable(_)) => {
            return Some(ErrorCode::AdmissionFenceUnavailable);
        }
        Some(_) => return None,
        None => {}
    }
    match cause.downcast_ref::<meerkat_core::config::ConfigError>()? {
        meerkat_core::config::ConfigError::AdmissionInProgress => {
            Some(ErrorCode::AdmissionInProgress)
        }
        meerkat_core::config::ConfigError::AdmissionFenceUnavailable(_) => {
            Some(ErrorCode::AdmissionFenceUnavailable)
        }
        _ => None,
    }
}

/// The runtime classes that carry a public code: native input refusal and
/// readiness, a teardown still in progress, and a session another runtime
/// owner hosts.
fn runtime_driver_error_code(error: &meerkat_runtime::RuntimeDriverError) -> Option<ErrorCode> {
    if let Some(detail) = meerkat::surface::native_admission_error_detail(error) {
        return Some(detail.code());
    }
    error
        .teardown_in_progress_session_error()
        .or_else(|| error.hosting_session_error())
        .map(|session_error| session_transport_error_code(&session_error))
}

#[cfg(feature = "mob")]
fn mob_owner_code(cause: &(dyn std::error::Error + 'static)) -> Option<OwnerCode> {
    if let Some(error) = cause.downcast_ref::<meerkat_mob::MobError>() {
        return Some(error.wire_error_code().into());
    }
    if let Some(error) = cause.downcast_ref::<meerkat_mob::MobRespawnError>() {
        return Some(error.wire_error_code().into());
    }
    if let Some(error) = cause.downcast_ref::<meerkat_mob_mcp::MobMcpDestroyError>() {
        return Some(error.wire_error_code().into());
    }
    None
}

/// The typed console detail of the owner that decided the exit status.
#[cfg(feature = "mob")]
fn deciding_mob_wire_detail(
    error: &anyhow::Error,
) -> Option<meerkat_contracts::wire::WireMobErrorDetail> {
    let owner = error.chain().find(|cause| owner_code(*cause).is_some())?;
    if let Some(error) = owner.downcast_ref::<meerkat_mob::MobError>() {
        return error.wire_detail();
    }
    if let Some(error) = owner.downcast_ref::<meerkat_mob::MobRespawnError>() {
        return error.wire_detail();
    }
    if let Some(error) = owner.downcast_ref::<meerkat_mob_mcp::MobMcpDestroyError>() {
        return error.wire_detail();
    }
    None
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{AgentError, SessionError, SessionId};
    use meerkat_runtime::RuntimeDriverError;

    /// The exit status the error-code reference documents for each public
    /// code. The match has no wildcard arm, so a new `ErrorCode` variant
    /// does not compile until it is given a documented status here.
    fn documented_exit_status(code: ErrorCode) -> u8 {
        match code {
            ErrorCode::SessionNotFound => 10,
            ErrorCode::ScheduleNotFound => 43,
            ErrorCode::SessionBusy => 11,
            ErrorCode::SessionNotRunning => 12,
            ErrorCode::RequestCancelled => 14,
            ErrorCode::ProviderError => 20,
            ErrorCode::BudgetExhausted => 21,
            ErrorCode::HookDenied => 22,
            ErrorCode::AgentError => 30,
            ErrorCode::CapabilityUnavailable => 40,
            ErrorCode::SkillNotFound => 41,
            ErrorCode::SkillResolutionFailed => 42,
            ErrorCode::InvalidParams => 2,
            ErrorCode::InternalError => 1,
            ErrorCode::DuplicateInput => 13,
            ErrorCode::SupervisorRotationIncomplete => 44,
            ErrorCode::ScopeDenied => 45,
            ErrorCode::HostUnavailable => 46,
            ErrorCode::StaleCursor => 47,
            ErrorCode::StaleFence => 48,
            ErrorCode::MemberReloadRequired => 49,
            ErrorCode::InputRefused => 50,
            ErrorCode::InputNotReady => 51,
            ErrorCode::SessionRuntimeUnavailable => 52,
            ErrorCode::ApprovalStoreUnavailable => 53,
            ErrorCode::ApprovalConflict => 54,
            ErrorCode::ApprovalCommitUnknown => 55,
            ErrorCode::StaleDeliveryScope => 56,
            ErrorCode::ForkInstructionRefused => 57,
            ErrorCode::AdmissionInProgress => 58,
            ErrorCode::AdmissionFenceUnavailable => 59,
        }
    }

    /// Every `ErrorCode` variant, through its JSON-RPC code (each variant has
    /// exactly one, and the contracts crate pins the round trip).
    fn every_error_code() -> Vec<ErrorCode> {
        (-32_700..=-32_000)
            .filter_map(ErrorCode::from_jsonrpc_code)
            .collect()
    }

    /// Table-driven over every `ErrorCode` variant: each exits with its
    /// documented status, which is its `cli_exit_code()`.
    #[test]
    fn every_error_code_exits_with_its_cli_exit_code() {
        let codes = every_error_code();
        assert_eq!(codes.len(), 31, "every ErrorCode variant is reached");
        for code in codes {
            let documented = documented_exit_status(code);
            assert_eq!(i32::from(documented), code.cli_exit_code(), "{code}");
            assert_eq!(code_exit_status(code), documented, "{code}");
            assert_ne!(documented, 0, "{code} must not exit as a success");
        }
    }

    /// A config write an admission fence refused before it started exits
    /// with the fence's code, whether the runtime, the store or the realm
    /// server writer reports it; any other config write failure carries no
    /// public code and exits 1.
    #[test]
    fn config_write_admission_refusals_exit_with_their_codes() {
        use meerkat_core::ConfigRuntimeError;
        let render = |error: &ConfigRuntimeError| format!("Failed to persist config: {error}");
        let cases = [
            (
                typed_failure(ConfigRuntimeError::AdmissionInProgress, render),
                58,
            ),
            (
                typed_failure(
                    ConfigRuntimeError::AdmissionFenceUnavailable("unopenable".into()),
                    render,
                ),
                59,
            ),
            (
                anyhow::Error::new(meerkat_core::config::ConfigError::AdmissionInProgress),
                58,
            ),
            (
                anyhow::Error::new(
                    meerkat_core::config::ConfigError::AdmissionFenceUnavailable(
                        "unopenable".into(),
                    ),
                ),
                59,
            ),
            (
                anyhow::Error::new(meerkat_core::McpRealmPersistError::Config(
                    ConfigRuntimeError::AdmissionInProgress,
                )),
                58,
            ),
            (typed_failure(ConfigRuntimeError::LockTimeout, render), 1),
        ];
        for (error, status) in cases {
            assert_eq!(failure_exit_status(&error), status, "{error:#}");
        }
    }

    /// A stock grant command the realm's admission fence refused exits with
    /// the fence's code: `rkat realm grant`'s own refusal, and the
    /// governance error of `govern` and `revoke-grant`, directly or as the
    /// grant command's refusal. Every other stock refusal exits 1.
    #[cfg(feature = "local-authorization")]
    #[test]
    fn stock_admission_refusals_exit_with_their_codes() {
        use meerkat::governed_realm::stock::StockGovernanceError;
        use meerkat::governed_realm::stock::issuance::StockGrantError;
        let cases = [
            (anyhow::Error::new(StockGrantError::AdmissionInProgress), 58),
            (
                anyhow::Error::new(StockGrantError::AdmissionFenceUnavailable(
                    "unopenable".into(),
                )),
                59,
            ),
            (
                anyhow::Error::new(StockGovernanceError::AdmissionInProgress),
                58,
            ),
            (
                anyhow::Error::new(StockGovernanceError::AdmissionFenceUnavailable(
                    "unopenable".into(),
                )),
                59,
            ),
            (
                anyhow::Error::new(StockGrantError::Refused(
                    StockGovernanceError::AdmissionInProgress,
                )),
                58,
            ),
            (anyhow::Error::new(StockGovernanceError::NotGoverned), 1),
            (anyhow::Error::new(StockGrantError::CommandUncertain), 1),
        ];
        for (error, status) in cases {
            assert_eq!(failure_exit_status(&error), status, "{error:#}");
        }
    }

    /// Typed session, agent and runtime errors exit with their owner's code;
    /// an untyped failure exits 1.
    #[test]
    fn typed_errors_exit_with_their_owner_code() {
        let cases = [
            (
                anyhow::Error::new(SessionError::NotFound {
                    id: SessionId::new(),
                }),
                Some(ErrorCode::SessionNotFound),
            ),
            (
                anyhow::Error::new(SessionError::Busy {
                    id: SessionId::new(),
                }),
                Some(ErrorCode::SessionBusy),
            ),
            (
                anyhow::Error::new(SessionError::ServedElsewhere {
                    id: SessionId::new(),
                }),
                Some(ErrorCode::SessionBusy),
            ),
            (
                anyhow::Error::new(SessionError::RuntimeUnavailable {
                    reason:
                        meerkat_core::authorization::ControllerReadinessFailure::AuthorityChanged,
                }),
                Some(ErrorCode::SessionRuntimeUnavailable),
            ),
            (
                anyhow::Error::new(SessionError::DurableTailHeldForRecovery {
                    id: SessionId::new(),
                }),
                Some(ErrorCode::SessionNotRunning),
            ),
            (
                anyhow::Error::new(meerkat::help::HelpSessionNotResumable {
                    session_id: SessionId::new(),
                }),
                Some(ErrorCode::SessionNotRunning),
            ),
            (
                anyhow::Error::new(SessionError::Store(Box::new(std::io::Error::other(
                    "store failed",
                )))),
                Some(ErrorCode::InternalError),
            ),
            (
                anyhow::Error::new(SessionError::Agent(AgentError::BuildError(
                    "no provider key".into(),
                ))),
                Some(ErrorCode::ProviderError),
            ),
            (
                anyhow::Error::new(AgentError::Cancelled),
                Some(ErrorCode::RequestCancelled),
            ),
            (
                anyhow::Error::new(AgentError::TokenBudgetExceeded { used: 2, limit: 1 }),
                Some(ErrorCode::BudgetExhausted),
            ),
            (
                anyhow::Error::new(AgentError::TimeBudgetExceeded {
                    elapsed_secs: 2,
                    limit_secs: 1,
                }),
                Some(ErrorCode::BudgetExhausted),
            ),
            (
                anyhow::Error::new(AgentError::MaxTurnsReached { turns: 3 }),
                Some(ErrorCode::InternalError),
            ),
            (
                anyhow::Error::new(AgentError::ConfigError("bad config".into())),
                Some(ErrorCode::InvalidParams),
            ),
            (
                anyhow::Error::new(RuntimeDriverError::InputRefused {
                    refusal: meerkat_core::OperationRefused::new(
                        meerkat_core::OperationRefusalKind::Denied,
                    ),
                }),
                Some(ErrorCode::InputRefused),
            ),
            (
                anyhow::Error::new(RuntimeDriverError::ControllerReadinessUnavailable {
                    reason:
                        meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
                }),
                Some(ErrorCode::InputNotReady),
            ),
            (
                anyhow::Error::new(RuntimeDriverError::ServedElsewhere {
                    session_id: SessionId::new(),
                }),
                Some(ErrorCode::SessionBusy),
            ),
            (
                anyhow::Error::new(RuntimeDriverError::HostingUnavailable {
                    session_id: SessionId::new(),
                }),
                Some(ErrorCode::SessionRuntimeUnavailable),
            ),
            (anyhow::Error::new(RuntimeDriverError::Destroyed), None),
            (anyhow::anyhow!("plain failure"), None),
        ];
        for (error, expected) in cases {
            assert_eq!(failure_error_code(&error), expected, "{error}");
            assert_eq!(
                failure_exit_status(&error),
                expected.map_or(crate::EXIT_ERROR, code_exit_status),
                "{error}"
            );
        }
    }

    /// A typed error under a context message keeps its owner's code and the
    /// exact message.
    #[test]
    fn typed_failure_keeps_the_owner_code_under_its_message() {
        let id = SessionId::new();
        let error = typed_failure(SessionError::NotFound { id: id.clone() }, |error| {
            format!("Failed to load session: {error}")
        });
        assert_eq!(
            error.to_string(),
            format!("Failed to load session: {}", SessionError::NotFound { id })
        );
        assert_eq!(failure_exit_status(&error), 10);

        let wrapped = error.context("outer context");
        assert_eq!(failure_exit_status(&wrapped), 10);

        let missing = SessionId::new();
        let not_found = session_not_found(&missing);
        assert_eq!(
            not_found.to_string(),
            format!("Session not found: {missing}")
        );
        assert_eq!(failure_exit_status(&not_found), 10);
    }

    /// The run and resume paths keep their typed errors: a runtime refusal
    /// exits 50, readiness 51, a session error with its session code, and a
    /// cancelled turn 14, each with its message unchanged.
    #[test]
    fn run_path_failures_keep_their_typed_owner() {
        let refused = crate::runtime_accept_failure(
            RuntimeDriverError::InputRefused {
                refusal: meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::Denied,
                ),
            },
            &Some("grant the route".to_string()),
        );
        assert!(
            refused
                .to_string()
                .starts_with("runtime accept failed: input refused"),
            "{refused}"
        );
        assert!(refused.to_string().contains("grant the route"), "{refused}");
        assert_eq!(failure_exit_status(&refused), 50);

        let not_ready = crate::runtime_accept_failure(
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: meerkat_core::authorization::ControllerReadinessFailure::Busy,
            },
            &None,
        );
        assert!(
            not_ready.to_string().starts_with("runtime accept failed: "),
            "{not_ready}"
        );
        assert_eq!(failure_exit_status(&not_ready), 51);

        let id = SessionId::new();
        let busy = crate::session_err_to_anyhow(SessionError::Busy { id: id.clone() });
        assert_eq!(
            busy.to_string(),
            format!("Session service error: {}", SessionError::Busy { id })
        );
        assert_eq!(failure_exit_status(&busy), 11);

        let provider = crate::session_err_to_anyhow(SessionError::Agent(AgentError::BuildError(
            "no provider key".into(),
        )));
        assert_eq!(failure_exit_status(&provider), 20);

        let cancelled = match crate::completion_outcome_to_cli_runtime_turn_result(
            meerkat_runtime::completion::CompletionOutcome::Cancelled,
            &SessionId::new(),
            &meerkat_core::connection::RealmId::parse("cli-exit").unwrap(),
            true,
        ) {
            Err(error) => error,
            Ok(_) => panic!("a cancelled completion is a failure"),
        };
        assert_eq!(cancelled.to_string(), "request cancelled");
        assert_eq!(failure_exit_status(&cancelled), 14);
    }

    /// A turn that ends in a hard failure (the time budget, the turn limit,
    /// a provider failure) exits 1, as JSON-RPC and REST report the same
    /// completion as an internal error.
    #[test]
    fn abandoned_turns_exit_with_the_generic_status() {
        for (kind, outcome) in [
            (
                meerkat_core::TurnTerminalCauseKind::TimeBudgetExceeded,
                meerkat_core::TurnTerminalOutcome::TimeBudgetExceeded,
            ),
            (
                meerkat_core::TurnTerminalCauseKind::TurnLimitReached,
                meerkat_core::TurnTerminalOutcome::Failed,
            ),
            (
                meerkat_core::TurnTerminalCauseKind::LlmFailure,
                meerkat_core::TurnTerminalOutcome::Failed,
            ),
        ] {
            let completion = meerkat_runtime::completion::CompletionOutcome::AbandonedWithError {
                reason: "turn failed".into(),
                error: meerkat_core::TurnErrorMetadata::terminal(kind, outcome, "turn failed"),
            };
            let error = match crate::completion_outcome_to_cli_runtime_turn_result(
                completion,
                &SessionId::new(),
                &meerkat_core::connection::RealmId::parse("cli-exit").unwrap(),
                true,
            ) {
                Err(error) => error,
                Ok(_) => panic!("an abandoned completion is a failure"),
            };
            assert_eq!(failure_exit_status(&error), crate::EXIT_ERROR, "{kind:?}");
        }
    }

    fn refused_level(reason: &str, supported: serde_json::Value) -> serde_json::Value {
        let facts = if supported.is_null() {
            serde_json::Value::Null
        } else {
            serde_json::json!("catalog")
        };
        serde_json::json!({
            "requested": "max",
            "provider": "anthropic",
            "model": "claude-opus-4-5",
            "route": {"backend": "anthropic_api", "api": "messages"},
            "reason": reason,
            "supported": supported,
            "facts": facts,
        })
    }

    fn refused_build(refusal: &serde_json::Value) -> anyhow::Error {
        let refusal: meerkat_core::ReasoningEffortRefusal =
            serde_json::from_value(refusal.clone()).unwrap();
        crate::session_err_to_anyhow(SessionError::Agent(AgentError::ReasoningEffortRefused(
            Box::new(refusal),
        )))
    }

    /// A refused reasoning effort level exits 2 and gives one failure
    /// document: `INVALID_PARAMS` with the typed refusal as its details and
    /// the stderr text as its message.
    #[test]
    fn a_refused_reasoning_effort_level_prints_its_document_and_exits_2() {
        let refusal = refused_level(
            "unsupported_level",
            serde_json::json!(["low", "medium", "high"]),
        );
        let error = refused_build(&refusal);
        assert_eq!(failure_exit_status(&error), 2);
        assert_eq!(failure_error_code(&error), Some(ErrorCode::InvalidParams));
        assert_eq!(
            reasoning_effort_refusal_document(&error),
            Some(serde_json::json!({
                "error": {
                    "code": "INVALID_PARAMS",
                    "category": "validation",
                    "message": error.to_string(),
                    "details": {
                        "kind": "reasoning_effort_refused",
                        "retryable": false,
                        "reasoning_effort_refusal": refusal,
                    },
                }
            }))
        );
        // No other failure has this document.
        for other in [
            crate::session_err_to_anyhow(SessionError::Busy {
                id: SessionId::new(),
            }),
            session_not_found(&SessionId::new()),
            anyhow::anyhow!("--comms-listen-tcp cannot be used with --no-comms"),
        ] {
            assert_eq!(reasoning_effort_refusal_document(&other), None, "{other}");
        }
    }

    /// Only the reason that can pass later with no configuration change is
    /// marked retryable; the exit status is 2 for every reason.
    #[test]
    fn the_transient_refusal_is_marked_on_the_cli() {
        for (reason, retryable) in [
            ("supported_levels_unavailable", true),
            ("supported_levels_unknown", false),
        ] {
            let error = refused_build(&refused_level(reason, serde_json::Value::Null));
            let document = reasoning_effort_refusal_document(&error).unwrap();
            let details = &document["error"]["details"];
            assert_eq!(details["retryable"], retryable, "{reason}");
            assert_eq!(details["reasoning_effort_refusal"]["reason"], reason);
            assert!(details["reasoning_effort_refusal"]["supported"].is_null());
            assert!(details["reasoning_effort_refusal"]["facts"].is_null());
            assert_eq!(failure_exit_status(&error), 2, "{reason}");
        }
    }

    /// A refusal that names no route renders: `route` is `null` in the
    /// document and the message leaves the route out.
    #[test]
    fn a_refusal_without_a_route_renders() {
        let mut refusal = refused_level("supported_levels_unknown", serde_json::Value::Null);
        refusal["route"] = serde_json::Value::Null;
        let error = refused_build(&refusal);
        let document = reasoning_effort_refusal_document(&error).unwrap();
        let details = &document["error"]["details"];
        assert!(details["reasoning_effort_refusal"]["route"].is_null());
        assert_eq!(details["reasoning_effort_refusal"], refusal);
        let message = document["error"]["message"].as_str().unwrap();
        assert!(!message.contains(" on "), "{message}");
        assert_eq!(failure_exit_status(&error), 2);
    }

    /// A turn that ended because the level was refused before a provider
    /// request fails exactly as a refused build does: the same typed owner,
    /// exit status 2 and the same failure document details.
    #[test]
    fn a_refusal_inside_a_turn_has_the_build_time_shape() {
        let refusal = refused_level("unsupported_level", serde_json::json!(["low", "medium"]));
        let typed: meerkat_core::ReasoningEffortRefusal =
            serde_json::from_value(refusal.clone()).unwrap();
        let mut turn_error = meerkat_core::TurnErrorMetadata::terminal(
            meerkat_core::TurnTerminalCauseKind::FatalFailure,
            meerkat_core::TurnTerminalOutcome::Failed,
            "reasoning effort refused",
        );
        turn_error.reason =
            Some(meerkat_core::event::AgentErrorReason::ReasoningEffortRefused { refusal: typed });
        let in_turn = match crate::completion_outcome_to_cli_runtime_turn_result(
            meerkat_runtime::completion::CompletionOutcome::AbandonedWithError {
                reason: "reasoning effort refused".to_string(),
                error: turn_error,
            },
            &SessionId::new(),
            &meerkat_core::connection::RealmId::parse("cli-exit").unwrap(),
            true,
        ) {
            Err(error) => error,
            Ok(_) => panic!("an abandoned completion is a failure"),
        };
        let at_build = refused_build(&refusal);
        assert_eq!(failure_exit_status(&in_turn), 2);
        assert_eq!(failure_error_code(&in_turn), failure_error_code(&at_build));
        let in_turn_document = reasoning_effort_refusal_document(&in_turn).unwrap();
        let at_build_document = reasoning_effort_refusal_document(&at_build).unwrap();
        assert_eq!(
            in_turn_document["error"]["details"],
            at_build_document["error"]["details"]
        );
        assert_eq!(in_turn_document["error"]["code"], "INVALID_PARAMS");
        assert_eq!(in_turn_document["error"]["message"], in_turn.to_string());
        assert_eq!(in_turn.to_string(), at_build.to_string());
    }

    /// Joining a shutdown failure onto an earlier failure keeps the earlier
    /// failure's typed owner, under the same combined message.
    #[test]
    fn shutdown_failures_keep_the_primary_typed_owner() {
        let mut slot = None;
        crate::accumulate_anyhow_error(
            &mut slot,
            anyhow::Error::new(SessionError::Busy {
                id: SessionId::new(),
            }),
        );
        crate::accumulate_anyhow_error(&mut slot, anyhow::anyhow!("drain failed"));
        let combined = slot.expect("both failures are kept");
        assert!(
            combined
                .to_string()
                .ends_with("; additionally failed during shutdown: drain failed"),
            "{combined}"
        );
        assert_eq!(failure_exit_status(&combined), 11);
    }
}

#[cfg(all(test, feature = "mob"))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod mob_tests {
    use super::*;

    fn scope_denied() -> meerkat_mob::MobError {
        meerkat_mob::MobError::ScopeDenied(meerkat_mob::ScopeDenial {
            required: meerkat_mob::machines::mob_machine::ControlScope::AdminHost,
            presented: std::collections::BTreeSet::from([
                meerkat_mob::machines::mob_machine::ControlScope::List,
            ]),
        })
    }

    fn member_reload_required() -> meerkat_mob::MobError {
        meerkat_mob::MobError::MemberReloadRequired {
            member_id: meerkat_mob::AgentIdentity::from("worker"),
            reason: "durability degraded".into(),
        }
    }

    /// Mob-family errors exit with their owner's `wire_error_code()`, not
    /// only the console detail codes; a mob error without a public code
    /// exits 1, even with a typed session error under it.
    #[test]
    fn mob_errors_exit_with_their_owner_code() {
        let worker = meerkat_mob::AgentIdentity::from("worker");
        let cases = [
            (
                crate::mob_anyhow(member_reload_required()),
                Some(ErrorCode::MemberReloadRequired),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::DeliveryScopeUnavailable {
                    agent_identity: worker.clone(),
                    reason: "no session binding".into(),
                }),
                Some(ErrorCode::CapabilityUnavailable),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::SessionError(
                    meerkat_core::SessionError::HostingUnavailable {
                        id: meerkat_core::SessionId::new(),
                    },
                )),
                Some(ErrorCode::SessionRuntimeUnavailable),
            ),
            (
                crate::mob_anyhow(scope_denied()),
                Some(ErrorCode::ScopeDenied),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::BridgeRequestTimedOut {
                    request_envelope_id: "env".to_string(),
                    timeout_ms: 30_000,
                }),
                Some(ErrorCode::HostUnavailable),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::StaleEventCursor {
                    after_cursor: 9,
                    latest_cursor: 4,
                }),
                Some(ErrorCode::StaleCursor),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::StaleFenceToken {
                    runtime_id: meerkat_mob::AgentRuntimeId::initial(worker.clone()),
                    expected: meerkat_mob::FenceToken::new(2),
                    actual: meerkat_mob::FenceToken::new(1),
                }),
                Some(ErrorCode::StaleFence),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::ForkInstructionRefused {
                    source_member_id: worker.clone(),
                    refusal: meerkat_core::ForkInstructionRefusal::new(
                        meerkat_core::ForkInstructionRefusalCause::PolicyRefuses,
                        Vec::new(),
                    ),
                }),
                Some(ErrorCode::ForkInstructionRefused),
            ),
            (
                anyhow::Error::new(meerkat_core::SessionError::ForkInstructionRefused(
                    meerkat_core::ForkInstructionRefusal::new(
                        meerkat_core::ForkInstructionRefusalCause::PolicyRefuses,
                        Vec::new(),
                    ),
                )),
                Some(ErrorCode::ForkInstructionRefused),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::StaleDeliveryScope {
                    agent_identity: worker,
                    expected_session: meerkat_core::SessionId::new(),
                    actual_session: None,
                }),
                Some(ErrorCode::StaleDeliveryScope),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobRespawnError::Mob(member_reload_required())),
                Some(ErrorCode::MemberReloadRequired),
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::MobNotFound(
                    meerkat_mob::MobId::from("missing"),
                )),
                None,
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::CommsError(
                    meerkat_core::comms::SendError::PeerOffline,
                )),
                None,
            ),
            (
                crate::mob_anyhow(meerkat_mob::MobError::SessionError(
                    meerkat_core::SessionError::NotFound {
                        id: meerkat_core::SessionId::new(),
                    },
                )),
                None,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(failure_error_code(&error), expected, "{error}");
            assert_eq!(
                failure_exit_status(&error),
                expected.map_or(crate::EXIT_ERROR, code_exit_status),
                "{error}"
            );
        }

        // The reported example: MEMBER_RELOAD_REQUIRED exits 49.
        assert_eq!(
            failure_exit_status(&crate::mob_anyhow(member_reload_required())),
            49
        );
    }

    /// The `detail:` line renders the deciding owner's typed pair, not a
    /// display string, and an error without a console detail prints none.
    #[test]
    fn detail_line_comes_from_the_deciding_mob_owner() {
        let detail = deciding_mob_wire_detail(&crate::mob_anyhow(scope_denied()))
            .expect("scope denial detail");
        assert_eq!(
            detail.detail_value().expect("detail serializes"),
            serde_json::json!({ "required": "admin_host", "presented": ["list"] })
        );
        assert!(deciding_mob_wire_detail(&crate::mob_anyhow(member_reload_required())).is_none());
        assert!(deciding_mob_wire_detail(&anyhow::anyhow!("boom")).is_none());
    }
}
