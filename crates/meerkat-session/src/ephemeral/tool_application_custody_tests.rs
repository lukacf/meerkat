#![allow(clippy::unwrap_used)]

use std::future::Future;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Barrier;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use super::{SessionError, SessionId, ToolApplicationCustody};

#[test]
fn tool_application_custody_active_work_refuses_retirement_without_sealing() {
    let custody = Arc::new(ToolApplicationCustody::default());
    let id = SessionId::new();
    let lease = custody.acquire(&id).unwrap();
    let retired = AtomicUsize::new(0);
    let result = custody.close_if_idle(&id, || {
        retired.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    assert!(matches!(result, Err(SessionError::Busy { id: failed }) if failed == id));
    assert_eq!(retired.load(Ordering::SeqCst), 0);
    assert!(matches!(
        custody.reserve_lifecycle(&id),
        Err(SessionError::Busy { id: failed }) if failed == id
    ));
    let later_lease = custody.acquire(&id).unwrap();
    drop(lease);
    drop(later_lease);
    custody
        .close_if_idle(&id, || {
            retired.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    assert_eq!(retired.load(Ordering::SeqCst), 1);
    assert!(matches!(
        custody.acquire(&id),
        Err(SessionError::NotFound { id: failed }) if failed == id
    ));
}

#[test]
fn tool_application_custody_temporary_reservations_reopen_only_after_the_last_drop() {
    let custody = Arc::new(ToolApplicationCustody::default());
    let id = SessionId::new();
    let first = custody.reserve_lifecycle(&id).unwrap();
    let second = custody.reserve_lifecycle(&id).unwrap();
    assert!(matches!(
        custody.acquire(&id),
        Err(SessionError::NotFound { id: failed }) if failed == id
    ));
    drop(first);
    assert!(matches!(
        custody.acquire(&id),
        Err(SessionError::NotFound { id: failed }) if failed == id
    ));
    drop(second);
    drop(custody.acquire(&id).unwrap());
}

#[test]
fn tool_application_custody_shutdown_is_not_undone_by_reservation_drop() {
    let custody = Arc::new(ToolApplicationCustody::default());
    let id = SessionId::new();
    let reservation = custody.reserve_lifecycle(&id).unwrap();
    custody.close();
    drop(reservation);
    assert!(matches!(
        custody.acquire(&id),
        Err(SessionError::NotFound { id: failed }) if failed == id
    ));
}

#[test]
fn tool_application_custody_failed_retirement_does_not_close_admission() {
    let custody = Arc::new(ToolApplicationCustody::default());
    let id = SessionId::new();
    let result = custody.close_if_idle(&id, || {
        Err::<(), _>(SessionError::Agent(meerkat_core::AgentError::Cancelled))
    });
    assert!(matches!(
        result,
        Err(SessionError::Agent(meerkat_core::AgentError::Cancelled))
    ));
    drop(custody.acquire(&id).unwrap());
}

#[test]
fn tool_application_custody_drain_waits_for_every_exact_lease() {
    let custody = Arc::new(ToolApplicationCustody::default());
    let id = SessionId::new();
    let first = custody.acquire(&id).unwrap();
    let second = custody.acquire(&id).unwrap();
    custody.close();
    let mut drain = Box::pin(custody.wait_drained());
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(drain.as_mut().poll(&mut context), Poll::Pending));
    drop(first);
    assert!(matches!(drain.as_mut().poll(&mut context), Poll::Pending));
    drop(second);
    assert!(matches!(drain.as_mut().poll(&mut context), Poll::Ready(())));
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::panic)]
fn tool_application_custody_acquisition_and_retirement_are_mutually_exclusive() {
    for _ in 0..64 {
        let custody = Arc::new(ToolApplicationCustody::default());
        let id = SessionId::new();
        let start = Barrier::new(2);
        let retired = AtomicUsize::new(0);
        let (acquired, closed) = std::thread::scope(|scope| {
            let acquire = scope.spawn(|| {
                start.wait();
                custody.acquire(&id)
            });
            let close = scope.spawn(|| {
                start.wait();
                custody.close_if_idle(&id, || {
                    retired.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            });
            // Retain the acquired lease until the racing close has returned.
            (acquire.join().unwrap(), close.join().unwrap())
        });
        match (acquired, closed) {
            (Ok(lease), Err(SessionError::Busy { id: failed })) => {
                assert_eq!(failed, id);
                assert_eq!(retired.load(Ordering::SeqCst), 0);
                drop(lease);
                drop(custody.acquire(&id).unwrap());
            }
            (Err(SessionError::NotFound { id: failed }), Ok(())) => {
                assert_eq!(failed, id);
                assert_eq!(retired.load(Ordering::SeqCst), 1);
                assert!(matches!(
                    custody.acquire(&id),
                    Err(SessionError::NotFound { id: failed }) if failed == id
                ));
            }
            _ => panic!("retirement and an entered application lease must never coexist"),
        }
    }
}
