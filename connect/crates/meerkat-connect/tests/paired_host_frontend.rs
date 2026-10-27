//! Local HTTP fixtures only. These exercise the actual client transport and
//! protocol, not PairedHost deployment, real pairing, Telegram or activation.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use meerkat_connect::paired_host::{
    FrontendCandidateRequest, FrontendChange, FrontendChangeDigest, FrontendChangeRead,
    FrontendChangeRejectionKind, FrontendChangeStatus, FrontendCompileDepth,
    FrontendExtensionChannel, FrontendExtensionSource, FrontendIngress, FrontendInventoryRead,
    FrontendInventoryRejectionKind, FrontendNewExtensionChange, FrontendNewExtensionPreparation,
    FrontendNewExtensionRequest, FrontendNewExtensionStatus, FrontendNewExtensionSubmission,
    FrontendParticipants, FrontendPreparation, FrontendPreparedChange, FrontendPrerequisiteKind,
    FrontendRejectionKind, FrontendRouting, FrontendSecretBindingStatus, FrontendSecretPresence,
    FrontendSubmission, FrontendSubmitter, PairedHostClient, PairedHostError, PairedHostOperation,
    PairedHostRefusal, PairingCode, TerminalPairingCode, UnconfirmedReason,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    task::JoinHandle,
    time::timeout,
};

const PAIR_COOKIE: &str = "fixture-private-pair-cookie";
const DEVICE_COOKIE: &str = "fixture-private-device-cookie";
const CODE: &str = "421735";
const FIXTURE_BOUND: Duration = Duration::from_secs(5);

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

fn pairing_requested() -> Reply {
    Reply::json(200, json!({"ok":true})).header("Set-Cookie", &format!(
        "hc_console_pair={PAIR_COOKIE}; Max-Age=600; Path=/console/pair; HttpOnly; SameSite=Strict"
    ))
}

fn pairing_confirmed() -> Reply {
    Reply::json(200, json!({"ok":true})).header("Set-Cookie", &format!(
        "hc_console_device={DEVICE_COOKIE}; Max-Age=15552000; Path=/; HttpOnly; SameSite=Lax"
    )).header("Set-Cookie", "hc_console_pair=; Max-Age=0; Path=/console/pair")
}

fn request() -> FrontendCandidateRequest {
    FrontendCandidateRequest {
        extension_id: "telegram".into(),
        channel_id: Some("telegram:primary".into()),
        ingress: FrontendIngress::Polling,
        participants: FrontendParticipants {
            approved_groups: vec!["-1001234567890".into()],
        },
    }
}

fn receipt() -> Value {
    json!({
        "valid":true,
        "problems":[],
        "missing_prerequisites":[{"kind":"secret","name":"telegram_bot_token"}],
        "current":{"activation_id":"210","policy_digest":"sha256:current-fixture"},
        "candidate":{"channel_row_digest":"sha256:row-fixture",
            "policy_digest":"hc-policy-content-v1:sha256:policy-fixture","compile_depth":"policy_bundle"},
        "compiled_is_not_ready":true
    })
}

fn inventory() -> Value {
    json!({
        "current":{"activation_id":"210","policy_digest":"sha256:current-fixture"},
        "extensions":[
            {"extension_id":"telegram","enabled":true,"wheel_present":true,
                "channels":[{"channel_id":"telegram:primary","ingress":["polling","webhook"],
                    "secrets":[{"name":"TELEGRAM_BOT_TOKEN","present":true},
                        {"name":"TELEGRAM_WEBHOOK_SECRET","present":false}],
                    "approved_groups":{"declared":["-1001234567890"],"discovery":"unavailable"},
                    "senders":{"identities":["identity:parent-1"],"informational":true}}]},
            {"extension_id":"whatsapp","enabled":false,"wheel_present":false,"channels":[]}
        ],
        "routing":"sender_identity"
    })
}

#[tokio::test]
async fn inventory_accepts_closed_legacy_environment_and_vault_host_facts() {
    let mut body = inventory();
    body["extensions"][0]["channels"][0]["secrets"] = json!([
        {"name":"LEGACY_SECRET","present":true},
        {"name":"ENV_SECRET","source":"env","present":false,"generation":null},
        {"name":"missing","source":"vault","present":false,"generation":null,"binding_status":null},
        {"name":"pending_restart","source":"vault","present":true,"generation":2,"binding_status":"committed_not_live"},
        {"name":"started","source":"vault","present":true,"generation":3,"binding_status":"live"}
    ]);
    let fixture = Fixture::new(vec![Reply::json(200, body.clone())]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let FrontendInventoryRead::Inventory(observed) = client.frontend_inventory().await.unwrap()
    else {
        panic!("expected current host inventory");
    };
    let secrets = &observed.extensions[0].channels[0].secrets;
    assert_eq!(secrets[1].name(), "ENV_SECRET");
    assert!(!secrets[1].present());
    assert!(matches!(
        secrets[3],
        FrontendSecretPresence::Vault {
            generation: Some(2),
            binding_status: Some(FrontendSecretBindingStatus::CommittedNotLive),
            ..
        }
    ));
    assert!(matches!(
        secrets[4],
        FrontendSecretPresence::Vault {
            generation: Some(3),
            binding_status: Some(FrontendSecretBindingStatus::Live),
            ..
        }
    ));
    assert_eq!(serde_json::to_value(observed).unwrap(), body);
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
}

#[tokio::test]
async fn inventory_secret_rows_refuse_unknown_or_mixed_schemas() {
    for row in [
        json!({"name":"secret","present":true,"generation":1}),
        json!({"name":"secret","present":true,"source":"unknown","generation":null}),
        json!({"name":"secret","present":true,"source":"env","generation":1}),
        json!({"name":"secret","present":true,"source":"env","generation":null,"binding_status":"live"}),
        json!({"name":"secret","present":false,"source":"vault","binding_status":null}),
        json!({"name":"secret","present":true,"source":"vault","generation":1}),
        json!({"name":"secret","present":true,"source":"vault","generation":1,"binding_status":"authenticated"}),
        json!({"name":"secret","present":true,"source":"vault","generation":1,"binding_status":"live","value":"must-not-escape"}),
    ] {
        let mut body = inventory();
        body["extensions"][0]["channels"][0]["secrets"] = json!([row]);
        let fixture = Fixture::new(vec![Reply::json(200, body)]).await;
        let mut client = PairedHostClient::new(&fixture.origin).unwrap();
        let error = client.frontend_inventory().await.unwrap_err();
        assert_eq!(
            error,
            PairedHostError::Unconfirmed {
                operation: PairedHostOperation::FrontendInventory,
                reason: UnconfirmedReason::UnexpectedResponse,
            }
        );
        assert!(!error.to_string().contains("must-not-escape"));
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn terminal_pairing_uses_only_the_captured_origin_and_device_cookie() {
    let fixture = Fixture::new(vec![pairing_confirmed(), Reply::json(200, inventory())]).await;
    let origin = fixture.origin.clone();
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let code = TerminalPairingCode::new("abcd-efgh-jkmn".into()).unwrap();
    assert_eq!(format!("{code:?}"), "TerminalPairingCode([redacted])");
    client.confirm_terminal_pairing(code).await.unwrap();
    assert!(matches!(
        client.frontend_inventory().await.unwrap(),
        FrontendInventoryRead::Inventory(_)
    ));
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/console/pair/terminal");
    assert_eq!(requests[0].headers["origin"], origin);
    assert!(!requests[0].headers.contains_key("cookie"));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        json!({"code":"ABCDEFGHJKMN"})
    );
    assert!(requests[1].headers["cookie"].contains(DEVICE_COOKIE));
}

#[tokio::test]
async fn terminal_pairing_preserves_refusals_and_does_not_fall_back_or_retry() {
    for (status, body, reason) in [
        (
            404,
            json!({"error":"not_found"}),
            Some(PairedHostRefusal::EndpointUnavailable),
        ),
        (
            403,
            json!({"error":"wrong_code"}),
            Some(PairedHostRefusal::WrongCode),
        ),
        (
            403,
            json!({"error":"no_pending_request"}),
            Some(PairedHostRefusal::NoPendingRequest),
        ),
        (
            403,
            json!({"error":"pairing_unavailable"}),
            Some(PairedHostRefusal::PairingUnavailable),
        ),
        (
            503,
            json!({"error":"pairing_unavailable"}),
            Some(PairedHostRefusal::PairingUnavailable),
        ),
        (
            429,
            json!({"error":"pairing_locked"}),
            Some(PairedHostRefusal::PairingLocked),
        ),
        (
            500,
            json!({"error":"unrecognized-private-host-detail"}),
            None,
        ),
    ] {
        let fixture = Fixture::new(vec![Reply::json(status, body)]).await;
        let mut client = PairedHostClient::new(&fixture.origin).unwrap();
        let error = client
            .confirm_terminal_pairing(TerminalPairingCode::new("ABCDEFGHJKMN".into()).unwrap())
            .await
            .unwrap_err();
        let expected = match reason {
            Some(reason) => PairedHostError::Refused {
                operation: PairedHostOperation::ConfirmTerminalPairing,
                reason,
            },
            None => PairedHostError::Unconfirmed {
                operation: PairedHostOperation::ConfirmTerminalPairing,
                reason: UnconfirmedReason::UnexpectedResponse,
            },
        };
        assert_eq!(error, expected);
        assert!(
            !error
                .to_string()
                .contains("unrecognized-private-host-detail")
        );
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/console/pair/terminal");
    }
}

#[test]
fn terminal_pairing_code_accepts_only_the_host_alphabet_and_optional_grouping() {
    for code in ["ABCDEFGHJKMN", "ABCD-EFGH-JKMN", "abcd-efgh-jkmn"] {
        assert!(TerminalPairingCode::new(code.into()).is_ok());
    }
    for code in [
        "421735",
        "ABCD-EFGH-JKM",
        "ABCD EFGH JKMN",
        "ABCD-EFGH-JKM0",
        "ABCD-EFGH-JKMI",
        "ABCD-EFGH-JKMN\n",
        "ABCD-EFGH-JKMé",
    ] {
        assert_eq!(
            TerminalPairingCode::new(code.into()).unwrap_err(),
            PairedHostError::InvalidTerminalPairingCode
        );
    }
}

#[tokio::test]
async fn inventory_get_is_fresh_typed_and_does_not_infer_mode_prerequisites_or_readiness() {
    let first = inventory();
    let mut later = first.clone();
    later["current"]["activation_id"] = json!("211");
    later["extensions"][0]["enabled"] = json!(false);
    let mut polling = receipt();
    polling["missing_prerequisites"] = json!([]);
    let fixture = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        Reply::json(200, first.clone()),
        Reply::json(200, polling.clone()),
        Reply::json(200, later.clone()),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client.request_pairing("inventory fixture").await.unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let FrontendInventoryRead::Inventory(result) = client.frontend_inventory().await.unwrap()
    else {
        panic!("typed inventory expected");
    };
    assert_eq!(result.routing, FrontendRouting::SenderIdentity);
    assert!(!result.extensions[0].channels[0].secrets[1].present());
    assert!(!result.extensions[1].enabled && !result.extensions[1].wheel_present);
    assert_eq!(serde_json::to_value(result).unwrap(), first);
    let FrontendPreparation::Candidate(result) =
        client.prepare_candidate(&request()).await.unwrap()
    else {
        panic!("typed candidate expected");
    };
    assert!(result.valid && result.compiled_is_not_ready);
    assert!(
        result.missing_prerequisites.is_empty(),
        "missing webhook secret is not a polling judgment"
    );
    assert_eq!(serde_json::to_value(result).unwrap(), polling);
    let FrontendInventoryRead::Inventory(result) = client.frontend_inventory().await.unwrap()
    else {
        panic!("fresh inventory expected");
    };
    assert_eq!(serde_json::to_value(result).unwrap(), later);
    let records = fixture.finish().await;
    assert_eq!(records.len(), 5);
    for index in [2, 4] {
        assert_eq!(records[index].method, "GET");
        assert_eq!(
            records[index].path,
            "/console/extensions/frontend/inventory"
        );
        assert!(records[index].body.is_empty());
        assert_eq!(records[index].headers["cache-control"], "no-store");
        assert_eq!(
            records[index].headers["cookie"],
            format!("hc_console_device={DEVICE_COOKIE}")
        );
        assert!(!records[index].headers.contains_key("authorization"));
    }
    let candidate: Value = serde_json::from_slice(&records[3].body).unwrap();
    assert_eq!(records[3].method, "POST");
    assert!(candidate.get("target_identity").is_none());
}

#[tokio::test]
async fn inventory_retains_snapshot_and_unavailable_failures_without_retry_or_cached_success() {
    let cases = [
        (
            409,
            "conflict",
            "snapshot_changed",
            "The current activation, policy or ledger changed during validation.",
            FrontendInventoryRejectionKind::Conflict,
        ),
        (
            503,
            "unavailable",
            "current_inputs_invalid",
            "The current ledger or compiled family contacts cannot be read.",
            FrontendInventoryRejectionKind::Unavailable,
        ),
        (
            500,
            "internal_error",
            "internal_error",
            "Validation stopped on an unexpected error.",
            FrontendInventoryRejectionKind::InternalError,
        ),
    ];
    let fixture = Fixture::new(
        cases
            .iter()
            .map(|(status, error, code, message, _)| {
                Reply::json(
                    *status,
                    json!({"error":error,"problems":[{"code":code,"path":"","message":message}]}),
                )
            })
            .collect(),
    )
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for (status, _, code, message, kind) in cases {
        let FrontendInventoryRead::Rejected(result) = client.frontend_inventory().await.unwrap()
        else {
            panic!("no inventory or readiness invented after a failed snapshot");
        };
        assert_eq!(result.http_status, status);
        assert_eq!(result.error, kind);
        assert_eq!(result.problems[0].code, code);
        assert_eq!(result.problems[0].message, message);
    }
    assert_eq!(
        fixture.finish().await.len(),
        3,
        "one request per explicit observation"
    );
}

#[tokio::test]
async fn inventory_refuses_unknown_secret_fields_false_routing_promises_and_unsafe_response_shapes()
{
    let mut unknown = inventory();
    unknown["extensions"][0]["channels"][0]["secrets"][0]["value"] = json!(DEVICE_COOKIE);
    let mut target = inventory();
    target["target_identity"] = json!("identity:parent-1");
    let mut routing = inventory();
    routing["routing"] = json!("target_agent");
    let mut senders = inventory();
    senders["extensions"][0]["channels"][0]["senders"]["informational"] = json!(false);
    let mut discovery = inventory();
    discovery["extensions"][0]["channels"][0]["approved_groups"]["discovery"] = json!("available");
    let mut missing = inventory();
    missing["current"]
        .as_object_mut()
        .unwrap()
        .remove("policy_digest");
    let duplicate = inventory().to_string().replacen(
        "\"enabled\":true",
        "\"enabled\":true,\"enabled\":false",
        1,
    );
    let mut no_store = Reply::json(200, inventory());
    no_store.headers.retain(|(name, _)| name != "Cache-Control");
    let mut replies: Vec<_> = [unknown, target, routing, senders, discovery, missing]
        .into_iter()
        .map(|value| Reply::json(200, value))
        .collect();
    replies.extend([Reply::bytes(200, duplicate.into_bytes()), no_store,
        Reply::json(409, json!({"error":"conflict","problems":[{"code":"snapshot_changed","path":"","message":DEVICE_COOKIE}]})),
        Reply::json(403, json!({"error":DEVICE_COOKIE,"code":CODE}))]);
    let count = replies.len();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..count {
        let error = client.frontend_inventory().await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::FrontendInventory,
                UnconfirmedReason::UnexpectedResponse
            )
        );
        assert_safe(&error);
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn inventory_has_bounded_bodies_and_no_redirect_or_origin_credential_fallback() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut streamed = Reply::bytes(200, vec![b'x'; 64 * 1024 + 1]);
    streamed.chunked = true;
    let first = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        Reply::json(
            403,
            json!({"error":"forbidden","code":"console_owner_required"}),
        ),
        Reply::bytes(200, vec![b'x'; 64 * 1024 + 1]),
        streamed,
        Reply::json(302, json!({})).header(
            "Location",
            &format!("http://{}/", destination.local_addr().unwrap()),
        ),
    ])
    .await;
    let second = Fixture::new(vec![Reply::json(
        403,
        json!({"error":"forbidden","code":"console_owner_required"}),
    )])
    .await;
    let mut client = PairedHostClient::new(&first.origin).unwrap();
    client.request_pairing("inventory fixture").await.unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let mut other = PairedHostClient::new(&second.origin).unwrap();
    for client in [&mut client, &mut other] {
        assert_eq!(
            client.frontend_inventory().await.unwrap_err(),
            PairedHostError::Refused {
                operation: PairedHostOperation::FrontendInventory,
                reason: PairedHostRefusal::ConsoleOwnerRequired,
            }
        );
    }
    for reason in [
        UnconfirmedReason::ResponseTooLarge,
        UnconfirmedReason::ResponseTooLarge,
        UnconfirmedReason::Redirect,
    ] {
        let error = client.frontend_inventory().await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(PairedHostOperation::FrontendInventory, reason)
        );
        assert_safe(&error);
    }
    assert!(
        timeout(Duration::from_millis(100), destination.accept())
            .await
            .is_err()
    );
    let first = first.finish().await;
    assert_eq!(first.len(), 6);
    assert_eq!(
        first[2].headers["cookie"],
        format!("hc_console_device={DEVICE_COOKIE}")
    );
    let second = second.finish().await;
    assert!(!second[0].headers.contains_key("cookie"));
}

