//! Panic capture at actor task boundaries: recover and sanitize the payload,
//! log it on change plus bounded repeat checkpoints, and feed the typed
//! [`MobError`] channel.
//!
//! WHY THIS MODULE EXISTS (field incident, 2026-07-29): a panic inside the
//! member-provisioning transaction (`PreparedServiceActorTransaction`, armed
//! by durable member-child rows carrying "runtime session unregistered"
//! residue from operator SIGKILLs) was caught at the actor's spawn-provision
//! `catch_unwind` boundary with the payload discarded (`Err(_)`), converted
//! to an opaque "task panicked" internal error, and immediately retried by
//! the identity-reconcile requeue. Nothing was logged at any level — and the
//! deployment piped stderr to /dev/null, so even the std panic-hook line
//! vanished. The result was an invisible panic furnace: 40+ minutes at 99%
//! CPU, reproduced across three releases, with each iteration paying for
//! DWARF symbolication (gimli) of a ~280MB binary inside the panic hook. A
//! swallowed payload plus an eager retry is a furnace, and per-panic
//! symbolication is the fuel.
//!
//! Every `catch_unwind` at an actor task boundary must therefore:
//! (a) recover the panic payload and `tracing::error!` it — immediately on
//!     change and at power-of-two repeat checkpoints, never per iteration
//!     (that would trade the silent furnace for a log flood) — so the
//!     underlying bug names itself and remains visibly active; and
//! (b) convert the caught panic into the typed [`MobError`] channel so it
//!     flows through the ordinary failure/disposition path (the same
//!     classification provisioning build failures take), instead of enabling
//!     a bespoke silent retry.
//!
//! panic=abort compatibility: under `panic = "abort"` `catch_unwind` never
//! observes a panic (the process dies at the panic site), so everything here
//! runs only when unwinding actually delivered a payload; nothing below
//! assumes a panic happened on the success path.

use crate::MobId;
use crate::error::MobError;
use crate::ids::AgentIdentity;
use futures::FutureExt;
use meerkat_core::panic_payload::{PanicPayloadLogDecision, PanicPayloadLogGate};
use std::future::Future;

/// Best-effort human-readable panic payload; thin delegation to the shared
/// [`meerkat_core::panic_payload`] helper (also used by the meerkat-runtime
/// attachment boundaries in `meerkat-runtime/src/panic_boundary.rs`).
pub(super) fn panic_payload_detail(payload: &(dyn std::any::Any + Send)) -> String {
    meerkat_core::panic_payload::panic_payload_detail(payload)
}

/// Bounded panic log gate keyed by provisioning stage and member identity.
///
/// The identity-reconcile requeue can legally re-run a panicking provision
/// transaction indefinitely; the incident above showed the payload must be
/// logged, and a per-iteration `error!` would be its own flood. It logs on
/// transition and at bounded repeat checkpoints. A successful provision
/// clears the entry so the next incident logs fresh.
#[derive(Debug, Default)]
pub(super) struct SpawnPanicLogLedger {
    gate: PanicPayloadLogGate,
}

impl SpawnPanicLogLedger {
    fn key(stage: &str, identity: &AgentIdentity) -> String {
        format!("{stage}:{}", identity.as_str())
    }

    /// Record the payload for this exact stage+identity boundary. New payloads
    /// log immediately; long-running repeats report at bounded checkpoints.
    fn observe(
        &self,
        stage: &str,
        identity: &AgentIdentity,
        detail: &str,
    ) -> PanicPayloadLogDecision {
        self.gate.observe(&Self::key(stage, identity), detail)
    }

    /// Forget the identity's recorded payload (called on provision success)
    /// so a later recurrence of the same panic logs again as a new incident.
    fn clear(&self, stage: &str, identity: &AgentIdentity) {
        self.gate.clear(&Self::key(stage, identity));
    }
}

/// Convert a caught provisioning panic into the typed spawn-failure error,
/// logging the recovered payload immediately and at bounded repeat checkpoints
/// per member.
pub(super) fn spawn_provision_panic_to_error(
    ledger: &SpawnPanicLogLedger,
    mob_id: &MobId,
    stage: &str,
    identity: &AgentIdentity,
    payload: Box<dyn std::any::Any + Send>,
) -> MobError {
    let detail = panic_payload_detail(payload.as_ref());
    let log_decision = ledger.observe(stage, identity, &detail);
    if log_decision.should_log {
        tracing::error!(
            mob_id = %mob_id,
            agent_identity = %identity,
            stage,
            panic = %detail,
            repeated_sightings = log_decision.repeated_sightings,
            "member provisioning panicked; payload recovered, sanitized, and converted to a typed spawn failure"
        );
    }
    MobError::Internal(format!("{stage} panicked for '{identity}': {detail}"))
}

