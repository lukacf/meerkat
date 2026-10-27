//! Scope-bound internal dispatch: the host-persisted [`DeliveryScope`], its
//! typed refusals, and what a scoped recovery established.
//!
//! A host captures an identity's exact delivery scope (Meta continuity plus
//! meerkat's native member scope), persists it, then dispatches against it.
//! The scoped path never materializes, repairs, retargets or prepares the
//! delivery: a scope that no longer matches is refused typed, and a lost
//! reply is recovered from the ORIGINAL session's ledger with the same scope.

use serde::{Deserialize, Serialize};

use super::types::{AgentIdentity, AgentRuntimeId, ContinuityGeneration, FencingToken, TurnOutput};

/// Serialized format version of [`DeliveryScope`]. A persisted scope with any
/// other version is refused as [`DeliveryScopeError::UnsupportedVersion`],
/// never read as a guess.
pub const DELIVERY_SCOPE_VERSION: u64 = 1;

/// One identity's exact delivery scope: the Meta continuity it was captured
/// under (runtime id, generation, lease fencing token) and meerkat's native
/// member scope (runtime incarnation, fence and session).
///
/// A selector and stale-binding guard, not a permission: the scoped dispatch
/// re-validates every atom against the identity's current binding, and
/// meerkat validates the native scope inside its own admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryScope {
    identity: AgentIdentity,
    agent_runtime_id: AgentRuntimeId,
    generation: ContinuityGeneration,
    lease_fencing_token: FencingToken,
    member: meerkat_mob::MemberDeliveryScope,
}

impl DeliveryScope {
    pub(crate) fn new(
        identity: AgentIdentity,
        agent_runtime_id: AgentRuntimeId,
        generation: ContinuityGeneration,
        lease_fencing_token: FencingToken,
        member: meerkat_mob::MemberDeliveryScope,
    ) -> Self {
        Self {
            identity,
            agent_runtime_id,
            generation,
            lease_fencing_token,
            member,
        }
    }

    #[must_use]
    pub fn identity(&self) -> &AgentIdentity {
        &self.identity
    }

    #[must_use]
    pub fn agent_runtime_id(&self) -> &AgentRuntimeId {
        &self.agent_runtime_id
    }

    #[must_use]
    pub fn generation(&self) -> ContinuityGeneration {
        self.generation
    }

    #[must_use]
    pub fn lease_fencing_token(&self) -> FencingToken {
        self.lease_fencing_token
    }

    /// meerkat's native member scope.
    #[must_use]
    pub fn member(&self) -> &meerkat_mob::MemberDeliveryScope {
        &self.member
    }

    /// The member session the scope pins.
    #[must_use]
    pub fn session_id(&self) -> &meerkat_core::types::SessionId {
        self.member.session_id()
    }

    /// The versioned persisted form.
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "version": DELIVERY_SCOPE_VERSION,
            "identity": self.identity.as_str(),
            "agent_runtime_id": self.agent_runtime_id.as_str(),
            "generation": self.generation.get(),
            "lease_fencing_token": self.lease_fencing_token.get(),
            "member": self.member.to_json_value(),
        })
    }

    /// Decode a persisted scope. An unknown version, of this envelope or of
    /// the native member scope inside it, is a typed error.
    ///
    /// # Errors
    ///
    /// [`DeliveryScopeError`] when the value is not a scope this build reads.
    pub fn from_json_value(value: serde_json::Value) -> Result<Self, DeliveryScopeError> {
        #[derive(Deserialize)]
        struct VersionProbe {
            version: Option<u64>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireV1 {
            #[allow(dead_code)]
            version: u64,
            identity: AgentIdentity,
            agent_runtime_id: AgentRuntimeId,
            generation: ContinuityGeneration,
            lease_fencing_token: FencingToken,
            member: serde_json::Value,
        }
        let probe =
            VersionProbe::deserialize(&value).map_err(|error| DeliveryScopeError::Malformed {
                reason: error.to_string(),
            })?;
        match probe.version {
            Some(DELIVERY_SCOPE_VERSION) => {}
            Some(found) => return Err(DeliveryScopeError::UnsupportedVersion { found }),
            None => {
                return Err(DeliveryScopeError::Malformed {
                    reason: "missing version".to_string(),
                });
            }
        }
        let wire = WireV1::deserialize(value).map_err(|error| DeliveryScopeError::Malformed {
            reason: error.to_string(),
        })?;
        let member = meerkat_mob::MemberDeliveryScope::from_json_value(wire.member)
            .map_err(DeliveryScopeError::Member)?;
        Ok(Self {
            identity: wire.identity,
            agent_runtime_id: wire.agent_runtime_id,
            generation: wire.generation,
            lease_fencing_token: wire.lease_fencing_token,
            member,
        })
    }
}

