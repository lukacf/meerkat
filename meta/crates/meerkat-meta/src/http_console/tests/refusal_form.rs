//! The form of a refusal a request gets on the Console: the answer names
//! the member as the request spelled it, on both RPC entries.

use super::*;
use crate::access::{AccessControlConfig, AgentResourceAttributes, PRIVATE_TO_LABEL};

const ADMIN: &str = "admin@example.test";
const CALLER: &str = "bob@example.test";
/// A member private to another person.
const HELD: &str = "identity:alice-side";
/// A name no member holds.
const UNHELD: &str = "identity:nobody";

type RowResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// One memory request by a signed-in caller who holds no grant, on a
/// Console whose access rules are on.
async fn refusal_for(
    runtime: &MobRuntime,
    controller: &AccessController,
    method: &str,
    identity: &str,
) -> Value {
    let view = controller.view_for_subject(Some(CALLER));
    let response = Box::pin(handle_console_runtime_rpc_with_visibility(
        runtime,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        &crate::console_aggregator::AllowAllConsoleVisibilityPolicy,
        rpc_request_with_params(
            method,
            json!({
                "identity": identity,
                "memory_id": "mem-1",
                "title": "A title",
                "body": "A body",
            }),
        ),
        true,
        false,
        Some(CALLER),
        Some(controller),
        Some(&view),
        None,
        None,
        None,
        None,
        None,
        &crate::http_console::ConsoleSendGovernance::default(),
        &crate::topology_control::TopologyApplyScope::Anonymous,
    ))
    .await;
    response["error"].clone()
}

async fn console_with_a_private_member(
    name: &str,
) -> Result<
    (tempfile::TempDir, MobRuntime, AccessController),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let (dir, runtime) = build_empty_console_test_runtime(name).await?;
    Ok((dir, runtime, rules_with_a_private_member()?))
}

/// Access rules that are on and grant nothing, over one private member.
fn rules_with_a_private_member()
-> Result<AccessController, Box<dyn std::error::Error + Send + Sync>> {
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        ..AccessControlConfig::default()
    })?;
    controller.record_agent_attributes(AgentResourceAttributes {
        identity: HELD.to_string(),
        agent_id: None,
        role: Some("worker".to_string()),
        labels: [(
            PRIVATE_TO_LABEL.to_string(),
            "console-caller:v1:alice".to_string(),
        )]
        .into_iter()
        .collect(),
    });
    Ok(controller)
}

/// A refused forget or update answers in one form whatever name the request
/// carries: the same code, action and message, and the member named exactly
/// as the caller spelled it.
#[tokio::test]
async fn a_refused_memory_change_names_the_member_as_the_caller_spelled_it() -> RowResult {
    let (_dir, runtime, controller) =
        console_with_a_private_member("console-memory-refusal-form").await?;
    for method in ["meta/agent_memory/forget", "meta/agent_memory/update"] {
        let held = refusal_for(&runtime, &controller, method, HELD).await;
        let unheld = refusal_for(&runtime, &controller, method, UNHELD).await;
        assert_eq!(
            held["data"]["kind"],
            json!("access_denied"),
            "{method}: {held:#?}"
        );
        assert_eq!(held["data"]["resource"], json!(HELD), "{method}: {held:#?}");
        assert_eq!(
            serde_json::to_string(&held)?,
            serde_json::to_string(&unheld)?.replace(UNHELD, HELD),
            "{method}: one form for both names"
        );
    }
    let _ = runtime.handle().stop().await;
    Ok(())
}

/// The entry of a Console that serves its runtimes through the aggregator
/// answers a refused request in the same one form.
#[tokio::test]
async fn a_refused_request_on_the_aggregator_entry_names_the_member_as_the_caller_spelled_it()
-> RowResult {
    let controller = rules_with_a_private_member()?;
    let view = controller.view_for_subject(Some(CALLER));
    let method = "meta/console/inspect_identity";
    let mut refusals = Vec::new();
    for identity in [HELD, UNHELD] {
        let response = handle_console_aggregator_rpc(
            None,
            rpc_request_with_params(method, json!({ "identity": identity })),
            true,
            false,
            Some(&controller),
            Some(&view),
            &crate::console_aggregator::ConsoleSendScope::Anonymous,
            None,
        )
        .await;
        refusals.push(response["error"].clone());
    }
    let (held, unheld) = (&refusals[0], &refusals[1]);
    assert_eq!(
        held["data"]["kind"],
        json!("access_denied"),
        "{method}: {held:#?}"
    );
    assert_eq!(held["data"]["resource"], json!(HELD), "{method}: {held:#?}");
    assert_eq!(
        serde_json::to_string(held)?,
        serde_json::to_string(unheld)?.replace(UNHELD, HELD),
        "{method}: one form for both names"
    );
    Ok(())
}