fn unconfirmed(operation: PairedHostOperation, reason: UnconfirmedReason) -> PairedHostError {
    PairedHostError::Unconfirmed { operation, reason }
}

fn assert_safe(error: &PairedHostError) {
    let text = format!("{error}; {error:?}");
    for private in [
        CODE,
        PAIR_COOKIE,
        DEVICE_COOKIE,
        "private-marker",
        "http://",
        "https://",
    ] {
        assert!(!text.contains(private), "value-free client error");
    }
    assert!(std::error::Error::source(error).is_none());
}

#[tokio::test]
async fn actual_transport_pairs_privately_and_preserves_valid_but_not_ready() {
    let expected = receipt();
    let fixture = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        Reply::json(200, expected.clone()),
    ])
    .await;
    let origin = fixture.origin.clone();
    let mut client = PairedHostClient::new(&origin).unwrap();
    let code = PairingCode::new(CODE.into()).unwrap();
    assert!(!format!("{code:?}").contains(CODE));
    client
        .request_pairing("meerkat-connect fixture")
        .await
        .unwrap();
    client.confirm_pairing(code).await.unwrap();
    let FrontendPreparation::Candidate(result) =
        client.prepare_candidate(&request()).await.unwrap()
    else {
        panic!("candidate receipt expected");
    };
    assert!(result.valid && result.compiled_is_not_ready);
    assert_eq!(
        result.missing_prerequisites[0].kind,
        FrontendPrerequisiteKind::Secret
    );
    assert_eq!(
        result.candidate.compile_depth,
        FrontendCompileDepth::PolicyBundle
    );
    assert_eq!(serde_json::to_value(&result).unwrap(), expected);
    assert!(!format!("{client:?}").contains(DEVICE_COOKIE));
    let requests = fixture.finish().await;
    assert_eq!(
        requests.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        [
            "/console/pair/request",
            "/console/pair/confirm",
            "/console/extensions/frontend/prepare"
        ]
    );
    for request in &requests {
        assert_eq!(request.headers["origin"], origin);
        assert_eq!(request.headers["content-type"], "application/json");
        assert!(!request.headers.contains_key("authorization"));
        assert!(!request.headers.contains_key("x-forwarded-host"));
    }
    assert!(!requests[0].headers.contains_key("cookie"));
    assert_eq!(
        requests[1].headers["cookie"],
        format!("hc_console_pair={PAIR_COOKIE}")
    );
    assert_eq!(
        requests[2].headers["cookie"],
        format!("hc_console_device={DEVICE_COOKIE}")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
        json!({"code":CODE})
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[2].body).unwrap(),
        serde_json::to_value(request()).unwrap()
    );
    assert!(
        !std::str::from_utf8(&requests[2].body)
            .unwrap()
            .contains(CODE)
    );
}

#[tokio::test]
async fn independent_origins_do_not_share_device_credentials_and_revocation_is_host_owned() {
    let refusal = json!({"error":"forbidden","code":"console_owner_required"});
    let first = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        Reply::json(403, refusal.clone()),
    ])
    .await;
    let second = Fixture::new(vec![Reply::json(403, refusal)]).await;
    let mut owner = PairedHostClient::new(&first.origin).unwrap();
    let mut other = PairedHostClient::new(&second.origin).unwrap();
    owner.request_pairing("first device").await.unwrap();
    owner
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    for client in [&mut other, &mut owner] {
        assert_eq!(
            client.prepare_candidate(&request()).await.unwrap_err(),
            PairedHostError::Refused {
                operation: PairedHostOperation::PrepareCandidate,
                reason: PairedHostRefusal::ConsoleOwnerRequired
            }
        );
    }
    let other_requests = second.finish().await;
    assert!(!other_requests[0].headers.contains_key("cookie"));
    let owner_requests = first.finish().await;
    assert_eq!(
        owner_requests.len(),
        3,
        "a refusal must not automatically restart pairing"
    );
    assert_eq!(
        owner_requests[2].headers["cookie"],
        format!("hc_console_device={DEVICE_COOKIE}")
    );
}

