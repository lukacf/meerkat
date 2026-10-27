//! Minimal Forge application facade, selected by the trusted Rust host.
//! Proposal extensions and revision semantics remain application-owned.
use super::*;

pub(super) fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,
        "required":["configuration","expected_revision"],
        "properties":{"configuration":{"type":"object"},"expected_revision":{}}})
}

/// Mirror only the facade's declared-key check. This is not general secret
/// detection: applications must keep confidential values out of other fields.
fn private_references(value: &Value, fields: &BTreeSet<String>) -> Result<(), ForgeError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if fields.contains(key) && child != &json!({"private_field":key}) {
                    return Err(invalid());
                }
                private_references(child, fields)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                private_references(child, fields)?;
            }
        }
        _ => (),
    }
    Ok(())
}

pub(super) fn observation(value: &Value, fields: &BTreeSet<String>) -> Result<(), ForgeError> {
    if !value.is_object() {
        return Err(invalid());
    }
    private_references(value, fields)
}

pub(super) fn prepare(raw: &str, fields: &BTreeSet<String>) -> Result<Box<RawValue>, ForgeError> {
    let value: Value = parse(raw)?;
    if !value.as_object().is_some_and(|object| {
        object.len() == 2
            && object.contains_key("configuration")
            && object.contains_key("expected_revision")
    }) || !value["configuration"].is_object()
    {
        return Err(invalid());
    }
    private_references(&value, fields)?;
    // No JSON-number conversion, revision arithmetic or default insertion is
    // used for the actual command. The original typed JSON is sent verbatim.
    RawValue::from_string(raw.to_owned()).map_err(|_| invalid())
}