impl Serialize for DeliveryScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_json_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DeliveryScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(value).map_err(serde::de::Error::custom)
    }
}

/// Why a persisted [`DeliveryScope`] could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliveryScopeError {
    /// Written with a scope format version this build does not read.
    UnsupportedVersion { found: u64 },
    /// Not a valid scope of its declared version.
    Malformed { reason: String },
    /// The native member scope inside it could not be decoded.
    Member(meerkat_mob::DeliveryScopeDecodeError),
}

impl std::fmt::Display for DeliveryScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { found } => {
                write!(f, "unsupported delivery scope version {found}")
            }
            Self::Malformed { reason } => write!(f, "malformed delivery scope: {reason}"),
            Self::Member(error) => write!(f, "delivery scope member: {error}"),
        }
    }
}

impl std::error::Error for DeliveryScopeError {}

impl DeliveryScopeError {
    /// Structured JSON-RPC `data` for this refusal.
    #[must_use]
    pub fn structured_data(&self) -> serde_json::Value {
        let unsupported_version = match self {
            Self::UnsupportedVersion { found }
            | Self::Member(meerkat_mob::DeliveryScopeDecodeError::UnsupportedVersion { found }) => {
                Some(*found)
            }
            _ => None,
        };
        serde_json::json!({
            "kind": "invalid_delivery_scope",
            "unsupported_version": unsupported_version,
        })
    }
}

/// Which atom of a saved [`DeliveryScope`] no longer matches the identity's
/// current binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeMismatch {
    /// The identity has no active continuity, or a different runtime id.
    RuntimeId,
    /// The identity's continuity generation moved (a reset or rebind).
    Generation,
    /// The identity runs under a different Core fence. Since 0.9.4 the
    /// scope's lease-token slot carries Core's fence, and Meta holds no
    /// lease.
    LeaseFencingToken,
    /// The native member scope does not name this identity's member.
    Member,
    /// The member's session, runtime incarnation or fence moved: meerkat
    /// refused the native scope at admission, or a scoped retirement found
    /// the identity bound to another session or member incarnation. Also a
    /// scope captured at another incarnation of the mob (one destroyed and
    /// created again under its id).
    MemberBinding,
}

impl ScopeMismatch {
    /// Stable wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeId => "runtime_id",
            Self::Generation => "generation",
            Self::LeaseFencingToken => "lease_fencing_token",
            Self::Member => "member",
            Self::MemberBinding => "member_binding",
        }
    }
}

/// A scoped dispatch or recovery that did not succeed.
///
/// `Unsupported`, `StaleScope` and `Rejected` are refusals before anything
/// was submitted. `Uncertain` is the only class whose admission fate is
/// unknown: recover it with the same scope, never by redispatching elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopedDeliveryError {
    /// The session bridge has no scoped seam (custom and remote bridges), or
    /// the member has no native session binding. Nothing was submitted.
    Unsupported { detail: String },
    /// The saved scope no longer matches. Nothing was submitted and nothing
    /// was retargeted to the current binding.
    StaleScope {
        mismatch: ScopeMismatch,
        detail: String,
    },
    /// Refused before admission (inactive identity, invalid delivery
    /// identity, lost lease, a typed meerkat refusal). Nothing was submitted.
    Rejected { detail: String },
    /// The admission round trip did not return an answer (timeout, actor
    /// stall or termination). The delivery may have been admitted.
    Uncertain { detail: String },
}