#[tokio::test]
async fn redirect_is_unconfirmed_and_never_sends_code_or_cookie_to_destination() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let location = format!(
        "http://{}/steal?private-marker",
        destination.local_addr().unwrap()
    );
    let fixture = Fixture::new(vec![
        pairing_requested(),
        Reply::json(307, json!({"private-marker":CODE})).header("Location", &location),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client.request_pairing("redirect fixture").await.unwrap();
    let error = client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        unconfirmed(
            PairedHostOperation::ConfirmPairing,
            UnconfirmedReason::Redirect
        )
    );
    assert_safe(&error);
    assert_eq!(fixture.finish().await.len(), 2);
    assert!(
        timeout(Duration::from_millis(50), destination.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn current_snapshot_problems_and_typed_request_rejection_are_preserved() {
    let problems = json!([
        {"code":"surfaces_lock_stale","path":"policy","message":"The candidate changes the desired extension surfaces; deployment/surfaces.lock must be regenerated and reviewed first."},
        {"code":"snapshot_changed","path":"","message":"The current activation, policy or ledger changed during validation."}
    ]);
    let mut changed = receipt();
    changed["valid"] = json!(false);
    changed["problems"] = problems;
    changed["candidate"]["compile_depth"] = json!("channel_binding");
    changed["candidate"]["policy_digest"] = Value::Null;
    changed["current"]["activation_id"] = Value::Null;
    changed["missing_prerequisites"] = json!([{"kind":"route","name":"telegram.webhook"}]);
    let rejected = json!({"error":"invalid_request","problems":[{
        "code":"invalid_value","path":"participants.approved_groups",
        "message":"A request field has a value outside its allowed form."
    }]});
    let fixture = Fixture::new(vec![
        Reply::json(200, changed.clone()),
        Reply::json(400, rejected.clone()),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let FrontendPreparation::Candidate(result) =
        client.prepare_candidate(&request()).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(serde_json::to_value(&result).unwrap(), changed);
    let mut malformed = request();
    malformed.participants.approved_groups = vec![String::new()];
    let FrontendPreparation::Rejected(result) = client.prepare_candidate(&malformed).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(result.http_status, 400);
    assert_eq!(result.error, FrontendRejectionKind::InvalidRequest);
    assert_eq!(
        serde_json::to_value(&result.problems).unwrap(),
        rejected["problems"]
    );
    let requests = fixture.finish().await;
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
        serde_json::to_value(malformed).unwrap(),
        "host validation must receive the actual candidate, not a rewritten policy"
    );
}

#[tokio::test]
async fn bounded_parser_refuses_ambiguous_or_secret_bearing_protocol_data() {
    let base = receipt();
    let mut not_contract = base.clone();
    not_contract["compiled_is_not_ready"] = json!(false);
    let mut contradictory = base.clone();
    contradictory["valid"] = json!(false);
    let mut unknown = base.clone();
    unknown["activation_token"] = json!("private-marker");
    let mut missing = base.clone();
    missing["current"]
        .as_object_mut()
        .unwrap()
        .remove("activation_id");
    let mut raw_problem = base.clone();
    raw_problem["valid"] = json!(false);
    raw_problem["problems"] =
        json!([{"code":"internal_error","path":"policy","message":"private-marker"}]);
    let mut obsolete_identity = base.clone();
    obsolete_identity["missing_prerequisites"] =
        json!([{"kind":"identity","name":"identity:parent-1"}]);
    let mut obsolete_problem = base.clone();
    obsolete_problem["valid"] = json!(false);
    obsolete_problem["problems"] = json!([{"code":"target_identity_unknown","path":"target_identity","message":"The target identity is not in PairedHost's roster."}]);
    let duplicate = serde_json::to_string(&base).unwrap().replacen(
        "\"valid\":true",
        "\"valid\":false,\"valid\":true",
        1,
    );
    let nested_duplicate = serde_json::to_string(&base).unwrap().replacen(
        "\"activation_id\":\"210\"",
        "\"activation_id\":\"private-marker\",\"activation_id\":\"210\"",
        1,
    );
    let mut replies: Vec<_> = [
        not_contract,
        contradictory,
        unknown,
        missing,
        raw_problem,
        obsolete_identity,
        obsolete_problem,
    ]
    .into_iter()
    .map(|value| Reply::json(200, value))
    .collect();
    replies.extend([
        Reply::bytes(200, duplicate.into_bytes()),
        Reply::bytes(200, nested_duplicate.into_bytes()),
        Reply::json(403, json!({"error":DEVICE_COOKIE,"code":CODE})),
        Reply::bytes(200, b"<html>private-marker</html>".to_vec()),
    ]);
    let count = replies.len();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..count {
        let error = client.prepare_candidate(&request()).await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::PrepareCandidate,
                UnconfirmedReason::UnexpectedResponse
            )
        );
        assert_safe(&error);
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn size_limits_cover_declared_streamed_and_cookie_responses_without_retry() {
    let oversized = vec![b'x'; 64 * 1024 + 1];
    let mut streamed = Reply::bytes(200, oversized.clone());
    streamed.chunked = true;
    let oversized_cookie = Reply::json(200, receipt()).header(
        "Set-Cookie",
        &format!("hc_console_device={}; Path=/", "x".repeat(4096)),
    );
    let fixture = Fixture::new(vec![
        Reply::bytes(200, oversized),
        streamed,
        oversized_cookie,
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..3 {
        let error = client.prepare_candidate(&request()).await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::PrepareCandidate,
                UnconfirmedReason::ResponseTooLarge
            )
        );
        assert_safe(&error);
    }
    assert_eq!(fixture.finish().await.len(), 3);
}

#[tokio::test]
async fn acknowledged_pairing_requires_the_expected_scoped_cookie_and_true_body() {
    let fixture = Fixture::new(vec![
        Reply::json(200, json!({"ok":true})),
        Reply::json(200, json!({"ok":true})).header(
            "Set-Cookie",
            "hc_console_pair=private-marker; Path=/foreign; HttpOnly",
        ),
        Reply::json(200, json!({"ok":false})).header(
            "Set-Cookie",
            "hc_console_pair=private-marker; Path=/console/pair; HttpOnly",
        ),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..3 {
        let error = client.request_pairing("fixture").await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::RequestPairing,
                UnconfirmedReason::UnexpectedResponse
            )
        );
        assert_safe(&error);
    }
    assert_eq!(
        fixture.finish().await.len(),
        3,
        "each attempt is explicit, never automatic"
    );
}

#[tokio::test]
async fn exact_pairing_refusals_are_values_not_raw_error_messages() {
    let fixture = Fixture::new(vec![
        Reply::json(429, json!({"error":"too_many_requests"})),
        Reply::json(403, json!({"error":"wrong_code"})),
        Reply::json(403, json!({"error":"no_pending_request"})),
        Reply::json(404, json!({"error":"not_found"})),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let errors = [
        client.request_pairing("fixture").await.unwrap_err(),
        client
            .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
            .await
            .unwrap_err(),
        client
            .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
            .await
            .unwrap_err(),
        client.prepare_candidate(&request()).await.unwrap_err(),
    ];
    let expected = [
        (
            PairedHostOperation::RequestPairing,
            PairedHostRefusal::TooManyRequests,
        ),
        (
            PairedHostOperation::ConfirmPairing,
            PairedHostRefusal::WrongCode,
        ),
        (
            PairedHostOperation::ConfirmPairing,
            PairedHostRefusal::NoPendingRequest,
        ),
        (
            PairedHostOperation::PrepareCandidate,
            PairedHostRefusal::EndpointUnavailable,
        ),
    ];
    for (error, (operation, reason)) in errors.iter().zip(expected) {
        assert_eq!(*error, PairedHostError::Refused { operation, reason });
        assert_safe(error);
    }
    assert_eq!(fixture.finish().await.len(), 4);
}

#[tokio::test]
async fn lost_pairing_response_is_unconfirmed_not_an_automatic_second_notification() {
    let mut lost = Reply::json(200, json!({"ok":true}));
    lost.close_without_reply = true;
    let fixture = Fixture::new(vec![lost]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let error = client.request_pairing("fixture").await.unwrap_err();
    assert_eq!(
        error,
        unconfirmed(
            PairedHostOperation::RequestPairing,
            UnconfirmedReason::Transport
        )
    );
    assert_safe(&error);
    assert_eq!(fixture.finish().await.len(), 1);
}

#[tokio::test]
async fn dropping_pending_pairing_observation_does_not_claim_or_retry_host_completion() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let arrived = Arc::new(Notify::new());
    let received = arrived.clone();
    let (release, released) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut stream, _) = timeout(FIXTURE_BOUND, listener.accept())
            .await
            .unwrap()
            .unwrap();
        let first = timeout(FIXTURE_BOUND, read_request(&mut stream))
            .await
            .unwrap();
        received.notify_one();
        timeout(FIXTURE_BOUND, released).await.unwrap().unwrap();
        pairing_requested().write(&mut stream).await;
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "no detached retry"
        );
        vec![first]
    });
    let fixture = Fixture {
        origin,
        task: Some(task),
    };
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let mut pending = Box::pin(client.request_pairing("fixture"));
    tokio::select! {
        _ = arrived.notified() => {},
        result = &mut pending => panic!("reply is still held, got {result:?}"),
        _ = tokio::time::sleep(FIXTURE_BOUND) => panic!("request did not reach fixture"),
    }
    drop(pending);
    release.send(()).unwrap();
    let records = fixture.finish().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path, "/console/pair/request");
    // No success/rollback receipt is invented for the dropped future. The
    // fixture may have accepted work before local observation was cancelled.
}

#[tokio::test]
async fn invalid_local_input_stays_local_and_request_schema_is_closed() {
    for origin in [
        "file:///tmp/private-marker",
        "https://owner:private-marker@example.test",
        "https://@example.test",
        "https://example.test/path",
        "https://example.test/path/..",
        "https://example.test/?code=private-marker",
        "https://example.test/#private-marker",
        "https://example.test\\",
        " https://example.test",
        "https://example.test\n",
    ] {
        let error = PairedHostClient::new(origin).unwrap_err();
        assert_eq!(error, PairedHostError::InvalidOrigin);
        assert_safe(&error);
    }
    for code in [
        "12345",
        "1234567",
        "１２３４５６",
        "private-marker",
        "123 45",
    ] {
        let error = PairingCode::new(code.into()).unwrap_err();
        assert_eq!(error, PairedHostError::InvalidPairingCode);
        assert_safe(&error);
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client =
        PairedHostClient::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    assert_eq!(
        client
            .request_pairing("private-marker\n")
            .await
            .unwrap_err(),
        PairedHostError::InvalidPairingLabel
    );
    let mut huge = request();
    huge.participants.approved_groups = vec!["x".repeat(64 * 1024)];
    assert_eq!(
        client.prepare_candidate(&huge).await.unwrap_err(),
        PairedHostError::RequestTooLarge
    );
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    for field in ["target_identity", "known_senders", "config_overrides"] {
        let mut extra = serde_json::to_value(request()).unwrap();
        extra[field] = json!({"secret":"private-marker"});
        assert!(serde_json::from_value::<FrontendCandidateRequest>(extra).is_err());
    }
    let mut null_channel = serde_json::to_value(request()).unwrap();
    null_channel["channel_id"] = Value::Null;
    assert!(
        serde_json::from_value::<FrontendCandidateRequest>(null_channel).is_err(),
        "explicit null must not be rewritten as an omitted channel"
    );
    let duplicate = r#"{"extension_id":"first","extension_id":"second","ingress":"polling","participants":{"approved_groups":[]}}"#;
    assert!(serde_json::from_str::<FrontendCandidateRequest>(duplicate).is_err());
    let mut without_channel = request();
    without_channel.channel_id = None;
    assert!(
        serde_json::to_value(without_channel)
            .unwrap()
            .get("channel_id")
            .is_none()
    );
}

const CHANGE_DIGEST: &str =
    "hc-frontend-change-v1:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const OTHER_CHANGE_DIGEST: &str =
    "hc-frontend-change-v1:sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const CHANGES_PATH: &str = "/console/extensions/frontend/change-requests";

fn new_extension_request() -> FrontendNewExtensionRequest {
    FrontendNewExtensionRequest {
        distribution: "paired-host-ext-signal".into(),
        version: "0.1.0".into(),
        source: FrontendExtensionSource {
            repo: "https://github.com/fixture/paired-host-ext-signal.git".into(),
            tag: "v0.1.0".into(),
            commit: "a".repeat(40),
            package_path: None,
        },
        configuration: meerkat_connect::paired_host::FrontendExtensionConfiguration::Channel(
            FrontendExtensionChannel {
                ingress: FrontendIngress::Polling,
                approved_groups: vec!["fixture-group".into()],
            },
        ),
    }
}

fn new_extension_raw() -> String {
    let proposal = serde_json::to_string(&new_extension_request()).unwrap();
    // Synthetic owner output. The client retains this exact summary and does
    // not calculate a summary, version or digest of its own.
    format!(
        r#"{{ "change_digest":"{CHANGE_DIGEST}","kind":"new_extension","base":{{"activation_id":"214","policy_digest":"fixture-policy","ledger_digest":"fixture-ledger"}},"proposal":{proposal},"unverified":["manifest","facades","channel_template","secrets"],"prerequisites":[{{"kind":"build_and_review","state":"pending"}}],"unified_summary":"new extension paired-host-ext-signal 0.1.0\nsource commit: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nchannel: polling, fixture-group\nunverified until built and reviewed\nsubmitting activates nothing\n"}}"#
    )
}

fn new_extension_prepared(raw: &str) -> Reply {
    Reply::bytes(
        200,
        format!(r#"{{"valid":true,"problems":[],"change":{raw}}}"#).into_bytes(),
    )
}

async fn capture_new_extension(client: &mut PairedHostClient) -> FrontendNewExtensionChange {
    let FrontendNewExtensionPreparation::Candidate(receipt) = client
        .prepare_new_extension(&new_extension_request())
        .await
        .unwrap()
    else {
        panic!("new extension candidate expected");
    };
    assert!(receipt.valid && receipt.problems.is_empty());
    receipt.change.expect("host supplied immutable proposal")
}

fn new_extension_built() -> Value {
    json!({
        "extension_id":"signal", "distribution":"paired-host-ext-signal", "version":"0.1.0",
        "source_commit":"a".repeat(40), "wheel_sha256":"b".repeat(64),
        "release_evidence_sha256":"c".repeat(64), "facades":["channel_ingress.v1","sender.v1"],
        "channel_template":{"id":"signal:primary","extension_defined":{"threshold":0.125}},
        "secret_names":["signal_token"]
    })
}

fn new_extension_status(state: &str) -> Value {
    if state == "rejected" {
        return json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time","kind":"new_extension",
            "state":"rejected","reason":"Operator declined this synthetic proposal","decided_at":"fixture-decision"});
    }
    let built = !matches!(state, "submitted" | "building");
    let mut value = json!({
        "request_id":CHANGE_DIGEST, "submitted_at":"fixture-time", "kind":"new_extension", "state":state,
        "staged_at": if state == "submitted" {Value::Null} else {json!("fixture-stage")},
        "built": if built {new_extension_built()} else {Value::Null},
        "prerequisites": if built {json!([{"kind":"secret","name":"signal_token","provisioned":false}])}
            else {json!([{"kind":"build_and_review","state":"pending"}])}
    });
    if matches!(state, "activating" | "activation_uncertain" | "applied") {
        value["activation_id"] = json!("215");
    }
    if state == "activation_uncertain" {
        value["journal_phase"] = json!("commit");
    }
    if state == "failed" {
        value["stage"] = json!("review");
        value["reason"] = json!("Synthetic operator refusal");
    }
    value
}

#[tokio::test]
async fn new_extension_request_captures_exact_summary_and_submits_only_to_the_paired_host() {
    let raw = new_extension_raw();
    let fixture = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        new_extension_prepared(&raw),
        submission_reply(201, &raw),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client
        .request_pairing("new extension fixture")
        .await
        .unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let change = capture_new_extension(&mut client).await;
    assert_eq!(change.proposal(), &new_extension_request());
    assert_eq!(change.base().activation_id.as_deref(), Some("214"));
    assert_eq!(change.change_digest().as_str(), CHANGE_DIGEST);
    assert!(
        change
            .unified_summary()
            .ends_with("submitting activates nothing\n")
    );
    assert_eq!(serde_json::to_string(&change).unwrap(), raw);
    let FrontendNewExtensionSubmission::Recorded(record) = client
        .submit_frontend_new_extension_change(&change)
        .await
        .unwrap()
    else {
        panic!("request acknowledgment expected");
    };
    assert_eq!(record.http_status, 201);
    assert_eq!(serde_json::to_value(&record).unwrap()["state"], "submitted");
    assert!(
        serde_json::to_value(&record)
            .unwrap()
            .get("activation_id")
            .is_none()
    );
    assert_eq!(
        client
            .submit_frontend_new_extension_change(&record.change)
            .await
            .unwrap_err(),
        PairedHostError::UnboundFrontendChange,
        "POST echo is not another prepared instruction"
    );
    let records = fixture.finish().await;
    assert_eq!(records.len(), 4);
    assert_eq!(records[2].path, "/console/extensions/frontend/prepare");
    assert_eq!(
        serde_json::from_slice::<Value>(&records[2].body).unwrap(),
        json!({"new_extension":new_extension_request()})
    );
    assert_eq!(records[3].path, CHANGES_PATH);
    assert_eq!(
        std::str::from_utf8(&records[3].body).unwrap(),
        format!(r#"{{"change_digest":"{CHANGE_DIGEST}","change":{raw}}}"#)
    );
    for request in &records[2..] {
        assert_eq!(request.method, "POST");
        assert_eq!(request.headers["origin"], client.selected_origin());
        assert_eq!(
            request.headers["cookie"],
            format!("hc_console_device={DEVICE_COOKIE}")
        );
        assert!(!request.headers.contains_key("authorization"));
        assert!(
            !std::str::from_utf8(&request.body)
                .unwrap()
                .contains(DEVICE_COOKIE)
        );
    }
}

#[tokio::test]
async fn new_extension_attempt_recorded_receipts_preserve_optional_host_observation() {
    let raw = new_extension_raw();
    // PR451 also returns the original attempt-less record for an idempotent
    // POST during a later attempt. Absence must not become a synthetic 1.
    let cases = [(201, Some(2)), (200, Some(999)), (200, None)];
    let mut replies = vec![new_extension_prepared(&raw)];
    for (status, attempt) in cases {
        let mut reply = submission_reply(status, &raw);
        if let Some(attempt) = attempt {
            let mut body: Value = serde_json::from_slice(&reply.body).unwrap();
            body["attempt"] = json!(attempt);
            reply.body = serde_json::to_vec(&body).unwrap();
        }
        replies.push(reply);
    }
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_new_extension(&mut client).await;
    for (status, attempt) in cases {
        // Each explicit transport call tests a distinct host reply. Approval
        // and one-use admission are exercised separately through the tool gate.
        let FrontendNewExtensionSubmission::Recorded(record) = client
            .submit_frontend_new_extension_change(&change)
            .await
            .unwrap()
        else {
            panic!("recorded owner request expected");
        };
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value.get("attempt"), attempt.map(|n| json!(n)).as_ref());
        assert_eq!(value["http_status"], status);
        assert_eq!(value["request_id"], CHANGE_DIGEST);
        assert_eq!(value["submitted_at"], "2026-10-07T00:00:00+00:00");
        assert_eq!(value["submitted_by"], "paired-device");
        assert_eq!(
            value["change"],
            serde_json::from_str::<Value>(&raw).unwrap()
        );
        assert!(value.get("activation_id").is_none());
        assert_eq!(
            client
                .submit_frontend_new_extension_change(&record.change)
                .await
                .unwrap_err(),
            PairedHostError::UnboundFrontendChange,
            "a later-attempt acknowledgment is not a prepared instruction"
        );
    }
    let records = fixture.finish().await;
    assert_eq!(records.len(), 4);
    for record in &records[1..] {
        assert_eq!(record.method, "POST");
        assert_eq!(record.path, CHANGES_PATH);
        assert_eq!(
            std::str::from_utf8(&record.body).unwrap(),
            format!(r#"{{"change_digest":"{CHANGE_DIGEST}","change":{raw}}}"#)
        );
    }
}

#[tokio::test]
async fn new_extension_attempt_progress_preserves_all_stages_and_raw_owner_template() {
    let expected: Vec<_> = [
        "submitted",
        "building",
        "built",
        "under_review",
        "activating",
        "activation_uncertain",
        "applied",
        "failed",
    ]
    .into_iter()
    .map(|state| {
        let mut value = new_extension_status(state);
        value["attempt"] = json!(if state == "failed" { 999 } else { 2 });
        value
    })
    .collect();
    let replies = expected
        .iter()
        .map(|value| {
            Reply::bytes(
                200,
                value
                    .to_string()
                    .replace("\"threshold\":0.125", "\"threshold\":1e-06")
                    .into_bytes(),
            )
        })
        .collect();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    for mut expected in expected {
        let FrontendChangeRead::NewExtensionStatus(observed) =
            client.frontend_change_status(&digest).await.unwrap()
        else {
            panic!("new extension owner progress expected");
        };
        assert_eq!(observed.request_id(), &digest);
        if !expected["built"].is_null() {
            expected["built"]["channel_template"]["extension_defined"]["threshold"] = json!(1e-06);
            assert!(serde_json::to_string(&observed).unwrap().contains("1e-06"));
        }
        assert_eq!(serde_json::to_value(observed).unwrap(), expected);
    }
    let records = fixture.finish().await;
    assert_eq!(records.len(), 8);
    assert!(
        records
            .iter()
            .all(|record| record.method == "GET" && record.body.is_empty())
    );
}

#[tokio::test]
async fn new_extension_attempt_refuses_malformed_values_and_keeps_closed_correlated_shapes() {
    let raw = new_extension_raw();
    let invalid = [
        json!(null),
        json!(false),
        json!("2"),
        json!(2.0),
        json!(-1),
        json!(0),
        json!(1),
        json!(1000),
        json!({}),
        json!([]),
    ];
    let mut post_bodies = Vec::new();
    let mut status_bodies = Vec::new();
    for attempt in invalid {
        let mut post: Value = serde_json::from_slice(&submission_reply(201, &raw).body).unwrap();
        post["attempt"] = attempt.clone();
        post_bodies.push(post.to_string());
        let mut status = new_extension_status("submitted");
        status["attempt"] = attempt;
        status_bodies.push(status.to_string());
    }
    let mut post: Value = serde_json::from_slice(&submission_reply(201, &raw).body).unwrap();
    post["attempt"] = json!(2);
    let mut crossed = post.clone();
    crossed["request_id"] = json!(OTHER_CHANGE_DIGEST);
    post_bodies.push(crossed.to_string());
    let mut changed = post.clone();
    changed["change"]["unified_summary"] = json!("different approved change");
    post_bodies.push(changed.to_string());
    let mut unknown = post.clone();
    unknown["retry_granted"] = json!(true);
    post_bodies.push(unknown.to_string());
    post_bodies.push(
        post.to_string()
            .replace("\"attempt\":2", "\"attempt\":2,\"attempt\":3"),
    );
    let mut status = new_extension_status("built");
    status["attempt"] = json!(2);
    let mut crossed = status.clone();
    crossed["request_id"] = json!(OTHER_CHANGE_DIGEST);
    status_bodies.push(crossed.to_string());
    let mut unknown = status.clone();
    unknown["retry_granted"] = json!(true);
    status_bodies.push(unknown.to_string());
    status_bodies.push(
        status
            .to_string()
            .replace("\"attempt\":2", "\"attempt\":2,\"attempt\":3"),
    );
    let mut rejected = new_extension_status("rejected");
    rejected["attempt"] = json!(2);
    status_bodies.push(rejected.to_string());
    status_bodies.push(
        json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time",
        "state":"submitted","attempt":2})
        .to_string(),
    );
    let post_count = post_bodies.len();
    let status_count = status_bodies.len();
    let mut replies = vec![new_extension_prepared(&raw)];
    replies.extend(
        post_bodies
            .into_iter()
            .map(|body| Reply::bytes(201, body.into_bytes())),
    );
    replies.extend(
        status_bodies
            .into_iter()
            .map(|body| Reply::bytes(200, body.into_bytes())),
    );
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_new_extension(&mut client).await;
    for _ in 0..post_count {
        assert_eq!(
            client
                .submit_frontend_new_extension_change(&change)
                .await
                .unwrap_err(),
            PairedHostError::Unconfirmed {
                operation: PairedHostOperation::SubmitFrontendChange,
                reason: UnconfirmedReason::UnexpectedResponse,
            }
        );
    }
    for _ in 0..status_count {
        assert_eq!(
            client
                .frontend_change_status(change.change_digest())
                .await
                .unwrap_err(),
            PairedHostError::Unconfirmed {
                operation: PairedHostOperation::FrontendChangeStatus,
                reason: UnconfirmedReason::UnexpectedResponse,
            }
        );
    }
    assert_eq!(fixture.finish().await.len(), 1 + post_count + status_count);

    let channel = change_raw();
    let mut reply = submission_reply(201, &channel);
    let mut body: Value = serde_json::from_slice(&reply.body).unwrap();
    body["attempt"] = json!(2);
    reply.body = serde_json::to_vec(&body).unwrap();
    let fixture = Fixture::new(vec![prepared_change_reply(&channel), reply]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_change(&mut client).await;
    assert_eq!(
        client.submit_frontend_change(&change).await.unwrap_err(),
        PairedHostError::Unconfirmed {
            operation: PairedHostOperation::SubmitFrontendChange,
            reason: UnconfirmedReason::UnexpectedResponse,
        },
        "channel changes do not have the new-extension attempt contract"
    );
    assert_eq!(fixture.finish().await.len(), 2);
}

#[tokio::test]
async fn new_extension_capture_never_crosses_client_host_or_model_reconstruction() {
    let fixture = Fixture::new(vec![new_extension_prepared(&new_extension_raw())]).await;
    let other = Fixture::new(vec![]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_new_extension(&mut client).await;
    let mut same_host = PairedHostClient::new(&fixture.origin).unwrap();
    let mut other_host = PairedHostClient::new(&other.origin).unwrap();
    for caller in [&mut same_host, &mut other_host] {
        assert_eq!(
            caller
                .submit_frontend_new_extension_change(&change)
                .await
                .unwrap_err(),
            PairedHostError::UnboundFrontendChange
        );
    }
    let unbound = serde_json::from_str::<FrontendNewExtensionChange>(&new_extension_raw()).unwrap();
    assert_eq!(
        client
            .submit_frontend_new_extension_change(&unbound)
            .await
            .unwrap_err(),
        PairedHostError::UnboundFrontendChange
    );
    assert_eq!(fixture.finish().await.len(), 1);
    assert!(other.finish().await.is_empty());
}

#[tokio::test]
async fn new_extension_prepare_requires_closed_owner_shape_and_exact_requested_proposal() {
    let base: Value = serde_json::from_str(&new_extension_raw()).unwrap();
    let mut unknown = base.clone();
    unknown["credential"] = json!(DEVICE_COOKIE);
    let mut nested = base.clone();
    nested["proposal"]["channel"]["target_identity"] = json!("other");
    let mut unverified = base.clone();
    unverified["unverified"] = json!([]);
    let mut verified = base.clone();
    verified["prerequisites"][0]["state"] = json!("ready");
    let mut null_kind = base.clone();
    null_kind["kind"] = Value::Null;
    let mut absent = base.clone();
    absent.as_object_mut().unwrap().remove("base");
    let mut missing_nullable = base.clone();
    missing_nullable["base"]
        .as_object_mut()
        .unwrap()
        .remove("activation_id");
    let duplicate = new_extension_raw().replace(
        "\"version\":\"0.1.0\"",
        "\"version\":\"0.1.0\",\"version\":\"9.0\"",
    );
    let mut replies: Vec<_> = [
        unknown,
        nested,
        unverified,
        verified,
        null_kind,
        absent,
        missing_nullable,
    ]
    .into_iter()
    .map(|v| new_extension_prepared(&v.to_string()))
    .collect();
    replies.extend([
        new_extension_prepared(&duplicate),
        Reply::json(200, json!({"valid":true,"problems":[],"change":null})),
        Reply::json(200, json!({"valid":true,"problems":[]})),
        Reply::json(200, json!({"valid":false,"problems":[],"change":null})),
        Reply::json(200, receipt()),
    ]);
    let malformed_count = replies.len();
    for path in ["distribution", "version"] {
        let mut crossed = base.clone();
        crossed["proposal"][path] = json!("different");
        replies.push(new_extension_prepared(&crossed.to_string()));
    }
    for path in ["repo", "tag", "commit"] {
        let mut crossed = base.clone();
        crossed["proposal"]["source"][path] = json!("different");
        replies.push(new_extension_prepared(&crossed.to_string()));
    }
    let mut ingress = base.clone();
    ingress["proposal"]["channel"]["ingress"] = json!("webhook");
    let mut groups = base.clone();
    groups["proposal"]["channel"]["approved_groups"] = json!([]);
    replies.extend([
        new_extension_prepared(&ingress.to_string()),
        new_extension_prepared(&groups.to_string()),
    ]);
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..malformed_count {
        let error = client
            .prepare_new_extension(&new_extension_request())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            PairedHostError::Unconfirmed {
                operation: PairedHostOperation::PrepareCandidate,
                reason: UnconfirmedReason::UnexpectedResponse
            }
        );
        assert!(!format!("{error:?} {error}").contains(DEVICE_COOKIE));
    }
    for _ in 0..7 {
        assert_eq!(
            client
                .prepare_new_extension(&new_extension_request())
                .await
                .unwrap_err(),
            PairedHostError::NewExtensionProposalMismatch
        );
    }
    assert_eq!(fixture.finish().await.len(), malformed_count + 7);
}

#[tokio::test]
async fn new_extension_canonical_spelling_refusal_and_host_problems_do_not_submit() {
    let problems = [
        (
            "source_not_allowed",
            "The source repository is not on the host's extension source allowlist.",
        ),
        (
            "extension_sources_invalid",
            "The host's extension source allowlist cannot be read.",
        ),
        (
            "tag_unresolved",
            "The tag could not be resolved to a commit in the source repository.",
        ),
        (
            "tag_commit_mismatch",
            "The tag resolves to a different commit than the one requested.",
        ),
        (
            "distribution_declared",
            "An extension with this distribution is already declared.",
        ),
        (
            "extension_id_declared",
            "The distribution name collides with a declared extension id.",
        ),
    ];
    let mut replies = vec![new_extension_prepared(&new_extension_raw())];
    replies.extend(problems.iter().map(|(code,message)| Reply::json(200,
        json!({"valid":false,"problems":[{"code":code,"path":"new_extension","message":message}],"change":null}))));
    replies.push(Reply::json(400,json!({"error":"invalid_request","problems":[{
        "code":"invalid_value","path":"new_extension.source.commit","message":"A request field has a value outside its allowed form."}]})));
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let mut noncanonical = new_extension_request();
    noncanonical.distribution = "PairedHost_Ext_Signal".into();
    assert_eq!(
        client
            .prepare_new_extension(&noncanonical)
            .await
            .unwrap_err(),
        PairedHostError::NewExtensionProposalMismatch
    );
    for (code, _) in problems {
        let FrontendNewExtensionPreparation::Candidate(receipt) = client
            .prepare_new_extension(&new_extension_request())
            .await
            .unwrap()
        else {
            panic!("typed preliminary refusal expected");
        };
        assert!(!receipt.valid && receipt.change.is_none());
        assert_eq!(receipt.problems[0].code, code);
    }
    let FrontendNewExtensionPreparation::Rejected(rejection) = client
        .prepare_new_extension(&new_extension_request())
        .await
        .unwrap()
    else {
        panic!("typed request refusal expected");
    };
    assert_eq!(rejection.http_status, 400);
    let records = fixture.finish().await;
    assert_eq!(records.len(), 8);
    assert!(records.iter().all(|r| r.path.ends_with("/prepare")));
}

#[tokio::test]
async fn new_extension_uncertain_submission_reads_status_without_replaying_post() {
    let raw = new_extension_raw();
    let fixture = Fixture::new(vec![
        new_extension_prepared(&raw),
        Reply::json(
            500,
            json!({"error":"internal_error","private":DEVICE_COOKIE}),
        ),
        Reply::json(404, json!({"state":"not_found"})),
        Reply::json(200, new_extension_status("submitted")),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_new_extension(&mut client).await;
    assert_eq!(
        client
            .submit_frontend_new_extension_change(&change)
            .await
            .unwrap_err(),
        PairedHostError::Unconfirmed {
            operation: PairedHostOperation::SubmitFrontendChange,
            reason: UnconfirmedReason::HostFailure
        }
    );
    assert_eq!(
        client
            .frontend_change_status(change.change_digest())
            .await
            .unwrap(),
        FrontendChangeRead::NotFound
    );
    assert!(matches!(
        client
            .frontend_change_status(change.change_digest())
            .await
            .unwrap(),
        FrontendChangeRead::NewExtensionStatus(FrontendNewExtensionStatus::Submitted { .. })
    ));
    let records = fixture.finish().await;
    assert_eq!(
        records
            .iter()
            .filter(|r| r.method == "POST" && r.path == CHANGES_PATH)
            .count(),
        1
    );
    assert_eq!(records.len(), 4);
    for record in &records[2..] {
        assert_eq!(record.method, "GET");
        assert_eq!(record.path, format!("{CHANGES_PATH}/{CHANGE_DIGEST}"));
        assert!(record.body.is_empty());
    }
}

#[tokio::test]
async fn new_extension_submission_checks_summary_echo_and_preserves_stale_base() {
    let raw = new_extension_raw();
    let mut changed: Value = serde_json::from_str(&raw).unwrap();
    changed["unified_summary"] = json!("different approved change");
    let rejection = |status, error, code, message| {
        Reply::json(
            status,
            json!({"error":error,"problems":[{"code":code,"path":"change","message":message}]}),
        )
    };
    let fixture = Fixture::new(vec![
        new_extension_prepared(&raw),
        submission_reply(201, &changed.to_string()),
        rejection(
            400,
            "summary_mismatch",
            "summary_mismatch",
            "The change's unified summary is not the rendering of its proposal.",
        ),
        rejection(
            409,
            "stale_base",
            "stale_base",
            "The change was prepared against a snapshot that is no longer current.",
        ),
        rejection(400, "summary_mismatch", "summary_mismatch", DEVICE_COOKIE),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_new_extension(&mut client).await;
    assert_eq!(
        client
            .submit_frontend_new_extension_change(&change)
            .await
            .unwrap_err(),
        PairedHostError::Unconfirmed {
            operation: PairedHostOperation::SubmitFrontendChange,
            reason: UnconfirmedReason::UnexpectedResponse
        }
    );
    for expected in [
        FrontendChangeRejectionKind::SummaryMismatch,
        FrontendChangeRejectionKind::StaleBase,
    ] {
        // Explicit calls exercise distinct server outcomes. There is no
        // automatic reprepare/retry inside any call, and no approval claim.
        let FrontendNewExtensionSubmission::Rejected(rejected) = client
            .submit_frontend_new_extension_change(&change)
            .await
            .unwrap()
        else {
            panic!("known host refusal expected");
        };
        assert_eq!(rejected.error, expected);
    }
    let error = client
        .submit_frontend_new_extension_change(&change)
        .await
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains(DEVICE_COOKIE));
    let records = fixture.finish().await;
    assert_eq!(records.len(), 5);
    assert_eq!(
        records
            .iter()
            .filter(|r| r.path.ends_with("/prepare"))
            .count(),
        1
    );
}

#[tokio::test]
async fn fresh_authenticated_digest_read_preserves_all_new_extension_stages_and_legacy_status() {
    let mut expected: Vec<_> = [
        "submitted",
        "building",
        "built",
        "under_review",
        "activating",
        "activation_uncertain",
        "applied",
        "failed",
        "rejected",
    ]
    .into_iter()
    .map(new_extension_status)
    .collect();
    let mut failed_before_build = new_extension_status("failed");
    failed_before_build["built"] = Value::Null;
    failed_before_build["stage"] = json!("building");
    failed_before_build["prerequisites"] = json!([{"kind":"build_and_review","state":"pending"}]);
    expected.push(failed_before_build);
    let mut replies = vec![pairing_requested(), pairing_confirmed()];
    replies.extend(expected.iter().cloned().map(|v| Reply::json(200, v)));
    replies.push(Reply::json(
        200,
        json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time","state":"submitted"}),
    ));
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client
        .request_pairing("fresh status-only client")
        .await
        .unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    for expected in expected {
        let FrontendChangeRead::NewExtensionStatus(observed) =
            client.frontend_change_status(&digest).await.unwrap()
        else {
            panic!("new extension stage expected");
        };
        assert_eq!(observed.request_id(), &digest);
        assert_eq!(serde_json::to_value(observed).unwrap(), expected);
        if expected.get("built").is_some_and(|v| !v.is_null()) {
            assert_eq!(
                expected["prerequisites"][0]["provisioned"], false,
                "even an applied host stage does not imply provider credentials"
            );
        }
    }
    assert!(matches!(
        client.frontend_change_status(&digest).await.unwrap(),
        FrontendChangeRead::Status(FrontendChangeStatus::Submitted { .. })
    ));
    let records = fixture.finish().await;
    assert_eq!(records.len(), 13);
    for request in &records[2..] {
        assert_eq!(request.method, "GET");
        assert_eq!(request.headers["cache-control"], "no-store");
        assert_eq!(
            request.headers["cookie"],
            format!("hc_console_device={DEVICE_COOKIE}")
        );
        assert!(request.body.is_empty());
    }
}

#[tokio::test]
async fn new_extension_status_refuses_unknown_fields_inconsistent_stages_and_secret_values() {
    let mut inputs = Vec::new();
    let mut unknown = new_extension_status("submitted");
    unknown["installed"] = json!(true);
    inputs.push(unknown);
    let mut kind = new_extension_status("submitted");
    kind["kind"] = json!("future_kind");
    inputs.push(kind);
    let mut kind_null = new_extension_status("submitted");
    kind_null["kind"] = Value::Null;
    inputs.push(kind_null);
    let mut no_kind = new_extension_status("submitted");
    no_kind.as_object_mut().unwrap().remove("kind");
    inputs.push(no_kind);
    let mut no_nullable = new_extension_status("submitted");
    no_nullable.as_object_mut().unwrap().remove("built");
    inputs.push(no_nullable);
    let mut crossed = new_extension_status("built");
    crossed["request_id"] = json!(OTHER_CHANGE_DIGEST);
    inputs.push(crossed);
    let mut premature = new_extension_status("built");
    premature["built"] = Value::Null;
    inputs.push(premature);
    let mut activation = new_extension_status("applied");
    activation["activation_id"] = Value::Null;
    inputs.push(activation);
    let mut secret = new_extension_status("built");
    secret["prerequisites"][0]["value"] = json!(DEVICE_COOKIE);
    inputs.push(secret);
    let mut names = new_extension_status("built");
    names["prerequisites"][0]["name"] = json!("unrelated_secret");
    inputs.push(names);
    let mut rejected = new_extension_status("rejected");
    rejected["built"] = Value::Null;
    inputs.push(rejected);
    let mut template = new_extension_status("built");
    template["built"]["channel_template"] = json!([]);
    inputs.push(template);
    let mut failed = new_extension_status("failed");
    failed["stage"] = json!("readiness");
    inputs.push(failed);
    let mut superseded = new_extension_status("submitted");
    superseded["state"] = json!("superseded");
    inputs.push(superseded);
    let mut secret_unknown = new_extension_status("built");
    secret_unknown["built"]["token"] = json!(DEVICE_COOKIE);
    inputs.push(secret_unknown);
    let duplicate = new_extension_status("built")
        .to_string()
        .replace("\"threshold\":0.125", "\"threshold\":0.125,\"threshold\":9");
    let mut replies: Vec<_> = inputs.into_iter().map(|v| Reply::json(200, v)).collect();
    replies.push(Reply::bytes(200, duplicate.into_bytes()));
    let count = replies.len();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    for _ in 0..count {
        let error = client.frontend_change_status(&digest).await.unwrap_err();
        assert_eq!(
            error,
            PairedHostError::Unconfirmed {
                operation: PairedHostOperation::FrontendChangeStatus,
                reason: UnconfirmedReason::UnexpectedResponse
            }
        );
        assert!(!format!("{error:?} {error}").contains(DEVICE_COOKIE));
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn new_extension_template_keeps_owner_number_tokens_and_requests_remain_closed_and_bounded() {
    let raw = new_extension_status("built")
        .to_string()
        .replace("\"threshold\":0.125", "\"threshold\":1e-06");
    assert!(raw.contains("1e-06"));
    let fixture = Fixture::new(vec![Reply::bytes(200, raw.into_bytes())]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let FrontendChangeRead::NewExtensionStatus(FrontendNewExtensionStatus::Built {
        built: Some(built),
        ..
    }) = client
        .frontend_change_status(&FrontendChangeDigest::new(CHANGE_DIGEST).unwrap())
        .await
        .unwrap()
    else {
        panic!("typed built evidence expected");
    };
    assert!(built.channel_template.get().contains("1e-06"));
    let mut huge = new_extension_request();
    huge.distribution = "x".repeat(64 * 1024);
    assert_eq!(
        client.prepare_new_extension(&huge).await.unwrap_err(),
        PairedHostError::RequestTooLarge
    );
    assert_eq!(fixture.finish().await.len(), 1);
    let mut unknown = serde_json::to_value(new_extension_request()).unwrap();
    unknown["channel"]["sender_identity"] = json!("chosen-agent");
    assert!(serde_json::from_value::<FrontendNewExtensionRequest>(unknown).is_err());
    let duplicate = serde_json::to_string(&new_extension_request())
        .unwrap()
        .replace(
            "\"version\":\"0.1.0\"",
            "\"version\":\"0.1.0\",\"version\":\"9.0\"",
        );
    assert!(serde_json::from_str::<FrontendNewExtensionRequest>(&duplicate).is_err());
}

// Synthetic owner JSON, including an extension-defined number whose original
// token must survive the POST. This fixture does not compile a host ledger.
fn change_raw() -> String {
    let before = r#"{"id":"telegram:primary","channel_type":"telegram","ingress":"polling","sender_id":"telegram","provider_account":"fixture-bot","principal_subject_id":"fixture-channel","source_event_type":"integer","secret_names":["telegram_bot_token"],"inbound_limits":{"max_text_utf8_bytes":4096,"max_caption_utf8_bytes":1024,"max_parts":10,"max_part_bytes":4096,"max_event_bytes":8192,"max_channel_stored_bytes":16384,"allowed_mime_types":["text/plain"]},"outbound_limits":{"max_text_utf8_bytes":4096,"max_caption_utf8_bytes":1024,"max_media_bytes":8192,"allowed_media_types":["text/plain"]},"dedup_horizon_seconds":86400,"features":["text"],"approved_groups":[],"polling":{"interval_seconds":1},"webhook":{"route_id":"fixture.webhook","callback_url":"https://fixture.invalid/webhook","source_event_pointer":"/id","registration_schema":"fixture.schema","registration":{"threshold":1e-06,"label":"fixture"}}}"#;
    let after = before.replace(
        "\"approved_groups\":[]",
        "\"approved_groups\":[\"-1001234567890\"]",
    );
    format!(
        r#"{{"change_digest":"{CHANGE_DIGEST}","base":{{"activation_id":"210","policy_digest":"sha256:current-fixture","ledger_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},"ledger_patch":{{"path":"deployment/extensions.toml","extension_id":"telegram","channel_id":"telegram:primary","before":{before},"after":{after}}},"unified_diff":"--- a/deployment/extensions.toml\n+++ b/deployment/extensions.toml\n@@ -1 +1 @@\n-approved_groups = []\n+approved_groups = [\"-1001234567890\"]\n"}}"#
    )
}

fn prepared_change_reply(raw: &str) -> Reply {
    let mut body = receipt().to_string();
    body.pop();
    body.push_str(",\"change\":");
    body.push_str(raw);
    body.push('}');
    Reply::bytes(200, body.into_bytes())
}

fn with_review_url(mut reply: Reply, path: Value) -> Reply {
    let mut body = String::from_utf8(reply.body).unwrap();
    body.pop();
    body.push_str(&format!(",\"review_url\":{path}}}"));
    reply.body = body.into_bytes();
    reply
}

fn review_dispatcher(
    binding: &meerkat_connect::paired_host_tool::PairedHostFrontend,
) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
    let options = meerkat_connect::setup::paired_host_frontend_session(
        meerkat_connect::SessionOptions::default(),
        binding,
    )
    .unwrap();
    Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
        options.build.external_tools.unwrap(),
        meerkat_core::ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap())
            .unwrap(),
    ))
}

async fn dispatch_review_candidate(
    dispatcher: &dyn meerkat_core::AgentToolDispatcher,
    id: &str,
    arguments: Value,
) -> Result<String, meerkat_core::ToolError> {
    dispatch_review_tool(
        dispatcher,
        id,
        meerkat_connect::paired_host_tool::PAIRED_HOST_FRONTEND_PREPARE,
        arguments,
    )
    .await
}

async fn dispatch_review_tool(
    dispatcher: &dyn meerkat_core::AgentToolDispatcher,
    id: &str,
    name: &str,
    arguments: Value,
) -> Result<String, meerkat_core::ToolError> {
    let arguments = serde_json::value::RawValue::from_string(arguments.to_string()).unwrap();
    dispatcher
        .dispatch(meerkat_core::types::ToolCallView {
            id,
            name,
            args: &arguments,
        })
        .await
        .map(|outcome| outcome.result.text_content())
}

#[tokio::test]
async fn prepared_review_handoff_preserves_both_kinds_and_console_projection() {
    use meerkat_connect::paired_host_tool::{
        PAIRED_HOST_FRONTEND_STATUS, PairedHostFrontend, PairedHostPreparationTools,
    };
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    for extension in [false, true] {
        let reply = || {
            if extension {
                with_review_url(new_extension_prepared(&new_extension_raw()), json!(path))
            } else {
                with_review_url(prepared_change_reply(&change_raw()), json!(path))
            }
        };
        let arguments = if extension {
            json!({"new_extension":new_extension_request()})
        } else {
            serde_json::to_value(request()).unwrap()
        };
        let original = if extension {
            new_extension_raw()
        } else {
            change_raw()
        };
        // Host readback may use different JSON formatting, and a stale draft
        // is still useful to review. Neither observation rebinds the change.
        let original_value: Value = serde_json::from_str(&original).unwrap();
        let readback = serde_json::to_string_pretty(&original_value).unwrap();
        let fixture = Fixture::new(vec![
            reply(),
            prepared_draft_reply(&original, true),
            prepared_draft_reply(&readback, false),
            reply(),
            prepared_draft_reply(&original, true),
        ])
        .await;
        let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
        let mut presenter = binding.take_secret_entry_presenter().unwrap();
        let dispatcher = review_dispatcher(&binding);
        let text = dispatch_review_candidate(&*dispatcher, "interactive", arguments.clone())
            .await
            .unwrap();
        let observed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(observed["review_presentation"], "queued");
        assert_eq!(observed["change"]["change_digest"], CHANGE_DIGEST);
        assert!(observed.get("review_url").is_none());
        assert!(!text.contains(&fixture.origin) && !text.contains(&path));
        if !extension {
            assert!(
                text.contains("\"threshold\":1e-06"),
                "owner RawValue must survive projection"
            );
        }
        let notice = timeout(FIXTURE_BOUND, presenter.next())
            .await
            .unwrap()
            .unwrap();
        assert!(!format!("{notice:?}").contains(&fixture.origin));
        assert!(!format!("{notice:?}").contains(&path));
        let mut output = Vec::new();
        notice.present(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(&format!("{}{path}", fixture.origin)));
        assert!(output.contains(CHANGE_DIGEST));
        assert!(output.contains(if extension {
            "Kind: new_extension"
        } else {
            "Kind: channel_config"
        }));
        assert!(output.contains("this change's current review and activation status"));
        let mut repeated = Vec::new();
        presenter.present_pending(&mut repeated).unwrap();
        assert!(repeated.is_empty());

        let ordinary = dispatch_review_tool(
            &*dispatcher,
            "ordinary-status",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":false}),
        )
        .await
        .unwrap();
        let ordinary: Value = serde_json::from_str(&ordinary).unwrap();
        assert_eq!(
            ordinary,
            json!({"state":"prepared","change":original_value,"base_current":true})
        );
        presenter.present_pending(&mut repeated).unwrap();
        assert!(
            repeated.is_empty(),
            "ordinary status must not redisplay a link"
        );
        let status = dispatch_review_tool(
            &*dispatcher,
            "explicit-review",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await
        .unwrap();
        assert!(!status.contains(&path) && !status.contains(&fixture.origin));
        let status: Value = serde_json::from_str(&status).unwrap();
        assert_eq!(status["review_presentation"], "queued");
        assert_eq!(status["base_current"], false);
        assert_eq!(status["change"], original_value);
        presenter.present_pending(&mut repeated).unwrap();
        let repeated = String::from_utf8(repeated).unwrap();
        assert_eq!(
            repeated
                .matches(&format!("{}{path}", fixture.origin))
                .count(),
            1
        );

        let console_binding =
            PairedHostPreparationTools::new(PairedHostClient::new(&fixture.origin).unwrap())
                .unwrap();
        let console =
            dispatch_review_candidate(&*console_binding.dispatcher(), "console", arguments)
                .await
                .unwrap();
        let console: Value = serde_json::from_str(&console).unwrap();
        assert_eq!(console["review_url"], path);
        assert!(console.get("review_presentation").is_none());
        let console_status = dispatch_review_tool(
            &*console_binding.dispatcher(),
            "console-status",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await
        .unwrap();
        let console_status: Value = serde_json::from_str(&console_status).unwrap();
        assert_eq!(console_status["review_presentation"], "unavailable");
        let requests = fixture.finish().await;
        assert_eq!(
            requests
                .iter()
                .map(|r| r.method.as_str())
                .collect::<Vec<_>>(),
            ["POST", "GET", "GET", "POST", "GET"]
        );
        assert!(requests.iter().all(|r| if r.method == "POST" {
            r.path == "/console/extensions/frontend/prepare"
        } else {
            r.path == format!("{CHANGES_PATH}/{CHANGE_DIGEST}")
        }));
    }
}

#[tokio::test]
async fn private_preparation_retains_review_only_for_both_candidate_kinds() {
    use meerkat_connect::paired_host_tool::{
        PAIRED_HOST_FRONTEND_STATUS, PairedHostPreparationTools,
    };
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    for extension in [false, true] {
        let (raw, reply, arguments) = if extension {
            (
                new_extension_raw(),
                new_extension_prepared(&new_extension_raw()),
                json!({"new_extension":new_extension_request()}),
            )
        } else {
            (
                change_raw(),
                prepared_change_reply(&change_raw()),
                serde_json::to_value(request()).unwrap(),
            )
        };
        let fixture = Fixture::new(vec![
            with_review_url(reply, json!(path)),
            prepared_draft_reply(&raw, false),
            Reply::json(
                403,
                json!({"error":"forbidden","code":"console_owner_required"}),
            ),
        ])
        .await;
        let (binding, mut presenter) = PairedHostPreparationTools::with_review_presenter(
            PairedHostClient::new(&fixture.origin).unwrap(),
        )
        .unwrap();
        let dispatcher = binding.dispatcher();
        let output = dispatch_review_candidate(&*dispatcher, "prepare", arguments)
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["review_presentation"], "queued");
        assert!(value.get("review_url").is_none());
        assert!(!output.contains(&path) && !output.contains(&fixture.origin));
        let mut private = Vec::new();
        presenter.present_pending(&mut private).unwrap();
        let private = String::from_utf8(private).unwrap();
        assert_eq!(
            private
                .matches(&format!("{}{path}", fixture.origin))
                .count(),
            1
        );
        assert!(private.contains(if extension {
            "Kind: new_extension"
        } else {
            "Kind: channel_config"
        }));
        let output = dispatch_review_tool(
            &*dispatcher,
            "readback",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await
        .unwrap();
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["review_presentation"], "queued");
        assert_eq!(value["base_current"], false);
        let mut repeated = Vec::new();
        presenter.present_pending(&mut repeated).unwrap();
        assert_eq!(repeated, private.as_bytes());
        let refused = dispatch_review_tool(
            &*dispatcher,
            "revoked",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await;
        // A host refusal may be a closed observed rejection or a fixed error;
        // neither permits redisplaying a retained link.
        if let Ok(output) = refused {
            assert!(!output.contains("\"review_presentation\":\"queued\""));
        }
        let mut absent = Vec::new();
        presenter.present_pending(&mut absent).unwrap();
        assert!(absent.is_empty());
        for name in [
            "paired_host_select_host",
            "paired_host_frontend_submit",
            "paired_host_extension_secret_create",
            "paired_host_extension_secret_status",
            "paired_host_extension_secret_cancel",
        ] {
            assert!(
                dispatch_review_tool(&*dispatcher, "excluded", name, json!({}))
                    .await
                    .is_err()
            );
        }
        let requests = fixture.finish().await;
        assert_eq!(
            requests
                .iter()
                .map(|r| r.method.as_str())
                .collect::<Vec<_>>(),
            ["POST", "GET", "GET"]
        );
    }
}

#[tokio::test]
async fn submitted_review_redisplay_preserves_status_attempts_and_the_original_private_path() {
    use meerkat_connect::paired_host_tool::{PAIRED_HOST_FRONTEND_STATUS, PairedHostFrontend};
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    for extension in [false, true] {
        let (prepared, arguments, statuses) = if extension {
            let statuses = [
                "submitted",
                "building",
                "built",
                "under_review",
                "activating",
                "activation_uncertain",
                "failed",
                "applied",
                "rejected",
            ]
            .into_iter()
            .map(|state| {
                let mut status = new_extension_status(state);
                if state != "rejected" {
                    status["attempt"] = json!(if state == "failed" { 3 } else { 2 });
                }
                status
            })
            .collect::<Vec<_>>();
            (
                with_review_url(new_extension_prepared(&new_extension_raw()), json!(path)),
                json!({"new_extension":new_extension_request()}),
                statuses,
            )
        } else {
            let statuses = vec![
                json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time","state":"submitted"}),
                json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time","state":"applied","activation_id":"215"}),
                json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time","state":"superseded","activation_id":null}),
                json!({"request_id":CHANGE_DIGEST,"submitted_at":"fixture-time","state":"rejected","reason":"Synthetic refusal","decided_at":"fixture-decision"}),
            ];
            (
                with_review_url(prepared_change_reply(&change_raw()), json!(path)),
                serde_json::to_value(request()).unwrap(),
                statuses,
            )
        };
        let mut replies = vec![prepared];
        for status in &statuses {
            replies.push(Reply::json(200, status.clone()));
            replies.push(Reply::json(200, status.clone()));
        }
        let mut crossed = statuses[0].clone();
        crossed["request_id"] = json!(OTHER_CHANGE_DIGEST);
        replies.push(Reply::json(200, crossed));
        replies.push(Reply::json(200, statuses[0].clone()));
        let fixture = Fixture::new(replies).await;
        let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
        let mut presenter = binding.take_secret_entry_presenter().unwrap();
        let dispatcher = review_dispatcher(&binding);
        dispatch_review_candidate(&*dispatcher, "prepare", arguments)
            .await
            .unwrap();
        presenter.present_pending(&mut Vec::new()).unwrap();
        let mut observations = Vec::new();
        for _ in &statuses {
            let ordinary = dispatch_review_tool(
                &*dispatcher,
                "ordinary",
                PAIRED_HOST_FRONTEND_STATUS,
                json!({"change_digest":CHANGE_DIGEST}),
            )
            .await
            .unwrap();
            let mut silent = Vec::new();
            presenter.present_pending(&mut silent).unwrap();
            let explicit = dispatch_review_tool(
                &*dispatcher,
                "explicit",
                PAIRED_HOST_FRONTEND_STATUS,
                json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
            )
            .await
            .unwrap();
            let mut private = Vec::new();
            presenter.present_pending(&mut private).unwrap();
            observations.push((ordinary, silent, explicit, private));
        }
        let crossed = dispatch_review_tool(
            &*dispatcher,
            "wrong-request",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await;
        let mut crossed_output = Vec::new();
        presenter.present_pending(&mut crossed_output).unwrap();
        let fresh = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
        let mut fresh_presenter = fresh.take_secret_entry_presenter().unwrap();
        let unretained = dispatch_review_tool(
            &*review_dispatcher(&fresh),
            "unretained",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await
        .unwrap();
        let mut fresh_output = Vec::new();
        fresh_presenter.present_pending(&mut fresh_output).unwrap();
        let origin = fixture.origin.clone();
        let requests = fixture.finish().await;
        // Settle every fixture request before the expected predecessor failure.
        assert_eq!(requests.len(), 1 + statuses.len() * 2 + 2);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/console/extensions/frontend/prepare");
        assert!(requests[1..].iter().all(|request| request.method == "GET"
            && request.path == format!("{CHANGES_PATH}/{CHANGE_DIGEST}")
            && request.body.is_empty()));
        assert!(crossed.is_err());
        assert!(crossed_output.is_empty());
        assert_eq!(
            serde_json::from_str::<Value>(&unretained).unwrap()["review_presentation"],
            "unavailable"
        );
        assert!(fresh_output.is_empty());
        for (expected, (ordinary, silent, explicit, private)) in statuses.iter().zip(observations) {
            assert_eq!(serde_json::from_str::<Value>(&ordinary).unwrap(), *expected);
            assert!(
                silent.is_empty(),
                "ordinary status must not redisplay a link"
            );
            assert!(!explicit.contains(&origin) && !explicit.contains(&path));
            let mut observed: Value = serde_json::from_str(&explicit).unwrap();
            let presentation = observed
                .as_object_mut()
                .unwrap()
                .remove("review_presentation");
            assert_eq!(
                observed, *expected,
                "presentation must preserve exact host status and attempt"
            );
            assert_eq!(
                presentation,
                Some(json!("queued")),
                "a retained request page remains useful after submission"
            );
            let private = String::from_utf8(private).unwrap();
            assert!(private.contains("PairedHost change review"));
            assert!(private.contains("this change's current review and activation status"));
            assert!(!private.contains("draft review"));
            assert_eq!(private.matches(&format!("{origin}{path}")).count(), 1);
            assert!(private.contains(if extension {
                "Kind: new_extension"
            } else {
                "Kind: channel_config"
            }));
        }
    }
}

#[tokio::test]
async fn missing_invalid_and_conflicting_review_candidates_never_create_a_notice() {
    use meerkat_connect::paired_host_tool::PairedHostFrontend;
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    let raw = change_raw();
    let conflict = raw.replace("@@ -1 +1 @@", "@@ -1 +1,2 @@");
    let fixture = Fixture::new(vec![
        prepared_change_reply(&raw),
        new_extension_prepared(&new_extension_raw()),
        with_review_url(
            prepared_change_reply(&raw),
            json!(format!("https://other.invalid{path}")),
        ),
        with_review_url(prepared_change_reply(&raw), json!(path)),
        with_review_url(prepared_change_reply(&conflict), json!(path)),
    ])
    .await;
    let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut presenter = binding.take_secret_entry_presenter().unwrap();
    let dispatcher = review_dispatcher(&binding);
    let channel = serde_json::to_value(request()).unwrap();
    // Use a distinct client for the missing new-extension receipt: this fixture
    // intentionally uses the same synthetic digest for its two different kinds.
    let other = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut other_presenter = other.take_secret_entry_presenter().unwrap();
    for (id, arguments, dispatcher) in [
        ("missing-channel", channel.clone(), dispatcher.clone()),
        (
            "missing-extension",
            json!({"new_extension":new_extension_request()}),
            review_dispatcher(&other),
        ),
    ] {
        let text = dispatch_review_candidate(&*dispatcher, id, arguments)
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["valid"].as_bool().unwrap());
        assert_eq!(value["review_presentation"], "unavailable");
        assert!(value.get("review_url").is_none());
    }
    let mut empty = Vec::new();
    presenter.present_pending(&mut empty).unwrap();
    other_presenter.present_pending(&mut empty).unwrap();
    assert!(empty.is_empty());
    let malformed = dispatch_review_candidate(&*dispatcher, "malformed", channel.clone())
        .await
        .unwrap_err();
    assert!(!malformed.to_string().contains("other.invalid"));
    presenter.present_pending(&mut empty).unwrap();
    assert!(empty.is_empty());
    dispatch_review_candidate(&*dispatcher, "retained", channel.clone())
        .await
        .unwrap();
    presenter.present_pending(&mut Vec::new()).unwrap();
    let unretained = dispatch_review_candidate(&*dispatcher, "conflict", channel)
        .await
        .unwrap_err();
    assert!(unretained.to_string().contains("conflicting content"));
    presenter.present_pending(&mut empty).unwrap();
    assert!(empty.is_empty());
    assert_eq!(fixture.finish().await.len(), 5);
}

#[tokio::test]
async fn full_and_disconnected_review_queues_preserve_the_particular_retained_draft() {
    use meerkat_connect::paired_host_tool::PairedHostFrontend;
    let second_digest = format!("hc-frontend-change-v1:sha256:{}", "b".repeat(64));
    assert_ne!(second_digest, CHANGE_DIGEST);
    let first_path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    let second_path = format!("/console/extensions/frontend/review/{second_digest}");
    let fixture = Fixture::new(vec![
        with_review_url(prepared_change_reply(&change_raw()), json!(first_path)),
        with_review_url(
            prepared_change_reply(&change_raw().replace(CHANGE_DIGEST, &second_digest)),
            json!(second_path),
        ),
        prepared_draft_reply(&change_raw().replace(CHANGE_DIGEST, &second_digest), true),
        with_review_url(prepared_change_reply(&change_raw()), json!(first_path)),
    ])
    .await;
    let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut presenter = binding.take_secret_entry_presenter().unwrap();
    let dispatcher = review_dispatcher(&binding);
    for (id, expected) in [("first", "queued"), ("second", "unavailable")] {
        let text =
            dispatch_review_candidate(&*dispatcher, id, serde_json::to_value(request()).unwrap())
                .await
                .unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["review_presentation"], expected);
        assert_eq!(value["valid"], true);
        assert!(value.get("review_url").is_none());
    }
    let mut output = Vec::new();
    presenter.present_pending(&mut output).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains(&format!("{}{first_path}", fixture.origin)));
    assert!(!output.contains(&second_digest));
    let recovered = dispatch_review_tool(
        &*dispatcher,
        "recover-second",
        meerkat_connect::paired_host_tool::PAIRED_HOST_FRONTEND_STATUS,
        json!({"change_digest":second_digest,"show_review":true}),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&recovered).unwrap()["review_presentation"],
        "queued"
    );
    assert!(!recovered.contains(&second_path));
    let mut second_output = Vec::new();
    presenter.present_pending(&mut second_output).unwrap();
    let second_output = String::from_utf8(second_output).unwrap();
    assert!(second_output.contains(&format!("{}{second_path}", fixture.origin)));
    assert!(!second_output.contains(&first_path));
    drop(presenter);
    let text = dispatch_review_candidate(
        &*dispatcher,
        "disconnected",
        serde_json::to_value(request()).unwrap(),
    )
    .await
    .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["review_presentation"], "unavailable");
    assert_eq!(value["change"]["change_digest"], CHANGE_DIGEST);
    let requests = fixture.finish().await;
    assert_eq!(
        requests
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "POST", "GET", "POST"]
    );
}

#[tokio::test]
async fn review_redisplay_requires_retained_path_and_current_host_access() {
    use meerkat_connect::paired_host_tool::{PAIRED_HOST_FRONTEND_STATUS, PairedHostFrontend};
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    let raw = change_raw();
    let fixture = Fixture::new(vec![
        with_review_url(prepared_change_reply(&raw), json!(path)),
        Reply::json(404, json!({"state":"not_found"})),
        Reply::json(403, json!({"error":"forbidden"})),
        Reply::json(503, json!({"error":"unavailable","problems":[{"code":"current_inputs_invalid","path":"","message":"The current ledger or compiled family contacts cannot be read."}]})),
        prepared_draft_reply(&raw.replace(CHANGE_DIGEST, OTHER_CHANGE_DIGEST), true),
        prepared_draft_reply(&raw, true),
        prepared_change_reply(&raw),
        prepared_draft_reply(&raw, true),
    ]).await;
    let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut presenter = binding.take_secret_entry_presenter().unwrap();
    let dispatcher = review_dispatcher(&binding);
    dispatch_review_candidate(
        &*dispatcher,
        "prepare",
        serde_json::to_value(request()).unwrap(),
    )
    .await
    .unwrap();
    presenter.present_pending(&mut Vec::new()).unwrap();
    // Unknown fields cannot smuggle a replacement URL or reach the host.
    assert!(
        dispatch_review_tool(
            &*dispatcher,
            "injected-url",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true,"url":"https://wrong.invalid"})
        )
        .await
        .is_err()
    );
    for (id, succeeds) in [
        ("missing", true),
        ("revoked", false),
        ("host-unavailable", true),
        ("wrong-digest", false),
    ] {
        let result = dispatch_review_tool(
            &*dispatcher,
            id,
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await;
        if succeeds {
            assert_eq!(
                serde_json::from_str::<Value>(&result.unwrap()).unwrap()["review_presentation"],
                "unavailable"
            );
        } else {
            assert!(result.is_err());
        }
        let mut output = Vec::new();
        presenter.present_pending(&mut output).unwrap();
        assert!(output.is_empty());
    }
    let fresh = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut fresh_presenter = fresh.take_secret_entry_presenter().unwrap();
    let fresh_dispatcher = review_dispatcher(&fresh);
    for prepared_without_link in [false, true] {
        if prepared_without_link {
            dispatch_review_candidate(
                &*fresh_dispatcher,
                "older-host-prepare",
                serde_json::to_value(request()).unwrap(),
            )
            .await
            .unwrap();
        }
        let result = dispatch_review_tool(
            &*fresh_dispatcher,
            "no-retained-link",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":CHANGE_DIGEST,"show_review":true}),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result).unwrap()["review_presentation"],
            "unavailable"
        );
        let mut output = Vec::new();
        fresh_presenter.present_pending(&mut output).unwrap();
        assert!(output.is_empty());
    }
    let requests = fixture.finish().await;
    assert_eq!(
        requests
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "GET", "GET", "GET", "GET", "GET", "POST", "GET"]
    );
    assert!(requests.iter().all(|r| if r.method == "POST" {
        r.path == "/console/extensions/frontend/prepare"
    } else {
        r.path == format!("{CHANGES_PATH}/{CHANGE_DIGEST}")
    }));
}

