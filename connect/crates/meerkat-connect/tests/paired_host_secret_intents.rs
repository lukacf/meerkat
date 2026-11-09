//! Synthetic loopback intent observations only. No secret entry, owner confirmation,
//! provider authentication, host feature enabling or live paired host qualification.

use meerkat_connect::paired_host::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};

const FIXTURE_BOUND: Duration = Duration::from_secs(5);
const INTENTS: &str = "/console/extensions/secrets/intents";
const ID: &str = "esi_aaaaaaaaaaaaaaaaaaaa";
const OTHER_ID: &str = "esi_bbbbbbbbbbbbbbbbbbbb";
const DEVICE_COOKIE: &str = "synthetic-intent-device-cookie";

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    chunked: bool,
    close_without_reply: bool,
}

impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self::bytes(status, serde_json::to_vec(&body).unwrap())
    }

    fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("Cache-Control".into(), "no-store".into()),
            ],
            body,
            chunked: false,
            close_without_reply: false,
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    async fn write(self, stream: &mut TcpStream) {
        if self.close_without_reply {
            return;
        }
        let mut head = format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\n", self.status);
        for (name, value) in self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if self.chunked {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
        } else {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", self.body.len()));
        }
        // Refused/oversized responses may be dropped before the fixture writes
        // all bytes. A closed peer is expected, not a reason to retry.
        if stream.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        if self.chunked {
            for chunk in self.body.chunks(4096) {
                if stream
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .is_err()
                    || stream.write_all(chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        } else {
            let _ = stream.write_all(&self.body).await;
        }
        let _ = stream.shutdown().await;
    }
}

struct Fixture {
    origin: String,
    task: Option<JoinHandle<Vec<Request>>>,
}

impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = timeout(FIXTURE_BOUND, listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                requests.push(
                    timeout(FIXTURE_BOUND, read_request(&mut stream))
                        .await
                        .unwrap(),
                );
                timeout(FIXTURE_BOUND, reply.write(&mut stream))
                    .await
                    .unwrap();
            }
            requests
        });
        Self {
            origin,
            task: Some(task),
        }
    }

    async fn finish(mut self) -> Vec<Request> {
        timeout(FIXTURE_BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let boundary = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(
            read > 0 && bytes.len() + read < 128 * 1024,
            "bounded fixture request"
        );
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = std::str::from_utf8(&bytes[..boundary]).unwrap();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap();
    let mut first = first.split_whitespace();
    let method = first.next().unwrap().to_owned();
    assert!(["GET", "POST"].contains(&method.as_str()));
    let path = first.next().unwrap().to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .map(|length| length.parse().unwrap())
        .unwrap_or(0);
    assert!(length <= 64 * 1024);
    while bytes.len() < boundary + length {
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
    }
    Request {
        method,
        path,
        headers,
        body: bytes[boundary..boundary + length].to_vec(),
    }
}

fn id() -> ExtensionSecretIntentId {
    ExtensionSecretIntentId::new(ID).unwrap()
}

fn ledger_request() -> ExtensionSecretIntentRequest {
    ExtensionSecretIntentRequest::Ledger {
        extension_id: "telegram".into(),
        binding_name: "telegram_bot_token".into(),
        base: FrontendChangeBase {
            activation_id: Some("214".into()),
            policy_digest: Some("fixture-policy-v1:sha256:fixture-policy".into()),
            ledger_digest: "sha256:fixture-ledger".into(),
        },
    }
}

fn candidate_request() -> ExtensionSecretIntentRequest {
    ExtensionSecretIntentRequest::Candidate {
        candidate: ExtensionSecretIntentCandidate {
            change_digest: FrontendChangeDigest::new(&format!(
                "paired-host-change-v1:sha256:{}",
                "a".repeat(64)
            ))
            .unwrap(),
            wheel_sha256: "b".repeat(64),
        },
        binding_name: "telegram_bot_token".into(),
    }
}

fn observation(state: &str) -> Value {
    let mut body = json!({
        "intent_id": ID,
        "state": state,
        "target": {"host":"fixture-paired-host","activation_id":"214", "extension_id":"telegram",
            "binding_name":"telegram_bot_token","channel_ids":["telegram:primary"],
            "consumer":"channel:telegram:primary","consumer_verified":true,"required":true},
        "present":false,"requires_overwrite":false,"presenter":"terminal",
        "entry_url":format!("{INTENTS}/{ID}/entry")
    });
    if ["awaiting_owner", "committed", "rejected"].contains(&state) {
        body["fingerprint"] = json!("1a2b3c4d");
        body["overwrite"] = json!(false);
    }
    if state == "committed" {
        body["generation"] = json!(1);
        body["binding_status"] = json!("committed_not_live");
    }
    if state == "rejected" {
        body["reason"] = json!("owner_rejected");
    }
    if state == "stale_base" {
        body["reason"] = json!("generation_conflict");
    }
    body
}

fn pairing() -> Reply {
    Reply::json(200, json!({"ok":true})).header(
        "Set-Cookie",
        &format!(
            "paired_host_device={DEVICE_COOKIE}; Max-Age=15552000; Path=/; HttpOnly; SameSite=Lax"
        ),
    )
}

fn assert_unconfirmed(error: &PairedHostError, operation: PairedHostOperation) {
    assert!(
        matches!(error, PairedHostError::Unconfirmed {operation: actual, ..} if *actual == operation)
    );
    assert!(!format!("{error:?} {error}").contains("private-response-sentinel"));
}

#[tokio::test]
async fn exact_ledger_intent_status_and_cancellation_use_only_captured_pairing_and_origin() {
    let fixture = Fixture::new(vec![
        pairing(),
        Reply::json(201, observation("awaiting_entry")),
        Reply::json(200, observation("awaiting_owner")),
        Reply::json(200, observation("cancelled")),
        Reply::json(409, json!({"error":"intent_closed","state":"cancelled"})),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client
        .confirm_terminal_pairing(TerminalPairingCode::new("ABCD-EFGH-JKMN".into()).unwrap())
        .await
        .unwrap();
    let request = ledger_request();
    let ExtensionSecretIntentCreation::Created(created) = client
        .create_extension_secret_intent(&request)
        .await
        .unwrap()
    else {
        panic!("created")
    };
    assert_eq!(created.request, request);
    assert_eq!(
        created.intent.state,
        ExtensionSecretIntentState::AwaitingEntry
    );
    assert_eq!(
        created.intent.entry_path_for_presenter(),
        format!("{INTENTS}/{ID}/entry")
    );
    assert!(!serde_json::to_string(&created).unwrap().contains("/entry"));
    assert!(!format!("{created:?}").contains("/entry"));
    let ExtensionSecretIntentRead::Status(entered) =
        client.extension_secret_intent_status(&id()).await.unwrap()
    else {
        panic!("status")
    };
    assert_eq!(entered.state, ExtensionSecretIntentState::AwaitingOwner);
    assert_eq!(entered.fingerprint.as_deref(), Some("1a2b3c4d"));
    assert!(matches!(
        client.cancel_extension_secret_intent(&id()).await.unwrap(),
        ExtensionSecretIntentCancellation::Cancelled(_)
    ));
    let ExtensionSecretIntentCancellation::Rejected(closed) =
        client.cancel_extension_secret_intent(&id()).await.unwrap()
    else {
        panic!("closed")
    };
    assert_eq!(
        closed.refusal,
        ExtensionSecretIntentRefusal::IntentClosed {
            state: ExtensionSecretIntentState::Cancelled
        }
    );
    let origin = fixture.origin.clone();
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 5);
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
        serde_json::to_value(request).unwrap()
    );
    for request in &requests[1..] {
        assert!(request.headers["cookie"].contains(DEVICE_COOKIE));
        assert!(!request.headers.contains_key("x-host-dev-token"));
        assert_eq!(request.headers["cache-control"], "no-store");
        if request.method == "POST" {
            assert_eq!(request.headers["origin"], origin);
            assert_eq!(request.headers["content-type"], "application/json");
        }
    }
    assert_eq!(requests[1].path, INTENTS);
    assert_eq!(requests[2].path, format!("{INTENTS}/{ID}"));
    assert_eq!(requests[3].path, format!("{INTENTS}/{ID}/cancel"));
    assert_eq!(requests[3].body, b"{}");
    assert!(requests.iter().all(|request| !request.path.ends_with("/entry") && !request.path.ends_with("/confirm")));
}

#[tokio::test]
async fn candidate_receipt_retains_requested_pins_and_legacy_unverified_observation() {
    // Older hosts could return this unresolved consumer. Preserve the fact;
    // it is not permission to commit. Corrected hosts refuse it with 409.
    let mut body = observation("awaiting_entry");
    body["target"]["consumer"] = json!("extension:telegram");
    body["target"]["consumer_verified"] = json!(false);
    body["target"]["channel_ids"] = json!([]);
    body["target"]["activation_id"] = Value::Null;
    let fixture = Fixture::new(vec![Reply::json(201, body)]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let request = candidate_request();
    let ExtensionSecretIntentCreation::Created(created) = client
        .create_extension_secret_intent(&request)
        .await
        .unwrap()
    else {
        panic!("created")
    };
    assert_eq!(created.request, request);
    assert!(!created.intent.target.consumer_verified);
    assert_eq!(created.intent.target.consumer, "extension:telegram");
    assert_eq!(created.intent.target.activation_id, None);
    let requests = fixture.finish().await;
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        serde_json::to_value(request).unwrap()
    );
    assert!(!requests[0].headers.contains_key("cookie"));
}

#[tokio::test]
async fn status_preserves_closed_states_and_host_reported_generation_observations() {
    let states = [
        ("awaiting_entry", ExtensionSecretIntentState::AwaitingEntry),
        ("awaiting_owner", ExtensionSecretIntentState::AwaitingOwner),
        ("committed", ExtensionSecretIntentState::Committed),
        ("rejected", ExtensionSecretIntentState::Rejected),
        ("cancelled", ExtensionSecretIntentState::Cancelled),
        ("superseded", ExtensionSecretIntentState::Superseded),
        ("stale_base", ExtensionSecretIntentState::StaleBase),
    ];
    let mut replies = Vec::new();
    for (name, _) in states {
        let mut body = observation(name);
        if ["cancelled", "superseded", "stale_base"].contains(&name) {
            body["fingerprint"] = json!("abcdef23");
            body["overwrite"] = json!(true);
        }
        if name == "committed" {
            body["binding_status"] = Value::Null;
        }
        replies.push(Reply::json(200, body));
    }
    for binding_status in ["superseded_generation", "live", "committed_not_live"] {
        let mut body = observation("committed");
        body["binding_status"] = json!(binding_status);
        replies.push(Reply::json(200, body));
    }
    let mut unresolved = observation("stale_base");
    unresolved["reason"] = json!("consumer_unresolved");
    replies.push(Reply::json(200, unresolved));
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for (_, expected) in states {
        let ExtensionSecretIntentRead::Status(observed) =
            client.extension_secret_intent_status(&id()).await.unwrap()
        else {
            panic!("status")
        };
        assert_eq!(observed.state, expected);
        if expected == ExtensionSecretIntentState::Committed {
            assert_eq!(observed.generation, Some(1));
            assert_eq!(observed.binding_status, None);
        }
        if expected == ExtensionSecretIntentState::Rejected {
            assert_eq!(
                observed.reason,
                Some(ExtensionSecretIntentClosure::OwnerRejected)
            );
        }
    }
    for expected in [
        ExtensionSecretIntentBindingStatus::SupersededGeneration,
        ExtensionSecretIntentBindingStatus::Live,
        ExtensionSecretIntentBindingStatus::CommittedNotLive,
    ] {
        let ExtensionSecretIntentRead::Status(observed) =
            client.extension_secret_intent_status(&id()).await.unwrap()
        else {
            panic!("status")
        };
        // Generation liveness comes from the host, never a local slot lookup.
        assert_eq!(observed.generation, Some(1));
        assert_eq!(observed.binding_status, Some(expected));
    }
    let ExtensionSecretIntentRead::Status(unresolved) =
        client.extension_secret_intent_status(&id()).await.unwrap()
    else {
        panic!("status")
    };
    assert_eq!(
        unresolved.reason,
        Some(ExtensionSecretIntentClosure::ConsumerUnresolved)
    );
    assert_eq!(fixture.finish().await.len(), 11);
}

#[tokio::test]
async fn invalid_reference_request_and_oversize_inputs_are_refused_before_transport() {
    for value in [
        "",
        "esi_short",
        "esi_abcdefghijkmnpqrstuv0",
        "esi_abcdefghijklmnopqrst",
        "../entry",
        "esi_abcdefghijkmnpqrstuvw/entry",
    ] {
        assert_eq!(
            ExtensionSecretIntentId::new(value),
            Err(PairedHostError::InvalidSecretIntentId)
        );
    }
    let mut client = PairedHostClient::new("http://127.0.0.1:9").unwrap();
    let mut request = candidate_request();
    if let ExtensionSecretIntentRequest::Candidate { candidate, .. } = &mut request {
        candidate.wheel_sha256 = "B".repeat(64);
    }
    assert_eq!(
        client
            .create_extension_secret_intent(&request)
            .await
            .unwrap_err(),
        PairedHostError::InvalidSecretIntentRequest
    );
    let mut request = ledger_request();
    if let ExtensionSecretIntentRequest::Ledger { binding_name, .. } = &mut request {
        *binding_name = "x".repeat(8192);
    }
    assert_eq!(
        client
            .create_extension_secret_intent(&request)
            .await
            .unwrap_err(),
        PairedHostError::RequestTooLarge
    );
    let mut encoded = serde_json::to_value(ledger_request()).unwrap();
    encoded["value"] = json!("not-an-accepted-field");
    assert!(serde_json::from_value::<ExtensionSecretIntentRequest>(encoded).is_err());
    let mut encoded = serde_json::to_value(candidate_request()).unwrap();
    encoded["extension_id"] = json!("mixed-alternatives");
    assert!(serde_json::from_value::<ExtensionSecretIntentRequest>(encoded).is_err());
}

#[tokio::test]
async fn changed_create_target_is_unconfirmed_and_never_recaptured() {
    for (field, value) in [
        ("binding_name", json!("other")),
        ("extension_id", json!("other")),
        ("activation_id", json!("215")),
    ] {
        let mut body = observation("awaiting_entry");
        body["target"][field] = value;
        let fixture = Fixture::new(vec![Reply::json(201, body)]).await;
        let mut client = PairedHostClient::new(&fixture.origin).unwrap();
        let error = client
            .create_extension_secret_intent(&ledger_request())
            .await
            .unwrap_err();
        assert_unconfirmed(&error, PairedHostOperation::CreateExtensionSecretIntent);
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn mismatched_paths_ids_fields_and_response_bounds_never_become_status() {
    let mut bodies = Vec::new();
    for path in [
        format!("https://other.invalid{INTENTS}/{ID}/entry"),
        format!("{INTENTS}/{OTHER_ID}/entry"),
        format!("{INTENTS}/{ID}/entry?value=private-response-sentinel"),
        format!("{INTENTS}/{ID}/entry#fragment"),
    ] {
        let mut body = observation("awaiting_entry");
        body["entry_url"] = json!(path);
        bodies.push(body);
    }
    let mut body = observation("awaiting_entry");
    body["intent_id"] = json!(OTHER_ID);
    body["entry_url"] = json!(format!("{INTENTS}/{OTHER_ID}/entry"));
    bodies.push(body);
    let mut body = observation("awaiting_entry");
    body["unexpected"] = json!("private-response-sentinel");
    bodies.push(body);
    let mut body = observation("awaiting_entry");
    body["fingerprint"] = Value::Null;
    bodies.push(body);
    let mut body = observation("awaiting_owner");
    body.as_object_mut().unwrap().remove("overwrite");
    bodies.push(body);
    let mut body = observation("committed");
    body.as_object_mut().unwrap().remove("binding_status");
    bodies.push(body);
    let mut body = observation("committed");
    body["generation"] = json!(0);
    bodies.push(body);
    let mut body = observation("awaiting_entry");
    body["requires_overwrite"] = json!(true);
    bodies.push(body);
    let mut body = observation("awaiting_entry");
    body["state"] = json!("ready");
    bodies.push(body);
    let mut replies: Vec<_> = bodies
        .into_iter()
        .map(|body| Reply::json(200, body))
        .collect();
    let duplicate = serde_json::to_string(&observation("awaiting_entry"))
        .unwrap()
        .replacen('{', "{\"state\":\"committed\",", 1);
    replies.push(Reply::bytes(200, duplicate.into_bytes()));
    let mut no_store = Reply::json(200, observation("awaiting_entry"));
    no_store.headers.retain(|(key, _)| key != "Cache-Control");
    replies.push(no_store);
    replies.push(Reply::bytes(200, vec![b'x'; 65537]));
    let mut chunked = Reply::bytes(200, vec![b'x'; 65537]);
    chunked.chunked = true;
    replies.push(chunked);
    let count = replies.len();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..count {
        let error = client
            .extension_secret_intent_status(&id())
            .await
            .unwrap_err();
        assert_unconfirmed(&error, PairedHostOperation::ExtensionSecretIntentStatus);
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn known_refusals_not_found_and_closed_cancel_remain_distinct() {
    let fixture = Fixture::new(vec![
        Reply::json(409, json!({"error":"stale_base","current":null})),
        Reply::json(409, json!({"error":"candidate_not_built"})),
        Reply::json(409, json!({"error":"consumer_unresolved"})),
        Reply::json(
            403,
            json!({"error":"forbidden","code":"paired_console_device_required"}),
        ),
        Reply::json(404, json!({"error":"not_found"})),
        Reply::json(409, json!({"error":"intent_closed","state":"committed"})),
        Reply::json(
            409,
            json!({"error":"intent_closed","state":"awaiting_owner"}),
        ),
        Reply::bytes(404, b"<html>disabled route</html>".to_vec()),
        Reply::json(
            404,
            json!({"error":"not_found","detail":"private-response-sentinel"}),
        ),
        Reply::json(
            409,
            json!({"error":"candidate_not_built","detail":"private-response-sentinel"}),
        ),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let ExtensionSecretIntentCreation::Rejected(stale) = client
        .create_extension_secret_intent(&ledger_request())
        .await
        .unwrap()
    else {
        panic!("stale")
    };
    assert_eq!(
        stale.refusal,
        ExtensionSecretIntentRefusal::StaleBase { current: None }
    );
    let ExtensionSecretIntentCreation::Rejected(unbuilt) = client
        .create_extension_secret_intent(&candidate_request())
        .await
        .unwrap()
    else {
        panic!("unbuilt")
    };
    assert_eq!(
        unbuilt.refusal,
        ExtensionSecretIntentRefusal::CandidateNotBuilt
    );
    let ExtensionSecretIntentCreation::Rejected(unresolved) = client
        .create_extension_secret_intent(&candidate_request())
        .await
        .unwrap()
    else {
        panic!("unresolved")
    };
    assert_eq!(
        unresolved.refusal,
        ExtensionSecretIntentRefusal::ConsumerUnresolved
    );
    assert!(matches!(
        client.extension_secret_intent_status(&id()).await.unwrap(),
        ExtensionSecretIntentRead::Rejected(_)
    ));
    assert_eq!(
        client.extension_secret_intent_status(&id()).await.unwrap(),
        ExtensionSecretIntentRead::NotFound
    );
    let ExtensionSecretIntentCancellation::Rejected(closed) =
        client.cancel_extension_secret_intent(&id()).await.unwrap()
    else {
        panic!("closed")
    };
    assert_eq!(
        closed.refusal,
        ExtensionSecretIntentRefusal::IntentClosed {
            state: ExtensionSecretIntentState::Committed
        }
    );
    assert_unconfirmed(
        &client
            .cancel_extension_secret_intent(&id())
            .await
            .unwrap_err(),
        PairedHostOperation::CancelExtensionSecretIntent,
    );
    for _ in 0..2 {
        assert_unconfirmed(
            &client
                .extension_secret_intent_status(&id())
                .await
                .unwrap_err(),
            PairedHostOperation::ExtensionSecretIntentStatus,
        );
    }
    assert_unconfirmed(
        &client
            .create_extension_secret_intent(&candidate_request())
            .await
            .unwrap_err(),
        PairedHostOperation::CreateExtensionSecretIntent,
    );
    assert_eq!(fixture.finish().await.len(), 10);
}

#[tokio::test]
async fn uncertain_effects_are_not_replayed_when_caller_requests_status() {
    for cancel in [false, true] {
        for failure in 0..3 {
            let mut reply = Reply::json(500, json!({"error":"private-response-sentinel"}));
            if failure == 1 {
                reply.close_without_reply = true;
            }
            if failure == 2 {
                reply = Reply::json(if cancel { 200 } else { 201 }, observation("committed"));
            }
            let fixture =
                Fixture::new(vec![reply, Reply::json(404, json!({"error":"not_found"}))]).await;
            let mut client = PairedHostClient::new(&fixture.origin).unwrap();
            let operation = if cancel {
                PairedHostOperation::CancelExtensionSecretIntent
            } else {
                PairedHostOperation::CreateExtensionSecretIntent
            };
            let error = if cancel {
                client
                    .cancel_extension_secret_intent(&id())
                    .await
                    .unwrap_err()
            } else {
                client
                    .create_extension_secret_intent(&ledger_request())
                    .await
                    .unwrap_err()
            };
            assert_unconfirmed(&error, operation);
            if failure == 0 {
                assert!(matches!(
                    error,
                    PairedHostError::Unconfirmed {
                        reason: UnconfirmedReason::HostFailure,
                        ..
                    }
                ));
            }
            // After a lost create reply this is only an unrelated known ID;
            // the client cannot recover the new, unknown intent by this read.
            assert_eq!(
                client.extension_secret_intent_status(&id()).await.unwrap(),
                ExtensionSecretIntentRead::NotFound
            );
            let requests = fixture.finish().await;
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].method, "POST");
            assert_eq!(requests[1].method, "GET");
            assert_eq!(
                requests[0].path,
                if cancel {
                    format!("{INTENTS}/{ID}/cancel")
                } else {
                    INTENTS.into()
                }
            );
        }
    }
}

#[tokio::test]
async fn independent_client_has_no_inherited_device_and_redirects_are_not_followed() {
    let first = Fixture::new(vec![
        pairing(),
        Reply::json(200, observation("awaiting_entry")),
    ])
    .await;
    let second = Fixture::new(vec![
        Reply::json(
            403,
            json!({"error":"forbidden","code":"paired_console_device_required"}),
        ),
        Reply::json(302, json!({})).header("Location", &first.origin),
    ])
    .await;
    let mut paired = PairedHostClient::new(&first.origin).unwrap();
    paired
        .confirm_terminal_pairing(TerminalPairingCode::new("ABCD-EFGH-JKMN".into()).unwrap())
        .await
        .unwrap();
    let mut other = PairedHostClient::new(&second.origin).unwrap();
    assert!(matches!(
        other.extension_secret_intent_status(&id()).await.unwrap(),
        ExtensionSecretIntentRead::Rejected(_)
    ));
    let error = other
        .cancel_extension_secret_intent(&id())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        PairedHostError::Unconfirmed {
            operation: PairedHostOperation::CancelExtensionSecretIntent,
            reason: UnconfirmedReason::Redirect
        }
    ));
    assert!(matches!(
        paired.extension_secret_intent_status(&id()).await.unwrap(),
        ExtensionSecretIntentRead::Status(_)
    ));
    let own = first.finish().await;
    let other = second.finish().await;
    assert!(own[1].headers["cookie"].contains(DEVICE_COOKIE));
    assert!(
        other
            .iter()
            .all(|request| !request.headers.contains_key("cookie")
                && !request.headers.contains_key("x-host-dev-token"))
    );
    assert_eq!(own.len(), 2);
    assert_eq!(other.len(), 2);
}