pub(super) fn proposal(value: &Value, fields: &BTreeSet<String>) -> Result<(), ForgeError> {
    let object = value.as_object().ok_or_else(invalid)?;
    if !["change_id", "configuration", "expected_revision"]
        .iter()
        .all(|key| object.contains_key(*key))
        || !value["configuration"].is_object()
        || !value["change_id"].as_str().is_some_and(id)
    {
        return Err(invalid());
    }
    // Do not impose Reading's participants, routing, principal fields,
    // integer revision or equality with the requested configuration. The app
    // owns its proposal; the person approves this complete returned proposal.
    private_references(value, fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn host() -> ForgeHost {
        ForgeHost {
            protocol: 1,
            host_id: format!("forge:{}", "a".repeat(64)),
            application_id: "custom-application".into(),
            source_digest: "b".repeat(64),
        }
    }
    fn fields() -> BTreeSet<String> {
        ["PRIVATE_TOKEN".to_owned()].into()
    }
    fn recipe() -> Recipe {
        Recipe::Application {
            expected: host(),
            private_fields: fields(),
        }
    }
    const PROPOSAL: &str = r#"{"change_id":"custom:delta-7", "configuration":{"PRIVATE_TOKEN":{"private_field":"PRIVATE_TOKEN"}}, "expected_revision":{"etag":"v/007","sequence":184467440737095516161}, "app_policy":{"review":"manual","future_field":[true,null]}}"#;
    fn reference() -> RecoveryReference {
        RecoveryReference {
            schema: 2,
            origin: "http://127.0.0.1:34567".into(),
            host: host(),
            change_id: "custom:delta-7".into(),
            proposal: RawValue::from_string(PROPOSAL.into()).unwrap(),
            application_private_fields: Some(fields()),
        }
    }
    fn prepared() -> Arc<Prepared> {
        Arc::new(Prepared {
            reference: reference(),
            expires_at: now() + 120,
            proof: "c".repeat(64),
        })
    }

    #[test]
    fn generic_prepare_keeps_opaque_revision_bytes_without_reading_fields() {
        for revision in [
            r#""etag/001""#,
            "null",
            "true",
            "[1,2]",
            r#"{"etag":"a","counter":184467440737095516161}"#,
        ] {
            let raw = format!(
                r#"{{ "configuration":{{"mode":"custom"}}, "expected_revision":{revision} }}"#
            );
            let request = recipe()
                .prepare(&raw, &json!({"choices":["custom"]}))
                .unwrap();
            let (path, body) = request.wire().unwrap();
            assert_eq!(path, "/console/forge/frontends/prepare");
            assert_eq!(body, raw);
            assert!(reading::prepare_body(&raw, &json!({"choices":["custom"]})).is_err());
        }
        assert!(
            prepare(
                r#"{"configuration":{},"expected_revision":0,"principal":"injected"}"#,
                &fields()
            )
            .is_err()
        );
        assert!(
            prepare(
                r#"{"configuration":{},"expected_revision":0,"expected_revision":1}"#,
                &fields()
            )
            .is_err()
        );
        assert!(prepare(r#"{"configuration":{},"expected_revision":NaN}"#, &fields()).is_err());
    }

    #[test]
    fn generic_proposal_extensions_are_reviewed_and_submitted_verbatim() {
        let request = recipe()
            .prepare(
                r#"{"configuration":{},"expected_revision":null}"#,
                &json!({}),
            )
            .unwrap();
        // The application may normalize configuration and add its own review
        // policy. The complete returned proposal, not the earlier request, is approved.
        recipe()
            .proposal(&parse::<Value>(PROPOSAL).unwrap(), &request, &host())
            .unwrap();
        let held = HeldForgeAction {
            call_id: "call".into(),
            arguments: r#"{"change_id":"custom:delta-7"}"#.into(),
            prepared: prepared(),
        };
        let review = held.review();
        assert_eq!(review["proposal_json"].as_str(), Some(PROPOSAL));
        assert!(review.get("proposal").is_none());
        let (path, body) = OperationRequest::Submit(held.prepared).wire().unwrap();
        assert_eq!(path, "/console/forge/frontends/submit");
        let envelope: BTreeMap<String, Box<RawValue>> = parse(&body).unwrap();
        assert_eq!(envelope["proposal"].get(), PROPOSAL);
        let approval: Value = parse(envelope["approval"].get()).unwrap();
        assert_eq!(approval["approved"], true);
        assert_eq!(approval["proof"], "c".repeat(64));
    }

    #[test]
    fn generic_private_field_literals_are_rejected_recursively() {
        let good = json!({"nested":[{"PRIVATE_TOKEN":{"private_field":"PRIVATE_TOKEN"}}]});
        observation(&good, &fields()).unwrap();
        for bad in [
            json!({"nested":[{"PRIVATE_TOKEN":"a-literal"}]}),
            json!({"nested":{"PRIVATE_TOKEN":{"private_field":"OTHER"}}}),
            json!({"PRIVATE_TOKEN":null}),
        ] {
            assert!(observation(&bad, &fields()).is_err());
            let raw = json!({"configuration":bad,"expected_revision":"opaque"}).to_string();
            assert!(prepare(&raw, &fields()).is_err());
        }
        // No guessed principal/participant policy or generic secret detector.
        observation(&json!({"custom_application_field":"ordinary"}), &fields()).unwrap();
    }

    #[test]
    fn generic_recovery_never_selects_host_source_or_private_field_policy() {
        let (front, _presenter) = ForgeFrontend::for_application(host(), fields()).unwrap();
        front.binding.recipe.reference(&reference()).unwrap();
        let mut wrong = reference();
        wrong.application_private_fields = Some(BTreeSet::new());
        assert!(front.binding.recipe.reference(&wrong).is_err());
        wrong = reference();
        wrong.host.application_id = "another-app".into();
        assert!(front.binding.recipe.reference(&wrong).is_err());
        wrong = reference();
        wrong.change_id = "another-change".into();
        assert!(front.binding.recipe.reference(&wrong).is_err());
        let mut changed = host();
        changed.source_digest = "d".repeat(64);
        assert!(matches!(
            front.binding.recipe.host(&changed),
            Err(ForgeError::SourceChanged)
        ));
        assert!(Recipe::Reading.reference(&reference()).is_err());
        let reading = super::super::tests::reference();
        assert!(front.binding.recipe.reference(&reading).is_err());
        assert!(
            serde_json::to_string(&reading)
                .unwrap()
                .find("application_private_fields")
                .is_none()
        );
        let mut page = recovery::Page {
            references: vec![reference(), wrong, reading],
            skipped: 0,
            more: false,
        };
        restrict_page(&front.binding.recipe, &mut page);
        assert_eq!(page.references.len(), 1);
        assert_eq!(page.skipped, 2);
        let state = front.binding.configuration.lock().unwrap();
        assert!(state.prepared.is_empty() && state.armed.is_none() && state.references.is_empty());
    }

    #[test]
    fn generic_application_observations_cannot_restore_write_permission() {
        let reference = reference();
        for raw in [
            r#"{"status":"committed","revision":184467440737095516161}"#,
            r#"{"application_result":{"custom_state":"configured"}}"#,
            r#"{"status":"unknown"}"#,
        ] {
            let mut state = ConfigurationState::default();
            state.uncertain = Some(Some(reference.change_id.clone()));
            state.submitted.insert(reference.change_id.clone());
            let before = state.uncertain.clone();
            let raw = RawValue::from_string(raw.into()).unwrap();
            let reply = submission_reply(&recipe(), &host(), &reference, &raw, None, &mut state).unwrap();
            assert_eq!(reply["observation_json"].as_str(), Some(raw.get()));
            assert_eq!(reply["recovery_only"], true);
            assert_eq!(state.uncertain, before);
            assert!(state.submitted.contains(&reference.change_id));
            recipe()
                .status(&parse(raw.get()).unwrap(), &reference, &host())
                .unwrap();
            assert_eq!(state.uncertain, before);
        }
    }

    #[test]
    fn generic_locator_roundtrip_preserves_exact_proposal_without_proof() {
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let first = tempfile::Builder::new()
            .prefix("setup-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(root.path())
            .unwrap();
        let next = tempfile::Builder::new()
            .prefix("setup-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(root.path())
            .unwrap();
        recovery::save(first.path(), &reference()).unwrap();
        let mut page = recovery::discover(next.path()).unwrap().page();
        restrict_page(&recipe(), &mut page);
        assert_eq!(page.references.len(), 1);
        assert_eq!(page.references[0].proposal.get(), PROPOSAL);
        let file = std::fs::read_dir(first.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = std::fs::read_to_string(file).unwrap();
        assert!(
            !bytes.contains("proof") && !bytes.contains("approved") && !bytes.contains("cookie")
        );
        assert_eq!(parse::<RecoveryReference>(&bytes).unwrap().schema, 2);
    }

    const OUTCOME_OBSERVATION: &str = r#"{ "status":"unknown", "revision":184467440737095516161 }"#;
    fn outcome_envelope(outcome: Option<&str>) -> String {
        let extra = outcome.map(|value| format!(",\"submission_outcome\":{value}")).unwrap_or_default();
        format!("{{\"protocol\":1,\"host\":{},\"observation\":{OUTCOME_OBSERVATION}{extra}}}", serde_json::to_string(&host()).unwrap())
    }
    fn known_outcome() -> &'static str {
        r#"{"change_id":"custom:delta-7","disposition":"known"}"#
    }
    #[derive(Clone)]
    struct OutcomeRequest {
        method: axum::http::Method,
        path: String,
        offer: Option<String>,
        private_session: bool,
        same_origin: bool,
        body: Vec<u8>,
    }
    struct OutcomePeer {
        origin: String,
        replies: Arc<Mutex<std::collections::VecDeque<(u16, Option<String>)>>>,
        requests: Arc<Mutex<Vec<OutcomeRequest>>>,
        stop: oneshot::Sender<()>,
        task: tokio::task::JoinHandle<()>,
    }
    impl OutcomePeer {
        async fn new(reply: Option<String>) -> Self {
            use axum::response::IntoResponse;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let replies = Arc::new(Mutex::new(std::collections::VecDeque::from([(200, reply)])));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let (recorded, queued, expected_origin) = (requests.clone(), replies.clone(), origin.clone());
            let app = axum::Router::new().fallback(move |method: axum::http::Method, uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let (recorded, queued, expected_origin) = (recorded.clone(), queued.clone(), expected_origin.clone());
                async move {
                    recorded.lock().unwrap().push(OutcomeRequest {
                        method, path: uri.path().into(),
                        offer: headers.get(SUBMISSION_OUTCOME_HEADER).map(|v| v.to_str().unwrap().to_owned()),
                        private_session: headers.get("cookie").is_some_and(|v| v == "fixture=session"),
                        same_origin: headers.get("origin").is_some_and(|v| v == expected_origin.as_str()),
                        body: body.to_vec(),
                    });
                    let (status, body) = queued.lock().unwrap().pop_front().expect("one reply per entered request");
                    let body = body.map_or_else(|| axum::body::Body::from_stream(futures_util::stream::once(async {
                        Err::<axum::body::Bytes, _>(std::io::Error::other("fixture lost reply"))
                    })), axum::body::Body::from);
                    (axum::http::StatusCode::from_u16(status).unwrap(), [("content-type", "application/json"), ("cache-control", "no-store")], body).into_response()
                }
            });
            let (stop, receive) = oneshot::channel();
            let task = tokio::spawn(async move {
                axum::serve(listener, app).with_graceful_shutdown(async { let _ = receive.await; }).await.unwrap();
            });
            Self { origin, replies, requests, stop, task }
        }
        fn push(&self, body: String) { self.replies.lock().unwrap().push_back((200, Some(body))); }
        fn client(&self) -> ForgeClient {
            let mut client = ForgeClient::new(&self.origin).unwrap();
            client.session = Some(HeaderValue::from_static("fixture=session"));
            client
        }
        async fn finish(self) -> Vec<OutcomeRequest> {
            let _ = self.stop.send(());
            self.task.await.unwrap();
            let requests = self.requests.lock().unwrap().clone();
            requests
        }
    }
    fn outcome_front(peer: &OutcomePeer) -> (ForgeFrontend, ForgePresenter, Arc<Prepared>, tempfile::TempDir) {
        let (front, presenter) = ForgeFrontend::for_application(host(), fields()).unwrap();
        *front.binding.state.lock().unwrap() = State::Captured { _captured: Captured {
            client: peer.client(), support: ForgeSupport::Declared { host: host() },
        }};
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
        let artifacts = directory.path().join("setup");
        std::fs::create_dir(&artifacts).unwrap();
        std::fs::set_permissions(&artifacts, std::fs::Permissions::from_mode(0o700)).unwrap();
        front.artifacts(&artifacts).unwrap();
        let mut selected = reference();
        selected.origin = peer.origin.clone();
        let prepared = Arc::new(Prepared { reference: selected, expires_at: now()+120, proof: "c".repeat(64) });
        {
            let mut state = front.binding.configuration.lock().unwrap();
            state.inventory = Some(json!({"old_revision":1}));
            state.references.insert(prepared.reference.change_id.clone(), prepared.reference.clone());
            state.prepared.insert(prepared.reference.change_id.clone(), prepared.clone());
            state.spent_calls.insert("older-call".into());
            state.submitted.insert("older-change".into());
        }
        let args = RawValue::from_string(json!({"change_id":prepared.reference.change_id}).to_string()).unwrap();
        let held = front.retain_action(ToolCallView { id:"current-submit", name:FORGE_SUBMIT, args:&args }).unwrap();
        front.arm_action(held).unwrap();
        (front, presenter, prepared, directory)
    }
    async fn outcome_dispatch(front: &ForgeFrontend, presenter: &mut ForgePresenter, name: &str, call_id: &str, args: &RawValue) -> Option<Value> {
        let tool = front.dispatch_configuration(ToolCallView { id:call_id, name, args });
        let present = async {
            let ForgePrompt::Operation(mut operation) = presenter.next().await.unwrap() else { panic!("expected operation") };
            let result = operation.perform().await;
            let projection = result.as_ref().ok().cloned();
            let _ = operation.response.take().unwrap().send(result);
            drop(operation);
            projection
        };
        let (tool, projection) = tokio::join!(tool, present);
        assert_eq!(tool.is_ok(), projection.is_some());
        projection
    }

    #[tokio::test]
    async fn generic_submit_known_disposition_requires_fresh_inventory_and_keeps_spent_approvals() {
        for outcome in [None, Some(known_outcome()), Some(r#"{"change_id":"custom:delta-7","disposition":"unknown"}"#)] {
            let known = outcome == Some(known_outcome());
            let peer = OutcomePeer::new(Some(outcome_envelope(outcome))).await;
            let (front, mut presenter, prepared, _directory) = outcome_front(&peer);
            let args = RawValue::from_string(r#"{"change_id":"custom:delta-7"}"#.into()).unwrap();
            let reply = outcome_dispatch(&front, &mut presenter, FORGE_SUBMIT, "current-submit", &args).await.unwrap();
            assert_eq!(reply["observation_json"].as_str(), Some(OUTCOME_OBSERVATION));
            assert_eq!(reply["recovery_only"], !known);
            assert_eq!(reply.get("submission_outcome").is_some(), outcome.is_some());
            {
                let state = front.binding.configuration.lock().unwrap();
                assert_eq!(state.uncertain.is_none(), known);
                assert_eq!(state.inventory.is_none(), known);
                assert!(state.submitted.contains("custom:delta-7") && state.submitted.contains("older-change"));
                assert!(state.spent_calls.contains("current-submit") && state.spent_calls.contains("older-call"));
                assert!(Arc::ptr_eq(&state.prepared["custom:delta-7"], &prepared));
                assert_eq!(state.references["custom:delta-7"].proposal.get(), PROPOSAL);
                assert!(state.armed.is_none() && !state.busy);
            }
            for call_id in ["current-submit", "new-call-old-change"] {
                assert!(front.retain_action(ToolCallView { id:call_id, name:FORGE_SUBMIT, args:&args }).is_err());
            }
            let prepare_args = RawValue::from_string(r#"{"configuration":{},"expected_revision":null}"#.into()).unwrap();
            assert!(front.dispatch_configuration(ToolCallView { id:"no-new-inventory", name:FORGE_PREPARE, args:&prepare_args }).await.is_err());
            let empty = RawValue::from_string("{}".into()).unwrap();
            if known {
                peer.push(outcome_envelope(None));
                assert!(front.dispatch_configuration(ToolCallView { id:"fresh-inventory", name:FORGE_INSPECT, args:&empty }).await.is_ok());
                let proposal = r#"{"change_id":"next-change","configuration":{},"expected_revision":null}"#;
                peer.push(format!("{{\"protocol\":1,\"host\":{},\"proposal\":{proposal},\"approval\":{{\"expires_at\":{},\"proof\":\"{}\"}}}}", serde_json::to_string(&host()).unwrap(), now()+120, "d".repeat(64)));
                assert!(outcome_dispatch(&front, &mut presenter, FORGE_PREPARE, "fresh-prepare", &prepare_args).await.is_some());
                let next = RawValue::from_string(r#"{"change_id":"next-change"}"#.into()).unwrap();
                assert!(front.retain_action(ToolCallView { id:"new-review", name:FORGE_SUBMIT, args:&next }).is_ok());
                assert!(front.retain_action(ToolCallView { id:"current-submit", name:FORGE_SUBMIT, args:&next }).is_err());
                assert!(front.dispatch_configuration(ToolCallView { id:"unapproved-next", name:FORGE_SUBMIT, args:&next }).await.is_err());
            } else {
                assert!(front.dispatch_configuration(ToolCallView { id:"restricted-inspect", name:FORGE_INSPECT, args:&empty }).await.is_err());
                peer.push(outcome_envelope(None));
                assert!(front.dispatch_configuration(ToolCallView { id:"exact-status", name:FORGE_STATUS, args:&args }).await.is_ok());
                assert_eq!(front.binding.configuration.lock().unwrap().uncertain, Some(Some("custom:delta-7".into())));
            }
            let calls = peer.finish().await;
            assert_eq!(calls.len(), if known {3} else {2});
            assert_eq!(calls[0].method, axum::http::Method::POST);
            assert_eq!(calls[0].path, format!("{SUPPORT_PATH}/submit"));
            assert_eq!(calls[0].offer.as_deref(), Some("1"));
            assert!(calls[0].private_session && calls[0].same_origin);
            let body: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(&calls[0].body).unwrap();
            assert_eq!(body["proposal"].get(), PROPOSAL);
            assert!(calls[1..].iter().all(|call| call.offer.is_none()));
        }
    }

    #[tokio::test]
    async fn generic_submit_bad_or_lost_outcomes_never_settle_current_uncertainty() {
        let mut replies = vec![None];
        for outcome in ["null", "{}", r#"{"change_id":"foreign","disposition":"known"}"#,
            r#"{"change_id":"","disposition":"known"}"#, r#"{"change_id":"custom:delta-7","disposition":"committed"}"#,
            r#"{"change_id":"custom:delta-7","disposition":true}"#, r#"{"change_id":"custom:delta-7","disposition":null}"#,
            r#"{"change_id":"custom:delta-7","disposition":"known","extra":0}"#,
            r#"{"change_id":"custom:delta-7","disposition":"known","disposition":"unknown"}"#] {
            replies.push(Some(outcome_envelope(Some(outcome))));
        }
        replies.push(Some(outcome_envelope(Some(known_outcome())).replacen("\"protocol\":1", "\"protocol\":1,\"submission_outcome\":null", 1)));
        replies.push(Some(outcome_envelope(Some(known_outcome())).replace(&"b".repeat(64), &"d".repeat(64))));
        replies.push(Some(outcome_envelope(Some(known_outcome())).replace(OUTCOME_OBSERVATION, r#"{"PRIVATE_TOKEN":"literal"}"#)));
        for reply in replies {
            let peer = OutcomePeer::new(reply).await;
            let (front, mut presenter, _prepared, _directory) = outcome_front(&peer);
            let args = RawValue::from_string(r#"{"change_id":"custom:delta-7"}"#.into()).unwrap();
            assert!(outcome_dispatch(&front, &mut presenter, FORGE_SUBMIT, "current-submit", &args).await.is_none());
            {
                let state = front.binding.configuration.lock().unwrap();
                assert_eq!(state.uncertain, Some(Some("custom:delta-7".into())));
                assert!(state.submitted.contains("custom:delta-7") && state.spent_calls.contains("current-submit"));
                assert!(state.inventory.is_some() && state.armed.is_none() && !state.busy);
            }
            let calls = peer.finish().await;
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].offer.as_deref(), Some("1"));
        }
    }

    #[tokio::test]
    async fn generic_outcome_is_rejected_without_exact_submit_negotiation() {
        for (method, path, body) in [
            (reqwest::Method::GET, SUPPORT_PATH.to_owned(), None),
            (reqwest::Method::GET, format!("{SUPPORT_PATH}/changes/custom:delta-7"), None),
            (reqwest::Method::POST, format!("{SUPPORT_PATH}/prepare"), Some("{}".into())),
            (reqwest::Method::POST, format!("{SUPPORT_PATH}/submit"), Some("{}".into())),
        ] {
            let peer = OutcomePeer::new(Some(outcome_envelope(Some(known_outcome())))).await;
            assert!(peer.client().management(method, &path, body).await.is_err());
            let calls = peer.finish().await;
            assert_eq!(calls.len(), 1);
            assert!(calls[0].offer.is_none());
        }
        // The legacy named Reading composition must not offer a new generic contract.
        let peer = OutcomePeer::new(Some(outcome_envelope(Some(known_outcome())))).await;
        let (front, _presenter) = ForgeFrontend::new();
        let mut operation = ForgeOperation { binding:front.binding.clone(), client:peer.client(), host:host(),
            request:Some(OperationRequest::Submit(prepared())), entered:false, response:None };
        assert!(operation.perform().await.is_err());
        drop(operation);
        assert!(peer.finish().await[0].offer.is_none());
    }

    #[test]
    fn generic_known_outcome_cannot_settle_foreign_or_recovered_custody() {
        let outcome: SubmissionOutcome = parse(known_outcome()).unwrap();
        let raw = RawValue::from_string(OUTCOME_OBSERVATION.into()).unwrap();
        for variant in 0..7 {
            let mut state = ConfigurationState::default();
            state.busy = true;
            state.uncertain = Some(Some("custom:delta-7".into()));
            state.submitted.insert("custom:delta-7".into());
            state.inventory = Some(json!({"revision":1}));
            match variant {
                0 => state.busy = false,
                1 => state.uncertain = Some(Some("another-change".into())),
                2 => state.recovered = Some(reference()),
                3 => state.auth_required = true,
                4 => { state.submitted.clear(); },
                5 => state.armed = Some(HeldForgeAction { call_id:"another".into(), arguments:"{}".into(), prepared:prepared() }),
                6 => state.uncertain = None,
                _ => unreachable!(),
            }
            let before = state.uncertain.clone();
            assert!(submission_reply(&recipe(), &host(), &reference(), &raw, Some(&outcome), &mut state).is_err());
            assert_eq!(state.uncertain, before);
            assert!(state.inventory.is_some());
        }
    }
}
