//! Synthetic store, newly allocated PTYs and loopback HTTP only. These tests
//! never construct OsDeviceStore, access user stdin or contact a real paired host.
use meerkat_connect::{
    paired_host_device::{DeviceStore, DeviceStoreError},
    pairing::pair_paired_host_with_store,
    terminal::TerminalInput,
};
use nix::{
    fcntl::{FcntlArg, fcntl},
    pty::openpty,
    sys::termios::{LocalFlags, Termios, tcgetattr},
    unistd::write,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, Write},
    os::fd::OwnedFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::{sleep, timeout},
};

const BOUND: Duration = Duration::from_secs(5);
const CODE: &str = "672491";
const DEVICE: &str = "synthetic-remembered-device";
const PAIR: &str = "synthetic-short-pair";

#[derive(Default)]
struct Store {
    values: Mutex<HashMap<String, Vec<u8>>>,
    failure: Option<DeviceStoreError>,
    write_uncertain: bool,
    ignore_delete: bool,
    reads: AtomicUsize,
    writes: AtomicUsize,
    deletes: AtomicUsize,
}
impl DeviceStore for Store {
    fn read(&self, origin: &str) -> Result<Option<Vec<u8>>, DeviceStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.failure {
            return Err(error);
        }
        Ok(self.values.lock().unwrap().get(origin).cloned())
    }
    fn write(&self, origin: &str, bytes: &[u8]) -> Result<(), DeviceStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.values
            .lock()
            .unwrap()
            .insert(origin.to_owned(), bytes.to_vec());
        if self.write_uncertain {
            Err(DeviceStoreError::Unconfirmed)
        } else {
            Ok(())
        }
    }
    fn delete(&self, origin: &str) -> Result<(), DeviceStoreError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        if !self.ignore_delete {
            self.values.lock().unwrap().remove(origin);
        }
        Ok(())
    }
}
fn at() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
fn record(origin: &str, expiry: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({"version":1,"origin":origin,"cookie":DEVICE,
        "expires_at":expiry,"secure":false}))
    .unwrap()
}
fn seeded(store: Store, origin: &str, bytes: Vec<u8>) -> Arc<Store> {
    store
        .values
        .lock()
        .unwrap()
        .insert(origin.to_owned(), bytes);
    Arc::new(store)
}
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
    input: TerminalInput,
    master: OwnedFd,
    observer: OwnedFd,
    original: Termios,
    flags: i32,
}
impl Pty {
    fn new() -> Self {
        let pty = openpty(None, None).unwrap();
        let observer = pty.slave.try_clone().unwrap();
        let original = tcgetattr(&observer).unwrap();
        let flags = fcntl(&observer, FcntlArg::F_GETFL).unwrap();
        Self {
            input: TerminalInput::from_owned_fd(pty.slave),
            master: pty.master,
            observer,
            original,
            flags,
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
fn send(fd: &OwnedFd, text: &str) {
    assert_eq!(write(fd, text.as_bytes()).unwrap(), text.len());
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
async fn hidden(fd: &OwnedFd) {
    timeout(BOUND, async {
        while tcgetattr(fd)
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
async fn human_pair(master: &OwnedFd, observer: &OwnedFd, output: &Output) {
    shown(output, "Type 'request'").await;
    send(master, "request\n");
    hidden(observer).await;
    send(master, &format!("{CODE}\n"));
}
async fn request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
        assert!(head.len() < 16384);
    }
    let head = String::from_utf8(head).unwrap();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, length)| length.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length < 8192);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    (head, body)
}
async fn reply(stream: &mut TcpStream, status: u16, body: Value, cookies: &[String]) {
    let body = body.to_string();
    let cookies = cookies
        .iter()
        .map(|cookie| format!("Set-Cookie: {cookie}\r\n"))
        .collect::<String>();
    let message = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\n{cookies}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(message.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}
async fn pair_server(listener: &TcpListener, duplicate: bool, confirmed_body: Value) {
    let (mut stream, _) = listener.accept().await.unwrap();
    let (head, body) = request(&mut stream).await;
    assert!(head.starts_with("POST /console/pair/request "));
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"label":"Meerkat Connect CLI"})
    );
    reply(
        &mut stream,
        200,
        json!({"ok":true}),
        &[format!(
            "paired_host_pair={PAIR}; Max-Age=600; Path=/console/pair; HttpOnly; SameSite=Strict"
        )],
    )
    .await;
    let (mut stream, _) = listener.accept().await.unwrap();
    let (head, body) = request(&mut stream).await;
    assert!(head.starts_with("POST /console/pair/confirm "));
    assert!(head.contains(PAIR));
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"code":CODE})
    );
    let mut cookies = vec![format!(
        "paired_host_device={DEVICE}; Max-Age=3600; Path=/; HttpOnly; SameSite=Lax"
    )];
    if duplicate {
        cookies.push("paired_host_device=contradictory-later-device; Max-Age=3600; Path=/; HttpOnly; SameSite=Lax".into());
    }
    reply(&mut stream, 200, confirmed_body, &cookies).await;
}
async fn host() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (listener, origin)
}
async fn no_request(listener: &TcpListener) {
    assert!(
        timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn terminal_pairing_uses_the_same_origin_store_and_reuses_the_saved_device() {
    let (listener, origin) = host().await;
    let store = Arc::new(Store::default());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = request(&mut stream).await;
        assert!(head.starts_with("POST /console/pair/terminal "));
        assert!(!head.contains("cookie:"));
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"code":"ABCDEFGHJKMN"})
        );
        reply(
            &mut stream,
            200,
            json!({"ok":true}),
            &[format!(
                "paired_host_device={DEVICE}; Max-Age=3600; Path=/; HttpOnly; SameSite=Lax"
            )],
        )
        .await;
    };
    let human = async {
        shown(&observed, "'terminal' to use a code").await;
        send(&pty.master, "terminal\n");
        hidden(&pty.observer).await;
        send(&pty.master, "ABCD-EFGH-JKMN\n");
    };
    let (paired, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            server,
            human
        )
    })
    .await
    .unwrap();
    drop(paired.unwrap().unwrap());
    pty.restored();
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    let saved: Value = serde_json::from_slice(&store.values.lock().unwrap()[&origin]).unwrap();
    assert_eq!(saved["origin"], origin);
    assert_eq!(saved["cookie"], DEVICE);
    assert!(
        !String::from_utf8(store.values.lock().unwrap()[&origin].clone())
            .unwrap()
            .contains("ABCDEFGHJKMN")
    );

    let mut fresh = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        shown(&observed, "A saved pairing is available").await;
        send(&fresh.master, "\n");
    };
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = request(&mut stream).await;
        assert!(head.starts_with("GET /console/extensions/frontend/inventory "));
        assert!(head.contains(DEVICE));
        assert!(body.is_empty());
        reply(
            &mut stream,
            200,
            json!({"current":{"activation_id":"fixture","policy_digest":null},
            "extensions":[],"routing":"sender_identity"}),
            &[],
        )
        .await;
    };
    let (paired, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut fresh.input,
                &mut output,
                std::future::pending()
            ),
            server,
            human
        )
    })
    .await
    .unwrap();
    assert!(paired.unwrap().is_some());
    fresh.restored();
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    assert!(!output.text().contains("ABCD-EFGH-JKMN"));
    assert!(!output.text().contains(DEVICE));
    no_request(&listener).await;
}
fn redacted(output: &Output) {
    for secret in [CODE, PAIR, DEVICE] {
        assert!(!output.text().contains(secret));
    }
}

