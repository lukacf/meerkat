//! Shared wire conversion errors and the typed mob console error details.
//!
//! Conversion error surfaced when a value has no safe counterpart across the
//! core/wire boundary. Extracted from `wire::live` so session-wire conversions
//! (`WireAssistantBlock`, `WireTranscriptSource`) do not depend on the
//! live RPC module.
//!
//! Also home to the §17.4 multi-host console error detail vocabulary
//! (`WireMobErrorDetail` and its per-code detail structs) — the ONE owner of
//! the `ErrorCode` ↔ detail-shape pairing every console surface (RPC, REST,
//! MCP, CLI) consumes.

use serde::{Deserialize, Serialize};

use super::mob::{WireHostRef, WireScopeDeniedDetail};
use crate::error::ErrorCode;

/// Typed `details` payload for [`ErrorCode::HostUnavailable`] (§17.4): the
/// member host was unreachable or a bridge request expired. Observation
/// degrades typed, never quiet (§9 partition row).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireHostUnavailableDetail {
    /// Host the failing hop resolved, when known. `None` when the failure
    /// happened before any host identity was resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<WireHostRef>,
    /// Present for bridge reply-deadline expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Typed `details` payload for [`ErrorCode::StaleCursor`] (§17.4): an event
/// cursor overran the retained window. `watermark` is the current position —
/// the §9 recovery fact the poller restarts from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireStaleCursorDetail {
    /// The current position — the recovery fact.
    pub watermark: u64,
    /// Present on bridge-cause overruns (the owning host's transcript/event
    /// domain generation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    /// The overrunning cursor, when known (local overruns name it; remote
    /// rejections do not carry it back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<u64>,
}

/// Typed `details` payload for [`ErrorCode::StaleFence`] (§17.4): a command
/// carried a superseded `(generation, fence)` tuple. Fence numbers are
/// present only when the local check produced them — remote `StaleFence`
/// rejections carry no numbers on the cause.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireStaleFenceDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<u64>,
}

/// Closed pairing of the four multi-host console [`ErrorCode`]s with their
/// typed detail payloads (§17.4, ADJ-P7-4).
///
/// Deliberately NOT serde-derived: the enum itself is never serialized. It
/// exists so the code↔detail pairing has exactly one compile-forced owner —
/// [`Self::code`] is exhaustive over the variants, and [`Self::detail_value`]
/// serializes the BARE inner struct, which keeps the phase-5 RPC ScopeDenied
/// wire shape (`data == {required, presented}`) byte-identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMobErrorDetail {
    ScopeDenied(WireScopeDeniedDetail),
    HostUnavailable(WireHostUnavailableDetail),
    StaleCursor(WireStaleCursorDetail),
    StaleFence(WireStaleFenceDetail),
}

impl WireMobErrorDetail {
    /// The wire [`ErrorCode`] paired with this detail shape. Exhaustive by
    /// construction — adding a variant forces a new arm.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::ScopeDenied(_) => ErrorCode::ScopeDenied,
            Self::HostUnavailable(_) => ErrorCode::HostUnavailable,
            Self::StaleCursor(_) => ErrorCode::StaleCursor,
            Self::StaleFence(_) => ErrorCode::StaleFence,
        }
    }

    /// Serialize the BARE inner detail struct (never an enum envelope) —
    /// the `data`/`details` carrier shape every surface ships.
    pub fn detail_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        match self {
            Self::ScopeDenied(detail) => serde_json::to_value(detail),
            Self::HostUnavailable(detail) => serde_json::to_value(detail),
            Self::StaleCursor(detail) => serde_json::to_value(detail),
            Self::StaleFence(detail) => serde_json::to_value(detail),
        }
    }
}

/// Safe projection of the actual credential-use disposition. This carries no
/// account, lease, token, or authorization authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireCredentialUseDisposition {
    Authorized,
    RefreshRequired,
    RefreshDisallowed,
    ReauthRequired,
    LeaseAbsent,
    AlreadyRefreshing,
}

impl From<meerkat_core::handles::CredentialUseDisposition> for WireCredentialUseDisposition {
    fn from(disposition: meerkat_core::handles::CredentialUseDisposition) -> Self {
        use meerkat_core::handles::CredentialUseDisposition;
        match disposition {
            CredentialUseDisposition::Authorized => Self::Authorized,
            CredentialUseDisposition::RefreshRequired => Self::RefreshRequired,
            CredentialUseDisposition::RefreshDisallowed => Self::RefreshDisallowed,
            CredentialUseDisposition::ReauthRequired => Self::ReauthRequired,
            CredentialUseDisposition::LeaseAbsent => Self::LeaseAbsent,
            CredentialUseDisposition::AlreadyRefreshing => Self::AlreadyRefreshing,
        }
    }
}