#[tokio::test]
async fn cancelled_preparation_cannot_publish_a_late_review_link() {
    use meerkat_connect::paired_host_tool::PairedHostFrontend;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let arrived = Arc::new(Notify::new());
    let received = arrived.clone();
    let (release, released) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut stream, _) = timeout(FIXTURE_BOUND, listener.accept())
            .await
            .unwrap()
            .unwrap();
        let request = timeout(FIXTURE_BOUND, read_request(&mut stream))
            .await
            .unwrap();
        received.notify_one();
        timeout(FIXTURE_BOUND, released).await.unwrap().unwrap();
        with_review_url(
            prepared_change_reply(&change_raw()),
            json!(format!(
                "/console/extensions/frontend/review/{CHANGE_DIGEST}"
            )),
        )
        .write(&mut stream)
        .await;
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
        vec![request]
    });
    let fixture = Fixture {
        origin,
        task: Some(task),
    };
    let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut presenter = binding.take_secret_entry_presenter().unwrap();
    let dispatcher = review_dispatcher(&binding);
    let mut pending = Box::pin(dispatch_review_candidate(
        &*dispatcher,
        "cancelled",
        serde_json::to_value(request()).unwrap(),
    ));
    tokio::select! {
        _ = arrived.notified() => {},
        _ = &mut pending => panic!("host response is still held"),
        _ = tokio::time::sleep(FIXTURE_BOUND) => panic!("prepare did not reach fixture"),
    }
    drop(pending);
    release.send(()).unwrap();
    assert_eq!(fixture.finish().await.len(), 1);
    let mut output = Vec::new();
    presenter.present_pending(&mut output).unwrap();
    assert!(output.is_empty());
}