#[tokio::test]
async fn confirmed_cookie_is_saved_once_and_a_fresh_client_reuses_it_only_after_inventory() {
    let (listener, origin) = host().await;
    let store = Arc::new(Store::default());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let before = at();
    let (result, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            pair_server(&listener, false, json!({"ok":true})),
            human_pair(&pty.master, &pty.observer, &observed)
        )
    })
    .await
    .unwrap();
    drop(result.unwrap().unwrap());
    pty.restored();
    redacted(&output);
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    let saved = store.values.lock().unwrap().get(&origin).unwrap().clone();
    let value: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(value["cookie"], DEVICE);
    assert_eq!(value["origin"], origin);
    let expiry = value["expires_at"].as_u64().unwrap();
    assert!(expiry >= before + 3600 && expiry <= at() + 3600);
    assert!(!String::from_utf8(saved.clone()).unwrap().contains(PAIR));
    assert!(!String::from_utf8(saved.clone()).unwrap().contains(CODE));
    let mut next = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        shown(&observed, "A saved pairing is available").await;
        send(&next.master, "\n");
    };
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = request(&mut stream).await;
        assert!(head.starts_with("GET /console/extensions/frontend/inventory "));
        assert!(head.contains(&format!("paired_host_device={DEVICE}")));
        assert!(!head.contains(PAIR));
        assert!(body.is_empty());
        reply(&mut stream, 200, json!({"current":{"activation_id":"210","policy_digest":"sha256:fixture"},"extensions":[],"routing":"sender_identity"}), &[]).await;
    };
    let (result, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut next.input,
                &mut output,
                std::future::pending()
            ),
            server,
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_some());
    next.restored();
    redacted(&output);
    assert_eq!(store.values.lock().unwrap()[&origin], saved);
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    no_request(&listener).await;
}