/// Audience-safe native readiness causes. These describe unavailable input or session operations;
/// they neither grant permission nor prescribe a blind retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireControllerReadinessFailure {
    PolicyUnavailable {},
    ExecutorUnavailable {},
    FactsUnavailable {},
    Busy {},
    AuthorityUnavailable {},
    AuthorityChanged {},
    UnsupportedScope {},
    CredentialUnusable {
        disposition: WireCredentialUseDisposition,
    },
    /// The existing core auth error discriminant, with no provider diagnostic.
    CredentialPreparationFailed {
        cause: meerkat_core::auth::AuthErrorKind,
    },
    PolicyChanged {},
    ReplacementNotEmpty {},
    /// A future native readiness cause without an available public projection.
    Unknown {},
}

impl From<meerkat_core::authorization::ControllerReadinessFailure>
    for WireControllerReadinessFailure
{
    fn from(reason: meerkat_core::authorization::ControllerReadinessFailure) -> Self {
        use meerkat_core::authorization::ControllerReadinessFailure;
        match reason {
            ControllerReadinessFailure::PolicyUnavailable => Self::PolicyUnavailable {},
            ControllerReadinessFailure::ExecutorUnavailable => Self::ExecutorUnavailable {},
            ControllerReadinessFailure::FactsUnavailable => Self::FactsUnavailable {},
            ControllerReadinessFailure::Busy => Self::Busy {},
            ControllerReadinessFailure::AuthorityUnavailable => Self::AuthorityUnavailable {},
            ControllerReadinessFailure::AuthorityChanged => Self::AuthorityChanged {},
            ControllerReadinessFailure::UnsupportedScope => Self::UnsupportedScope {},
            ControllerReadinessFailure::CredentialUnusable { disposition } => {
                Self::CredentialUnusable {
                    disposition: disposition.into(),
                }
            }
            ControllerReadinessFailure::CredentialPreparationFailed { kind } => {
                Self::CredentialPreparationFailed { cause: kind }
            }
            ControllerReadinessFailure::PolicyChanged => Self::PolicyChanged {},
            ControllerReadinessFailure::ReplacementNotEmpty => Self::ReplacementNotEmpty {},
            _ => Self::Unknown {},
        }
    }
}

/// Closed input-refusal projection. Stale preparation is represented by
/// `WireControllerReadinessFailure::PolicyChanged {}`, never by this kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireInputRefusalKind {
    Denied,
    MalformedFacts,
}

/// Canonical native input code/detail pairing, shared by protocol surfaces.
/// Only typed, audience-safe owner dispositions are projected; no Debug or
/// Display payload, principal, resource, or credential is serialized here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireInputAdmissionErrorDetail {
    Refused {
        kind: WireInputRefusalKind,
    },
    NotReady {
        reason: WireControllerReadinessFailure,
    },
}

impl WireInputAdmissionErrorDetail {
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Refused { .. } => ErrorCode::InputRefused,
            Self::NotReady { .. } => ErrorCode::InputNotReady,
        }
    }

    /// Build the safe shared envelope without swallowing serialization errors.
    pub fn to_wire_error(&self) -> Result<crate::error::WireError, serde_json::Error> {
        let message = match self {
            Self::Refused { .. } => "input refused",
            Self::NotReady { .. } => "input readiness unavailable",
        };
        Ok(crate::error::WireError::new(self.code(), message)
            .with_details(serde_json::to_value(self)?))
    }
}

