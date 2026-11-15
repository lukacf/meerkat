//! meerkat-live — Live multimodal transport for Meerkat.
//!
//! Composable live-channel bridge between browser/test clients and
//! `LiveAdapterHost`. The base transport is WebSocket; the `webrtc` feature
//! adds WebRTC SDP answer signaling plus media/data-channel termination. A
//! Meerkat surface can mount the relevant axum router or listener explicitly.
//!
//! On WebSocket, the client sends `LiveInputChunk` JSON and receives
//! `LiveAdapterObservation` JSON. Both transports use token-bound channel
//! admission.

pub mod host;
mod projection_queue;
pub mod provider_webrtc;
pub mod transport;
#[cfg(feature = "webrtc")]
pub mod webrtc;
pub mod wire_input;

pub use host::{
    DEFAULT_LIVE_TOOL_TIMEOUT, DeltaIdentity, LiveAdapterHost, LiveAdapterHostError,
    LiveAssistantOutputAddress, LiveChannelCloseCommitAuthority, LiveChannelCloseCommitTarget,
    LiveChannelCloseObservation, LiveChannelCloseProjectionLease, LiveChannelId,
    LiveChannelOpenAuthority, LiveChannelRefreshError, LiveChannelRefresher,
    LiveChannelStatusCommitAuthority, LiveChannelStatusObservation, LiveCommandAcceptanceKind,
    LiveCommandQueueAcceptance, LiveProjectionBoundaryHold, LiveProjectionError,
    LiveProjectionSink, LiveRefreshQueueAcceptance, LiveToolDispatchError, LiveToolDispatchTimeout,
    LiveToolDispatcher, LiveTranscriptIdentity, LiveTranscriptIdentityError, NoOpProjectionSink,
    ObservationOutcome, ObservationRouting, ToolDispatchSkipReason,
};
pub use provider_webrtc::{
    LiveProviderInputLatency, LiveQuietAppendTiming, LiveRuntimeBindingFence,
    LiveRuntimeBindingGeneration, LiveSidebandAnnouncement, LiveSidebandAppendAttempt,
    LiveSidebandAppendAuthority, LiveSidebandCommand, LiveSidebandCommandDelivery,
    LiveSidebandCommandError, LiveSidebandDelegationRef, LiveSidebandFinishedNoticeAuthority,
    LiveSidebandNarrationAuthority, LiveSidebandObservation, LiveSidebandObservationKind,
    LiveSidebandProviderCommand, LiveSidebandReleaseAuthority, LiveSidebandTranscriptItemRef,
    LiveSidebandTurnRef, LiveSidebandTurnRole, LiveWebrtcAdmissionSealError,
    LiveWebrtcAnswerAdmissionSeal, LiveWebrtcRuntimeBinding, ProviderWebrtcBinding,
    ProviderWebrtcBoundReadyReceipt, ProviderWebrtcBroker, ProviderWebrtcBrokerAnswer,
    ProviderWebrtcBrokerError, ProviderWebrtcEofEvidence, ProviderWebrtcOffer,
    ProviderWebrtcPendingBoundReadyResolver, ProviderWebrtcPendingBoundReadySeal,
    ProviderWebrtcSidebandSession, ProviderWebrtcSignalingError,
};
pub use transport::{
    LIVE_WS_PATH, LiveChannelCloseFeedback, LiveChannelStatusFeedback, LiveTokenString,
    LiveWsState, LiveWsTokenAdmission, LiveWsTokenAdmissionPublicErrorClass,
    LiveWsTokenAdmissionRejection, LiveWsTokenAuthority, LiveWsTokenIssue, live_ws_router,
    serve_live_ws_listener,
};
#[cfg(feature = "webrtc")]
pub use webrtc::{
    LIVE_WEBRTC_ANSWER_METHOD, LIVE_WEBRTC_ANSWER_PATH, LiveWebrtcAdmittedOffer,
    LiveWebrtcAnswerAccepted, LiveWebrtcAnswerTransport, LiveWebrtcBindingRequest, LiveWebrtcError,
    LiveWebrtcState, WebrtcAudioBridge, live_webrtc_router, serve_live_ws_and_webrtc_listener,
};
pub use wire_input::{
    LiveInputChunkDecodeError, live_input_chunk_decode_rejection, live_input_chunk_from_wire,
};

