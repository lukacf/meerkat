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
//!
//! `rkat run --output json` also writes the failure as a document on stdout
//! ([`failure_document`]): the same typed owner's code and wire data in the
//! [`WireError`] envelope. A turn that ended in failure ([`FailedTurn`])
//! carries its typed turn error there under the shared turn-failure code,
//! and still exits [`EXIT_ERROR`](crate::EXIT_ERROR): the document's code is
//! never the exit status of a failed turn.

use meerkat_contracts::error::{
    agent_transport_error_code, session_error_details, session_transport_error_code,
    turn_failure_error_code,
};
use meerkat_contracts::{ErrorCode, WireError, WireRunFailure};

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

/// A turn that ended in failure: the runtime's reason text and the typed
/// turn error it completed the turn with.
///
/// It is a typed owner without an exit code of its own: the command exits
/// [`EXIT_ERROR`](crate::EXIT_ERROR) whatever the turn's cause, and the
/// failure document reads the cause's code from the typed error.
#[derive(Debug)]
pub(crate) struct FailedTurn {
    reason: String,
    error: meerkat_core::TurnErrorMetadata,
}

impl FailedTurn {
    pub(crate) fn new(reason: String, error: meerkat_core::TurnErrorMetadata) -> Self {
        Self { reason, error }
    }
}

impl std::fmt::Display for FailedTurn {
    /// The text stderr has always shown for a failed turn: the reason, then
    /// the typed error as JSON.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "turn abandoned: {}; error=", self.reason)?;
        match serde_json::to_string(&self.error) {
            Ok(json) => f.write_str(&json),
            Err(_) => f.write_str("<unserializable>"),
        }
    }
}

impl std::error::Error for FailedTurn {}

/// Whether a failed command also writes its failure document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureDocument {
    /// stderr only: every command and output mode but the one below.
    None,
    /// `rkat run` with JSON output: the document is the last one on stdout.
    JsonStdout,
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

/// The failure document of a failed command: the first typed owner in
/// `error`'s chain decides its code and details, as it decides the exit
/// status, and the message is the text stderr shows.
///
/// - A failed turn: the shared turn-failure code of its cause kind, and the
///   typed turn error as `details.error`.
/// - Another typed owner with a public code: that code, and the owner's wire
///   data when it has any, as the other surfaces report it (a session
///   error's data, a refused or not-ready input's admission detail, a
///   hosting or teardown refusal's session data, a refused help-session
///   resume's data, a mob error's typed console detail).
/// - A typed owner without a public code, and a failure with no typed owner:
///   `INTERNAL_ERROR`, no details.
pub(crate) fn failure_document(error: &anyhow::Error) -> WireRunFailure {
    let owner = error
        .chain()
        .find_map(|cause| owner_code(cause).map(|code| (cause, code)));
    let (code, details) = match owner {
        Some((cause, code)) => match cause.downcast_ref::<FailedTurn>() {
            Some(turn) => (
                turn_failure_error_code(&turn.error),
                serde_json::to_value(&turn.error)
                    .ok()
                    .map(|turn_error| serde_json::json!({ "error": turn_error })),
            ),
            None => match code {
                OwnerCode::Public(code) => (code, owner_details(cause)),
                OwnerCode::Uncoded => (ErrorCode::InternalError, None),
            },
        },
        None => (ErrorCode::InternalError, None),
    };
    let mut wire = WireError::new(code, error.to_string());
    wire.details = details;
    WireRunFailure { error: wire }
}