#[tokio::test]
async fn review_output_failure_does_not_erase_retained_submission_or_expose_the_url() {
    use meerkat_connect::{
        SessionOptions,
        approval::LocalApprovals,
        paired_host_tool::{PAIRED_HOST_FRONTEND_SUBMIT, PairedHostFrontend},
        setup::paired_host_frontend_session,
    };
    use meerkat_core::{
        AgentToolDispatcher, ApprovalDecision, ExecutionPolicyGatedDispatcher, SessionId,
        ToolExecutionPolicy, agent::ToolDispatchContext, types::ToolCallView,
    };
    struct BrokenOutput;
    impl std::io::Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("private-output-failure-sentinel"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    let fixture = Fixture::new(vec![with_review_url(
        prepared_change_reply(&change_raw()),
        json!(path),
    )])
    .await;
    let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let mut presenter = binding.take_secret_entry_presenter().unwrap();
    let dispatcher = review_dispatcher(&binding);
    let text = dispatch_review_candidate(
        &*dispatcher,
        "prepared",
        serde_json::to_value(request()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["review_presentation"],
        "queued"
    );
    let error = presenter.present_pending(&mut BrokenOutput).unwrap_err();
    let error = format!("{error:?} {error}");
    assert!(error.contains(CHANGE_DIGEST));
    for private in [
        fixture.origin.as_str(),
        path.as_str(),
        "private-output-failure-sentinel",
    ] {
        assert!(!error.contains(private));
    }
    presenter.present_pending(&mut Vec::new()).unwrap();

    // A fresh native approval request must still be built from the exact
    // retained candidate. Denying it sends no submission to the fixture.
    let directory = tempfile::tempdir().unwrap();
    let (approvals, mut decisions) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut options = paired_host_frontend_session(SessionOptions::default(), &binding).unwrap();
    approvals
        .attach_paired_host_direct(&mut options, &binding)
        .unwrap();
    let session_id = SessionId::new();
    approvals.bind_session(&session_id).unwrap();
    let dispatcher = ExecutionPolicyGatedDispatcher::new(
        options.build.external_tools.unwrap(),
        ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
    )
    .with_dispatch_admission(options.build.tool_dispatch_admission.unwrap());
    let task = tokio::spawn(async move {
        let arguments = serde_json::value::RawValue::from_string(
            json!({"change_digest":CHANGE_DIGEST}).to_string(),
        )
        .unwrap();
        dispatcher
            .dispatch_with_context(
                ToolCallView {
                    id: "after-output-failure",
                    name: PAIRED_HOST_FRONTEND_SUBMIT,
                    args: &arguments,
                },
                &ToolDispatchContext::default().with_runtime_identity(session_id, None),
            )
            .await
    });
    let mut decision = timeout(FIXTURE_BOUND, decisions.next())
        .await
        .unwrap()
        .unwrap();
    decision.decide(ApprovalDecision::Deny).unwrap();
    assert!(
        timeout(FIXTURE_BOUND, task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(!approvals.has_uncertain_dispatch());
    assert_eq!(fixture.finish().await.len(), 1);
}

fn prepared_draft_reply(raw: &str, base_current: bool) -> Reply {
    Reply::bytes(
        200,
        format!(r#"{{"state":"prepared","change":{raw},"base_current":{base_current}}}"#)
            .into_bytes(),
    )
}

fn submission_reply(status: u16, raw: &str) -> Reply {
    Reply::bytes(status, format!(r#"{{"request_id":"{CHANGE_DIGEST}","state":"submitted","submitted_at":"2026-10-07T00:00:00+00:00","change":{raw},"submitted_by":"paired-device"}}"#).into_bytes())
}

#[tokio::test]
async fn review_paths_are_optional_exact_and_preserve_both_prepared_change_kinds() {
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    let channel = change_raw();
    let extension = new_extension_raw();
    let fixture = Fixture::new(vec![
        prepared_change_reply(&channel),
        with_review_url(prepared_change_reply(&channel), json!(path)),
        new_extension_prepared(&extension),
        with_review_url(new_extension_prepared(&extension), json!(path)),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for expected in [None, Some(path.as_str())] {
        let FrontendPreparation::Candidate(receipt) =
            client.prepare_candidate(&request()).await.unwrap()
        else {
            panic!("candidate expected");
        };
        assert_eq!(receipt.review_url.as_deref(), expected);
        assert_eq!(
            serde_json::to_string(receipt.change.as_ref().unwrap()).unwrap(),
            channel
        );
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value.get("review_url").and_then(Value::as_str), expected);
        assert_eq!(value.get("review_url").is_some(), expected.is_some());
    }
    for expected in [None, Some(path.as_str())] {
        let FrontendNewExtensionPreparation::Candidate(receipt) = client
            .prepare_new_extension(&new_extension_request())
            .await
            .unwrap()
        else {
            panic!("new extension candidate expected");
        };
        assert_eq!(receipt.review_url.as_deref(), expected);
        assert_eq!(
            serde_json::to_string(receipt.change.as_ref().unwrap()).unwrap(),
            extension
        );
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value.get("review_url").and_then(Value::as_str), expected);
        assert_eq!(value.get("review_url").is_some(), expected.is_some());
    }
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 4);
    assert!(
        requests
            .iter()
            .all(|r| r.method == "POST" && r.path == "/console/extensions/frontend/prepare")
    );
}

#[tokio::test]
async fn review_paths_refuse_null_cross_origin_altered_digest_and_nonchange_receipts() {
    let path = format!("/console/extensions/frontend/review/{CHANGE_DIGEST}");
    let paths = [
        Value::Null,
        json!(false),
        json!(""),
        json!(format!("https://private-marker.invalid{path}")),
        json!(format!("http://127.0.0.1:1{path}")),
        json!(format!("//private-marker.invalid{path}")),
        json!(format!(
            "/console/extensions/frontend/review/{OTHER_CHANGE_DIGEST}"
        )),
        json!(format!("{path}?private-marker")),
        json!(format!("{path}#private-marker")),
        json!(format!("{path}/submit")),
        json!(path.replace(':', "%3A")),
        json!(format!("/console/../{path}")),
    ];
    let mut replies = Vec::new();
    for path in &paths {
        replies.push(with_review_url(
            prepared_change_reply(&change_raw()),
            path.clone(),
        ));
        replies.push(with_review_url(
            new_extension_prepared(&new_extension_raw()),
            path.clone(),
        ));
    }
    // These are accepted predecessor receipts, but neither has a valid change
    // for a successor review path to identify.
    let no_change = receipt();
    let invalid_extension = json!({"valid":false,"problems":[{
        "code":"invalid_value","path":"new_extension.source.commit",
        "message":"A request field has a value outside its allowed form."}],"change":null});
    replies.extend([
        Reply::json(200, no_change.clone()),
        with_review_url(Reply::json(200, no_change), json!(path)),
        Reply::json(200, invalid_extension.clone()),
        with_review_url(Reply::json(200, invalid_extension), json!(path)),
    ]);
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let expected = unconfirmed(
        PairedHostOperation::PrepareCandidate,
        UnconfirmedReason::UnexpectedResponse,
    );
    for _ in &paths {
        for error in [
            client.prepare_candidate(&request()).await.unwrap_err(),
            client
                .prepare_new_extension(&new_extension_request())
                .await
                .unwrap_err(),
        ] {
            assert_eq!(error, expected);
            assert_safe(&error);
        }
    }
    assert!(matches!(
        client.prepare_candidate(&request()).await.unwrap(),
        FrontendPreparation::Candidate(_)
    ));
    let error = client.prepare_candidate(&request()).await.unwrap_err();
    assert_eq!(error, expected);
    assert_safe(&error);
    assert!(matches!(
        client
            .prepare_new_extension(&new_extension_request())
            .await
            .unwrap(),
        FrontendNewExtensionPreparation::Candidate(_)
    ));
    let error = client
        .prepare_new_extension(&new_extension_request())
        .await
        .unwrap_err();
    assert_eq!(error, expected);
    assert_safe(&error);
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), paths.len() * 2 + 4);
    assert!(
        requests
            .iter()
            .all(|r| r.method == "POST" && r.path == "/console/extensions/frontend/prepare")
    );
}

#[tokio::test]
async fn prepared_draft_readback_preserves_owner_bytes_but_never_recovers_client_binding() {
    let channel = change_raw();
    let extension = new_extension_raw();
    let fixture = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        prepared_change_reply(&channel),
        prepared_draft_reply(&channel, true),
        prepared_draft_reply(&channel, false),
        new_extension_prepared(&extension),
        prepared_draft_reply(&extension, true),
        prepared_draft_reply(&extension, false),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client
        .request_pairing("draft readback fixture")
        .await
        .unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    let _original_channel = capture_change(&mut client).await;
    for (raw, is_new_extension) in [(&channel, false), (&extension, true)] {
        if is_new_extension {
            let _original_extension = capture_new_extension(&mut client).await;
        }
        for base_current in [true, false] {
            let FrontendChangeRead::Prepared(draft) =
                client.frontend_change_status(&digest).await.unwrap()
            else {
                panic!("prepared draft expected");
            };
            assert_eq!(draft.change.change_digest(), &digest);
            assert_eq!(draft.base_current, base_current);
            assert_eq!(serde_json::to_string(&draft.change).unwrap(), *raw);
            assert_eq!(
                serde_json::to_value(&draft).unwrap(),
                json!({
                    "state":"prepared", "change":serde_json::from_str::<Value>(raw).unwrap(),
                    "base_current":base_current
                })
            );
            let error = match &draft.change {
                FrontendPreparedChange::Channel(change) => {
                    assert!(!is_new_extension);
                    client.submit_frontend_change(change).await.unwrap_err()
                }
                FrontendPreparedChange::NewExtension(change) => {
                    assert!(is_new_extension);
                    client
                        .submit_frontend_new_extension_change(change)
                        .await
                        .unwrap_err()
                }
            };
            assert_eq!(error, PairedHostError::UnboundFrontendChange);
            assert_safe(&error);
        }
    }
    let requests = fixture.finish().await;
    assert_eq!(
        requests.len(),
        8,
        "pairing, two preparations and four GETs only"
    );
    assert_eq!(requests.iter().filter(|r| r.method == "GET").count(), 4);
    assert!(
        !requests
            .iter()
            .any(|r| r.method == "POST" && r.path == CHANGES_PATH)
    );
    for record in requests.iter().filter(|r| r.method == "GET") {
        assert_eq!(record.path, format!("{CHANGES_PATH}/{CHANGE_DIGEST}"));
        assert_eq!(record.headers["cache-control"], "no-store");
        assert_eq!(
            record.headers["cookie"],
            format!("hc_console_device={DEVICE_COOKIE}")
        );
        assert!(record.body.is_empty());
    }
}

#[tokio::test]
async fn prepared_draft_readback_refuses_unknown_shapes_kinds_and_mismatched_digest() {
    let mut replies = Vec::new();
    for raw in [change_raw(), new_extension_raw()] {
        let base = json!({"state":"prepared", "change":serde_json::from_str::<Value>(&raw).unwrap(), "base_current":true});
        for field in ["state", "change", "base_current"] {
            let mut missing = base.clone();
            missing.as_object_mut().unwrap().remove(field);
            replies.push(Reply::json(200, missing));
        }
        for (field, value) in [
            ("request_id", json!(CHANGE_DIGEST)),
            ("submitted_at", json!("fixture-time")),
            ("kind", json!("new_extension")),
            (
                "review_url",
                json!(format!(
                    "/console/extensions/frontend/review/{CHANGE_DIGEST}"
                )),
            ),
            ("base_current", Value::Null),
            ("base_current", json!("true")),
        ] {
            let mut malformed = base.clone();
            malformed[field] = value;
            replies.push(Reply::json(200, malformed));
        }
        for (field, value) in [
            ("change_digest", json!(OTHER_CHANGE_DIGEST)),
            ("kind", Value::Null),
            ("kind", json!("future_kind")),
            ("private-marker", json!(DEVICE_COOKIE)),
        ] {
            let mut malformed = base.clone();
            malformed["change"][field] = value;
            replies.push(Reply::json(200, malformed));
        }
        let duplicate = format!(
            r#"{{"state":"prepared","change":{raw},"base_current":true,"base_current":false}}"#
        );
        replies.push(Reply::bytes(200, duplicate.into_bytes()));
        let duplicate_change = raw.replace(
            "\"change_digest\":",
            &format!("\"change_digest\":\"{OTHER_CHANGE_DIGEST}\",\"change_digest\":"),
        );
        replies.push(prepared_draft_reply(&duplicate_change, true));
    }
    let count = replies.len();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    for _ in 0..count {
        let error = client.frontend_change_status(&digest).await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::FrontendChangeStatus,
                UnconfirmedReason::UnexpectedResponse
            )
        );
        assert_safe(&error);
    }
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), count);
    assert!(
        requests
            .iter()
            .all(|r| r.method == "GET" && r.body.is_empty())
    );
}