/// Run one provisioning attempt under the actor's panic-capture boundary.
///
/// The future is polled to completion exactly once; a caught panic becomes a
/// typed [`MobError`] for the caller's ordinary disposition path. This
/// boundary itself NEVER retries — retry policy belongs to the caller
/// (identity reconcile / spawn disposition), where it is visible and typed.
pub(super) async fn run_spawn_provision_guarded<T, F>(
    ledger: &SpawnPanicLogLedger,
    mob_id: &MobId,
    stage: &str,
    identity: &AgentIdentity,
    provision: F,
) -> Result<T, MobError>
where
    F: Future<Output = Result<T, MobError>>,
{
    match std::panic::AssertUnwindSafe(provision).catch_unwind().await {
        Ok(result) => {
            if result.is_ok() {
                ledger.clear(stage, identity);
            }
            result
        }
        Err(payload) => Err(spawn_provision_panic_to_error(
            ledger, mob_id, stage, identity, payload,
        )),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_error_logs() -> (Arc<Mutex<Vec<u8>>>, tracing::subscriber::DefaultGuard) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer_buf = SharedBuf(Arc::clone(&buf));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_writer(move || writer_buf.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (buf, guard)
    }

    fn captured(buf: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().expect("log buffer lock").clone())
            .expect("captured logs should be utf8")
    }

    #[test]
    fn payload_detail_recovers_str_string_and_degrades_other_types() {
        assert_eq!(panic_payload_detail(&"boom"), "boom");
        assert_eq!(panic_payload_detail(&"boom".to_string()), "boom");
        assert_eq!(panic_payload_detail(&42_u32), "non-string panic payload");
    }

    #[tokio::test]
    async fn panicking_provision_yields_typed_error_with_payload_and_single_poll() {
        let (buf, _guard) = capture_error_logs();
        let ledger = SpawnPanicLogLedger::default();
        let mob_id = MobId::from("panic-capture-mob");
        let identity = AgentIdentity::from("member-child");
        let polls = Arc::new(AtomicUsize::new(0));

        let result: Result<(), MobError> =
            run_spawn_provision_guarded(&ledger, &mob_id, "spawn provisioning task", &identity, {
                let polls = Arc::clone(&polls);
                async move {
                    polls.fetch_add(1, Ordering::SeqCst);
                    panic!("runtime session unregistered residue");
                }
            })
            .await;

        // (b) The caught panic is the ordinary typed error, carrying the
        // recovered payload — not an opaque "task panicked" marker.
        let error = result.expect_err("a panicking provision must fail");
        let MobError::Internal(message) = &error else {
            panic!("caught panic must classify as MobError::Internal, got {error:?}");
        };
        assert!(
            message.contains("runtime session unregistered residue"),
            "typed error must carry the panic payload, got: {message}"
        );
        assert!(
            message.contains("spawn provisioning task"),
            "typed error must name the stage, got: {message}"
        );

        // (d) The boundary itself never loops: one call, one poll-through.
        assert_eq!(
            polls.load(Ordering::SeqCst),
            1,
            "the panic boundary must not retry the provision internally"
        );

        // (a) The payload appears in the tracing output.
        let logs = captured(&buf);
        assert!(
            logs.contains("runtime session unregistered residue"),
            "the panic payload must be logged, got: {logs}"
        );
        assert!(
            logs.contains("member-child"),
            "the log line must carry member identity context, got: {logs}"
        );
    }

    #[tokio::test]
    async fn repeated_identical_panics_log_once_and_distinct_payloads_log_again() {
        let (buf, _guard) = capture_error_logs();
        let ledger = SpawnPanicLogLedger::default();
        let mob_id = MobId::from("panic-capture-mob");
        let identity = AgentIdentity::from("member-child");

        for _ in 0..3 {
            let result: Result<(), MobError> = run_spawn_provision_guarded(
                &ledger,
                &mob_id,
                "spawn provisioning task",
                &identity,
                async { panic!("repeated furnace payload") },
            )
            .await;
            // Every iteration still yields the typed error with the payload;
            // only the error! line is rate-limited.
            let message = result.expect_err("panic must fail").to_string();
            assert!(message.contains("repeated furnace payload"));
        }

        // (c) Rate limit holds: three identical panics, one error! line.
        let logs = captured(&buf);
        assert_eq!(
            logs.matches("repeated furnace payload").count(),
            1,
            "identical repeated panics must log exactly once, got: {logs}"
        );

        // A DISTINCT payload is a new incident and must log.
        let _: Result<(), MobError> = run_spawn_provision_guarded(
            &ledger,
            &mob_id,
            "spawn provisioning task",
            &identity,
            async { panic!("different furnace payload") },
        )
        .await;
        let logs = captured(&buf);
        assert_eq!(logs.matches("different furnace payload").count(), 1);
    }

    #[tokio::test]
    async fn long_running_identical_panic_reports_repeat_count_without_flooding() {
        let (buf, _guard) = capture_error_logs();
        let ledger = SpawnPanicLogLedger::default();
        let mob_id = MobId::from("panic-capture-mob");
        let identity = AgentIdentity::from("member-child");

        for _ in 0..9 {
            let _: Result<(), MobError> = run_spawn_provision_guarded(
                &ledger,
                &mob_id,
                "spawn provisioning task",
                &identity,
                async { panic!("persistent furnace payload") },
            )
            .await;
        }

        let logs = captured(&buf);
        assert_eq!(
            logs.matches("persistent furnace payload").count(),
            2,
            "first sighting plus the eighth repeat should log, got: {logs}"
        );
        assert!(
            logs.contains("repeated_sightings=8"),
            "periodic report must expose its cumulative repeat count: {logs}"
        );
    }

    #[tokio::test]
    async fn successful_provision_clears_the_ledger_so_a_recurrence_logs_fresh() {
        let (buf, _guard) = capture_error_logs();
        let ledger = SpawnPanicLogLedger::default();
        let mob_id = MobId::from("panic-capture-mob");
        let identity = AgentIdentity::from("member-child");

        let panic_once = || {
            run_spawn_provision_guarded(
                &ledger,
                &mob_id,
                "spawn provisioning task",
                &identity,
                async { panic!("recurring incident payload") },
            )
        };

        let _: Result<(), MobError> = panic_once().await;
        run_spawn_provision_guarded(
            &ledger,
            &mob_id,
            "spawn provisioning task",
            &identity,
            async { Ok(()) },
        )
        .await
        .expect("successful provision must pass through");
        let _: Result<(), MobError> = panic_once().await;

        let logs = captured(&buf);
        assert_eq!(
            logs.matches("recurring incident payload").count(),
            2,
            "a success between incidents must reset the rate limit, got: {logs}"
        );
    }
}