/// The wire data of the typed owner that decided the code, read from the
/// projection the other surfaces report the same error with: a session
/// error's data, an agent error's refused reasoning effort level (the data
/// of the session error the other surfaces wrap it in), a runtime driver
/// error's native admission detail or the data of its teardown or hosting
/// session error, a refused help-session resume's data, and a mob error's
/// typed console detail. No data is built here, so the document holds what
/// those surfaces hold and nothing more.
fn owner_details(owner: &(dyn std::error::Error + 'static)) -> Option<serde_json::Value> {
    if let Some(error) = owner.downcast_ref::<meerkat_core::SessionError>() {
        return session_error_details(error);
    }
    // This command reports a session's agent error as the agent error
    // itself, so a refused reasoning effort level arrives here without its
    // session error. Its details are that session error's own projection;
    // the variant is named because an agent error cannot be cloned into one
    // generically. An agent error whose session projection carries data and
    // that has no arm here prints no details (a refused tool grant gate; see
    // the row `agent_errors_with_wire_data_have_a_failure_document_decision`).
    if let Some(meerkat_core::AgentError::ReasoningEffortRefused(refusal)) =
        owner.downcast_ref::<meerkat_core::AgentError>()
    {
        return session_error_details(&meerkat_core::SessionError::Agent(
            meerkat_core::AgentError::ReasoningEffortRefused(refusal.clone()),
        ));
    }
    if let Some(error) = owner.downcast_ref::<meerkat_runtime::RuntimeDriverError>() {
        return runtime_driver_error_details(error);
    }
    if let Some(refusal) = owner.downcast_ref::<meerkat::help::HelpSessionNotResumable>() {
        return Some(refusal.data());
    }
    #[cfg(feature = "mob")]
    if let Some(detail) = mob_wire_detail(owner) {
        return detail.detail_value().ok();
    }
    None
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
                meerkat_core::AgentError::ReasoningEffortRefused(refusal.clone()),
            ))
        }
        _ => None,
    }
}

/// Write the failure document of `error` as one JSON document on stdout,
/// when `document` asks for it.
pub(crate) fn write_failure_document(error: &anyhow::Error, document: FailureDocument) {
    if document != FailureDocument::JsonStdout {
        return;
    }
    match serde_json::to_string_pretty(&failure_document(error)) {
        Ok(json) => println!("{json}"),
        Err(render) => eprintln!("Warning: failed to render the failure document: {render}"),
    }
}

/// Report a failed command on stderr and return its exit status. A mob
/// error that carries a typed console detail also prints one `detail:` line
/// rendered from the typed fields, never from a display string. After the
/// stderr lines, `document` decides whether the failure document follows on
/// stdout.
pub(crate) fn report_failure(
    error: &anyhow::Error,
    document: FailureDocument,
) -> std::process::ExitCode {
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
    write_failure_document(error, document);
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
    // A failed turn exits with the generic status whatever its cause kind.
    if cause.downcast_ref::<FailedTurn>().is_some() {
        return Some(OwnerCode::Uncoded);
    }
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
    runtime_driver_session_error(error)
        .map(|session_error| session_transport_error_code(&session_error))
}

/// The wire data of the runtime classes that carry a public code: the
/// details of the shared native admission envelope, or the data of the
/// teardown or hosting session error the code is read from.
fn runtime_driver_error_details(
    error: &meerkat_runtime::RuntimeDriverError,
) -> Option<serde_json::Value> {
    if let Some(detail) = meerkat::surface::native_admission_error_detail(error) {
        return detail.to_wire_error().ok().and_then(|wire| wire.details);
    }
    runtime_driver_session_error(error)
        .and_then(|session_error| session_error_details(&session_error))
}

/// The typed session error of a teardown still in progress or of a session
/// this runtime owner may not host.
fn runtime_driver_session_error(
    error: &meerkat_runtime::RuntimeDriverError,
) -> Option<meerkat_core::SessionError> {
    error
        .teardown_in_progress_session_error()
        .or_else(|| error.hosting_session_error())
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
    mob_wire_detail(owner)
}