async fn capture_change(client: &mut PairedHostClient) -> FrontendChange {
    let FrontendPreparation::Candidate(receipt) =
        client.prepare_candidate(&request()).await.unwrap()
    else {
        panic!("candidate expected");
    };
    assert!(receipt.valid && receipt.compiled_is_not_ready);
    assert!(
        !receipt.missing_prerequisites.is_empty(),
        "valid does not mean ready"
    );
    receipt.change.expect("owner supplied a change")
}

#[tokio::test]
async fn prepared_change_submission_preserves_exact_owner_bytes_and_only_acknowledges_a_request() {
    let raw = change_raw();
    // Durable JSON storage can reorder keys and whitespace, but not values.
    let reordered = raw.replace(
        "\"threshold\":1e-06,\"label\":\"fixture\"",
        "\"label\":\"fixture\", \"threshold\":1e-06",
    );
    let fixture = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        prepared_change_reply(&raw),
        submission_reply(201, &reordered),
        submission_reply(200, &reordered),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    assert_eq!(client.selected_origin(), fixture.origin);
    client.request_pairing("change fixture").await.unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let change = capture_change(&mut client).await;
    assert_eq!(change.change_digest().as_str(), CHANGE_DIGEST);
    assert_eq!(change.ledger_patch().path, "deployment/extensions.toml");
    assert_eq!(change.base().activation_id.as_deref(), Some("210"));
    assert_eq!(serde_json::to_string(&change).unwrap(), raw);
    assert!(
        change
            .unified_diff()
            .contains("+approved_groups = [\"-1001234567890\"]")
    );
    for expected_status in [201, 200] {
        let FrontendSubmission::Recorded(record) =
            client.submit_frontend_change(&change).await.unwrap()
        else {
            panic!("record expected");
        };
        assert_eq!(record.http_status, expected_status);
        assert_eq!(record.request_id.as_str(), CHANGE_DIGEST);
        assert_eq!(record.submitted_by, FrontendSubmitter::PairedDevice);
        assert_eq!(serde_json::to_value(&record).unwrap()["state"], "submitted");
        assert!(
            serde_json::to_value(&record)
                .unwrap()
                .get("activation_id")
                .is_none()
        );
        // A POST echo is not a new prepared, client-bound instruction.
        assert_eq!(
            client
                .submit_frontend_change(&record.change)
                .await
                .unwrap_err(),
            PairedHostError::UnboundFrontendChange
        );
    }
    let requests = fixture.finish().await;
    assert_eq!(
        requests.len(),
        5,
        "two explicit submissions, never an internal replay"
    );
    for record in &requests[3..] {
        assert_eq!(record.path, CHANGES_PATH);
        assert_eq!(record.method, "POST");
        assert_eq!(record.headers["origin"], client.selected_origin());
        assert_eq!(
            record.headers["cookie"],
            format!("hc_console_device={DEVICE_COOKIE}")
        );
        assert!(!record.headers.contains_key("authorization"));
        let body = std::str::from_utf8(&record.body).unwrap();
        assert_eq!(
            body,
            format!("{{\"change_digest\":\"{CHANGE_DIGEST}\",\"change\":{raw}}}")
        );
        assert!(
            body.contains("\"threshold\":1e-06"),
            "no numeric canonicalization"
        );
        assert!(!body.contains(CODE) && !body.contains(DEVICE_COOKIE));
    }
}

