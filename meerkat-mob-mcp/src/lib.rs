use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use meerkat_mob::Prefab;
use meerkat_mob::{ids::MobId, MobBuilder, MobHandle, MobStorage, MobState};
use meerkat_mob::store::InMemoryMobEventStore;
use meerkat_store::MemoryStore;
use serde_json::to_value;

pub struct MobMcpState {
    mobs: RwLock<std::collections::BTreeMap<String, MobEntry>>,
    _storage_root: PathBuf,
}

#[derive(Clone)]
pub(crate) struct MobEntry {
    handle: MobHandle,
    storage: MobStorage,
}

impl MobMcpState {
    pub fn new(storage_root: PathBuf) -> Self {
        Self {
            mobs: RwLock::new(std::collections::BTreeMap::new()),
            _storage_root: storage_root,
        }
    }

    pub async fn insert(&self, id: String, handle: MobHandle, storage: MobStorage) {
        self.mobs
            .write()
            .await
            .insert(id, MobEntry { handle, storage });
    }

    pub async fn get(&self, id: &str) -> Option<MobHandle> {
        self.mobs.read().await.get(id).map(|entry| entry.handle.clone())
    }

    pub(crate) async fn get_entry(&self, id: &str) -> Option<MobEntry> {
        self.mobs.read().await.get(id).cloned()
    }

    pub(crate) async fn remove(&self, id: &str) -> Option<MobEntry> {
        self.mobs.write().await.remove(id)
    }

}

#[derive(Serialize, Deserialize)]
struct RpcRequest {
    id: Value,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub struct RpcResponse {
    jsonrpc: String,
    id: Value,
    result: Option<Value>,
    error: Option<Value>,
}

fn respond_err(message: impl Into<String>) -> Value {
    json!({"error": {"message": message.into()}})
}

pub async fn handle_tool_call(state: &MobMcpState, method: &str, params: Option<Value>) -> Value {
    match method {
        "mob_create" => create_mob(state, params).await,
        "mob_prefabs" => mob_prefabs().await,
        "mob_stop" => mob_stop(state, params).await,
        "mob_resume" => mob_resume(state, params).await,
        "mob_destroy" => mob_destroy(state, params).await,
        "mob_complete" => mob_complete(state, params).await,
        "mob_list" => mob_list(state).await,
        "mob_status" => mob_status(state, params).await,
        "mob_spawn" => mob_spawn(state, params).await,
        "mob_retire" => mob_retire(state, params).await,
        "mob_wire" => mob_wire(state, params).await,
        "mob_unwire" => mob_unwire(state, params).await,
        "mob_list_meerkats" => mob_list_meerkats(state, params).await,
        "mob_turn" => mob_external_turn(state, params).await,
        "mob_external_turn" => mob_external_turn(state, params).await,
        "mob_events" => mob_events(state, params).await,
        _ => respond_err("unknown method"),
    }
}

pub async fn handle_rpc_line(state: &MobMcpState, line: &str) -> Value {
    match serde_json::from_str::<RpcRequest>(line) {
        Ok(payload) => {
            let result = handle_tool_call(state, &payload.method, payload.params).await;
            if result.get("error").is_some() {
                json!(RpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: payload.id,
                    result: None,
                    error: result.get("error").cloned(),
                })
            } else {
                json!(RpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: payload.id,
                    result: Some(result),
                    error: None,
                })
            }
        }
        Err(err) => json!(RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::Null,
            result: None,
            error: Some(json!({"message": err.to_string()})),
        }),
    }
}

