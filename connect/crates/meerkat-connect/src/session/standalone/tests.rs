//! Consumer helper controls. These do not qualify a native issuer, staged
//! authentication, ticketed actor, installed gateway or terminal subprocess.

use super::*;
use std::{
    error::Error,
    sync::{Mutex, atomic::{AtomicUsize, Ordering}},
    task::Poll,
};
use meerkat::governed_realm::{staged::StagedRealmError, stock::issuance::StockGrantError};
use tokio::sync::oneshot;

#[derive(Clone, Default)]
struct ObservedOutput(Arc<Mutex<Vec<u8>>>);

impl Write for ObservedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[tokio::test]
async fn entered_command_settles_after_cancellation_or_listener_error_without_repoll() {
    for listener_error in [false, true] {
        let entries = Arc::new(AtomicUsize::new(0));
        let settlements = Arc::new(AtomicUsize::new(0));
        let (release, released) = oneshot::channel();
        let (stop, mut stopped) = oneshot::channel::<io::Result<()>>();
        let mut consumed = false;
        let interrupt = futures_util::future::poll_fn(move |cx| {
            assert!(!consumed, "a consumed terminal interrupt was polled again");
            match Pin::new(&mut stopped).poll(cx) {
                Poll::Ready(result) => {
                    consumed = true;
                    Poll::Ready(result.expect("one terminal signal"))
                }
                Poll::Pending => Poll::Pending,
            }
        });
        tokio::pin!(interrupt);
        let command = {
            let entries = entries.clone();
            let settlements = settlements.clone();
            async move {
                entries.fetch_add(1, Ordering::SeqCst);
                let result: Result<StockGrantOutcome, StockGrantError> =
                    released.await.expect("retain this exact entered command");
                settlements.fetch_add(1, Ordering::SeqCst);
                result
            }
        };
        let mut output = ObservedOutput::default();
        let observed = output.clone();
        let joined = join_entered(command, &mut output, interrupt.as_mut());
        tokio::pin!(joined);
        assert!(joined.as_mut().now_or_never().is_none());
        assert_eq!(entries.load(Ordering::SeqCst), 1);
        stop.send(if listener_error { Err(io::Error::other("listener failed")) } else { Ok(()) })
            .expect("deliver cancellation after command entry");
        assert!(joined.as_mut().now_or_never().is_none(),
            "cancellation must not return while the entered command is pending");
        assert_eq!(observed.0.lock().unwrap().as_slice(), b"Waiting for native setup startup to settle\n");
        assert_eq!(settlements.load(Ordering::SeqCst), 0);
        release.send(Err(StockGrantError::CommandUncertain)).expect("one command outcome");
        let (outcome, signal, notice) = joined.await;
        notice.expect("waiting notice was written before command settlement");
        assert_eq!(entries.load(Ordering::SeqCst), 1);
        assert_eq!(settlements.load(Ordering::SeqCst), 1);
        let outcome = outcome.expect("entered command result was retained");
        assert!(matches!(&outcome, Err(StockGrantError::CommandUncertain)));
        if listener_error {
            assert_eq!(signal.expect("listener outcome").unwrap_err().to_string(), "listener failed");
        } else {
            signal.expect("cancellation outcome").expect("ordinary cancellation");
        }
        let directory = tempfile::tempdir().expect("private outcome directory");
        record(directory.path(), "mcp-grant-outcome", &format!("{outcome:#?}"))
            .expect("retain the observed outcome after cancellation");
        let mut output = Vec::new();
        report_grant(&mut output, "Setup helper access", outcome.as_ref()).expect("show actual classification");
        let output = String::from_utf8(output).expect("terminal text");
        assert!(output.contains("uncertain command outcome"));
        assert!(output.contains("No retry was started"));
        assert!(!output.contains("was granted"));
        let saved = std::fs::read_to_string(directory.path().join("standalone-mcp-grant-outcome.txt"))
            .expect("historical result");
        assert!(saved.contains("CommandUncertain"));
    }
}

#[tokio::test]
async fn ready_cancellation_or_listener_error_does_not_enter_command() {
    for listener_error in [false, true] {
        let entries = AtomicUsize::new(0);
        let mut consumed = false;
        let interrupt = futures_util::future::poll_fn(|_| {
            assert!(!consumed, "a consumed terminal interrupt was polled again");
            consumed = true;
            Poll::Ready(if listener_error { Err(io::Error::other("listener failed")) } else { Ok(()) })
        });
        tokio::pin!(interrupt);
        let command = async { entries.fetch_add(1, Ordering::SeqCst); };
        let mut output = Vec::new();
        let (outcome, signal, notice) = join_entered(command, &mut output, interrupt.as_mut()).await;
        notice.expect("no notice was needed");
        assert!(output.is_empty(), "unentered work is not reported as pending");
        assert!(outcome.is_none());
        assert_eq!(entries.load(Ordering::SeqCst), 0);
        assert_eq!(signal.expect("ready signal").is_err(), listener_error);
    }
}

