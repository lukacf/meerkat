# Mobs

This is the detailed reference for Meerkat mobs across Rust SDK, CLI, MCP, REST, RPC, Python SDK, and TypeScript SDK.

## Positioning

- Mobs are an optional extension for multi-agent orchestration.
- Base Meerkat workflows remain session/turn centered.
- On CLI, primary mob UX is tool-driven through `run`/`resume`.
- Direct `rkat mob ...` is the explicit operational surface.

## Runtime model

Core entities:

- `Mob`: persisted aggregate (definition, members, status, events).
- `Meerkat member`: spawned runtime participant identified by `meerkat_id`.
- `Profile`: role contract (model/tools/skills posture).
- `Wiring`: peer graph edges.
- `Mob event`: append-only lifecycle records.
- `Flow run` (optional): DAG execution record with step and failure ledgers.

Lifecycle:

1. create
2. spawn
3. wire
4. turn and/or flow runs
5. stop/resume/complete
6. destroy

## Definition model

Common sections:

- `[mob]`
- `[profiles.<name>]`
- `[skills.<name>]` (optional)
- `[wiring]`
- `[topology]` (optional)
- `[supervisor]` (optional)
- `[limits]` (optional)
- `[flows.<flow_id>]` (optional)

Important semantics:

- `orchestrator` chooses the orchestration role.
- `external_addressable` gates external turnability.
- `wiring.auto_wire_orchestrator` and `wiring.role_wiring` shape default graph edges.
- topology rules can enforce strict role-level communication policy.

## Rust SDK (detailed)

Primary crates:

- `meerkat_mob` for runtime and state.
- `meerkat_mob_mcp` for mob tool dispatcher and in-memory state helper.

### Core Rust types

- `MobDefinition`
- `MobStorage`
- `MobBuilder`
- `MobHandle`
- `MobSessionService`
- `MobState`
- `MobRun`, `MobRunStatus`
- `FlowRunConfig`

### `MobBuilder` API

- `MobBuilder::new(definition, storage)`
- `MobBuilder::for_resume(storage)`
- `.with_session_service(Arc<dyn MobSessionService>)`
- `.allow_ephemeral_sessions(bool)`
- `.notify_orchestrator_on_resume(bool)`
- `.register_tool_bundle(name, dispatcher)`
- `.create().await`
- `.resume().await`

### `MobHandle` API

- inspection: `status()`, `definition()`, `roster()`, `list_meerkats()`, `events()`
- membership: `spawn_member_ref*`, `retire`
- graph: `wire`, `unwire`
- turns: `external_turn`, `internal_turn`
- lifecycle: `stop`, `resume`, `complete`, `destroy`
- flows: `list_flows`, `run_flow`, `flow_status`, `cancel_flow`
- tasks: `task_create`, `task_update`, `task_list`, `task_get`
- shutdown: `shutdown`

### Rust example: full lifecycle via `MobBuilder` + `MobHandle`

```rust
use std::sync::Arc;
use meerkat_mob::{
    FlowId, MeerkatId, MobBuilder, MobDefinition, MobSessionService, MobStorage, ProfileName,
};

async fn run_mob(
    definition_toml: &str,
    session_service: Arc<dyn MobSessionService>,
) -> Result<(), Box<dyn std::error::Error>> {
    let definition = MobDefinition::from_toml(definition_toml)?;
    let storage = MobStorage::redb("./mob.redb")?;

    let handle = MobBuilder::new(definition, storage)
        .with_session_service(session_service)
        .create()
        .await?;

    handle
        .spawn_member_ref(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await?;
    handle
        .spawn_member_ref(ProfileName::from("worker"), MeerkatId::from("worker-1"), None)
        .await?;
    handle
        .wire(MeerkatId::from("lead-1"), MeerkatId::from("worker-1"))
        .await?;
    handle
        .external_turn(
            MeerkatId::from("lead-1"),
            "Coordinate a short execution plan.".to_string(),
        )
        .await?;

    let run_id = handle
        .run_flow(FlowId::from("release_flow"), serde_json::json!({"severity":"critical"}))
        .await?;
    let _run = handle.flow_status(run_id).await?;

    handle.complete().await?;
    Ok(())
}
```

### Rust example: high-level in-memory mob state helper