#[derive(Serialize, Deserialize)]
struct CreateParams {
    mob_id: String,
    definition: Option<String>,
    definition_file: Option<String>,
    prefab: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct MobRef {
    mob_id: String,
}

#[derive(Serialize, Deserialize)]
struct SpawnParams {
    mob_id: String,
    profile: String,
    key: String,
    message: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TurnParams {
    mob_id: String,
    meerkat_id: String,
    message: String,
}

#[derive(Serialize, Deserialize)]
struct WireParams {
    mob_id: String,
    a: String,
    b: String,
}

#[derive(Serialize, Deserialize)]
struct EventsParams {
    mob_id: String,
    after_cursor: Option<u64>,
    limit: Option<usize>,
}

fn resolve_prefab(name: &str) -> Option<meerkat_mob::MobDefinition> {
    match name.to_lowercase().replace('_', "-").as_str() {
        "coding-swarm" => Some(Prefab::CodingSwarm.definition()),
        "code-review" => Some(Prefab::CodeReview.definition()),
        "research-team" => Some(Prefab::ResearchTeam.definition()),
        "pipeline" => Some(Prefab::Pipeline.definition()),
        _ => None,
    }
}

fn toml_from_file(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

async fn create_mob(state: &MobMcpState, params: Option<Value>) -> Value {
    let params = match params.and_then(|raw| serde_json::from_value::<CreateParams>(raw).ok()) {
        Some(p) => p,
        None => return respond_err("invalid params"),
    };

    if state.get(&params.mob_id).await.is_some() {
        return respond_err("mob already exists");
    }

    let definition: meerkat_mob::MobDefinition = if let Some(definition_toml) = params.definition {
        match meerkat_mob::MobDefinition::from_toml(&definition_toml) {
            Ok(def) => def,
            Err(err) => return respond_err(format!("invalid definition: {err}")),
        }
    } else if let Some(path) = params.definition_file {
        match toml_from_file(Path::new(&path)) {
            Ok(raw) => match meerkat_mob::MobDefinition::from_toml(&raw) {
                Ok(def) => def,
                Err(err) => return respond_err(format!("invalid definition file: {err}")),
            },
            Err(err) => return respond_err(format!("could not read definition_file: {err}")),
        }
    } else if let Some(prefab_name) = params.prefab {
        match resolve_prefab(&prefab_name) {
            Some(def) => def,
            None => return respond_err("unknown prefab"),
        }
    } else {
        return respond_err("missing definition, definition_file, or prefab");
    };

    let storage = MobStorage::new(
        Arc::new(MemoryStore::default()),
        InMemoryMobEventStore::new(MobId::from(params.mob_id.as_str())),
    );

    let handle = match MobBuilder::new(definition, storage.clone()).create().await {
        Ok(handle) => handle,
        Err(err) => return respond_err(format!("{}", err)),
    };

    state
        .insert(params.mob_id.clone(), handle, storage)
        .await;

    json!({"mob_id": params.mob_id})
}

async fn mob_prefabs() -> Value {
    let prefabs = Prefab::list()
        .into_iter()
        .map(|prefab| {
            json!({
                "name": prefab.name,
                "description": prefab.description,
            })
        })
        .collect::<Vec<_>>();
    json!({ "prefabs": prefabs })
}

async fn mob_stop(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    match handle.stop().await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_resume(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };

    let entry = match state.get_entry(&id).await {
        Some(entry) => entry,
        None => return respond_err("unknown mob"),
    };

    if entry.handle.status() == MobState::Completed {
        return respond_err("cannot resume completed mob");
    }

    if entry.handle.status() == MobState::Stopped {
        return json!({"ok":true});
    }

    let new_handle = match MobBuilder::for_resume(entry.storage.clone())
        .resume()
        .await
    {
        Ok(handle) => handle,
        Err(err) => return respond_err(format!("{err}")),
    };

    state
        .insert(id, new_handle, entry.storage)
        .await;

    json!({"ok": true})
}

async fn mob_destroy(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(entry) = state.remove(&id).await else {
        return respond_err("unknown mob");
    };
    match entry.handle.destroy().await {
        Ok(_) => json!({"ok":true}),
        Err(err) => {
            state.insert(id, entry.handle.clone(), entry.storage).await;
            respond_err(err.to_string())
        }
    }
}

async fn mob_complete(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    match handle.complete().await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_list(state: &MobMcpState) -> Value {
    let mobs = state
        .mobs
        .read()
        .await
        .iter()
        .map(|(id, entry)| {
            json!({
                "mob_id": id,
                "status": format!("{:?}", entry.handle.status()),
            })
        })
        .collect::<Vec<_>>();
    json!({"mobs": mobs})
}

async fn mob_status(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    let meerkats = handle
        .list_meerkats(None)
        .into_iter()
        .map(|entry| {
            json!({
                "meerkat_id": entry.meerkat_id.as_str(),
                "profile": entry.profile.as_str(),
                "session_id": entry.session_id.to_string(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": format!("{:?}", handle.status()),
        "meerkats": meerkats,
    })
}

async fn mob_spawn(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<SpawnParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle
        .spawn(&p.profile.into(), p.key.into(), p.message)
        .await
    {
        Ok(id) => json!({"meerkat_id": id.as_str()}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_retire(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<TurnParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle.retire(&p.meerkat_id.into()).await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_wire(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<WireParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle
        .wire(&p.a.into(), &p.b.into())
        .await
    {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_unwire(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<WireParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle
        .unwire(&p.a.into(), &p.b.into())
        .await
    {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_list_meerkats(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(v) => v.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    let meerkats = handle
        .list_meerkats(None)
        .into_iter()
        .map(|entry| {
            json!({
                "meerkat_id": entry.meerkat_id.as_str(),
                "profile": entry.profile.as_str(),
                "session_id": entry.session_id.to_string(),
            })
        })
        .collect::<Vec<_>>();
    json!({"meerkats": meerkats})
}

async fn mob_external_turn(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<TurnParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle
        .external_turn(&p.meerkat_id.into(), p.message)
        .await
    {
        Ok(run) => json!({"text": run.text}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_events(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<EventsParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    let events = match handle.poll_events(p.after_cursor, p.limit).await {
        Ok(v) => v,
        Err(err) => return respond_err(err.to_string()),
    };
    let serialized = events
        .into_iter()
        .map(|event| to_value(event).unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    json!({"events": serialized})
}