impl std::fmt::Display for ScopedDeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { detail } => write!(f, "scoped delivery unsupported: {detail}"),
            Self::StaleScope { mismatch, detail } => {
                write!(f, "stale delivery scope ({}): {detail}", mismatch.as_str())
            }
            Self::Rejected { detail } => write!(f, "scoped delivery rejected: {detail}"),
            Self::Uncertain { detail } => {
                write!(f, "scoped delivery outcome uncertain: {detail}")
            }
        }
    }
}

impl std::error::Error for ScopedDeliveryError {}

impl ScopedDeliveryError {
    /// Stable wire kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "scoped_delivery_unsupported",
            Self::StaleScope { .. } => "stale_delivery_scope",
            Self::Rejected { .. } => "scoped_delivery_rejected",
            Self::Uncertain { .. } => "scoped_delivery_uncertain",
        }
    }

    /// Whether the delivery may have been admitted.
    #[must_use]
    pub const fn admission_possible(&self) -> bool {
        matches!(self, Self::Uncertain { .. })
    }

    /// Structured JSON-RPC `data` for this error.
    #[must_use]
    pub fn structured_data(&self) -> serde_json::Value {
        let mut data = serde_json::json!({
            "kind": self.kind(),
            "admission_possible": self.admission_possible(),
        });
        if let (Self::StaleScope { mismatch, .. }, serde_json::Value::Object(fields)) =
            (self, &mut data)
        {
            fields.insert(
                "mismatch".to_string(),
                serde_json::Value::from(mismatch.as_str()),
            );
        }
        data
    }
}

/// Why a retirement at a scope
/// ([`super::IdentityRuntime::retire_at_scope_tracked`]) did not retire.
///
/// `Unsupported`, `StaleScope`, `Rejected` and `Refused` refuse to retire
/// the scope's member. Core retires the member's `spawned_by` descendants
/// first and checks the scope in every step, so a `StaleScope` or `Rejected`
/// Core reaches after its first step leaves the descendants it already
/// retired retired. `Retire` is the retirement's own failure, or a
/// retirement whose outcome is unknown.
#[derive(Debug)]
#[non_exhaustive]
pub enum ScopedRetireError {
    /// The session bridge has no scoped seam, or the identity is externally
    /// bound: its scope cannot be checked. Nothing was retired.
    Unsupported { detail: String },
    /// Core found another incarnation of the identity current, the scope's
    /// incarnation current under another session, or the scope captured at
    /// another incarnation of the mob. Final for this scope: capture it
    /// again. The scope's member was not retired; when
    /// Core found it after its first step (a successor minted while the
    /// cascade ran), the descendants it retired before stay retired.
    StaleScope {
        mismatch: ScopeMismatch,
        detail: String,
    },
    /// The scope's member was not retired: a scope of another identity or
    /// member, Core's live scope that could not be read, a mob that is not
    /// running, or a runtime that no longer admits identity operations (it
    /// is shutting down, or its mob is closed). Meta refuses the first two
    /// before Core is asked; when Core refuses after its first step (the mob
    /// stopping while the cascade ran), the descendants it retired before
    /// stay retired.
    Rejected { detail: String },
    /// Refused by Meta before Core was asked, with the identity runtime's
    /// typed error: a host-adopted identity
    /// ([`super::IdentityRuntimeError::IdentityHostAdopted`]: Core holds its
    /// identity intent row, so no Meta verb retires it), or one whose
    /// identity history Core could not read
    /// ([`super::IdentityRuntimeError::IdentityHistoryUnavailable`]).
    /// Nothing was retired.
    Refused(super::IdentityRuntimeError),
    /// meerkat's retirement of the scope's incarnation failed or is stuck.
    /// Also the tracked operation ending without a result (it panicked),
    /// after which whether the identity was retired is unknown: read its
    /// status.
    Retire(super::IdentityRuntimeError),
}

