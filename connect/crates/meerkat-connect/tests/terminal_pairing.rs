//! Only newly allocated PTYs and loopback HTTP fixtures are used here. No test
//! opens /dev/tty, reads process stdin, signals a user session or contacts a host.

use std::{
    io::{self, Write},
    os::fd::{AsFd, OwnedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use meerkat_connect::{pairing::pair_paired_host, terminal::TerminalInput};
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    poll::{PollFd, PollFlags, poll},
    pty::openpty,
    sys::termios::{LocalFlags, SpecialCharacterIndices, Termios, tcgetattr},
    unistd::{pipe, read, write},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::{sleep, timeout},
};

const BOUND: Duration = Duration::from_secs(5);
const CODE: &str = "672491";
const TERMINAL_CODE: &str = "ABCD-EFGH-JKMN";

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Pty {
    master: OwnedFd,
    observer: OwnedFd,
    original: Termios,
    flags: i32,
    input: TerminalInput,
}

impl Pty {
    fn new() -> Self {
        let pty = openpty(None, None).unwrap();
        let observer = pty.slave.try_clone().unwrap();
        let original = tcgetattr(&observer).unwrap();
        assert!(original.local_flags.contains(LocalFlags::ECHO));
        let flags = fcntl(&observer, FcntlArg::F_GETFL).unwrap();
        let master_flags = OFlag::from_bits_retain(fcntl(&pty.master, FcntlArg::F_GETFL).unwrap());
        fcntl(
            &pty.master,
            FcntlArg::F_SETFL(master_flags | OFlag::O_NONBLOCK),
        )
        .unwrap();
        Self {
            master: pty.master,
            observer,
            original,
            flags,
            input: TerminalInput::from_owned_fd(pty.slave),
        }
    }

    fn restored(&self) {
        assert_eq!(tcgetattr(&self.observer).unwrap(), self.original);
        assert_eq!(
            fcntl(&self.observer, FcntlArg::F_GETFL).unwrap(),
            self.flags
        );
    }
}

fn send(fd: &OwnedFd, bytes: &[u8]) {
    assert_eq!(write(fd, bytes).unwrap(), bytes.len());
}

fn echoed(fd: &OwnedFd) -> Vec<u8> {
    let mut all = Vec::new();
    let mut bytes = [0; 512];
    loop {
        let mut fds = [PollFd::new(fd.as_fd(), PollFlags::POLLIN)];
        if poll(&mut fds, 0u16).unwrap() == 0 {
            break;
        }
        match read(fd, &mut bytes) {
            Ok(0) => break,
            Ok(n) => all.extend_from_slice(&bytes[..n]),
            Err(nix::errno::Errno::EAGAIN) => break,
            Err(error) => panic!("isolated PTY read failed: {error}"),
        }
    }
    all
}

async fn hidden(observer: &OwnedFd) {
    timeout(BOUND, async {
        while tcgetattr(observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
        {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

async fn shown(output: &Output, text: &str) {
    timeout(BOUND, async {
        while !output.text().contains(text) {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn request_driven_reads_hide_the_code_and_discard_secret_phase_typeahead() {
    let mut pty = Pty::new();
    send(&pty.master, b"ordinary first line\nold queued line\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "ordinary first line"
    );
    pty.restored();
    let _ = echoed(&pty.master);
    let mut output = Output::default();
    let reader = pty.input.read_pairing_code(&mut output);
    let writer = async {
        hidden(&pty.observer).await;
        send(&pty.master, format!("{CODE}\n{CODE}\n").as_bytes());
    };
    let (result, ()) = timeout(BOUND, async { tokio::join!(reader, writer) })
        .await
        .unwrap();
    assert_eq!(
        format!("{:?}", result.unwrap().unwrap()),
        "PairingCode([redacted])"
    );
    pty.restored();
    assert!(
        echoed(&pty.master).is_empty(),
        "protected entry echoed bytes"
    );
    assert!(!output.text().contains(CODE));
    send(&pty.master, b"fresh ordinary line\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "fresh ordinary line"
    );
    pty.restored();
}

#[tokio::test]
async fn dropping_a_pending_secret_future_restores_exact_flags_and_flushes_partial_code() {
    let mut pty = Pty::new();
    let flags = OFlag::from_bits_retain(pty.flags) | OFlag::O_NONBLOCK;
    fcntl(&pty.observer, FcntlArg::F_SETFL(flags)).unwrap();
    pty.flags = flags.bits();
    let mut output = Output::default();
    let mut pending = Box::pin(pty.input.read_pairing_code(&mut output));
    tokio::select! {
        result = &mut pending => panic!("secret unexpectedly completed: {result:?}"),
        () = hidden(&pty.observer) => (),
    }
    send(&pty.master, &CODE.as_bytes()[..3]);
    drop(pending);
    pty.restored();
    assert!(echoed(&pty.master).is_empty());
    send(&pty.master, b"after cancelled secret\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "after cancelled secret"
    );
    assert!(!output.text().contains(&CODE[..3]));
    pty.restored();
}

#[tokio::test]
async fn eof_quit_and_output_failure_restore_the_terminal() {
    for entry in [None, Some("/quit")] {
        let mut pty = Pty::new();
        let eof = pty.original.control_chars[SpecialCharacterIndices::VEOF as usize];
        let mut output = Output::default();
        let writer = async {
            hidden(&pty.observer).await;
            match entry {
                Some(text) => send(&pty.master, format!("{text}\n").as_bytes()),
                None => send(&pty.master, &[eof]),
            }
        };
        let (result, ()) = timeout(BOUND, async {
            tokio::join!(pty.input.read_pairing_code(&mut output), writer)
        })
        .await
        .unwrap();
        assert!(result.unwrap().is_none());
        pty.restored();
        assert!(echoed(&pty.master).is_empty());
    }
    struct FailingOutput;
    impl Write for FailingOutput {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("fixture output closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut pty = Pty::new();
    assert!(
        pty.input
            .read_pairing_code(&mut FailingOutput)
            .await
            .is_err()
    );
    pty.restored();
}

#[tokio::test]
async fn nonterminal_secret_entry_refuses_before_any_host_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (read_end, _write_end) = pipe().unwrap();
    let mut input = TerminalInput::from_owned_fd(read_end);
    let mut output = Output::default();
    let error = pair_paired_host(&origin, &mut input, &mut output, std::future::pending())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("protected entry requires a terminal")
    );
    assert!(output.text().is_empty());
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

struct Request {
    head: String,
    body: Value,
}

async fn request(socket: &mut TcpStream) -> Request {
    timeout(BOUND, async {
        let mut bytes = Vec::new();
        let end = loop {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 8192);
            if bytes.ends_with(b"\r\n\r\n") {
                break bytes.len();
            }
        };
        let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
        let length: usize = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .unwrap()
            .parse()
            .unwrap();
        assert!(length < 8192);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        Request {
            head,
            body: serde_json::from_slice(&body).unwrap(),
        }
    })
    .await
    .unwrap()
}

async fn acknowledgement(socket: &mut TcpStream, confirmed: bool) {
    let cookie = if confirmed {
        "paired_host_device=synthetic-device-cookie; Path=/; HttpOnly; SameSite=Lax"
    } else {
        "paired_host_pair=synthetic-pair-cookie; Path=/console/pair; HttpOnly; SameSite=Strict"
    };
    let bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nSet-Cookie: {cookie}\r\nConnection: close\r\n\r\n{{\"ok\":true}}"
    );
    socket.write_all(bytes.as_bytes()).await.unwrap();
    socket.shutdown().await.unwrap();
}

async fn local_pairing_code_correction(terminal: bool, accepted: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let requests = AtomicUsize::new(0);
    let confirmations = AtomicUsize::new(0);
    let corrected = AtomicBool::new(false);
    let (choice, malformed, valid, guidance, endpoint) = if terminal {
        (
            "terminal",
            "ABCD-ILOO-1234",
            TERMINAL_CODE,
            "Please enter a 12-symbol host-terminal pairing code",
            "/console/pair/terminal",
        )
    } else {
        (
            "request",
            "67249",
            CODE,
            "Please enter a six-digit pairing code",
            "/console/pair/confirm",
        )
    };
    let server = async {
        if !terminal {
            let (mut socket, _) = listener.accept().await.unwrap();
            let received = request(&mut socket).await;
            requests.fetch_add(1, Ordering::SeqCst);
            assert!(received.head.starts_with("POST /console/pair/request "));
            assert_eq!(received.body, json!({"label":"Meerkat Connect CLI"}));
            acknowledgement(&mut socket, false).await;
        }
        let (mut socket, _) = listener.accept().await.unwrap();
        let received = request(&mut socket).await;
        confirmations.fetch_add(1, Ordering::SeqCst);
        assert!(received.head.starts_with(&format!("POST {endpoint} ")));
        assert_eq!(
            received.body,
            json!({"code": if terminal { "ABCDEFGHJKMN" } else { CODE }})
        );
        if terminal {
            assert!(!received.head.contains("cookie:"));
        } else {
            assert!(
                received
                    .head
                    .contains("paired_host_pair=synthetic-pair-cookie")
            );
        }
        if accepted {
            acknowledgement(&mut socket, true).await;
        } else {
            let body = r#"{"error":"wrong_code"}"#;
            socket.write_all(format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
            ).as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    };
    let human = async {
        shown(&observed, "Type 'request'").await;
        send(&pty.master, format!("{choice}\n").as_bytes());
        hidden(&pty.observer).await;
        let _ = echoed(&pty.master);
        send(&pty.master, format!("{malformed}\n").as_bytes());
        shown(&observed, guidance).await;
        assert!(
            !tcgetattr(&pty.observer)
                .unwrap()
                .local_flags
                .contains(LocalFlags::ECHO)
        );
        assert!(echoed(&pty.master).is_empty());
        assert_eq!(requests.load(Ordering::SeqCst), usize::from(!terminal));
        assert_eq!(confirmations.load(Ordering::SeqCst), 0);
        send(&pty.master, format!("{valid}\n").as_bytes());
        corrected.store(true, Ordering::SeqCst);
    };
    let result = {
        let paired = pair_paired_host(&origin, &mut pty.input, &mut output, std::future::pending());
        let fixture = async {
            tokio::join!(server, human);
        };
        tokio::pin!(paired, fixture);
        timeout(BOUND, async {
            tokio::select! {
                result = &mut paired => {
                    if result.is_ok() { fixture.await; }
                    result
                }
                () = &mut fixture => paired.await,
            }
        })
        .await
        .unwrap()
    };
    pty.restored();
    assert!(echoed(&pty.master).is_empty());
    if accepted {
        assert!(
            result
                .expect("local format correction must keep pairing open")
                .is_some()
        );
    } else {
        use meerkat_connect::paired_host::{PairedHostError, PairedHostRefusal};
        match result.unwrap_err() {
            meerkat_connect::ConnectError::Native { operation, source } => {
                assert_eq!(operation, "confirm paired host pairing");
                assert!(matches!(
                    source.downcast_ref::<PairedHostError>(),
                    Some(PairedHostError::Refused {
                        reason: PairedHostRefusal::WrongCode,
                        ..
                    })
                ));
            }
            error => panic!("expected unchanged host refusal: {error:?}"),
        }
    }
    assert!(
        corrected.load(Ordering::SeqCst),
        "the human must supply a corrected code"
    );
    assert_eq!(requests.load(Ordering::SeqCst), usize::from(!terminal));
    assert_eq!(confirmations.load(Ordering::SeqCst), 1);
    for secret in [
        malformed,
        valid,
        "ABCDEFGHJKMN",
        "synthetic-pair-cookie",
        "synthetic-device-cookie",
    ] {
        assert!(!output.text().contains(secret));
    }
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    send(&pty.master, b"continue my frontend setup\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "continue my frontend setup"
    );
    pty.restored();
}

#[tokio::test]
async fn local_pairing_code_correction_telegram_keeps_original_request() {
    local_pairing_code_correction(false, true).await;
}

#[tokio::test]
async fn local_pairing_code_correction_terminal_sends_only_corrected_code() {
    local_pairing_code_correction(true, true).await;
}

#[tokio::test]
async fn local_pairing_code_correction_never_retries_host_rejection() {
    for terminal in [false, true] {
        local_pairing_code_correction(terminal, false).await;
    }
}

#[tokio::test]
async fn local_pairing_code_correction_can_quit_eof_or_cancel_without_confirmation() {
    for terminal in [false, true] {
        for exit in ["quit", "eof", "interrupt"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let mut pty = Pty::new();
            let eof = pty.original.control_chars[SpecialCharacterIndices::VEOF as usize];
            let mut output = Output::default();
            let observed = output.clone();
            let requests = AtomicUsize::new(0);
            let (cancel, cancelled) = oneshot::channel();
            let server = async {
                if !terminal {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let received = request(&mut socket).await;
                    requests.fetch_add(1, Ordering::SeqCst);
                    assert!(received.head.starts_with("POST /console/pair/request "));
                    acknowledgement(&mut socket, false).await;
                }
            };
            let human = async {
                shown(&observed, "Type 'request'").await;
                send(
                    &pty.master,
                    if terminal {
                        b"terminal\n"
                    } else {
                        b"request\n"
                    },
                );
                hidden(&pty.observer).await;
                let _ = echoed(&pty.master);
                send(&pty.master, b"invalid-code\n");
                shown(&observed, "Please enter a ").await;
                assert!(
                    !tcgetattr(&pty.observer)
                        .unwrap()
                        .local_flags
                        .contains(LocalFlags::ECHO)
                );
                assert_eq!(requests.load(Ordering::SeqCst), usize::from(!terminal));
                match exit {
                    "quit" => send(&pty.master, b"/quit\n"),
                    "eof" => send(&pty.master, &[eof]),
                    "interrupt" => {
                        send(&pty.master, b"partial-secret");
                        cancel.send(()).unwrap();
                    }
                    _ => unreachable!(),
                }
            };
            let (result, (), ()) = timeout(BOUND, async {
                tokio::join!(
                    pair_paired_host(&origin, &mut pty.input, &mut output, async {
                        // For /quit and EOF the sender closes without signalling.
                        if cancelled.await.is_err() {
                            std::future::pending::<()>().await;
                        }
                        Ok(())
                    }),
                    server,
                    human
                )
            })
            .await
            .unwrap();
            assert!(result.unwrap().is_none());
            pty.restored();
            assert!(echoed(&pty.master).is_empty());
            assert!(!output.text().contains("invalid-code"));
            assert!(!output.text().contains("partial-secret"));
            assert!(output.text().contains(if terminal {
                "before the terminal code was sent"
            } else {
                "code request was not undone"
            }));
            assert_eq!(requests.load(Ordering::SeqCst), usize::from(!terminal));
            assert!(
                timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err()
            );
            send(&pty.master, b"ordinary after correction\n");
            assert_eq!(
                timeout(BOUND, pty.input.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                "ordinary after correction"
            );
            pty.restored();
        }
    }
}

#[tokio::test]
async fn malformed_encoding_and_oversized_codes_remain_fatal_for_both_formats() {
    for terminal in [false, true] {
        for (bytes, kind, message) in [
            (
                vec![0xff, b'\n'],
                io::ErrorKind::InvalidData,
                "terminal input is not UTF-8",
            ),
            (
                [vec![b'X'; 65], vec![b'\n']].concat(),
                io::ErrorKind::InvalidInput,
                "terminal input exceeds the supported line limit",
            ),
        ] {
            let mut pty = Pty::new();
            let mut output = Output::default();
            let human = async {
                hidden(&pty.observer).await;
                send(&pty.master, &bytes);
            };
            let reader = async {
                if terminal {
                    pty.input
                        .read_terminal_pairing_code(&mut output)
                        .await
                        .map(|code| code.map(|_| ()))
                } else {
                    pty.input
                        .read_pairing_code(&mut output)
                        .await
                        .map(|code| code.map(|_| ()))
                }
            };
            let (result, ()) = timeout(BOUND, async { tokio::join!(reader, human) })
                .await
                .unwrap();
            let error = result.unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), message);
            assert!(!output.text().contains("Please enter"));
            pty.restored();
            assert!(echoed(&pty.master).is_empty());
        }
    }
}

#[tokio::test]
async fn explicit_pairing_uses_hidden_entry_and_never_returns_secret_input_to_the_conversation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let received = AtomicUsize::new(0);
    let server = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let first = request(&mut socket).await;
        received.fetch_add(1, Ordering::SeqCst);
        assert!(
            first
                .head
                .starts_with("POST /console/pair/request HTTP/1.1\r\n")
        );
        assert_eq!(first.body, json!({"label":"Meerkat Connect CLI"}));
        acknowledgement(&mut socket, false).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        let second = request(&mut socket).await;
        received.fetch_add(1, Ordering::SeqCst);
        assert!(
            second
                .head
                .starts_with("POST /console/pair/confirm HTTP/1.1\r\n")
        );
        assert!(
            second
                .head
                .contains("paired_host_pair=synthetic-pair-cookie")
        );
        assert_eq!(second.body, json!({"code":CODE}));
        acknowledgement(&mut socket, true).await;
    };
    let human = async {
        shown(&observed, "Type 'request'").await;
        send(&pty.master, b"continue\n");
        shown(&observed, "Type 'request' to send a code").await;
        // The presenter has not made a request for an ordinary response.
        sleep(Duration::from_millis(30)).await;
        assert_eq!(received.load(Ordering::SeqCst), 0);
        send(&pty.master, b"request\n");
        hidden(&pty.observer).await;
        let _ = echoed(&pty.master);
        send(&pty.master, format!("{CODE}\n").as_bytes());
    };
    let paired = pair_paired_host(&origin, &mut pty.input, &mut output, std::future::pending());
    let (result, (), ()) = timeout(BOUND, async { tokio::join!(paired, server, human) })
        .await
        .unwrap();
    assert!(result.unwrap().is_some());
    assert_eq!(received.load(Ordering::SeqCst), 2);
    pty.restored();
    assert!(echoed(&pty.master).is_empty());
    let text = output.text();
    assert!(!text.contains(CODE));
    assert!(!text.contains("synthetic-pair-cookie"));
    assert!(!text.contains("synthetic-device-cookie"));
    assert!(text.contains("The paired host keeps the paired device after Connect exits"));
    send(&pty.master, b"my frontend request\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "my frontend request"
    );
}

#[tokio::test]
async fn cancelling_code_entry_keeps_the_acknowledged_host_request_visible() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let (cancel, cancelled) = oneshot::channel();
    let server = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let first = request(&mut socket).await;
        assert!(first.head.starts_with("POST /console/pair/request "));
        acknowledgement(&mut socket, false).await;
    };
    let human = async {
        shown(&observed, "Type 'request'").await;
        send(&pty.master, b"request\n");
        hidden(&pty.observer).await;
        send(&pty.master, &CODE.as_bytes()[..3]);
        cancel.send(()).unwrap();
    };
    let paired = pair_paired_host(&origin, &mut pty.input, &mut output, async {
        cancelled.await.unwrap();
        Ok(())
    });
    let (result, (), ()) = timeout(BOUND, async { tokio::join!(paired, server, human) })
        .await
        .unwrap();
    assert!(result.unwrap().is_none());
    pty.restored();
    assert!(output.text().contains("code request was not undone"));
    assert!(!output.text().contains(&CODE[..3]));
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    send(&pty.master, b"ordinary after cancellation\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "ordinary after cancellation"
    );
}

#[tokio::test]
async fn pending_http_interruption_is_unconfirmed_and_never_retried() {
    for during_confirm in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let (cancel, cancelled) = oneshot::channel();
        let server = async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let first = request(&mut socket).await;
            assert!(first.head.starts_with("POST /console/pair/request "));
            if during_confirm {
                acknowledgement(&mut socket, false).await;
                let (mut socket, _) = listener.accept().await.unwrap();
                let second = request(&mut socket).await;
                assert_eq!(second.body, json!({"code":CODE}));
                cancel.send(()).unwrap();
                sleep(Duration::from_millis(40)).await;
            } else {
                cancel.send(()).unwrap();
                sleep(Duration::from_millis(40)).await;
            }
        };
        let human = async {
            shown(&observed, "Type 'request'").await;
            send(&pty.master, b"request\n");
            if during_confirm {
                hidden(&pty.observer).await;
                send(&pty.master, format!("{CODE}\n").as_bytes());
            }
        };
        let paired = pair_paired_host(&origin, &mut pty.input, &mut output, async {
            cancelled.await.unwrap();
            Ok(())
        });
        let (result, (), ()) = timeout(BOUND, async { tokio::join!(paired, server, human) })
            .await
            .unwrap();
        let error = result.unwrap_err().to_string();
        assert!(error.contains("paired host outcome is unconfirmed"));
        assert!(error.contains("nothing was rolled back"));
        assert!(!error.contains(CODE));
        assert!(!output.text().contains(CODE));
        pty.restored();
        assert!(
            timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn terminal_code_entry_is_hidden_and_uses_no_telegram_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let server = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let received = request(&mut socket).await;
        assert!(
            received
                .head
                .starts_with("POST /console/pair/terminal HTTP/1.1\r\n")
        );
        assert!(!received.head.contains("cookie:"));
        assert_eq!(received.body, json!({"code":"ABCDEFGHJKMN"}));
        acknowledgement(&mut socket, true).await;
    };
    let human = async {
        shown(&observed, "'terminal' to use a code").await;
        send(&pty.master, b"terminal\n");
        hidden(&pty.observer).await;
        let _ = echoed(&pty.master);
        send(
            &pty.master,
            format!("{TERMINAL_CODE}\n{TERMINAL_CODE}\n").as_bytes(),
        );
    };
    let (result, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host(&origin, &mut pty.input, &mut output, std::future::pending()),
            server,
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_some());
    pty.restored();
    assert!(echoed(&pty.master).is_empty());
    assert!(!output.text().contains(TERMINAL_CODE));
    assert!(!output.text().contains("ABCDEFGHJKMN"));
    assert!(!output.text().contains("synthetic-device-cookie"));
    assert!(
        output
            .text()
            .contains("Connect does not change that setting")
    );
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    send(&pty.master, b"my actual frontend question\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "my actual frontend question"
    );
    pty.restored();
}

#[tokio::test]
async fn terminal_code_cancellation_sends_nothing_and_flushes_partial_input() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let (cancel, cancelled) = oneshot::channel();
    let human = async {
        shown(&observed, "'terminal' to use a code").await;
        send(&pty.master, b"terminal\n");
        hidden(&pty.observer).await;
        let _ = echoed(&pty.master);
        send(&pty.master, b"ABCD-E");
        cancel.send(()).unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host(&origin, &mut pty.input, &mut output, async {
                cancelled.await.unwrap();
                Ok(())
            }),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    pty.restored();
    assert!(echoed(&pty.master).is_empty());
    assert!(!output.text().contains("ABCD-E"));
    assert!(output.text().contains("before the terminal code was sent"));
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    send(&pty.master, b"after protected input\n");
    assert_eq!(
        timeout(BOUND, pty.input.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "after protected input"
    );
}

#[tokio::test]
async fn terminal_pairing_lost_response_remains_uncertain_without_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let server = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let received = request(&mut socket).await;
        assert!(received.head.starts_with("POST /console/pair/terminal "));
        assert_eq!(received.body, json!({"code":"ABCDEFGHJKMN"}));
        socket.shutdown().await.unwrap();
    };
    let human = async {
        shown(&observed, "'terminal' to use a code").await;
        send(&pty.master, b"terminal\n");
        hidden(&pty.observer).await;
        let _ = echoed(&pty.master);
        send(&pty.master, format!("{TERMINAL_CODE}\n").as_bytes());
    };
    let (result, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host(&origin, &mut pty.input, &mut output, std::future::pending()),
            server,
            human
        )
    })
    .await
    .unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains("outcome is unconfirmed"));
    assert!(!error.contains("ABCDEFGHJKMN"));
    assert!(!output.text().contains(TERMINAL_CODE));
    pty.restored();
    assert!(echoed(&pty.master).is_empty());
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}