#[tokio::test]
async fn contradictory_cookie_or_failed_confirmation_never_reaches_store_write() {
    for (duplicate, body) in [
        (true, json!({"ok":true})),
        (false, json!({"ok":false})),
        (false, json!({"ok":true,"unexpected":"synthetic"})),
    ] {
        let (listener, origin) = host().await;
        let store = Arc::new(Store::default());
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let (result, (), ()) = timeout(BOUND, async {
            tokio::join!(
                pair_paired_host_with_store(
                    &origin,
                    store.clone(),
                    &mut pty.input,
                    &mut output,
                    std::future::pending()
                ),
                pair_server(&listener, duplicate, body),
                human_pair(&pty.master, &pty.observer, &observed)
            )
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        assert!(store.values.lock().unwrap().is_empty());
        pty.restored();
        redacted(&output);
        no_request(&listener).await;
    }
}

#[tokio::test]
async fn host_refusal_and_transport_loss_retain_saved_pairing_without_automatic_code_request() {
    for refused in [true, false] {
        let (listener, origin) = host().await;
        let saved = record(&origin, at() + 3600);
        let store = seeded(Store::default(), &origin, saved.clone());
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let human = async {
            shown(&observed, "A saved pairing is available").await;
            send(&pty.master, "\n");
            if refused {
                shown(&observed, "Type 'request'").await;
                send(&pty.master, "/quit\n");
            }
        };
        let server = async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (head, _) = request(&mut stream).await;
            assert!(head.starts_with("GET /console/extensions/frontend/inventory "));
            if refused {
                reply(
                    &mut stream,
                    403,
                    json!({"error":"forbidden","code":"console_owner_required"}),
                    &[],
                )
                .await;
            }
        };
        let (result, (), ()) = timeout(BOUND, async {
            tokio::join!(
                pair_paired_host_with_store(
                    &origin,
                    store.clone(),
                    &mut pty.input,
                    &mut output,
                    std::future::pending()
                ),
                server,
                human
            )
        })
        .await
        .unwrap();
        if refused {
            assert!(result.unwrap().is_none());
            assert!(output.text().contains("not deleted or treated as revoked"));
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("saved credential was retained")
            );
        }
        assert_eq!(store.values.lock().unwrap()[&origin], saved);
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        assert_eq!(store.deletes.load(Ordering::SeqCst), 0);
        redacted(&output);
        pty.restored();
        no_request(&listener).await;
    }
}

#[tokio::test]
async fn unavailable_store_requires_deliberate_memory_choice_or_exit() {
    for error in [
        DeviceStoreError::Locked,
        DeviceStoreError::Unavailable,
        DeviceStoreError::Ambiguous,
    ] {
        let (listener, origin) = host().await;
        let store = Arc::new(Store {
            failure: Some(error),
            ..Store::default()
        });
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let human = async {
            shown(&observed, "Type 'memory'").await;
            send(&pty.master, "/quit\n");
        };
        let (result, ()) = timeout(BOUND, async {
            tokio::join!(
                pair_paired_host_with_store(
                    &origin,
                    store.clone(),
                    &mut pty.input,
                    &mut output,
                    std::future::pending()
                ),
                human
            )
        })
        .await
        .unwrap();
        assert!(result.unwrap().is_none());
        assert!(output.text().contains("No plaintext fallback"));
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        no_request(&listener).await;
    }
    let (listener, origin) = host().await;
    let store = Arc::new(Store {
        failure: Some(DeviceStoreError::Locked),
        ..Store::default()
    });
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        shown(&observed, "Type 'memory'").await;
        send(&pty.master, "memory\n");
        human_pair(&pty.master, &pty.observer, &observed).await;
    };
    let (result, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            pair_server(&listener, false, json!({"ok":true})),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_some());
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    assert!(store.values.lock().unwrap().is_empty());
    redacted(&output);
    pty.restored();
}