impl ScopedRetireError {
    /// The scoped-delivery refusal a scope check answered, as a refusal to
    /// retire.
    pub(crate) fn from_scope_refusal(error: ScopedDeliveryError) -> Self {
        match error {
            ScopedDeliveryError::Unsupported { detail } => Self::Unsupported { detail },
            ScopedDeliveryError::StaleScope { mismatch, detail } => {
                Self::StaleScope { mismatch, detail }
            }
            ScopedDeliveryError::Rejected { detail }
            | ScopedDeliveryError::Uncertain { detail } => Self::Rejected { detail },
        }
    }
}

impl std::fmt::Display for ScopedRetireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { detail } => write!(f, "scoped retirement unsupported: {detail}"),
            Self::StaleScope { mismatch, detail } => {
                write!(
                    f,
                    "stale retirement scope ({}): {detail}",
                    mismatch.as_str()
                )
            }
            Self::Rejected { detail } => write!(f, "scoped retirement rejected: {detail}"),
            Self::Refused(error) => write!(f, "scoped retirement refused: {error}"),
            Self::Retire(error) => write!(f, "retirement failed: {error}"),
        }
    }
}

impl std::error::Error for ScopedRetireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused(error) | Self::Retire(error) => Some(error),
            _ => None,
        }
    }
}

/// What a scoped dispatch's admission proved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScopedDispatchReceipt {
    /// The scope the delivery was admitted under, as supplied.
    pub scope: DeliveryScope,
    /// meerkat's work reference, derived from the delivery's idempotency key.
    pub work_ref: String,
    /// The admission stage this receipt proves. Never a durable-input claim
    /// unless the stage says so.
    pub stage: meerkat_mob::WorkAdmissionStage,
    /// The member session the delivery was admitted to (the scope's session,
    /// validated by meerkat's admission authority).
    pub session_id: meerkat_core::types::SessionId,
}

impl ScopedDispatchReceipt {
    /// The receipt of an admission meerkat reported as admitted to
    /// `admitted_session`. meerkat names the admitted session on every
    /// scope-bound receipt and validates it against the scope inside
    /// admission. A receipt that names no session, or another one, does not
    /// attribute the admitted delivery to this scope; it is never filled in
    /// with the scope's own session, and the delivery's fate is left to
    /// recovery at the scope.
    pub(crate) fn admitted(
        scope: DeliveryScope,
        work_ref: String,
        stage: meerkat_mob::WorkAdmissionStage,
        admitted_session: Option<meerkat_core::types::SessionId>,
    ) -> Result<Self, ScopedDeliveryError> {
        match admitted_session {
            Some(session_id) if &session_id == scope.session_id() => Ok(Self {
                scope,
                work_ref,
                stage,
                session_id,
            }),
            other => Err(ScopedDeliveryError::Uncertain {
                detail: format!(
                    "the admission receipt names session {}, not the scope's session {}",
                    other.map_or_else(|| "none".to_string(), |session| session.to_string()),
                    scope.session_id()
                ),
            }),
        }
    }
}

/// Why a scoped recovery could not establish the delivery's state. Every
/// cause means "unknown": none is absence and none permits a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopedRecoveryUnresolved {
    /// The mob has no runtime adapter to read the original session's store.
    RuntimeAdapterUnavailable,
    /// The original session is unknown to the runtime store.
    OriginalSessionUnknown,
    /// The original owner could not answer.
    OriginalOwnerUnavailable { detail: String },
    /// The evidence read did not finish before the deadline.
    EvidenceReadTimedOut,
    /// The mob's durable member history never bound the scope's runtime
    /// incarnation, fence and session together, so the named session was
    /// not read. This includes a scope captured at another incarnation of the
    /// mob, and a version-1 scope the mob's incarnation does not honour.
    ScopeBindingUnproven,
}