/// Stable tracing target for every wait on the live close path.
///
/// Each awaited step of a channel close (custody revocation, provider close
/// request, drain observation, transport retirement, generated commit, peer
/// cleanup) emits one `info` line when the wait starts and one when it ends,
/// carrying the step name and the elapsed time. Filter on this target to see
/// where a close spends its time without enabling debug output elsewhere.
pub const LIVE_CLOSE_TRACE_TARGET: &str = "meerkat::live_close";

/// Await one step of a live close under [`LIVE_CLOSE_TRACE_TARGET`] tracing.
pub async fn traced_live_close_step<T, F>(
    channel_id: Option<&LiveChannelId>,
    step: &'static str,
    step_future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let channel = channel_id.map_or("-", LiveChannelId::as_str);
    let started = std::time::Instant::now();
    tracing::info!(
        target: LIVE_CLOSE_TRACE_TARGET,
        channel,
        step,
        "live close step started"
    );
    let output = step_future.await;
    tracing::info!(
        target: LIVE_CLOSE_TRACE_TARGET,
        channel,
        step,
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "live close step finished"
    );
    output
}

// E26 regression: `meerkat-live` must not depend on `meerkat-runtime`. The
// dependency direction is: `meerkat-live` owns the live-adapter host
// (`crate::host`) and the transport (`crate::transport`); surfaces compose
// the two with their own session-runtime backing. A previous design had
// `LiveAdapterHost` living in `meerkat-runtime` and `meerkat-live`
// depending on it — wrong direction (transport pulling runtime). This
// inline test parses `Cargo.toml` at compile time and asserts that no
// dependency table names `meerkat-runtime`. Anyone who re-introduces the dep will
// fail this test before E26 silently regresses.
#[cfg(test)]
mod e26_dependency_direction {
    /// The crate's manifest, parsed. `cargo package` rewrites dependency
    /// lines into `[dependencies.<name>]` tables, so a line scan would pass in
    /// the workspace and fail in the published package.
    fn manifest() -> toml::Table {
        toml::from_str(include_str!("../Cargo.toml")).expect("meerkat-live Cargo.toml parses")
    }

    /// Every dependency table: normal, dev and build, and their
    /// `[target.<cfg>]` forms.
    fn dependency_tables(manifest: &toml::Table) -> Vec<&toml::Table> {
        let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
        let mut tables: Vec<&toml::Table> = kinds
            .iter()
            .filter_map(|kind| manifest.get(*kind).and_then(toml::Value::as_table))
            .collect();
        if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
            for target in targets.values().filter_map(toml::Value::as_table) {
                tables.extend(
                    kinds
                        .iter()
                        .filter_map(|kind| target.get(*kind).and_then(toml::Value::as_table)),
                );
            }
        }
        tables
    }

    /// E26: `meerkat-live` must not list `meerkat-runtime` as a dependency.
    /// We read `Cargo.toml` via `include_str!` (compile-time embed, no I/O
    /// at test runtime) and look for the crate name as a key of every
    /// dependency table, whatever shape the dependency takes.
    #[test]
    fn cargo_toml_does_not_depend_on_meerkat_runtime() {
        let manifest = manifest();
        for table in dependency_tables(&manifest) {
            assert!(
                !table.contains_key("meerkat-runtime"),
                "E26 regression: meerkat-live must not depend on meerkat-runtime"
            );
        }
    }

    #[test]
    fn webrtc_media_dependencies_are_feature_gated() {
        let manifest = manifest();
        let default = manifest
            .get("features")
            .and_then(|features| features.get("default"))
            .and_then(toml::Value::as_array)
            .expect("meerkat-live declares its default feature set");
        assert!(
            default.is_empty(),
            "meerkat-live default feature set must not enable WebRTC/media: {default:?}"
        );
        let dependencies = manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .expect("meerkat-live has a [dependencies] table");
        for dep in ["webrtc", "webrtc-media", "opus", "rubato"] {
            let spec = dependencies
                .get(dep)
                .unwrap_or_else(|| panic!("missing optional {dep} dependency"));
            assert_eq!(
                spec.get("optional").and_then(toml::Value::as_bool),
                Some(true),
                "{dep} dependency must stay optional: {spec}"
            );
        }
    }
}