#[tokio::test]
async fn change_submission_cannot_cross_client_host_or_deserialized_model_data() {
    let forbidden = || {
        Reply::json(
            403,
            json!({"error":"forbidden","code":"console_owner_required"}),
        )
    };
    let first = Fixture::new(vec![
        pairing_requested(),
        pairing_confirmed(),
        prepared_change_reply(&change_raw()),
        forbidden(),
    ])
    .await;
    let second = Fixture::new(vec![forbidden()]).await;
    let mut client = PairedHostClient::new(&first.origin).unwrap();
    client.request_pairing("origin fixture").await.unwrap();
    client
        .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
        .await
        .unwrap();
    let change = capture_change(&mut client).await;
    let mut same_origin = PairedHostClient::new(&first.origin).unwrap();
    let mut other = PairedHostClient::new(&second.origin).unwrap();
    let unbound: FrontendChange = serde_json::from_str(&change_raw()).unwrap();
    for caller in [&mut same_origin, &mut other] {
        assert_eq!(
            caller.submit_frontend_change(&change).await.unwrap_err(),
            PairedHostError::UnboundFrontendChange
        );
    }
    assert_eq!(
        client.submit_frontend_change(&unbound).await.unwrap_err(),
        PairedHostError::UnboundFrontendChange
    );
    assert_eq!(
        client.submit_frontend_change(&change).await.unwrap_err(),
        PairedHostError::Refused {
            operation: PairedHostOperation::SubmitFrontendChange,
            reason: PairedHostRefusal::ConsoleOwnerRequired
        }
    );
    assert_eq!(
        other
            .frontend_change_status(change.change_digest())
            .await
            .unwrap_err(),
        PairedHostError::Refused {
            operation: PairedHostOperation::FrontendChangeStatus,
            reason: PairedHostRefusal::ConsoleOwnerRequired
        }
    );
    assert_eq!(first.finish().await.len(), 4);
    let requests = second.finish().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert!(!requests[0].headers.contains_key("cookie"));
}