/// Conversion error surfaced when a wire variant has no core counterpart or
/// when a core-only internal event has no safe public-wire representation.
///
/// Fires for the explicit fail-loud `Unknown` wire variants —
/// `WireLiveTransportBootstrap::Unknown`, `WireLiveAdapterObservation::Unknown`,
/// `WireLiveContinuityMode::Unknown`, `WireLiveResponseModality::Unknown`,
/// `WireLiveAdapterStatus::Unknown`, `WireLiveAdapterErrorCode::Unknown`,
/// `WireTranscriptSource::Unknown`, `WireAssistantBlock::Unknown`. These are
/// the wire mirrors' fail-loud sentinels for forward-converted future core
/// variants. The internal realtime user-content variant is rejected in the
/// forward direction because it may contain private image bytes. Conversion
/// returns this error instead of silently fabricating a placeholder or
/// exposing internal content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WireConversionError {
    /// A domain history page cannot be projected without producing an
    /// unrepresentable or non-advancing wire cursor.
    #[error("invalid member-history page projection: {debug}")]
    MemberHistoryPage { debug: String },
    /// Wire transport is the explicit-Unknown sentinel; no inverse mapping
    /// exists. Carries the original debug payload for server logs.
    #[error("unknown wire transport variant: {debug}")]
    Transport { debug: String },
    /// Wire observation is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire observation variant: {debug}")]
    Observation { debug: String },
    /// Wire continuity mode is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire continuity-mode variant: {debug}")]
    Continuity { debug: String },
    /// Wire response modality is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire response-modality variant: {debug}")]
    ResponseModality { debug: String },
    /// Wire adapter status is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire adapter-status variant: {debug}")]
    Status { debug: String },
    /// Wire adapter error-code is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire adapter-error-code variant: {debug}")]
    ErrorCode { debug: String },
    /// Wire config-rejection reason is the explicit-Unknown sentinel; no
    /// inverse mapping exists. Carries the original debug payload for
    /// server logs.
    #[error("unknown wire config-rejection-reason variant: {debug}")]
    ConfigRejectionReason { debug: String },
    /// Wire transcript-source is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    /// R7-4 (P3 dogma): mirrors the live-wire `Unknown` pattern for
    /// `WireTranscriptSource` so future core variants are not silently
    /// misattributed as `Spoken`.
    #[error("unknown wire transcript-source variant: {debug}")]
    TranscriptSource { debug: String },
    /// Wire assistant-block is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    /// R7-5 (P3 dogma): the reverse direction previously fabricated an
    /// empty `AssistantBlock::Text` from `WireAssistantBlock::Unknown`,
    /// silently producing a zero-length text block on the canonical
    /// transcript. Now surfaces as a typed error.
    #[error("unknown wire assistant-block variant: {debug}")]
    AssistantBlock { debug: String },
    /// Wire provider is the explicit-Unknown sentinel; no inverse mapping
    /// exists. Carries the original debug payload for server logs.
    #[error("unknown wire provider variant: {debug}")]
    Provider { debug: String },
    /// Public transcript rewrite message could not be converted into a core
    /// transcript message.
    #[error("invalid transcript rewrite message: {debug}")]
    TranscriptMessage { debug: String },
    /// Rewrite ingress attempted to mint a runtime-authority transcript role.
    /// `compaction_summary` is runtime-mintable only: hosts may declare
    /// `conversational` or `injected_context` on rewrite ingress, never the
    /// compaction-boundary marker the transcript-continuity save-guard trusts.
    /// Fail-closed rejection, not silent role laundering.
    #[error("transcript role is not host-mintable via rewrite ingress: {debug}")]
    TranscriptRole { debug: String },
    /// Wire degradation-reason is the explicit-Unknown sentinel; no inverse
    /// mapping exists.
    #[error("unknown wire degradation-reason variant: {debug}")]
    DegradationReason { debug: String },
    /// The core realtime transcript event carries canonical user content and
    /// is intentionally not representable on the public live-observation
    /// wire. This error carries no source payload so an attempted conversion
    /// cannot leak inline image bytes through diagnostics.
    #[error("internal realtime user-content event has no public wire representation")]
    InternalRealtimeUserContent,
    #[error("internal realtime playback authority event has no public transcript projection")]
    InternalRealtimePlaybackAuthority,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::wire::mob::WireControlScope;

    /// T-B2: `code()` pairing per variant, bare-struct serialization for
    /// ScopeDenied byte-identical to the phase-5 `{required, presented}`
    /// shape, serde round-trips, and `deny_unknown_fields` rejects.
    #[test]
    fn wire_mob_error_detail_code_pairing_and_bare_serialization() {
        let scope = WireMobErrorDetail::ScopeDenied(WireScopeDeniedDetail {
            required: WireControlScope::AdminHost,
            presented: vec![WireControlScope::List],
        });
        let host = WireMobErrorDetail::HostUnavailable(WireHostUnavailableDetail {
            host: Some(WireHostRef("host-peer-1".to_string())),
            timeout_ms: Some(15_000),
        });
        let cursor = WireMobErrorDetail::StaleCursor(WireStaleCursorDetail {
            watermark: 42,
            generation: Some(3),
            requested: None,
        });
        let fence = WireMobErrorDetail::StaleFence(WireStaleFenceDetail {
            runtime_id: Some("worker#2".to_string()),
            expected: Some(2),
            actual: Some(1),
        });

        assert_eq!(scope.code(), ErrorCode::ScopeDenied);
        assert_eq!(host.code(), ErrorCode::HostUnavailable);
        assert_eq!(cursor.code(), ErrorCode::StaleCursor);
        assert_eq!(fence.code(), ErrorCode::StaleFence);

        // The ScopeDenied detail_value is the BARE phase-5 shape —
        // byte-identical `{required, presented}`, no enum envelope.
        assert_eq!(
            scope.detail_value().expect("scope detail serializes"),
            serde_json::json!({
                "required": "admin_host",
                "presented": ["list"],
            })
        );

        // Optional absence stays typed absence: no null keys on the wire.
        let sparse = WireMobErrorDetail::HostUnavailable(WireHostUnavailableDetail {
            host: None,
            timeout_ms: None,
        });
        assert_eq!(
            sparse.detail_value().expect("sparse detail serializes"),
            serde_json::json!({})
        );
        assert_eq!(
            cursor.detail_value().expect("cursor detail serializes"),
            serde_json::json!({ "watermark": 42, "generation": 3 })
        );

        // Serde round-trips for the three new structs.
        let host_detail = WireHostUnavailableDetail {
            host: Some(WireHostRef("host-peer-1".to_string())),
            timeout_ms: Some(15_000),
        };
        let round: WireHostUnavailableDetail =
            serde_json::from_value(serde_json::to_value(&host_detail).expect("encode host detail"))
                .expect("decode host detail");
        assert_eq!(round, host_detail);

        let cursor_detail = WireStaleCursorDetail {
            watermark: 42,
            generation: None,
            requested: Some(41),
        };
        let round: WireStaleCursorDetail = serde_json::from_value(
            serde_json::to_value(&cursor_detail).expect("encode cursor detail"),
        )
        .expect("decode cursor detail");
        assert_eq!(round, cursor_detail);

        let fence_detail = WireStaleFenceDetail {
            runtime_id: None,
            expected: None,
            actual: None,
        };
        let round: WireStaleFenceDetail = serde_json::from_value(
            serde_json::to_value(&fence_detail).expect("encode fence detail"),
        )
        .expect("decode fence detail");
        assert_eq!(round, fence_detail);

        // deny_unknown_fields rejects on all three new structs.
        assert!(
            serde_json::from_value::<WireHostUnavailableDetail>(
                serde_json::json!({ "host": "h", "surprise": true })
            )
            .is_err(),
            "WireHostUnavailableDetail must reject unknown fields"
        );
        assert!(
            serde_json::from_value::<WireStaleCursorDetail>(
                serde_json::json!({ "watermark": 1, "surprise": true })
            )
            .is_err(),
            "WireStaleCursorDetail must reject unknown fields"
        );
        assert!(
            serde_json::from_value::<WireStaleFenceDetail>(serde_json::json!({ "surprise": 1 }))
                .is_err(),
            "WireStaleFenceDetail must reject unknown fields"
        );
    }

    /// T-B4 (agreement half; the const-map exact values are pinned in
    /// `error::tests::multi_host_error_codes_render_exact_values`):
    /// `WireMobErrorDetail::code()` agrees with the four const maps —
    /// jsonrpc -32025..-32028 (round-tripping via `from_jsonrpc_code`),
    /// http 403/503/410/409, cli 45/46/47/48.
    #[test]
    fn wire_mob_error_detail_codes_agree_with_const_maps() {
        let details = [
            (
                WireMobErrorDetail::ScopeDenied(WireScopeDeniedDetail {
                    required: WireControlScope::AdminHost,
                    presented: Vec::new(),
                }),
                -32025,
                403,
                45,
            ),
            (
                WireMobErrorDetail::HostUnavailable(WireHostUnavailableDetail {
                    host: None,
                    timeout_ms: None,
                }),
                -32026,
                503,
                46,
            ),
            (
                WireMobErrorDetail::StaleCursor(WireStaleCursorDetail {
                    watermark: 0,
                    generation: None,
                    requested: None,
                }),
                -32027,
                410,
                47,
            ),
            (
                WireMobErrorDetail::StaleFence(WireStaleFenceDetail {
                    runtime_id: None,
                    expected: None,
                    actual: None,
                }),
                -32028,
                409,
                48,
            ),
        ];
        for (detail, jsonrpc, http, cli) in details {
            let code = detail.code();
            assert_eq!(code.jsonrpc_code(), jsonrpc, "{code:?} jsonrpc agreement");
            assert_eq!(
                ErrorCode::from_jsonrpc_code(jsonrpc),
                Some(code),
                "{code:?} jsonrpc round-trip"
            );
            assert_eq!(code.http_status(), http, "{code:?} http agreement");
            assert_eq!(code.cli_exit_code(), cli, "{code:?} cli agreement");
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod native_admission_wire_tests {
    use super::*;
    use crate::error::ErrorCategory;
    use serde_json::json;

    #[test]
    fn native_admission_details_are_closed_and_preserve_canonical_kinds() {
        let cases = [
            WireInputAdmissionErrorDetail::Refused {
                kind: WireInputRefusalKind::Denied,
            },
            WireInputAdmissionErrorDetail::Refused {
                kind: WireInputRefusalKind::MalformedFacts,
            },
            WireInputAdmissionErrorDetail::NotReady {
                reason: WireControllerReadinessFailure::CredentialUnusable {
                    disposition: WireCredentialUseDisposition::ReauthRequired,
                },
            },
        ];
        for case in cases {
            let encoded = serde_json::to_value(case).unwrap();
            let round: WireInputAdmissionErrorDetail =
                serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(round, case);
            let mut extra = encoded;
            extra["principal"] = json!("private-principal-canary");
            assert!(serde_json::from_value::<WireInputAdmissionErrorDetail>(extra).is_err());
        }
        assert_eq!(
            serde_json::to_value(cases[0]).unwrap(),
            json!({"type":"refused","kind":"denied"})
        );
        for bad in [
            json!({"type":"refused"}),
            json!({"type":"refused","kind":"made_up"}),
            json!({"type":"not_ready","reason":{"kind":"busy","token":"private-token-canary"}}),
            json!({"type":"not_ready","reason":{"kind":"credential_unusable"}}),
            json!({"type":"not_ready","reason":{"kind":"credential_unusable","disposition":"made_up"}}),
        ] {
            assert!(serde_json::from_value::<WireInputAdmissionErrorDetail>(bad).is_err());
        }
    }

    #[test]
    fn native_admission_code_pairing_and_allocations_are_exact() {
        for (detail, code, rpc, http, cli, category) in [
            (
                WireInputAdmissionErrorDetail::Refused {
                    kind: WireInputRefusalKind::Denied,
                },
                ErrorCode::InputRefused,
                -32030,
                403,
                50,
                ErrorCategory::Hook,
            ),
            (
                WireInputAdmissionErrorDetail::NotReady {
                    reason: WireControllerReadinessFailure::PolicyUnavailable {},
                },
                ErrorCode::InputNotReady,
                -32031,
                503,
                51,
                ErrorCategory::Capability,
            ),
        ] {
            assert_eq!(detail.code(), code);
            let wire = detail.to_wire_error().unwrap();
            assert_eq!(wire.code, code);
            assert_eq!(wire.category, category);
            assert_eq!(wire.details, Some(serde_json::to_value(detail).unwrap()));
            assert_eq!(code.jsonrpc_code(), rpc);
            assert_eq!(code.http_status(), http);
            assert_eq!(code.cli_exit_code(), cli);
            assert_eq!(ErrorCode::from_jsonrpc_code(rpc), Some(code));
        }
        assert_eq!(ErrorCode::MemberReloadRequired.jsonrpc_code(), -32029);
        assert_eq!(ErrorCode::MemberReloadRequired.cli_exit_code(), 49);
    }

    #[test]
    fn readiness_envelopes_do_not_prescribe_automatic_retry() {
        for reason in [
            WireControllerReadinessFailure::Busy {},
            WireControllerReadinessFailure::PolicyUnavailable {},
            WireControllerReadinessFailure::PolicyChanged {},
            WireControllerReadinessFailure::UnsupportedScope {},
            WireControllerReadinessFailure::ReplacementNotEmpty {},
            WireControllerReadinessFailure::CredentialUnusable {
                disposition: WireCredentialUseDisposition::ReauthRequired,
            },
        ] {
            let detail = WireInputAdmissionErrorDetail::NotReady { reason };
            let wire = detail.to_wire_error().unwrap();
            let encoded = serde_json::to_value(wire).unwrap();
            assert_eq!(
                encoded,
                json!({
                    "code": "INPUT_NOT_READY",
                    "category": "capability",
                    "message": "input readiness unavailable",
                    "details": serde_json::to_value(detail).unwrap(),
                })
            );
        }
    }

    #[test]
    fn native_admission_refusal_decoder_rejects_reprepare_required() {
        let decoded = serde_json::from_value::<WireInputAdmissionErrorDetail>(json!({
            "type": "refused",
            "kind": "reprepare_required",
        }));
        assert!(
            decoded.is_err(),
            "stale preparation is readiness, not input refusal: {decoded:?}"
        );
    }
}
