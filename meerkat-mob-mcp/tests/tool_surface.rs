use serde_json::json;

use meerkat_mob_mcp::{MobMcpState, handle_tool_call};

#[tokio::test]
async fn mob_prefabs_lists_available_prefabs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = MobMcpState::new(temp.path().to_path_buf()).await;
    let result = handle_tool_call(&state, "mob_prefabs", None).await;
    let prefabs = result
        .get("prefabs")
        .and_then(|v| v.as_array())
        .expect("prefabs array");
    assert!(
        prefabs.iter().any(|entry| {
            entry
                .get("name")
                .and_then(|name| name.as_str())
                .is_some_and(|name| name == "coding-swarm")
        }),
        "expected coding-swarm in prefab list"
    );
}

#[tokio::test]
async fn mob_turn_alias_routes_to_external_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = MobMcpState::new(temp.path().to_path_buf()).await;
    let create = handle_tool_call(
        &state,
        "mob_create",
        Some(json!({
            "mob_id": "mcp-turn-mob",
            "prefab": "coding-swarm"
        })),
    )
    .await;
    assert!(create.get("error").is_none(), "create error: {create}");

    let spawn = handle_tool_call(
        &state,
        "mob_spawn",
        Some(json!({
            "mob_id": "mcp-turn-mob",
            "profile": "coordinator",
            "key": "coord-1"
        })),
    )
    .await;
    let meerkat_id = spawn
        .get("meerkat_id")
        .and_then(|v| v.as_str())
        .expect("spawn returned meerkat_id");

    let turn = handle_tool_call(
        &state,
        "mob_turn",
        Some(json!({
            "mob_id": "mcp-turn-mob",
            "meerkat_id": meerkat_id,
            "message": "hello"
        })),
    )
    .await;
    assert!(turn.get("error").is_none(), "mob_turn error: {turn}");
    assert!(
        turn.get("text")
            .and_then(|v| v.as_str())
            .is_some_and(|text| text.contains("hello")),
        "unexpected turn response: {turn}"
    );
}

#[tokio::test]
async fn choke_mob_007_mcp_tool_handler_maps_to_mob_handle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = MobMcpState::new(temp.path().to_path_buf()).await;
    let created = handle_tool_call(
        &state,
        "mob_create",
        Some(json!({
            "mob_id": "choke-7",
            "prefab": "coding-swarm"
        })),
    )
    .await;
    assert!(created.get("error").is_none(), "create error: {created}");

    let spawned = handle_tool_call(
        &state,
        "mob_spawn",
        Some(json!({
            "mob_id": "choke-7",
            "profile": "coordinator",
            "key": "coordinator-1"
        })),
    )
    .await;
    assert!(spawned.get("error").is_none(), "spawn error: {spawned}");

    let listed = handle_tool_call(
        &state,
        "mob_list_meerkats",
        Some(json!({"mob_id":"choke-7"})),
    )
    .await;
    let meerkats = listed
        .get("meerkats")
        .and_then(|v| v.as_array())
        .expect("meerkats list");
    assert_eq!(meerkats.len(), 1);
}

#[tokio::test]
async fn e2e_mob_006_and_007_mcp_roundtrip() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = MobMcpState::new(temp.path().to_path_buf()).await;
    let create = handle_tool_call(
        &state,
        "mob_create",
        Some(json!({
            "mob_id": "mcp-e2e",
            "prefab": "coding-swarm"
        })),
    )
    .await;
    assert!(create.get("error").is_none(), "create error: {create}");

    let a = handle_tool_call(
        &state,
        "mob_spawn",
        Some(json!({
            "mob_id": "mcp-e2e",
            "profile": "coordinator",
            "key": "a"
        })),
    )
    .await;
    let b = handle_tool_call(
        &state,
        "mob_spawn",
        Some(json!({
            "mob_id": "mcp-e2e",
            "profile": "developer",
            "key": "b"
        })),
    )
    .await;
    let a_id = a.get("meerkat_id").and_then(|v| v.as_str()).expect("a id");
    let b_id = b.get("meerkat_id").and_then(|v| v.as_str()).expect("b id");

    let wire = handle_tool_call(
        &state,
        "mob_wire",
        Some(json!({"mob_id":"mcp-e2e","a":a_id,"b":b_id})),
    )
    .await;
    assert!(wire.get("error").is_none(), "wire error: {wire}");

    let stop = handle_tool_call(&state, "mob_stop", Some(json!({"mob_id":"mcp-e2e"}))).await;
    assert!(stop.get("error").is_none(), "stop error: {stop}");
    let resume = handle_tool_call(&state, "mob_resume", Some(json!({"mob_id":"mcp-e2e"}))).await;
    assert!(resume.get("error").is_none(), "resume error: {resume}");

    let listed = handle_tool_call(
        &state,
        "mob_list_meerkats",
        Some(json!({"mob_id":"mcp-e2e"})),
    )
    .await;
    let meerkats = listed
        .get("meerkats")
        .and_then(|v| v.as_array())
        .expect("meerkats");
    assert_eq!(meerkats.len(), 2);

    let turn = handle_tool_call(
        &state,
        "mob_external_turn",
        Some(json!({"mob_id":"mcp-e2e","meerkat_id":a_id,"message":"ping"})),
    )
    .await;
    assert!(turn.get("error").is_none(), "turn error: {turn}");
    let events = handle_tool_call(&state, "mob_events", Some(json!({"mob_id":"mcp-e2e"}))).await;
    assert!(
        events
            .get("events")
            .and_then(|v| v.as_array())
            .is_some_and(|arr| !arr.is_empty()),
        "events should not be empty: {events}"
    );
}

#[tokio::test]
async fn mob_state_persists_across_state_reloads() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_one = MobMcpState::new(temp.path().to_path_buf()).await;
    let created = handle_tool_call(
        &state_one,
        "mob_create",
        Some(json!({
            "mob_id": "persisted",
            "prefab": "coding-swarm"
        })),
    )
    .await;
    assert!(created.get("error").is_none(), "create error: {created}");

    let first_list = handle_tool_call(&state_one, "mob_list", None).await;
    let first_count = first_list
        .get("mobs")
        .and_then(|v| v.as_array())
        .map(|items| items.len())
        .unwrap_or_default();
    assert_eq!(first_count, 1);

    let state_two = MobMcpState::new(temp.path().to_path_buf()).await;
    let second_list = handle_tool_call(&state_two, "mob_list", None).await;
    let ids = second_list
        .get("mobs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            entry
                .get("mob_id")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
        })
        .collect::<Vec<_>>();
    assert!(ids.iter().any(|id| id == "persisted"));
}