#[tokio::test]
async fn lost_change_post_stays_unresolved_through_not_found_then_explicit_readback_without_replay()
{
    let mut lost = submission_reply(201, &change_raw());
    lost.close_without_reply = true;
    let fixture = Fixture::new(vec![
        prepared_change_reply(&change_raw()),
        lost,
        Reply::json(404, json!({"state":"not_found"})),
        Reply::json(
            200,
            json!({"request_id":CHANGE_DIGEST,"state":"submitted","submitted_at":"fixture-time"}),
        ),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_change(&mut client).await;
    let error = client.submit_frontend_change(&change).await.unwrap_err();
    assert_eq!(
        error,
        unconfirmed(
            PairedHostOperation::SubmitFrontendChange,
            UnconfirmedReason::Transport
        )
    );
    assert_safe(&error);
    assert_eq!(
        client
            .frontend_change_status(change.change_digest())
            .await
            .unwrap(),
        FrontendChangeRead::NotFound
    );
    assert!(matches!(
        client
            .frontend_change_status(change.change_digest())
            .await
            .unwrap(),
        FrontendChangeRead::Status(FrontendChangeStatus::Submitted { .. })
    ));
    let requests = fixture.finish().await;
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.path == CHANGES_PATH && r.method == "POST")
            .count(),
        1
    );
    for request in &requests[2..] {
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, format!("{CHANGES_PATH}/{CHANGE_DIGEST}"));
        assert!(request.body.is_empty());
        assert_eq!(request.headers["cache-control"], "no-store");
        assert_eq!(request.headers["origin"], client.selected_origin());
    }
}

#[tokio::test]
async fn submission_server_failure_can_follow_publication_and_requires_readback_without_replay() {
    let fixture = Fixture::new(vec![prepared_change_reply(&change_raw()),
        Reply::json(500, json!({"error":"internal_error","problems":[{"code":"internal_error","path":"","message":"Validation stopped on an unexpected error."}]})),
        Reply::json(404, json!({"state":"not_found"})),
        Reply::json(200, json!({"request_id":CHANGE_DIGEST,"state":"submitted","submitted_at":"fixture-time"})),
        Reply::json(503, json!({"error":"unavailable","problems":[{"code":"current_inputs_invalid","path":"","message":"The current ledger or compiled family contacts cannot be read."}]}))]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_change(&mut client).await;
    let error = client.submit_frontend_change(&change).await.unwrap_err();
    assert_eq!(
        error,
        unconfirmed(
            PairedHostOperation::SubmitFrontendChange,
            UnconfirmedReason::HostFailure
        )
    );
    assert_safe(&error);
    assert_eq!(
        client
            .frontend_change_status(change.change_digest())
            .await
            .unwrap(),
        FrontendChangeRead::NotFound
    );
    assert!(matches!(
        client
            .frontend_change_status(change.change_digest())
            .await
            .unwrap(),
        FrontendChangeRead::Status(FrontendChangeStatus::Submitted { .. })
    ));
    let FrontendChangeRead::Rejected(failure) = client
        .frontend_change_status(change.change_digest())
        .await
        .unwrap()
    else {
        panic!("failed GET evidence expected");
    };
    assert_eq!(failure.error, FrontendChangeRejectionKind::Unavailable);
    assert_eq!(
        fixture
            .finish()
            .await
            .iter()
            .filter(|r| r.method == "POST" && r.path == CHANGES_PATH)
            .count(),
        1
    );
}

#[tokio::test]
async fn change_status_preserves_host_states_without_deriving_provider_readiness() {
    let records = [
        json!({"request_id":CHANGE_DIGEST,"state":"submitted","submitted_at":"fixture-time"}),
        json!({"request_id":CHANGE_DIGEST,"state":"applied","submitted_at":"fixture-time","activation_id":"214"}),
        json!({"request_id":CHANGE_DIGEST,"state":"superseded","submitted_at":"fixture-time","activation_id":null}),
        json!({"request_id":CHANGE_DIGEST,"state":"rejected","submitted_at":"fixture-time","reason":"Operator declined this fixture","decided_at":"fixture-decision-time"}),
    ];
    let fixture = Fixture::new(
        records
            .iter()
            .map(|record| Reply::json(200, record.clone()))
            .collect(),
    )
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    for expected in records {
        let FrontendChangeRead::Status(status) =
            client.frontend_change_status(&digest).await.unwrap()
        else {
            panic!("host state expected");
        };
        assert_eq!(status.request_id(), &digest);
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value, expected);
        assert!(value.get("ready").is_none());
    }
    assert_eq!(fixture.finish().await.len(), 4);
}

#[tokio::test]
async fn submission_preserves_stale_base_and_closed_host_refusals_without_reprepare() {
    let cases = [
        (
            409,
            "stale_base",
            "stale_base",
            "The change was prepared against a snapshot that is no longer current.",
            FrontendChangeRejectionKind::StaleBase,
        ),
        (
            400,
            "diff_mismatch",
            "diff_mismatch",
            "The change's unified diff is not the rendering of its ledger patch.",
            FrontendChangeRejectionKind::DiffMismatch,
        ),
        (
            400,
            "digest_mismatch",
            "digest_mismatch",
            "The change digest does not match the submitted change.",
            FrontendChangeRejectionKind::DigestMismatch,
        ),
        (
            409,
            "candidate_invalid",
            "change_not_reproducible",
            "The submitted change is not the change PairedHost derives for this candidate from the current snapshot.",
            FrontendChangeRejectionKind::CandidateInvalid,
        ),
    ];
    let mut replies = vec![prepared_change_reply(&change_raw())];
    replies.extend(cases.iter().map(|(status, error, code, message, _)| {
        Reply::json(
            *status,
            json!({"error":error,"problems":[{"code":code,"path":"change","message":message}]}),
        )
    }));
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_change(&mut client).await;
    for (http_status, _, code, _, kind) in cases {
        let FrontendSubmission::Rejected(rejection) =
            client.submit_frontend_change(&change).await.unwrap()
        else {
            panic!("typed rejection expected");
        };
        assert_eq!(rejection.http_status, http_status);
        assert_eq!(rejection.error, kind);
        assert_eq!(rejection.problems[0].code, code);
    }
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.path.ends_with("/prepare"))
            .count(),
        1,
        "no automatic prepare or resubmit after stale base"
    );
}

#[tokio::test]
async fn malformed_or_crossed_prepare_changes_never_become_submit_capable() {
    let base: Value = serde_json::from_str(&change_raw()).unwrap();
    let mut unknown = base.clone();
    unknown["activation_token"] = json!(DEVICE_COOKIE);
    let mut crossed = base.clone();
    crossed["base"]["activation_id"] = json!("999");
    let mut path = base.clone();
    path["ledger_patch"]["path"] = json!("deployment/senders.toml");
    let mut extension = base.clone();
    extension["ledger_patch"]["extension_id"] = json!("whatsapp");
    let mut channel = base.clone();
    channel["ledger_patch"]["channel_id"] = json!("telegram:other");
    let mut ingress = base.clone();
    ingress["ledger_patch"]["after"]["ingress"] = json!("webhook");
    let mut groups = base.clone();
    groups["ledger_patch"]["after"]["approved_groups"] = json!([]);
    let mut absent = base.clone();
    absent["base"]
        .as_object_mut()
        .unwrap()
        .remove("activation_id");
    let mut digest = base.clone();
    digest["change_digest"] = json!(format!("{CHANGE_DIGEST}?code={CODE}"));
    let mut rows = base.clone();
    rows["ledger_patch"]["before"] = json!([]);
    let duplicate =
        change_raw().replace("\"threshold\":1e-06", "\"threshold\":1e-06,\"threshold\":2");
    let mut replies: Vec<_> = [
        unknown, crossed, path, extension, channel, ingress, groups, absent, digest, rows,
    ]
    .iter()
    .map(|value| prepared_change_reply(&value.to_string()))
    .collect();
    replies.push(prepared_change_reply(&duplicate));
    let count = replies.len();
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..count {
        let error = client.prepare_candidate(&request()).await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::PrepareCandidate,
                UnconfirmedReason::UnexpectedResponse
            )
        );
        assert_safe(&error);
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn new_change_response_parsing_refuses_contradictions_and_keeps_errors_value_free() {
    let crossed = change_raw().replace("+approved_groups =", "+features =");
    let mut no_store = submission_reply(201, &change_raw());
    no_store.headers.retain(|(key, _)| key != "Cache-Control");
    let fixture=Fixture::new(vec![prepared_change_reply(&change_raw()),
        submission_reply(201,&crossed),
        Reply::json(201,json!({"request_id":OTHER_CHANGE_DIGEST,"state":"submitted","submitted_at":"fixture","change":serde_json::from_str::<Value>(&change_raw()).unwrap(),"submitted_by":"paired-device"})),
        Reply::json(409,json!({"error":"stale_base","problems":[{"code":"stale_base","path":"change.base","message":DEVICE_COOKIE}]})),
        Reply::json(200,json!({"state":"applied","activation_id":"214"})),
        no_store]).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let change = capture_change(&mut client).await;
    for _ in 0..5 {
        let error = client.submit_frontend_change(&change).await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(
                PairedHostOperation::SubmitFrontendChange,
                UnconfirmedReason::UnexpectedResponse
            )
        );
        assert_safe(&error);
    }
    assert_eq!(fixture.finish().await.len(), 6);
}

#[tokio::test]
async fn status_requires_exact_correlation_closed_states_fresh_json_and_bounded_origin_transport() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let submitted =
        json!({"request_id":CHANGE_DIGEST,"state":"submitted","submitted_at":"fixture"});
    let mut crossed = submitted.clone();
    crossed["request_id"] = json!(OTHER_CHANGE_DIGEST);
    let mut unknown = submitted.clone();
    unknown["credential"] = json!(DEVICE_COOKIE);
    let mut future = submitted.clone();
    future["state"] = json!("ready");
    let mut null_applied = submitted.clone();
    null_applied["state"] = json!("applied");
    null_applied["activation_id"] = Value::Null;
    let mut no_store = Reply::json(200, submitted.clone());
    no_store.headers.retain(|(key, _)| key != "Cache-Control");
    let duplicate =
        submitted
            .to_string()
            .replacen("\"submitted\"", "\"submitted\",\"state\":\"applied\"", 1);
    let mut replies: Vec<_> = [crossed, unknown, future, null_applied]
        .into_iter()
        .map(|value| Reply::json(200, value))
        .collect();
    replies.extend([
        Reply::bytes(200, duplicate.into_bytes()),
        no_store,
        Reply::json(404, json!({"state":"not_found","request_id":CHANGE_DIGEST})),
        Reply::json(404, json!({"error":"not_found"})),
    ]);
    let malformed_count = replies.len();
    replies.push(Reply::bytes(200, vec![b'x'; 64 * 1024 + 1]));
    replies.push(Reply::json(307, json!({})).header(
        "Location",
        &format!(
            "http://{}/private-marker",
            destination.local_addr().unwrap()
        ),
    ));
    let fixture = Fixture::new(replies).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = FrontendChangeDigest::new(CHANGE_DIGEST).unwrap();
    for reason in
        std::iter::repeat_n(UnconfirmedReason::UnexpectedResponse, malformed_count).chain([
            UnconfirmedReason::ResponseTooLarge,
            UnconfirmedReason::Redirect,
        ])
    {
        let error = client.frontend_change_status(&digest).await.unwrap_err();
        assert_eq!(
            error,
            unconfirmed(PairedHostOperation::FrontendChangeStatus, reason)
        );
        assert_safe(&error);
    }
    assert_eq!(fixture.finish().await.len(), malformed_count + 2);
    assert!(
        timeout(Duration::from_millis(100), destination.accept())
            .await
            .is_err()
    );
    for text in [
        "private-marker",
        "hc-frontend-change-v1:sha256:../private-marker",
        &format!("{CHANGE_DIGEST}/../private-marker"),
        &CHANGE_DIGEST.to_uppercase(),
    ] {
        let error = FrontendChangeDigest::new(text).unwrap_err();
        assert_eq!(error, PairedHostError::InvalidChangeDigest);
        assert_safe(&error);
    }
}