impl ScopedRecoveryUnresolved {
    /// Stable wire code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::RuntimeAdapterUnavailable => "runtime_adapter_unavailable",
            Self::OriginalSessionUnknown => "original_session_unknown",
            Self::OriginalOwnerUnavailable { .. } => "original_owner_unavailable",
            Self::EvidenceReadTimedOut => "evidence_read_timed_out",
            Self::ScopeBindingUnproven => "scope_binding_unproven",
        }
    }
}

/// What a scoped recovery established about one delivery, read from the
/// scope's original session ledger only.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopedRecovery {
    /// An authoritative point-in-time miss in the original session. An
    /// outstanding admission can still land later: never permission to retry.
    Absent,
    /// The original session holds the delivery's input, not yet terminal.
    /// `durable_witness` is `true` only when a committed store row backs it.
    InFlight {
        input_id: String,
        phase: serde_json::Value,
        durable_witness: bool,
    },
    /// The delivery's turn completed; `output` is its own bounded output.
    Completed {
        input_id: String,
        output: TurnOutput,
    },
    /// The delivery's turn reached a failed terminal.
    Failed { input_id: String, error: String },
    /// The input reached a terminal disposition without a run of its own.
    TerminalWithoutRun {
        input_id: String,
        terminal: serde_json::Value,
        last_run_id: Option<String>,
    },
    /// Evidence exists but is internally inconsistent.
    Broken {
        input_id: Option<String>,
        reason: String,
    },
    /// The original owner could not establish the state.
    Unresolved { cause: ScopedRecoveryUnresolved },
}

impl ScopedRecovery {
    /// Project meerkat's scoped work state.
    pub(crate) fn from_native(work: meerkat_mob::ScopedWorkState) -> Self {
        use meerkat_mob::ScopedWorkState;
        match work {
            ScopedWorkState::Absent => Self::Absent,
            ScopedWorkState::InFlight {
                input_id,
                phase,
                durable_witness,
            } => Self::InFlight {
                input_id: input_id.to_string(),
                phase: to_json(&phase),
                durable_witness,
            },
            ScopedWorkState::Terminal { input_id, result } => {
                let input_id = input_id.to_string();
                match result {
                    Ok(result) => {
                        let bounded = result.result();
                        let text = bounded.text();
                        let output = if text.is_empty() {
                            TurnOutput::Empty
                        } else {
                            TurnOutput::Text {
                                text: text.to_string(),
                                truncated: matches!(
                                    bounded.status(),
                                    meerkat_mob::BoundedHelperResultStatus::CompletedTruncated
                                ),
                            }
                        };
                        Self::Completed { input_id, output }
                    }
                    Err(meerkat_mob::BoundedTurnFailure::CompletedWithoutResult { .. }) => {
                        Self::Completed {
                            input_id,
                            output: TurnOutput::NoOwnResult,
                        }
                    }
                    Err(failure) => Self::Failed {
                        input_id,
                        error: failure.to_string(),
                    },
                }
            }
            ScopedWorkState::TerminalWithoutRun {
                input_id,
                terminal,
                last_run_id,
            } => Self::TerminalWithoutRun {
                input_id: input_id.to_string(),
                terminal: to_json(&terminal),
                last_run_id: last_run_id.map(|run_id| run_id.to_string()),
            },
            ScopedWorkState::Broken { input_id, reason } => Self::Broken {
                input_id: input_id.map(|input_id| input_id.to_string()),
                reason,
            },
            ScopedWorkState::Unresolved { cause } => Self::Unresolved {
                cause: match cause {
                    meerkat_mob::ScopedRecoveryUnresolved::RuntimeAdapterUnavailable => {
                        ScopedRecoveryUnresolved::RuntimeAdapterUnavailable
                    }
                    meerkat_mob::ScopedRecoveryUnresolved::OriginalSessionUnknown => {
                        ScopedRecoveryUnresolved::OriginalSessionUnknown
                    }
                    meerkat_mob::ScopedRecoveryUnresolved::OriginalOwnerUnavailable { detail } => {
                        ScopedRecoveryUnresolved::OriginalOwnerUnavailable { detail }
                    }
                    meerkat_mob::ScopedRecoveryUnresolved::EvidenceReadTimedOut => {
                        ScopedRecoveryUnresolved::EvidenceReadTimedOut
                    }
                    // Core holds no binding of the scope's member in this mob
                    // incarnation either way: the scope was captured at
                    // another incarnation of the mob, or is a version-1 scope
                    // this incarnation does not honour. Nothing was read.
                    meerkat_mob::ScopedRecoveryUnresolved::ScopeBindingUnproven
                    | meerkat_mob::ScopedRecoveryUnresolved::IncarnationNotHosted => {
                        ScopedRecoveryUnresolved::ScopeBindingUnproven
                    }
                    other => ScopedRecoveryUnresolved::OriginalOwnerUnavailable {
                        detail: format!("{other:?}"),
                    },
                },
            },
            other => Self::Broken {
                input_id: None,
                reason: format!("unrecognized scoped work state: {other:?}"),
            },
        }
    }
}