#[tokio::test]
async fn uncertain_save_never_retries_or_claims_rollback_and_needs_deliberate_continue() {
    let (listener, origin) = host().await;
    let store = Arc::new(Store {
        write_uncertain: true,
        ..Store::default()
    });
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        human_pair(&pty.master, &pty.observer, &observed).await;
        shown(&observed, "Type 'continue'").await;
        send(&pty.master, "continue\n");
    };
    let (result, (), ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            pair_server(&listener, false, json!({"ok":true})),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_some());
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    assert!(store.values.lock().unwrap().contains_key(&origin));
    assert!(output.text().contains("not confirmed"));
    assert!(output.text().contains("enrollment remains"));
    redacted(&output);
    pty.restored();
    no_request(&listener).await;
}

#[tokio::test]
async fn forget_covers_valid_expired_and_invalid_entries_and_requires_actual_absence() {
    for case in 0..4 {
        let (listener, origin) = host().await;
        let bytes = match case {
            1 => record(&origin, at() - 1),
            2 => b"malformed synthetic record".to_vec(),
            _ => record(&origin, at() + 3600),
        };
        let store = seeded(
            Store {
                ignore_delete: case == 3,
                ..Store::default()
            },
            &origin,
            bytes.clone(),
        );
        store.values.lock().unwrap().insert(
            "https://other.example".into(),
            b"other origin untouched".to_vec(),
        );
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let human = async {
            shown(&observed, "'forget'").await;
            send(&pty.master, "forget\n");
        };
        let (result, ()) = timeout(BOUND, async {
            tokio::join!(
                pair_paired_host_with_store(
                    &origin,
                    store.clone(),
                    &mut pty.input,
                    &mut output,
                    std::future::pending()
                ),
                human
            )
        })
        .await
        .unwrap();
        if case == 3 {
            assert!(result.unwrap_err().to_string().contains("unconfirmed"));
            assert_eq!(store.values.lock().unwrap()[&origin], bytes);
            assert!(!output.text().contains("credential is absent"));
        } else {
            assert!(result.unwrap().is_none());
            assert!(!store.values.lock().unwrap().contains_key(&origin));
            assert!(output.text().contains("enrollment was not revoked"));
        }
        assert!(
            store
                .values
                .lock()
                .unwrap()
                .contains_key("https://other.example")
        );
        assert_eq!(store.deletes.load(Ordering::SeqCst), 1);
        no_request(&listener).await;
        pty.restored();
        redacted(&output);
    }
}

#[tokio::test]
async fn mismatched_origin_record_is_not_restored_and_no_host_request_is_sent() {
    let (listener, origin) = host().await;
    let saved = record("http://127.0.0.1:1", at() + 3600);
    let store = seeded(Store::default(), &origin, saved.clone());
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        shown(&observed, "record is invalid").await;
        send(&pty.master, "/quit\n");
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    assert_eq!(store.values.lock().unwrap()[&origin], saved);
    no_request(&listener).await;
    redacted(&output);
    pty.restored();
}

struct BlockingRead {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    done: AtomicUsize,
}
impl DeviceStore for BlockingRead {
    fn read(&self, _: &str) -> Result<Option<Vec<u8>>, DeviceStoreError> {
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.release.lock().unwrap().recv().unwrap();
        self.done.store(1, Ordering::SeqCst);
        Ok(None)
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<(), DeviceStoreError> {
        panic!("cancelled workflow must not save")
    }
    fn delete(&self, _: &str) -> Result<(), DeviceStoreError> {
        panic!("cancelled workflow must not delete")
    }
}
#[tokio::test]
async fn cancellation_waits_for_owned_store_call_then_stops_without_pairing() {
    let (listener, origin) = host().await;
    let (entered, did_enter) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let store = Arc::new(BlockingRead {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(released),
        done: AtomicUsize::new(0),
    });
    let (cancel, cancelled) = oneshot::channel();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        did_enter.await.unwrap();
        cancel.send(()).unwrap();
        shown(
            &observed,
            "Stopping after the credential-store operation finishes",
        )
        .await;
        assert_eq!(store.done.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            pair_paired_host_with_store(
                &origin,
                store.clone(),
                &mut pty.input,
                &mut output,
                async {
                    cancelled.await.unwrap();
                    Ok(())
                }
            ),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    assert_eq!(store.done.load(Ordering::SeqCst), 1);
    assert!(!output.text().contains("Type 'request'"));
    no_request(&listener).await;
    pty.restored();
}
