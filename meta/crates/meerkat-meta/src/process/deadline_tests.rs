#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
use serde_json::{Value, json};

use super::{
    ACTIONS, COMPLETION_GATE, OPERATION_STARTED, POLL_BUDGETS, PollBudget, ProcessBoundaryError,
    run_child,
};
use crate::process::run_process_json_line;

const FIXTURE: &str = r"
import json, os, pathlib, socket, sys, time
folder, mode, nonce, address = sys.argv[1:]
folder = pathlib.Path(folder)
parent = os.getpid()
(folder/'entered.json').write_text(json.dumps({'pid': parent, 'ppid': os.getppid(), 'nonce': nonce}))
if mode.startswith('descendant-'):
    descendant = os.fork()
    if descendant == 0:
        # This process is deliberately outside the one-shot direct-child owner.
        # Its bounded control socket lets the test release it on every path.
        host, port = address.split(':')
        with socket.create_connection((host, int(port)), timeout=5) as control:
            control.settimeout(5)
            control.sendall((json.dumps({'pid': os.getpid(), 'created_by': parent, 'nonce': nonce})+'\n').encode())
            stream = control.makefile('rb')
            if stream.readline() == b'probe\n':
                try:
                    os.write(1, b'x')
                    result = 'reader-open'
                except BrokenPipeError:
                    result = 'reader-closed'
                control.sendall((result+'\n').encode())
                stream.readline()
        os._exit(0)
    if mode == 'descendant-success':
        print('{}', flush=True)
        sys.exit(0)
if mode in ('json-hold', 'json-exit'):
    print('{}', flush=True)
elif mode == 'late-json-hold':
    time.sleep(0.30)
    print('{}', flush=True)
elif mode in ('eof-hold', 'empty-exit'):
    os.close(1)
elif mode == 'crlf':
    os.write(1, b'{}\r\n')
elif mode == 'unterminated':
    os.write(1, b'{}')
elif mode == 'invalid-json':
    print('not-json', flush=True)
elif mode == 'invalid-utf8':
    os.write(1, b'\xff\n')
elif mode == 'excess':
    print('{}', flush=True)
    remaining = b'x' * (2 * 1024 * 1024)
    while remaining:
        remaining = remaining[os.write(1, remaining):]
    (folder/'drained').write_text('complete')
if mode.endswith('-hold') or mode == 'descendant-timeout':
    # A fixture watchdog bounds prerequisite failures without the harness
    # pretending its own release was settlement performed by the candidate.
    until = time.monotonic() + 5
    while not (folder/'release').exists() and time.monotonic() < until:
        time.sleep(0.005)
";

