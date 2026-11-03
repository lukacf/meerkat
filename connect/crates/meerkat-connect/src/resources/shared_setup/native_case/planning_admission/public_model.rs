//! Controlled model decisions; every tool result is produced by the real native
//! dispatcher. The story is read from the actual public MCP response.
use super::*;
use futures_util::StreamExt;
pub(super) const OPERATION: &str = "get_top_hackernews_stories";
const SETUP: &[&str] = &[
    "setup_prepare",
    "setup_help",
    "setup_check",
    "setup_install",
    "setup_job",
    "setup_reconcile",
];
#[derive(Default)]
pub(super) struct PublicScript {
    pub(super) release_b: AtomicBool,
    pub(super) b_context_observed: AtomicBool,
    pub(super) issued: Mutex<BTreeMap<String, Value>>,
    pub(super) observed: Mutex<BTreeMap<String, Value>>,
    pub(super) answer: Mutex<Option<String>>,
    sequence: AtomicUsize,
}
fn users(request: &LlmRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect()
}
fn context(request: &LlmRequest) -> Option<Value> {
    users(request).iter().rev().find_map(|text| {
        let value: Value = serde_json::from_str(text).ok()?;
        (value["purpose"] == "toolkit-workflow-inline-context-v1").then_some(value)
    })
}
impl PublicScript {
    fn call(
        &self,
        request: &LlmRequest,
        key: &str,
        name: &str,
        mut arguments: Value,
    ) -> Vec<LlmEvent> {
        if SETUP.contains(&name) {
            let envelope = context(request).expect("actual G context");
            arguments["setup_acquisition"] =
                envelope["membership"]["identity"]["acquisition"]["id"].clone();
            assert!(arguments["setup_acquisition"].is_string());
        }
        assert!(
            request.tools.iter().any(|tool| tool.name == name),
            "actual native surface lacks {name}"
        );
        assert!(
            self.issued
                .lock()
                .unwrap()
                .insert(key.into(), json!({"name":name,"arguments":arguments}))
                .is_none(),
            "duplicate controlled effect {key}"
        );
        vec![
            LlmEvent::ToolCallComplete {
                id: key.into(),
                name: name.into(),
                args: arguments,
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
        ]
    }
    fn response(&self, request: &LlmRequest) -> Vec<LlmEvent> {
        let mut results = BTreeMap::new();
        for message in &request.messages {
            if let Message::ToolResults { results: rows, .. } = message {
                for row in rows {
                    if !self.issued.lock().unwrap().contains_key(&row.tool_use_id) {
                        continue;
                    }
                    assert!(
                        !row.is_error,
                        "actual tool {} refused: {}",
                        row.tool_use_id,
                        row.text_content()
                    );
                    let value = if row.tool_use_id == "b-public-read" {
                        let blocks = serde_json::Deserializer::from_str(&row.text_content())
                            .into_iter::<Value>()
                            .collect::<Result<Vec<_>, _>>()
                            .unwrap();
                        assert_eq!(
                            blocks.len(),
                            2,
                            "actual Agno text and structured result pair"
                        );
                        let stories: Value =
                            serde_json::from_str(blocks[1]["value"].as_str().unwrap()).unwrap();
                        assert_eq!(blocks[0], stories);
                        assert_eq!(stories.as_array().unwrap().len(), 1);
                        let story = &stories[0];
                        assert!(story["id"].as_u64().is_some_and(|n| n > 0));
                        assert!(!story["title"].as_str().unwrap().trim().is_empty());
                        json!({"story":story,"content":row.text_content()})
                    } else {
                        serde_json::from_str(&row.text_content()).unwrap()
                    };
                    results.insert(row.tool_use_id.clone(), value.clone());
                    let old = self
                        .observed
                        .lock()
                        .unwrap()
                        .insert(row.tool_use_id.clone(), value.clone());
                    assert!(
                        old.as_ref().is_none_or(|old| old == &value),
                        "native result changed"
                    );
                }
            }
        }
        let texts = users(request);
        let seq = self.sequence.fetch_add(1, Ordering::AcqRel);
        if texts.iter().any(|text| text == P_TEXT) {
            return done("Planning parent is ready.");
        }
        if texts.iter().any(|text| text == G_TEXT) {
            let Some(envelope) = context(request) else {
                return done("Dedicated G awaits native waiter context.");
            };
            if envelope["recipient"]["original"]["route"] == "original-a" {
                return done("A context received; no service effect was requested for A.");
            }
            assert_eq!(envelope["recipient"]["original"]["route"], "original-b");
            assert!(self.release_b.load(Ordering::Acquire));
            let acquisition = envelope["membership"]["identity"]["acquisition"]["id"]
                .as_str()
                .expect("actual delivered membership names acquisition");
            let offered = |name: &str| request.tools.iter().any(|tool| tool.name == name);
            if !SETUP.iter().all(|name| offered(name)) {
                assert!(seq < 150, "bounded native deferred catalog discovery");
                let found = results
                    .iter()
                    .rev()
                    .find(|(key, _)| key.starts_with("g-search-"));
                if let Some((_, found)) = found {
                    let names = found["results"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|row| row["name"].as_str())
                        .filter(|name| SETUP.contains(name))
                        .collect::<BTreeSet<_>>();
                    if names.len() == SETUP.len()
                        && found["pending_sources"]
                            .as_array()
                            .is_none_or(Vec::is_empty)
                    {
                        return self.call(
                            request,
                            &format!("g-load-{seq}"),
                            "tool_catalog_load",
                            json!({"names":SETUP}),
                        );
                    }
                }
                return self.call(
                    request,
                    &format!("g-search-{seq}"),
                    "tool_catalog_search",
                    json!({"query":"setup","limit":50}),
                );
            }
            let selected = json!({"family":"agno","profile":"hackernews","operations":[OPERATION],"values":{"timeout":7}});
            if !results.contains_key("g-prepare") {
                return self.call(
                    request,
                    "g-prepare",
                    "setup_prepare",
                    json!({"family":"agno"}),
                );
            }
            for (key, label) in [("g-prepare", "prepare"), ("g-install", "install")] {
                if let Some(first) = results.get(key) {
                    let job = first["job"]["id"].as_str().unwrap();
                    let latest = results
                        .iter()
                        .filter(|(_, v)| v["job"]["id"] == job)
                        .map(|(_, v)| v)
                        .find(|v| v["job"]["status"] == "succeeded")
                        .unwrap_or(first);
                    assert!(
                        latest["job"]["status"] == "running"
                            || latest["job"]["status"] == "succeeded",
                        "actual {label} failure: {latest}"
                    );
                    if latest["job"]["status"] == "running" {
                        assert!(seq < 1000, "bounded actual worker observation");
                        return self.call(
                            request,
                            &format!("g-{label}-poll-{seq}"),
                            "setup_job",
                            json!({"job_id":job}),
                        );
                    }
                }
            }
            if !results.contains_key("g-help") {
                return self.call(
                    request,
                    "g-help",
                    "setup_help",
                    json!({"family":"agno","profile":"hackernews"}),
                );
            }
            assert_eq!(results["g-help"]["authentication"], json!({"kind":"none"}));
            assert!(
                results["g-help"]["operations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["id"] == OPERATION && row["tool"] == OPERATION)
            );
            if !results.contains_key("g-check") {
                return self.call(request, "g-check", "setup_check", selected);
            }
            let checked = &results["g-check"];
            assert_eq!(checked["scope"], "configuration_only");
            assert_eq!(checked["credential_environment_references"], json!([]));
            if !results.contains_key("g-install") {
                let mut args = selected;
                args["validation_id"] = checked["validation_id"].clone();
                args["configuration_sha256"] = checked["configuration_sha256"].clone();
                args["setup_acquisition"] = json!(acquisition);
                return self.call(request, "g-install", "setup_install", args);
            }
            let job = results["g-install"]["job"]["id"].as_str().unwrap();
            if !results.contains_key("g-reconcile") {
                return self.call(
                    request,
                    "g-reconcile",
                    "setup_reconcile",
                    json!({"job_id":job}),
                );
            }
            assert_eq!(results["g-reconcile"]["observation"]["status"], "matched");
            if !results.contains_key("g-public-access") {
                return self.call(request,"g-public-access",meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST,
                    json!({"job_id":job,"agent":member("original-b").member,"operations":[OPERATION],"expires_in_ms":180_000,
                        "setup_acquisition":acquisition,"tool_policy":{"tools":[],"catalog":"r2"}}));
            }
            let result = &results["g-public-access"];
            assert_eq!(result["grant_issued"], true);
            assert_eq!(result["fresh_instruction_retained"], true);
            assert_eq!(
                result["ready"], false,
                "helper does not assert native attachment or business completion"
            );
            return done(
                "B's reviewed public service instruction is retained; actual use remains B's work.",
            );
        }
        let tag = if texts.iter().any(|text| text.contains("sdk-planning-a")) {
            "a"
        } else {
            "b"
        };
        let key = format!("{tag}-planning-find");
        if !results.contains_key(&key) {
            return self.call(request,&key,toolkit_mobkit::FIND_AND_INSTALL,
                json!({"request":format!("sdk-planning-{tag}: read one public Hacker News story using the packaged integration")}));
        }
        let original_count = texts
            .iter()
            .filter(|text| text.contains(&format!("sdk-planning-{tag}")))
            .count();
        if original_count == 1 {
            return done("The independently authenticated request is waiting for setup.");
        }
        assert_eq!(
            tag, "b",
            "cancelled A cannot receive a fresh ordinary continuation"
        );
        assert_eq!(
            original_count, 2,
            "one original and exactly one fresh B continuation"
        );
        if !results.contains_key("b-search") {
            return self.call(
                request,
                "b-search",
                "tool_catalog_search",
                json!({"query":OPERATION,"limit":50}),
            );
        }
        let found = &results["b-search"];
        assert_eq!(found["catalog_exact"], true);
        assert!(
            found["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == OPERATION)
        );
        if !results.contains_key("b-load") {
            return self.call(
                request,
                "b-load",
                "tool_catalog_load",
                json!({"names":[OPERATION]}),
            );
        }
        if !results.contains_key("b-public-read") {
            return self.call(
                request,
                "b-public-read",
                OPERATION,
                json!({"num_stories":1}),
            );
        }
        let story = &results["b-public-read"]["story"];
        let answer = format!(
            "Hacker News story {}: {}",
            story["id"].as_u64().unwrap(),
            story["title"].as_str().unwrap()
        );
        assert!(
            self.answer
                .lock()
                .unwrap()
                .replace(answer.clone())
                .is_none()
        );
        done(&answer)
    }
}

#[async_trait]
impl LlmClient for PublicScript {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        Ok(())
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(model_selection())
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        PlainModelRoute::new(ENDPOINT, MODEL)
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("governed native request required")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let authorization = request
            .authorization()
            .expect("actual native model authorization");
        let prepared = match authorization.prepare(model_facts()) {
            Ok(value) => value,
            Err(error) => {
                return Box::pin(futures_util::stream::iter([Err(
                    LlmError::from_operation_authorization(error),
                )]));
            }
        };
        let current = match prepared.current() {
            Ok(value) => value,
            Err(error) => {
                return Box::pin(futures_util::stream::iter([Err(
                    LlmError::from_operation_authorization(error),
                )]));
            }
        };
        if let Err(error) =
            current.observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
        {
            return Box::pin(futures_util::stream::iter([Err(
                LlmError::from_operation_observation(error),
            )]));
        }
        if let Err(error) = current.observe_outcome(
            meerkat_core::authorization::OperationObservedOutcome::HttpResponse { status: 200 },
        ) {
            return Box::pin(futures_util::stream::iter([Err(
                LlmError::from_operation_observation(error),
            )]));
        }
        Box::pin(
            futures_util::stream::once(async move {
                if context(request.request())
                    .is_some_and(|value| value["recipient"]["original"]["route"] == "original-b")
                {
                    self.b_context_observed.store(true, Ordering::Release);
                    tokio::time::timeout(PUBLIC_BOUND, async {
                        while !self.release_b.load(Ordering::Acquire) {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .expect("the test must cancel A before releasing B's actual context");
                }
                // This only paces the controlled model's job/catalog observations;
                // it does not retry an effect or claim native readiness.
                tokio::time::sleep(Duration::from_millis(200)).await;
                futures_util::stream::iter(self.response(request.request()).into_iter().map(Ok))
            })
            .flatten(),
        )
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}
