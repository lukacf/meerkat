//! Inert HTTP peer for the factory-built native Anthropic client in tests.
//! This supplies only transport responses. It creates no client, credential
//! lifecycle event, controller pin, grant, invocation or admission witness.

use std::{future::Future, io, pin::Pin, sync::{Arc, Mutex}, time::Duration};
use axum::{Router, body::Bytes, extract::State,
    http::{HeaderMap, Method, StatusCode, Uri}, response::IntoResponse};
use meerkat_core::{ConfigDelta, ConfigStore, FileConfigStore, ResolvedStorage};
use serde_json::{Value, json};
use tokio::sync::oneshot;

const KEY: &str = "synthetic-native-model-peer-key";
const BINDING: &str = "fixture";

pub type Reply = Pin<Box<dyn Future<Output = io::Result<String>> + Send>>;
pub type Handler = Arc<dyn Fn(Value) -> Reply + Send + Sync>;

#[derive(Clone)]
struct PeerState {
    authority: String,
    model: String,
    handler: Handler,
    failures: Arc<Mutex<Vec<&'static str>>>,
}

pub struct ModelPeer {
    origin: String,
    state: PeerState,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<io::Result<()>>,
}

impl ModelPeer {
    pub async fn open(model: &str, handler: Handler) -> io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let authority = listener.local_addr()?.to_string();
        let origin = format!("http://{authority}");
        let state = PeerState { authority, model: model.into(), handler,
            failures: Default::default() };
        let app = Router::new().fallback(http).with_state(state.clone());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async { let _ = stopped.await; }).await
        });
        Ok(Self { origin, state, stop, task })
    }

    /// Patch the isolated realm's own document. The explicit binding below
    /// selects this inline fixture key and loopback endpoint, never an account
    /// inherited from the user's environment or another realm.
    pub async fn configure(&self, storage: &ResolvedStorage) -> io::Result<()> {
        let global = storage.layout.user_rkat_root()
            .ok_or_else(|| io::Error::other("Fixture has no isolated config root"))?
            .join("config.toml");
        let source = meerkat_store::FilesystemRealmConfigSource::new(
            &storage.locator.state_root, global, meerkat_models::canonical());
        let path = source.config_doc_path(&storage.locator.realm);
        let store = FileConfigStore::new(path, meerkat_models::canonical());
        store.patch(ConfigDelta(json!({
            "agent": {"model": self.state.model},
            "realm": {(storage.locator.realm.as_str()): {
                "default_binding": BINDING,
                "backend": {(BINDING): {"provider":"anthropic",
                    "backend_kind":"anthropic_api", "base_url":self.origin}},
                "auth": {(BINDING): {"provider":"anthropic", "auth_method":"api_key",
                    "source":{"kind":"inline_secret", "secret":KEY}}},
                "binding": {(BINDING): {"backend_profile":BINDING,
                    "auth_profile":BINDING, "default_model":self.state.model}}
            }}
        }))).await.map_err(io::Error::other)?;
        Ok(())
    }

    /// Close only after the caller has settled its native owner and released
    /// any scenario-held reply. A timeout is reported after joining this same
    /// server task; it never substitutes an abort for graceful completion.
    pub async fn finish(self) -> io::Result<()> {
        let Self { stop, mut task, state, .. } = self;
        let _ = stop.send(());
        let joined = match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
            Ok(result) => result.map_err(io::Error::other)?,
            Err(error) => {
                task.await.map_err(io::Error::other)??;
                return Err(io::Error::other(error));
            }
        };
        joined?;
        let failures = state.failures.lock().unwrap();
        if !failures.is_empty() {
            return Err(io::Error::other(format!(
                "The native model peer refused an unexpected request at {}", failures.join(", "))));
        }
        Ok(())
    }
}

pub fn model_request(realm: &str, model: &str)
    -> io::Result<meerkat::CreateSessionModelResolutionRequest>
{
    Ok(meerkat::CreateSessionModelResolutionRequest {
        model: Some(model.into()), provider: Some(meerkat_core::Provider::Anthropic),
        auth_binding: Some(meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse(realm).map_err(io::Error::other)?,
            binding: meerkat_core::BindingId::parse(BINDING).map_err(io::Error::other)?,
            profile: None, origin: meerkat_core::BindingOrigin::Configured,
        }),
        ..Default::default()
    })
}

async fn http(State(state): State<PeerState>, method: Method, uri: Uri,
    headers: HeaderMap, body: Bytes) -> axum::response::Response
{
    let refused = |stage| {
        state.failures.lock().unwrap().push(stage);
        (StatusCode::BAD_REQUEST, "Unexpected native model fixture request").into_response()
    };
    if method != Method::POST || uri.path() != "/v1/messages" || uri.query().is_some()
        || headers.get("host").and_then(|value| value.to_str().ok()) != Some(state.authority.as_str())
        || headers.get("x-api-key").and_then(|value| value.to_str().ok()) != Some(KEY)
        || headers.contains_key("authorization")
    { return refused("route-or-credential"); }
    let Ok(body) = serde_json::from_slice::<Value>(&body) else { return refused("json"); };
    if body["model"].as_str() != Some(state.model.as_str()) || body["stream"] != true
        || !body["messages"].is_array()
    { return refused("model-or-messages"); }
    match (state.handler)(body).await {
        Ok(reply) => (StatusCode::OK, [("content-type", "text/event-stream")], reply).into_response(),
        Err(_) => refused("scenario"),
    }
}

pub fn text_reply(text: &str) -> String {
    response([
        json!({"type":"content_block_start", "index":0,
            "content_block":{"type":"text", "text":""}}),
        json!({"type":"content_block_delta", "index":0,
            "delta":{"type":"text_delta", "text":text}}),
    ], "end_turn")
}

pub fn tool_reply(id: &str, name: &str, args: &Value) -> String {
    response([
        json!({"type":"content_block_start", "index":0,
            "content_block":{"type":"tool_use", "id":id, "name":name, "input":{}}}),
        json!({"type":"content_block_delta", "index":0,
            "delta":{"type":"input_json_delta", "partial_json":args.to_string()}}),
    ], "tool_use")
}

fn response(content: [Value; 2], stop: &str) -> String {
    let events = std::iter::once(json!({"type":"message_start",
        "message":{"usage":{"input_tokens":1, "output_tokens":0}}}))
        .chain(content).chain([
            json!({"type":"content_block_stop", "index":0}),
            json!({"type":"message_delta", "usage":{"output_tokens":1},
                "delta":{"stop_reason":stop}}),
            json!({"type":"message_stop"}),
        ]);
    events.map(|event| format!("data: {event}\n\n")).collect()
}