struct Observation {
    result: Result<String, ProcessBoundaryError>,
    elapsed: Duration,
    returned_before_release: bool,
    child: Value,
    actions: Vec<(u32, &'static str)>,
    operation_started: Option<Instant>,
    poll_budgets: Vec<PollBudget>,
    excess_drained: bool,
    holder: Option<Value>,
    holder_probe: HolderProbe,
    holder_control_eof: bool,
}

/// The descendant's answer to the reader probe, kept literally: the line it
/// sent back, how many bytes were read, and the write or read error, if any.
#[derive(Debug, Default)]
struct HolderProbe {
    reply: Option<String>,
    read_bytes: Option<usize>,
    error: Option<String>,
}

impl HolderProbe {
    /// The exact reader-closed verdict; `None` when no reply was read.
    fn reader_closed(&self) -> Option<bool> {
        self.reply
            .as_deref()
            .map(|reply| reply == "reader-closed\n")
    }
}

fn wait_for_receipt(path: &Path) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(receipt) = serde_json::from_slice(&bytes)
        {
            return Some(receipt);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn accept_holder(listener: &TcpListener) -> Option<BufReader<TcpStream>> {
    listener.set_nonblocking(true).ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets can inherit the listener's nonblocking mode.
                // This control channel uses bounded blocking reads below.
                stream.set_nonblocking(false).ok()?;
                stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .ok()?;
                return Some(BufReader::new(stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

fn exercise(mode: &str, timeout: Duration) -> Observation {
    let dir = tempfile::tempdir().expect("fixture directory");
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture control listener");
    let address = listener.local_addr().expect("fixture address").to_string();
    let nonce = dir.path().to_string_lossy().into_owned();
    let args = vec![
        "-c".to_owned(),
        FIXTURE.to_owned(),
        nonce.clone(),
        mode.to_owned(),
        nonce.clone(),
        address,
    ];
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        ACTIONS.with_borrow_mut(Vec::clear);
        POLL_BUDGETS.with_borrow_mut(Vec::clear);
        OPERATION_STARTED.with(|value| value.set(None));
        let started = Instant::now();
        let result = run_process_json_line("python3", &args, &[], timeout);
        let actions = ACTIONS.with_borrow(Clone::clone);
        let poll_budgets = POLL_BUDGETS.with_borrow(Clone::clone);
        let operation_started = OPERATION_STARTED.with(std::cell::Cell::get);
        let _ = sender.send((
            result,
            started.elapsed(),
            actions,
            operation_started,
            poll_budgets,
        ));
    });
    // Collect prerequisite observations without panicking while owners run.
    let child = wait_for_receipt(&dir.path().join("entered.json"));
    let mut holder = mode
        .starts_with("descendant-")
        .then(|| accept_holder(&listener))
        .flatten();
    let holder_receipt = holder.as_mut().and_then(|holder| {
        let mut line = String::new();
        holder.read_line(&mut line).ok()?;
        serde_json::from_str::<Value>(&line).ok()
    });
    let before_release = receiver.recv_timeout(Duration::from_millis(1500)).ok();
    let returned_before_release = before_release.is_some();
    let mut holder_probe = HolderProbe::default();
    let mut holder_control_eof = false;
    if let Some(holder) = &mut holder {
        match holder.get_mut().write_all(b"probe\n") {
            Ok(()) => {
                let mut response = String::new();
                match holder.read_line(&mut response) {
                    Ok(read) => {
                        holder_probe.read_bytes = Some(read);
                        holder_probe.reply = Some(response);
                    }
                    Err(error) => holder_probe.error = Some(format!("read: {error}")),
                }
            }
            Err(error) => holder_probe.error = Some(format!("write: {error}")),
        }
        let _ = holder.get_mut().write_all(b"release\n");
        let mut remaining = Vec::new();
        holder_control_eof = holder.read_to_end(&mut remaining).is_ok();
    }
    let release = fs::write(dir.path().join("release"), b"release");
    let settled = before_release.or_else(|| receiver.recv_timeout(Duration::from_secs(7)).ok());
    let joined = worker.join();
    // Only assert after releasing both fixtures and joining the actual caller.
    assert!(release.is_ok(), "fixture release failed");
    assert!(joined.is_ok(), "process caller panicked");
    let child = child.expect("actual Python child must establish its exact receipt");
    assert_eq!(child["ppid"], json!(std::process::id()));
    assert_eq!(child["nonce"], nonce);
    let pid =
        Pid::from_raw(i32::try_from(child["pid"].as_u64().expect("child PID")).expect("PID range"))
            .expect("nonzero PID");
    // WNOWAIT makes even this negative check non-reaping. The fixture PID was
    // authenticated before the caller returned, not discovered by process scan.
    assert_eq!(
        waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG
        )
        .expect_err("the direct child must already be reaped"),
        rustix::io::Errno::CHILD
    );
    let (result, elapsed, actions, operation_started, poll_budgets) =
        settled.expect("released child caller must settle");
    Observation {
        result,
        elapsed,
        returned_before_release,
        child,
        actions,
        operation_started,
        poll_budgets,
        excess_drained: dir.path().join("drained").exists(),
        holder: holder_receipt,
        holder_probe,
        holder_control_eof,
    }
}

fn assert_one_reap(observation: &Observation) {
    let pid =
        u32::try_from(observation.child["pid"].as_u64().expect("child PID")).expect("PID range");
    let actions: Vec<_> = observation
        .actions
        .iter()
        .filter_map(|(id, action)| (*id == pid).then_some(*action))
        .collect();
    assert_eq!(
        actions.iter().filter(|action| **action == "wait").count(),
        1
    );
    assert_eq!(actions.last(), Some(&"reaped"));
    let reaped = actions
        .iter()
        .position(|action| *action == "reaped")
        .expect("reap action");
    assert!(!actions[reaped + 1..].contains(&"kill"));
}

#[test]
fn process_deadline_includes_exit_after_json_and_eof() {
    for mode in ["json-hold", "eof-hold", "late-json-hold"] {
        let observation = exercise(mode, Duration::from_millis(500));
        assert!(
            observation.returned_before_release,
            "{mode}: caller exceeded observation window"
        );
        assert_eq!(
            observation.result,
            Err(ProcessBoundaryError::Timeout { timeout_ms: 500 })
        );
        assert!(observation.elapsed < Duration::from_millis(1500));
        assert_one_reap(&observation);
        if mode == "late-json-hold" {
            let deadline = observation
                .operation_started
                .expect("actual operation start")
                + Duration::from_millis(500);
            let post_line: Vec<_> = observation
                .poll_budgets
                .iter()
                .filter(|budget| budget.after_first_line)
                .collect();
            assert!(
                !post_line.is_empty(),
                "actual first line must precede an exit wait"
            );
            for budget in post_line {
                assert!(
                    budget.observed + budget.wait <= deadline,
                    "post-line poll must use the remaining original budget, not a reset"
                );
            }
        }
    }
}

#[test]
fn process_deadline_preserves_first_line_and_empty_output_semantics() {
    for mode in ["json-exit", "crlf", "unterminated"] {
        let observation = exercise(mode, Duration::from_secs(2));
        assert_eq!(observation.result, Ok("{}".to_owned()), "{mode}");
        assert_one_reap(&observation);
    }
    let empty = exercise("empty-exit", Duration::from_secs(2));
    assert_eq!(empty.result, Err(ProcessBoundaryError::EmptyOutput));
    let invalid = exercise("invalid-json", Duration::from_secs(2));
    assert_eq!(invalid.result, Err(ProcessBoundaryError::InvalidJsonLine));
    let invalid_utf8 = exercise("invalid-utf8", Duration::from_secs(2));
    assert!(matches!(
        invalid_utf8.result,
        Err(ProcessBoundaryError::Io(_))
    ));
}

#[test]
fn process_deadline_drains_excess_stdout_before_child_exit() {
    let observation = exercise("excess", Duration::from_secs(2));
    assert_eq!(observation.result, Ok("{}".to_owned()));
    assert!(
        observation.excess_drained,
        "child must complete the real 2 MiB pipe write"
    );
    assert_one_reap(&observation);
}

#[test]
fn process_deadline_child_exit_wakes_poll_while_descendant_retains_stdout() {
    // Concurrent spawns in one process share its descriptor table: run as
    // the only test of a child process.
    if !crate::test_process_isolation::run_body_in_isolated_process(
        module_path!(),
        "process_deadline_child_exit_wakes_poll_while_descendant_retains_stdout",
    ) {
        return;
    }
    let observation = exercise("descendant-success", Duration::from_secs(2));
    assert!(observation.returned_before_release);
    assert_eq!(observation.result, Ok("{}".to_owned()));
    let holder = observation
        .holder
        .as_ref()
        .expect("actual descendant control receipt");
    assert_eq!(holder["created_by"], observation.child["pid"]);
    assert_eq!(holder["nonce"], observation.child["nonce"]);
    assert_eq!(
        observation.holder_probe.reader_closed(),
        Some(true),
        "the descendant must find the reader closed; probe: {:?}",
        observation.holder_probe
    );
    assert!(
        observation.holder_control_eof,
        "fixture control socket must close after release"
    );
    assert_one_reap(&observation);
}

#[test]
fn process_deadline_timeout_closes_reader_even_when_descendant_holds_writer() {
    // Concurrent spawns in one process share its descriptor table: run as
    // the only test of a child process.
    if !crate::test_process_isolation::run_body_in_isolated_process(
        module_path!(),
        "process_deadline_timeout_closes_reader_even_when_descendant_holds_writer",
    ) {
        return;
    }
    let observation = exercise("descendant-timeout", Duration::from_millis(500));
    assert!(observation.returned_before_release);
    assert_eq!(
        observation.result,
        Err(ProcessBoundaryError::Timeout { timeout_ms: 500 })
    );
    let holder = observation
        .holder
        .as_ref()
        .expect("actual descendant control receipt");
    assert_eq!(holder["created_by"], observation.child["pid"]);
    assert_eq!(holder["nonce"], observation.child["nonce"]);
    assert_eq!(
        observation.holder_probe.reader_closed(),
        Some(true),
        "no detached reader may retain the pipe; probe: {:?}",
        observation.holder_probe
    );
    assert!(observation.holder_control_eof);
    assert_one_reap(&observation);
}

#[test]
fn process_deadline_exit_at_expired_boundary_is_not_reaped_before_kill_decision() {
    ACTIONS.with_borrow_mut(Vec::clear);
    let child = Command::new("python3")
        .args(["-c", "print('{}', flush=True)"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("real fixture child");
    let pid = Pid::from_child(&child);
    // Establish the disputed boundary deterministically: the OS has observed
    // exit but NOWAIT has not released the PID. The actual expired operation
    // then makes its kill decision before the sole Child::wait call.
    let observed = waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
    );
    let result = run_child(child, Instant::now(), Duration::ZERO);
    assert!(observed.expect("non-reaping exit observation").is_some());
    assert_eq!(result, Err(ProcessBoundaryError::Timeout { timeout_ms: 0 }));
    assert_eq!(
        waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG
        )
        .expect_err("caller must have performed the sole reap"),
        rustix::io::Errno::CHILD
    );
    let actions = ACTIONS.with_borrow(Clone::clone);
    assert_eq!(
        actions,
        vec![
            (pid.as_raw_pid() as u32, "kill"),
            (pid.as_raw_pid() as u32, "observer_join"),
            (pid.as_raw_pid() as u32, "wait"),
            (pid.as_raw_pid() as u32, "reaped"),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn process_deadline_sync_facade_remains_callable_inside_tokio() {
    let observation = exercise("json-exit", Duration::from_secs(2));
    assert_eq!(observation.result, Ok("{}".to_owned()));
    assert_one_reap(&observation);
    // exercise uses a worker for its cancellation-safe fixture orchestration;
    // this additional call enters the actual facade on the Tokio worker itself.
    assert_eq!(
        run_process_json_line(
            "python3",
            &["-c".to_owned(), "print('{}')".to_owned()],
            &[],
            Duration::from_secs(2)
        ),
        Ok("{}".to_owned())
    );
}

#[test]
fn process_deadline_missing_stdout_still_reaps_the_spawned_child() {
    ACTIONS.with_borrow_mut(Vec::clear);
    let child = Command::new("python3")
        .args(["-c", "import time; time.sleep(5)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("real fixture child without piped stdout");
    let pid = Pid::from_child(&child);
    let result = run_child(child, Instant::now(), Duration::from_millis(500));
    assert_eq!(result, Err(ProcessBoundaryError::MissingStdout));
    assert_eq!(
        waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG
        )
        .expect_err("missing-stdout failure must still reap"),
        rustix::io::Errno::CHILD
    );
    assert_eq!(
        ACTIONS.with_borrow(Clone::clone),
        vec![
            (pid.as_raw_pid() as u32, "kill"),
            (pid.as_raw_pid() as u32, "wait"),
            (pid.as_raw_pid() as u32, "reaped"),
        ]
    );
}

#[test]
fn process_deadline_observer_error_wakes_the_same_readiness_wait() {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    let this_process = Pid::from_raw(std::process::id() as i32).expect("own PID");
    // The observer cannot wait for its own process as a child. This exercises
    // the real error path, without signaling a foreign PID or fabricating exit.
    let (notification, observer) = super::observe_exit(this_process).expect("observer thread");
    let mut fds = [PollFd::new(&notification, PollFlags::IN)];
    let wait = Timespec::try_from(Duration::from_secs(3)).expect("poll timeout");
    let ready = poll(&mut fds, Some(&wait));
    let result = super::join_observer(observer);
    assert_eq!(ready.expect("notification poll"), 1);
    assert!(!fds[0].revents().is_empty());
    assert_eq!(
        result
            .expect_err("waitid must refuse non-child")
            .raw_os_error(),
        Some(rustix::io::Errno::CHILD.raw_os_error())
    );
}

#[test]
fn process_deadline_completion_after_final_read_cannot_be_late_success() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        ACTIONS.with_borrow_mut(Vec::clear);
        COMPLETION_GATE.with_borrow_mut(|gate| *gate = Some((entered_tx, release_rx)));
        let result = run_process_json_line(
            "python3",
            &["-c".to_owned(), "print('{}', flush=True)".to_owned()],
            &[],
            Duration::from_secs(1),
        );
        let actions = ACTIONS.with_borrow(Clone::clone);
        let _ = result_tx.send((result, actions));
    });
    // The production path enters this one-use test gate only after actual
    // line completion and actual non-reaping exit observation. Hold that
    // completed read/join segment until its recorded absolute deadline.
    let entered = entered_rx.recv_timeout(Duration::from_secs(3)).ok();
    let mut deadline_passed = false;
    let early = entered.and_then(|deadline| {
        let response = result_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok();
        deadline_passed = Instant::now() >= deadline;
        response
    });
    let _ = release_tx.send(());
    drop(release_tx);
    let result = early
        .clone()
        .or_else(|| result_rx.recv_timeout(Duration::from_secs(3)).ok());
    let joined = worker.join();
    // A missing handshake or an early error cannot strand the actual caller.
    assert!(joined.is_ok());
    assert!(
        entered.is_some(),
        "fixture must reach actual final completion observations"
    );
    assert!(early.is_none(), "gate must hold the actual completion path");
    assert!(deadline_passed);
    let (result, actions) = result.expect("released actual caller must settle");
    assert_eq!(
        result,
        Err(ProcessBoundaryError::Timeout { timeout_ms: 1000 })
    );
    assert_eq!(
        actions
            .iter()
            .filter(|(_, action)| *action == "wait")
            .count(),
        1
    );
    assert_eq!(actions.last().map(|(_, action)| *action), Some("reaped"));
    let pid = Pid::from_raw(actions.last().expect("child action").0 as i32).expect("child PID");
    assert_eq!(
        waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG
        )
        .expect_err("late completion must still reap the direct child"),
        rustix::io::Errno::CHILD
    );
}