```rust
use meerkat_mob::{MeerkatId, Prefab, ProfileName};
use meerkat_mob_mcp::MobMcpState;

async fn in_memory() -> Result<(), Box<dyn std::error::Error>> {
    let state = MobMcpState::new_in_memory();
    let mob_id = state.mob_create_prefab(Prefab::Pipeline).await?;

    state
        .mob_spawn(
            &mob_id,
            ProfileName::from("lead"),
            MeerkatId::from("lead-1"),
            None,
        )
        .await?;
    let _status = state.mob_status(&mob_id).await?;
    Ok(())
}
```

## Shared integration model (`meerkat-mob-mcp`)

Outside direct `meerkat_mob` usage, mob capability is provided by composing
`meerkat_mob_mcp::MobMcpDispatcher` into `SessionBuildOptions.external_tools`.

```rust
use std::sync::Arc;
use meerkat_core::service::SessionBuildOptions;
use meerkat_core::AgentToolDispatcher;
use meerkat_mob::MobSessionService;
use meerkat_mob_mcp::{MobMcpDispatcher, MobMcpState};

fn mob_external_tools(
    session_service: Arc<dyn MobSessionService>,
) -> Arc<dyn AgentToolDispatcher> {
    let state = Arc::new(MobMcpState::new(session_service));
    Arc::new(MobMcpDispatcher::new(state))
}

let build = SessionBuildOptions {
    external_tools: Some(mob_external_tools(session_service)),
    ..Default::default()
};
```

## Surface matrix

| Surface | Mob access | Current behavior |
|---|---|---|
| CLI `run` / `resume` | `mob_*` tools in prompt-driven runs | Primary CLI mob UX |
| CLI `rkat mob ...` | direct command lifecycle | Secondary explicit operational surface |
| RPC | session/turn methods | `mob_*` capability via composed `meerkat-mob-mcp` dispatcher |
| REST | session HTTP endpoints | `mob_*` capability via composed `meerkat-mob-mcp` dispatcher |
| MCP | `meerkat_*` session tools | `mob_*` capability via composed `meerkat-mob-mcp` dispatcher |
| Python SDK | RPC wrapper | inherits `mob_*` capability from RPC host composition |
| TypeScript SDK | RPC wrapper | inherits `mob_*` capability from RPC host composition |

## Multi-surface examples

### CLI tool-driven (primary)

```bash
rkat run "Create a mob with one lead and three workers, wire lead to all workers, and report status."
rkat resume <session_id> "Retire worker-2 and add worker-4, then summarize."
```

### CLI direct commands (explicit operational)

```bash
rkat mob create --definition mob.toml
rkat mob spawn team-mob lead lead-1
rkat mob wire team-mob lead-1 worker-1
rkat mob status team-mob
```

### RPC

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"session/create","params":{"prompt":"Use mob_* tools to create a lead/worker mob and return status."}}
```

### REST

```bash
curl -X POST http://127.0.0.1:8080/sessions \
  -H "Content-Type: application/json" \
  -d '{"prompt":"Use mob_* tools to create a lead/worker mob and return status."}'
```

### MCP

```json
{
  "name": "meerkat_run",
  "arguments": {
    "prompt": "Use mob_* tools to create a lead/worker mob and return status."
  }
}
```

### Python SDK

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect(realm_id="team-alpha")
result = await client.create_session("Design a mob topology for release triage.")
print(result.text)
await client.close()
```

### TypeScript SDK

```typescript
import { MeerkatClient } from "@meerkat/sdk";

const client = new MeerkatClient();
await client.connect({ realmId: "team-alpha" });
const result = await client.createSession({
  prompt: "Use mob_* tools to create a lead/worker mob and return status.",
});
console.log(result.text);
await client.close();
```

## Flows (subfeature)

Flows add DAG orchestration to mobs.

Flow essentials:

- `depends_on` + `depends_on_mode` (`all`/`any`)
- `dispatch_mode` (`one_to_one`/`fan_out`/`fan_in`)
- `collection_policy` (`any`/`all`/`quorum`)
- optional `condition` and `branch`
- persisted `step_ledger` and `failure_ledger`

Operational flow controls:

- list flows
- run flow
- check flow status
- cancel flow

## Practical guidance

Use mobs when you need:

- long-lived role-based multi-agent systems,
- explicit peer graph control,
- durable operational history.

Use plain sessions when:

- single-agent execution is sufficient,
- no shared graph/lifecycle state is required.