/// Serialize a meerkat runtime vocabulary value (serde-derived, infallible in
/// practice) for the wire, keeping a failure visible instead of dropping it.
fn to_json<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value)
        .unwrap_or_else(|error| serde_json::json!({ "unserializable": error.to_string() }))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn member_json() -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "runtime_id": {"identity": "worker", "generation": 3},
            "fence_token": 7,
            "session_id": meerkat_core::types::SessionId::new().to_string(),
        })
    }

    fn scope() -> DeliveryScope {
        DeliveryScope::new(
            AgentIdentity::parse("personal:alice").expect("identity"),
            AgentRuntimeId::parse("rt-alice-1").expect("runtime id"),
            ContinuityGeneration::new(2),
            FencingToken::new(5),
            meerkat_mob::MemberDeliveryScope::from_json_value(member_json()).expect("native scope"),
        )
    }

    #[test]
    fn the_persisted_scope_round_trips_byte_exact_with_both_versions() {
        let original = scope();
        let value = original.to_json_value();
        assert_eq!(value["version"], serde_json::json!(DELIVERY_SCOPE_VERSION));
        assert_eq!(value["member"]["version"], serde_json::json!(1));
        let text = serde_json::to_string(&value).expect("persist");
        let reread: serde_json::Value = serde_json::from_str(&text).expect("reread");
        let decoded = DeliveryScope::from_json_value(reread.clone()).expect("decode");
        assert_eq!(decoded, original);
        assert_eq!(decoded.to_json_value(), reread);
        let via_serde: DeliveryScope = serde_json::from_str(&text).expect("serde decode");
        assert_eq!(via_serde, original);
        assert_eq!(decoded.session_id(), original.member().session_id());
    }

    #[test]
    fn an_unknown_envelope_or_member_version_is_a_typed_error() {
        let mut value = scope().to_json_value();
        value["version"] = serde_json::json!(2);
        assert_eq!(
            DeliveryScope::from_json_value(value),
            Err(DeliveryScopeError::UnsupportedVersion { found: 2 })
        );
        let mut value = scope().to_json_value();
        value["member"]["version"] = serde_json::json!(9);
        let error = DeliveryScope::from_json_value(value).expect_err("member version");
        assert_eq!(
            error,
            DeliveryScopeError::Member(meerkat_mob::DeliveryScopeDecodeError::UnsupportedVersion {
                found: 9
            })
        );
        assert_eq!(error.structured_data()["unsupported_version"], 9);
    }

    #[test]
    fn a_missing_version_unknown_field_or_bad_atom_is_malformed_never_guessed() {
        let mut missing = scope().to_json_value();
        missing.as_object_mut().expect("object").remove("version");
        assert!(matches!(
            DeliveryScope::from_json_value(missing),
            Err(DeliveryScopeError::Malformed { .. })
        ));
        let mut extra = scope().to_json_value();
        extra["session_override"] = serde_json::json!("other");
        assert!(matches!(
            DeliveryScope::from_json_value(extra),
            Err(DeliveryScopeError::Malformed { .. })
        ));
        let mut bad_identity = scope().to_json_value();
        bad_identity["identity"] = serde_json::json!("has space");
        assert!(matches!(
            DeliveryScope::from_json_value(bad_identity),
            Err(DeliveryScopeError::Malformed { .. })
        ));
        let mut string_version = scope().to_json_value();
        string_version["version"] = serde_json::json!("1");
        assert!(matches!(
            DeliveryScope::from_json_value(string_version),
            Err(DeliveryScopeError::Malformed { .. })
        ));
    }

    #[test]
    fn an_admission_receipt_attributes_only_the_scopes_own_session() {
        let scope = scope();
        let stage = meerkat_mob::WorkAdmissionStage::IngressAccepted;
        let receipt = ScopedDispatchReceipt::admitted(
            scope.clone(),
            "work".to_string(),
            stage,
            Some(scope.session_id().clone()),
        )
        .expect("the scope's own session");
        assert_eq!(&receipt.session_id, scope.session_id());
        for admitted in [None, Some(meerkat_core::types::SessionId::new())] {
            assert!(matches!(
                ScopedDispatchReceipt::admitted(scope.clone(), "work".to_string(), stage, admitted),
                Err(ScopedDeliveryError::Uncertain { .. })
            ));
        }
    }

    #[test]
    fn only_an_uncertain_admission_is_reported_as_possibly_admitted() {
        let stale = ScopedDeliveryError::StaleScope {
            mismatch: ScopeMismatch::MemberBinding,
            detail: "moved".to_string(),
        };
        assert_eq!(stale.structured_data()["kind"], "stale_delivery_scope");
        assert_eq!(stale.structured_data()["mismatch"], "member_binding");
        for refusal in [
            stale,
            ScopedDeliveryError::Unsupported {
                detail: String::new(),
            },
            ScopedDeliveryError::Rejected {
                detail: String::new(),
            },
        ] {
            assert!(!refusal.admission_possible(), "{refusal:?}");
            assert_eq!(refusal.structured_data()["admission_possible"], false);
        }
        let uncertain = ScopedDeliveryError::Uncertain {
            detail: String::new(),
        };
        assert!(uncertain.admission_possible());
        assert_eq!(
            uncertain.structured_data()["kind"],
            "scoped_delivery_uncertain"
        );
    }

    #[test]
    fn every_native_work_state_keeps_its_own_class() {
        use meerkat_mob::ScopedWorkState;
        assert_eq!(
            ScopedRecovery::from_native(ScopedWorkState::Absent),
            ScopedRecovery::Absent
        );
        let unresolved = ScopedRecovery::from_native(ScopedWorkState::Unresolved {
            cause: meerkat_mob::ScopedRecoveryUnresolved::EvidenceReadTimedOut,
        });
        assert_eq!(
            unresolved,
            ScopedRecovery::Unresolved {
                cause: ScopedRecoveryUnresolved::EvidenceReadTimedOut
            }
        );
        let broken = ScopedRecovery::from_native(ScopedWorkState::Broken {
            input_id: None,
            reason: "inconsistent".to_string(),
        });
        assert_eq!(
            broken,
            ScopedRecovery::Broken {
                input_id: None,
                reason: "inconsistent".to_string()
            }
        );
    }
}