/// The typed console detail of a mob-family owner.
#[cfg(feature = "mob")]
fn mob_wire_detail(
    owner: &(dyn std::error::Error + 'static),
) -> Option<meerkat_contracts::wire::WireMobErrorDetail> {
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

    /// The typed owners of the run and resume paths each give a failure
    /// document whose code is their public code, with the stderr text as its
    /// message, and keep their exit status.
    #[test]
    fn a_typed_failure_before_the_turn_prints_its_code() {
        let id = SessionId::new();
        let cases = [
            (
                crate::runtime_accept_failure(
                    RuntimeDriverError::InputRefused {
                        refusal: meerkat_core::OperationRefused::new(
                            meerkat_core::OperationRefusalKind::Denied,
                        ),
                    },
                    &Some("grant the route".to_string()),
                ),
                "INPUT_REFUSED",
                50,
            ),
            (
                crate::runtime_accept_failure(
                    RuntimeDriverError::ControllerReadinessUnavailable {
                        reason: meerkat_core::authorization::ControllerReadinessFailure::Busy,
                    },
                    &None,
                ),
                "INPUT_NOT_READY",
                51,
            ),
            (
                crate::session_err_to_anyhow(SessionError::Busy { id: id.clone() }),
                "SESSION_BUSY",
                11,
            ),
            (
                crate::session_err_to_anyhow(SessionError::Agent(AgentError::BuildError(
                    "no provider key".into(),
                ))),
                "PROVIDER_ERROR",
                20,
            ),
            (session_not_found(&id), "SESSION_NOT_FOUND", 10),
            (
                typed_failure(AgentError::Cancelled, |_| "request cancelled".to_string()),
                "REQUEST_CANCELLED",
                14,
            ),
        ];
        for (error, code, status) in cases {
            let document = serde_json::to_value(failure_document(&error)).unwrap();
            assert_eq!(document["error"]["code"], code, "{error}");
            assert_eq!(document["error"]["message"], error.to_string(), "{error}");
            let expected = failure_error_code(&error).expect("a typed owner with a code");
            assert_eq!(
                document["error"]["category"],
                serde_json::json!(expected.category()),
                "{error}"
            );
            assert_eq!(failure_exit_status(&error), status, "{error}");
        }
    }

    /// A typed session error's wire data is the document's details.
    #[test]
    fn a_session_error_document_carries_its_wire_data() {
        let id = SessionId::new();
        let error = crate::session_err_to_anyhow(SessionError::ServedElsewhere { id: id.clone() });
        let document = serde_json::to_value(failure_document(&error)).unwrap();
        assert_eq!(document["error"]["code"], "SESSION_BUSY");
        assert_eq!(
            document["error"]["details"],
            serde_json::json!({
                "kind": "session_served_elsewhere",
                "session_id": id.to_string(),
            })
        );
        // An error without wire data has no details key at all.
        let plain = serde_json::to_value(failure_document(&session_not_found(&id))).unwrap();
        assert!(plain["error"].get("details").is_none(), "{plain}");
    }

    /// A refused or not-ready input's document carries the admission detail
    /// the shared projection gives the other surfaces for it, and nothing
    /// else: a caller can tell a changed policy from an unusable credential
    /// without reading text. The route hint stays in the message.
    #[test]
    fn a_refused_or_not_ready_input_document_carries_its_admission_detail() {
        use meerkat_core::OperationRefusalKind as Refusal;
        use meerkat_core::authorization::ControllerReadinessFailure as Readiness;
        let refused = |kind| RuntimeDriverError::InputRefused {
            refusal: meerkat_core::OperationRefused::new(kind),
        };
        let not_ready = |reason| RuntimeDriverError::ControllerReadinessUnavailable { reason };
        let policy_changed = serde_json::json!({
            "type": "not_ready",
            "reason": { "kind": "policy_changed" },
        });
        let cases = [
            (
                refused(Refusal::Denied),
                "INPUT_REFUSED",
                serde_json::json!({ "type": "refused", "kind": "denied" }),
            ),
            (
                refused(Refusal::MalformedFacts),
                "INPUT_REFUSED",
                serde_json::json!({ "type": "refused", "kind": "malformed_facts" }),
            ),
            (
                refused(Refusal::ReprepareRequired),
                "INPUT_NOT_READY",
                policy_changed.clone(),
            ),
            (
                not_ready(Readiness::PolicyChanged),
                "INPUT_NOT_READY",
                policy_changed,
            ),
            (
                not_ready(Readiness::CredentialUnusable {
                    disposition: meerkat_core::handles::CredentialUseDisposition::ReauthRequired,
                }),
                "INPUT_NOT_READY",
                serde_json::json!({
                    "type": "not_ready",
                    "reason": {
                        "kind": "credential_unusable",
                        "disposition": "reauth_required",
                    },
                }),
            ),
        ];
        for (error, code, details) in cases {
            let shared = meerkat::surface::native_admission_error_detail(&error)
                .expect("a native admission error")
                .to_wire_error()
                .expect("the detail serializes")
                .details
                .expect("the shared envelope has details");
            let failure =
                crate::runtime_accept_failure(error, &Some("grant the route".to_string()));
            let document = serde_json::to_value(failure_document(&failure)).unwrap();
            assert_eq!(document["error"]["code"], code, "{failure}");
            assert_eq!(document["error"]["details"], details, "{failure}");
            assert_eq!(document["error"]["details"], shared, "{failure}");
            assert_eq!(
                document["error"]["message"],
                failure.to_string(),
                "{failure}"
            );
        }
    }

    /// A session another runtime owner hosts, a hosting claim that is
    /// unavailable and a teardown still in progress give the document the
    /// data of the session error their code is read from, as JSON-RPC and
    /// REST report them.
    #[test]
    fn a_hosting_or_teardown_refusal_document_carries_its_session_data() {
        let id = SessionId::new();
        let runtime_id = || meerkat_runtime::identifiers::LogicalRuntimeId::new("runtime-1");
        let teardown = |name: &str| {
            serde_json::json!({
                "kind": "runtime_teardown_in_progress",
                "code": "SESSION_BUSY",
                "teardown": name,
                "runtime_id": "runtime-1",
                "retryable": true,
                "authority_retained": true,
            })
        };
        let cases = [
            (
                RuntimeDriverError::ServedElsewhere {
                    session_id: id.clone(),
                },
                "SESSION_BUSY",
                serde_json::json!({
                    "kind": "session_served_elsewhere",
                    "session_id": id.to_string(),
                }),
            ),
            (
                RuntimeDriverError::HostingUnavailable {
                    session_id: id.clone(),
                },
                "SESSION_RUNTIME_UNAVAILABLE",
                serde_json::json!({
                    "kind": "session_hosting_unavailable",
                    "session_id": id.to_string(),
                }),
            ),
            (
                RuntimeDriverError::UnregisterInProgress {
                    runtime_id: runtime_id(),
                },
                "SESSION_BUSY",
                teardown("unregister"),
            ),
            (
                RuntimeDriverError::RuntimeStopInProgress {
                    runtime_id: runtime_id(),
                },
                "SESSION_BUSY",
                teardown("stop"),
            ),
        ];
        for (error, code, details) in cases {
            let session_error = error
                .teardown_in_progress_session_error()
                .or_else(|| error.hosting_session_error())
                .expect("a hosting or teardown error");
            let shared = session_error_details(&session_error).expect("typed session data");
            let failure = typed_failure(error, |error| {
                format!("runtime executor registration failed: {error}")
            });
            let document = serde_json::to_value(failure_document(&failure)).unwrap();
            assert_eq!(document["error"]["code"], code, "{failure}");
            assert_eq!(document["error"]["details"], details, "{failure}");
            assert_eq!(document["error"]["details"], shared, "{failure}");
            assert_eq!(
                failure_exit_status(&failure),
                code_exit_status(failure_error_code(&failure).expect("a public code")),
                "{failure}"
            );
        }
    }

    /// A refused help-session resume gives the document the refusal's own
    /// data, as the other surfaces report it.
    #[test]
    fn a_refused_help_session_resume_document_carries_its_data() {
        let id = SessionId::new();
        let refusal = meerkat::help::HelpSessionNotResumable {
            session_id: id.clone(),
        };
        let error = anyhow::Error::new(refusal.clone());
        let document = serde_json::to_value(failure_document(&error)).unwrap();
        assert_eq!(document["error"]["code"], "SESSION_NOT_RUNNING");
        assert_eq!(
            document["error"]["details"],
            serde_json::json!({
                "kind": "help_session_not_resumable",
                "session_id": id.to_string(),
            })
        );
        assert_eq!(document["error"]["details"], refusal.data());
        assert_eq!(document["error"]["message"], error.to_string());
        assert_eq!(failure_exit_status(&error), 12);
    }

    /// A failure with no typed owner, and a typed owner without a public
    /// code, print `INTERNAL_ERROR` with no details and exit 1.
    #[test]
    fn an_untyped_failure_prints_internal_error_and_exits_1() {
        for error in [
            anyhow::anyhow!("--comms-listen-tcp cannot be used with --no-comms"),
            anyhow::Error::new(RuntimeDriverError::Destroyed),
        ] {
            let document = serde_json::to_value(failure_document(&error)).unwrap();
            assert_eq!(
                document,
                serde_json::json!({
                    "error": {
                        "code": "INTERNAL_ERROR",
                        "category": "internal",
                        "message": error.to_string(),
                    }
                }),
                "{error}"
            );
            assert_eq!(failure_exit_status(&error), crate::EXIT_ERROR, "{error}");
        }
    }

    fn abandoned_turn(reason: &str, error: meerkat_core::TurnErrorMetadata) -> anyhow::Error {
        match crate::completion_outcome_to_cli_runtime_turn_result(
            meerkat_runtime::completion::CompletionOutcome::AbandonedWithError {
                reason: reason.to_string(),
                error,
            },
            &SessionId::new(),
            &meerkat_core::connection::RealmId::parse("cli-exit").unwrap(),
            true,
        ) {
            Err(error) => error,
            Ok(_) => panic!("an abandoned completion is a failure"),
        }
    }

    /// A failed turn's document carries the code of its cause kind (the
    /// shared turn-failure mapping) and the typed turn error as
    /// `details.error`; its stderr text is what it was before the error was
    /// typed, and it still exits 1 whatever the document's code.
    #[test]
    fn a_failed_turn_prints_its_cause_code_and_exits_1() {
        use meerkat_core::{TurnTerminalCauseKind as Kind, TurnTerminalOutcome as Outcome};
        for (kind, outcome, code) in [
            (Kind::LlmFailure, Outcome::Failed, "PROVIDER_ERROR"),
            (Kind::RetryExhausted, Outcome::Failed, "PROVIDER_ERROR"),
            (
                Kind::TimeBudgetExceeded,
                Outcome::TimeBudgetExceeded,
                "BUDGET_EXHAUSTED",
            ),
            (Kind::HookDenied, Outcome::Failed, "HOOK_DENIED"),
            (Kind::TurnLimitReached, Outcome::Failed, "INTERNAL_ERROR"),
            (Kind::ToolFailure, Outcome::Failed, "INTERNAL_ERROR"),
        ] {
            let mut turn_error =
                meerkat_core::TurnErrorMetadata::terminal(kind, outcome, "provider said no");
            turn_error.retryable = Some(kind == Kind::LlmFailure);
            let error = abandoned_turn("turn failed", turn_error.clone());
            let text = format!(
                "turn abandoned: turn failed; error={}",
                serde_json::to_string(&turn_error).unwrap()
            );
            assert_eq!(error.to_string(), text, "{kind:?}");

            let document = serde_json::to_value(failure_document(&error)).unwrap();
            assert_eq!(document["error"]["code"], code, "{kind:?}");
            assert_eq!(
                document["error"]["code"],
                serde_json::json!(meerkat_contracts::error::turn_failure_error_code(
                    &turn_error
                )),
                "{kind:?}"
            );
            assert_eq!(document["error"]["message"], text, "{kind:?}");
            assert_eq!(
                document["error"]["details"],
                serde_json::json!({ "error": turn_error }),
                "{kind:?}"
            );
            assert_eq!(failure_exit_status(&error), crate::EXIT_ERROR, "{kind:?}");
            assert_eq!(failure_error_code(&error), None, "{kind:?}");

            // A shutdown failure joined onto it keeps the typed turn error.
            let mut slot = Some(error);
            crate::accumulate_anyhow_error(&mut slot, anyhow::anyhow!("drain failed"));
            let joined = slot.expect("both failures are kept");
            let joined_document = serde_json::to_value(failure_document(&joined)).unwrap();
            assert_eq!(joined_document["error"]["code"], code, "{kind:?}");
            assert_eq!(
                joined_document["error"]["details"],
                serde_json::json!({ "error": turn_error }),
                "{kind:?}"
            );
            assert_eq!(failure_exit_status(&joined), crate::EXIT_ERROR, "{kind:?}");
        }
    }

    /// A turn abandoned because its session's build refused a scoped tool
    /// grant gate stays a failed turn on the CLI: exit status 1 and the
    /// turn-failure document, with the typed refusal inside the turn error.
    /// (JSON-RPC answers the same turn as an invalid-params refusal.)
    #[test]
    fn a_refused_grant_gate_turn_stays_a_failed_turn() {
        let mut turn_error = meerkat_core::TurnErrorMetadata::terminal(
            meerkat_core::TurnTerminalCauseKind::FatalFailure,
            meerkat_core::TurnTerminalOutcome::Failed,
            "grant gate refused",
        );
        turn_error.reason = Some(
            meerkat_core::event::AgentErrorReason::ToolGrantGateRefused {
                refusal: meerkat_core::tool_grant::GrantGateRefusal::NoGrantService,
            },
        );
        let error = abandoned_turn("grant gate refused", turn_error.clone());
        let document = serde_json::to_value(failure_document(&error)).unwrap();
        assert_eq!(document["error"]["code"], "INTERNAL_ERROR");
        assert_eq!(
            document["error"]["details"],
            serde_json::json!({ "error": turn_error })
        );
        assert_eq!(failure_exit_status(&error), crate::EXIT_ERROR);
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

    /// The failure document of `error`, as the JSON `rkat run` writes.
    fn failure_document_json(error: &anyhow::Error) -> serde_json::Value {
        serde_json::to_value(failure_document(error)).unwrap()
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
            failure_document_json(&error),
            serde_json::json!({
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
            })
        );
        // No other failure's document carries the refusal.
        for other in [
            crate::session_err_to_anyhow(SessionError::Busy {
                id: SessionId::new(),
            }),
            session_not_found(&SessionId::new()),
            anyhow::anyhow!("--comms-listen-tcp cannot be used with --no-comms"),
        ] {
            let document = failure_document_json(&other);
            assert_ne!(
                document["error"]["details"]["kind"], "reasoning_effort_refused",
                "{other}"
            );
            assert!(
                document["error"]["details"]
                    .get("reasoning_effort_refusal")
                    .is_none(),
                "{other}"
            );
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
            let document = failure_document_json(&error);
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
        let document = failure_document_json(&error);
        let details = &document["error"]["details"];
        assert!(details["reasoning_effort_refusal"]["route"].is_null());
        assert_eq!(details["reasoning_effort_refusal"], refusal);
        let message = document["error"]["message"].as_str().unwrap();
        assert!(!message.contains(" on "), "{message}");
        assert_eq!(failure_exit_status(&error), 2);
    }

    /// A turn that ended because the level was refused before a provider
    /// request fails exactly as a refused build does: the same typed owner,
    /// exit status 2 and the same failure document, never the failed-turn
    /// document of another cause.
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
        turn_error.reason = Some(
            meerkat_core::event::AgentErrorReason::ReasoningEffortRefused {
                refusal: Box::new(typed),
            },
        );
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
        let in_turn_document = failure_document_json(&in_turn);
        let at_build_document = failure_document_json(&at_build);
        assert_eq!(
            in_turn_document["error"]["details"],
            at_build_document["error"]["details"]
        );
        assert_eq!(in_turn_document["error"]["code"], "INVALID_PARAMS");
        assert_eq!(in_turn_document["error"]["message"], in_turn.to_string());
        assert_eq!(in_turn.to_string(), at_build.to_string());
    }

    /// Every agent error whose session error carries wire data on the other
    /// surfaces has a decision here, because this command reports the agent
    /// error without its session error: the document carries that data, or
    /// it prints no details on purpose. The two below are every such agent
    /// error (meerkat-core's row
    /// `the_agent_errors_with_wire_data_are_the_listed_ones` names them and
    /// does not compile when a variant is added without being placed).
    #[test]
    fn agent_errors_with_wire_data_have_a_failure_document_decision() {
        // A refused reasoning effort level: the session projection's data.
        let refusal: meerkat_core::ReasoningEffortRefusal = serde_json::from_value(refused_level(
            "unsupported_level",
            serde_json::json!(["low", "medium"]),
        ))
        .unwrap();
        let projection = session_error_details(&SessionError::Agent(
            AgentError::ReasoningEffortRefused(Box::new(refusal.clone())),
        ));
        assert!(projection.is_some());
        let bare = anyhow::Error::from(AgentError::ReasoningEffortRefused(Box::new(refusal)));
        assert_eq!(failure_document(&bare).error.details, projection);

        // A refused scoped tool grant gate: no details. Its session error
        // carries data, and this command does not print it for the bare
        // agent error.
        let gate = || {
            AgentError::ToolGrantGateRefused(Box::new(
                meerkat_core::tool_grant::GrantGateRefusal::NoGrantService,
            ))
        };
        assert!(session_error_details(&SessionError::Agent(gate())).is_some());
        assert_eq!(
            failure_document(&anyhow::Error::from(gate())).error.details,
            None
        );
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

    /// A mob error's failure document carries its public code and, when it
    /// has a typed console detail, that detail as the document's details.
    #[test]
    fn a_mob_error_document_carries_its_typed_detail() {
        let denied = crate::mob_anyhow(scope_denied());
        let document = serde_json::to_value(failure_document(&denied)).unwrap();
        assert_eq!(document["error"]["code"], "SCOPE_DENIED");
        assert_eq!(
            document["error"]["details"],
            serde_json::json!({ "required": "admin_host", "presented": ["list"] })
        );
        assert_eq!(failure_exit_status(&denied), 45);

        let reload = crate::mob_anyhow(member_reload_required());
        let document = serde_json::to_value(failure_document(&reload)).unwrap();
        assert_eq!(document["error"]["code"], "MEMBER_RELOAD_REQUIRED");
        assert!(document["error"].get("details").is_none(), "{document}");
        assert_eq!(failure_exit_status(&reload), 49);
    }
}