#[test]
fn staged_refusal_is_not_reported_as_an_entered_uncertain_grant() {
    for error in [StagedRealmError::Ended,
        StagedRealmError::Grant(StockGrantError::CommandUncertain)]
    {
        let grant_entered = matches!(&error, StagedRealmError::Grant(_));
        let outcome = Err(error);
        let mut output = Vec::new();
        report_staged_grant(&mut output, &outcome).expect("borrow actual staged result type");
        let output = String::from_utf8(output).expect("terminal text");
        assert_eq!(output.contains("uncertain command outcome"), grant_entered);
        assert_eq!(output.contains("before this grant command entered"), !grant_entered);
        assert!(!output.contains("was granted"));
        // Reporting borrows the exact native error. It does not replace or
        // flatten the original staged result used by the caller.
        assert_eq!(matches!(outcome, Err(StagedRealmError::Grant(StockGrantError::CommandUncertain))), grant_entered);
    }
}

struct RefusedOutput;

impl Write for RefusedOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::Error::other("output closed")) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[tokio::test]
async fn failed_waiting_notice_keeps_entered_native_error_and_settlement() {
    let (release, released) = oneshot::channel::<Result<(), ConnectError>>();
    let (stop, mut stopped) = oneshot::channel();
    let mut consumed = false;
    let interrupt = futures_util::future::poll_fn(|cx| {
        assert!(!consumed, "the consumed interrupt cannot be polled again");
        match Pin::new(&mut stopped).poll(cx) {
            Poll::Ready(result) => { consumed = true; Poll::Ready(result.unwrap()) },
            Poll::Pending => Poll::Pending,
        }
    });
    tokio::pin!(interrupt);
    let entries = AtomicUsize::new(0);
    let settlements = AtomicUsize::new(0);
    let command = async {
        entries.fetch_add(1, Ordering::SeqCst);
        let result = released.await.unwrap();
        settlements.fetch_add(1, Ordering::SeqCst);
        result
    };
    let mut output = RefusedOutput;
    let joined = join_entered(command, &mut output, interrupt.as_mut());
    tokio::pin!(joined);
    assert!(joined.as_mut().now_or_never().is_none());
    stop.send(Ok(())).unwrap();
    assert!(joined.as_mut().now_or_never().is_none(), "notice failure cannot drop entered work");
    assert_eq!(entries.load(Ordering::SeqCst), 1);
    assert_eq!(settlements.load(Ordering::SeqCst), 0);
    let primary = ConnectError::native("Issue reviewed setup access", StockGrantError::CommandUncertain);
    let identity = match &primary {
        ConnectError::Native { source, .. } => source.as_ref() as *const (dyn Error + Send + Sync),
        _ => unreachable!(),
    };
    release.send(Err(primary)).unwrap();
    let (outcome, signal, notice) = joined.await;
    signal.unwrap().unwrap();
    assert_eq!(entries.load(Ordering::SeqCst), 1);
    assert_eq!(settlements.load(Ordering::SeqCst), 1);
    let result = combine(outcome.expect("same entered result"), notice);
    let ConnectError::Cleanup { primary, cleanup } = result.unwrap_err() else {
        panic!("native error and notice error must remain separate");
    };
    let ConnectError::Native { source, .. } = *primary else { panic!("native error remains primary"); };
    assert!(std::ptr::eq(identity, source.as_ref()));
    assert!(matches!(source.downcast_ref::<StockGrantError>(), Some(StockGrantError::CommandUncertain)));
    assert!(matches!(*cleanup, ConnectError::Native { operation: "Show setup outcome", .. }));
}

#[test]
fn native_finish_failure_remains_primary_when_record_and_output_fail() {
    let directory = tempfile::tempdir().expect("outcome directory");
    let obstructed = directory.path().join("standalone-stage-finish.txt");
    std::fs::create_dir(&obstructed).expect("refuse create-new outcome file");
    let primary = ConnectError::native("Finish governed setup stage", StagedRealmError::GovernanceUnavailable);
    let source_identity = match &primary {
        ConnectError::Native { source, .. } => source.as_ref() as *const (dyn Error + Send + Sync),
        _ => unreachable!(),
    };
    let retained = record(directory.path(), "stage-finish", "GovernanceUnavailable");
    assert!(retained.is_err());
    let shown = report(&mut RefusedOutput, "Setup ownership is unavailable.");
    assert!(shown.is_err());
    let result = combine::<()>(Err(primary), combine(retained, shown));
    let ConnectError::Cleanup { primary, cleanup } = result.unwrap_err() else {
        panic!("both primary and reporting failures must survive");
    };
    let ConnectError::Native { operation, source } = *primary else {
        panic!("native failure remains primary");
    };
    assert_eq!(operation, "Finish governed setup stage");
    assert!(std::ptr::eq(source_identity, source.as_ref()));
    assert!(matches!(source.downcast_ref::<StagedRealmError>(), Some(StagedRealmError::GovernanceUnavailable)));
    let ConnectError::Cleanup { primary: retention, cleanup: output } = *cleanup else {
        panic!("both reporting failures remain separate");
    };
    assert!(retention.to_string().contains("GovernanceUnavailable"));
    assert!(matches!(*output, ConnectError::Native { operation: "Show setup outcome", .. }));
    assert!(obstructed.is_dir(), "the failed record never overwrites existing evidence");
}
