//! `catch_unwind` boundary guard for machine attachment/publication sites.
//!
//! Twin of `meerkat-mob/src/runtime/panic_capture.rs`, built on the shared
//! `meerkat_core::panic_payload` helpers.
//!
//! WHY (field incident, 2026-07-29): a panic during member provisioning was
//! caught at a `catch_unwind` boundary with the payload discarded (`|_|`),
//! converted to an opaque typed error, and eagerly retried by the caller —
//! an invisible 99%-CPU panic furnace for 40+ minutes across three releases,
//! fueled by per-panic DWARF symbolication (gimli, ~280MB binary) inside the
//! panic hook, whose stderr line the deployment piped to /dev/null. The
//! machine's executor-factory / surface-activation / surface-publication
//! catches sit directly on the session-service create path that mob
//! provisioning transactions await, so a swallowed payload here leaves the
//! same furnace invisible. Every catch must therefore (a) recover and log
//! the payload — once per distinct payload per boundary+session, never per
//! iteration — and (b) carry the payload inside the typed error so it
//! survives even where logs are filtered.
//!
//! panic=abort compatibility: `catch_unwind` observes nothing there; this
//! code only runs when unwinding actually delivered a payload.

use crate::traits::RuntimeDriverError;
use meerkat_core::SessionId;
use meerkat_core::panic_payload::{PanicPayloadLogGate, panic_payload_detail};

/// Run one synchronous boundary action under panic capture.
///
/// The action runs exactly once; a caught panic becomes a typed
/// [`RuntimeDriverError`] built by `build_message` from the recovered payload
/// detail (append the detail so existing message prefixes stay stable). This
/// boundary itself NEVER retries — retry policy belongs to the caller, where
/// it is visible and typed. A success clears the gate key so a later
/// recurrence of the same panic logs fresh.
pub(crate) fn run_boundary_guarded<T>(
    gate: &PanicPayloadLogGate,
    boundary: &'static str,
    session_id: &SessionId,
    build_message: impl FnOnce(&str) -> String,
    action: impl FnOnce() -> T,
) -> Result<T, RuntimeDriverError> {
    let gate_key = format!("{boundary}:{session_id}");
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        Ok(value) => {
            gate.clear(&gate_key);
            Ok(value)
        }
        Err(payload) => {
            let detail = panic_payload_detail(payload.as_ref());
            if gate.first_sighting(&gate_key, &detail) {
                tracing::error!(
                    %session_id,
                    boundary,
                    panic = %detail,
                    "runtime attachment boundary panicked; payload recovered and converted to a typed driver error (logged once per distinct payload)"
                );
            }
            Err(RuntimeDriverError::Internal(build_message(&detail)))
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
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
    fn panicking_boundary_yields_typed_error_with_payload_and_logs_it() {
        let (buf, _guard) = capture_error_logs();
        let gate = PanicPayloadLogGate::default();
        let session_id = SessionId::new();

        let result: Result<(), RuntimeDriverError> = run_boundary_guarded(
            &gate,
            "executor-factory",
            &session_id,
            |detail| {
                format!("executor factory panicked while attaching session {session_id}: {detail}")
            },
            || panic!("unregistered residue factory panic"),
        );

        // (b) The typed error carries the recovered payload, so it survives
        // even where logs are filtered.
        let error = result.expect_err("a panicking boundary must fail");
        let RuntimeDriverError::Internal(message) = &error else {
            panic!("caught panic must classify as RuntimeDriverError::Internal, got {error:?}");
        };
        assert!(
            message.contains("executor factory panicked while attaching session"),
            "the historical message prefix must be preserved, got: {message}"
        );
        assert!(
            message.contains("unregistered residue factory panic"),
            "typed error must carry the panic payload, got: {message}"
        );

        // (a) The payload appears in the tracing output with context.
        let logs = captured(&buf);
        assert!(
            logs.contains("unregistered residue factory panic"),
            "the panic payload must be logged, got: {logs}"
        );
        assert!(
            logs.contains(&session_id.to_string()),
            "the log line must carry the session context, got: {logs}"
        );
    }

    #[test]
    fn repeated_identical_panics_log_once_and_success_resets_the_gate() {
        let (buf, _guard) = capture_error_logs();
        let gate = PanicPayloadLogGate::default();
        let session_id = SessionId::new();
        for _ in 0..3 {
            let result: Result<(), RuntimeDriverError> = run_boundary_guarded(
                &gate,
                "executor-factory",
                &session_id,
                |detail| format!("boundary panicked: {detail}"),
                || panic!("repeated boundary payload"),
            );
            // Every iteration still yields the typed error with the payload;
            // only the error! line is rate-limited.
            let message = result.expect_err("panic must fail").to_string();
            assert!(message.contains("repeated boundary payload"));
        }

        // (c) Rate limit holds: three identical panics, one error! line.
        let logs = captured(&buf);
        assert_eq!(
            logs.matches("repeated boundary payload").count(),
            1,
            "identical repeated panics must log exactly once, got: {logs}"
        );

        // A success at the boundary resets the gate.
        run_boundary_guarded(
            &gate,
            "executor-factory",
            &session_id,
            |detail| format!("boundary panicked: {detail}"),
            || (),
        )
        .expect("non-panicking boundary must pass through");
        let _: Result<(), RuntimeDriverError> = run_boundary_guarded(
            &gate,
            "executor-factory",
            &session_id,
            |detail| format!("boundary panicked: {detail}"),
            || panic!("repeated boundary payload"),
        );
        let logs = captured(&buf);
        assert_eq!(
            logs.matches("repeated boundary payload").count(),
            2,
            "a success between incidents must reset the rate limit, got: {logs}"
        );
    }
}
