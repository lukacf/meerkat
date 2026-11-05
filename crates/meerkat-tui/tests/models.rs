//! `/model` end to end through the real runtime-backed service with a
//! scripted LLM: the picker shows every endpoint's live list, a bare id
//! resolves to the endpoint listing it, the switch goes through the
//! runtime's session-LLM reconfigure host and the next turn's request
//! carries the new model; the endpoint's connection and the model's profile
//! are in the realm's own config document (so `rkat` resolves the session
//! too), the endpoint is derived from the session's binding on resume, and
//! the cost totals survive a resume in the session's cost record.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::{Rig, RigOptions, ScriptedLlmClient, Step, Workspace, args_with};
use meerkat_core::ConfigStore;
use meerkat_core::Usage;
use meerkat_tui::bootstrap;
use meerkat_tui::endpoints::{ENDPOINT_REALM, Endpoint};
use meerkat_tui::models::{EndpointListing, ModelEntry, ModelLister, ModelSource};
use meerkat_tui::runner::Start;
use meerkat_tui::runner::cost_store::load_cost;
use meerkat_tui::ui::{Effect, Msg, SessionCost, StatusUpdate};

/// An Anthropic-dialect gateway, defined on the command line.
fn proxy_args() -> Vec<String> {
    [
        "--set",
        "endpoints.proxy.dialect=anthropic",
        "--set",
        "endpoints.proxy.base_url=http://127.0.0.1:9",
        "--set",
        // Keys come from the environment only; `PATH` is set wherever
        // tests run, and the scripted client never sends it.
        "endpoints.proxy.api_key_env=PATH",
    ]
    .map(str::to_string)
    .to_vec()
}

/// The proxy lists `my-alias` (a model only it knows) with a 50k window.
fn lister() -> ModelLister {
    Arc::new(|endpoints: Vec<Endpoint>| {
        Box::pin(async move {
            endpoints
                .into_iter()
                .map(|e| {
                    let models = if e.name == "proxy" {
                        vec![ModelEntry {
                            id: "my-alias".into(),
                            display_name: Some("My Alias".into()),
                            source: ModelSource::Provider,
                            context_window: Some(50_000),
                            max_output_tokens: Some(4_096),
                        }]
                    } else {
                        Vec::new()
                    };
                    (
                        e.name,
                        Ok(EndpointListing {
                            models,
                            prices: Vec::new(),
                        }),
                    )
                })
                .collect()
        })
    })
}

fn options() -> RigOptions {
    RigOptions {
        args: proxy_args(),
        lister: Some(lister()),
        ..RigOptions::default()
    }
}

fn is_model(msg: &Msg, endpoint: &str, model: &str) -> bool {
    matches!(msg, Msg::Status(StatusUpdate::Model { provider, model: m }) if provider == endpoint && m == model)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_picker_switches_the_live_model_and_the_next_turn_uses_it() {
    let ws = Workspace::new();
    let realm = bootstrap::resolve_scope(&args_with(&ws.root, &[]))
        .unwrap()
        .realm_root();
    let llm = ScriptedLlmClient::new(vec![Step::Text("one"), Step::Text("two")]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, options()).await;
    let id = rig.first_session.clone();
    rig.wait_for(|m| is_model(m, "anthropic", common::MODEL))
        .await;

    rig.send(Effect::StartTurn("first".into()));
    rig.turn_finished().await;
    assert_eq!(llm.recorded()[0].model, common::MODEL);

    // The picker: the current endpoint first, its current model first; the
    // proxy's live list.
    rig.send(Effect::RequestModelPicker { refresh: false });
    let Msg::ShowModelPicker(groups) = rig.wait_for(|m| matches!(m, Msg::ShowModelPicker(_))).await
    else {
        unreachable!()
    };
    assert_eq!(groups[0].endpoint, "anthropic");
    assert_eq!(groups[0].models[0].id, common::MODEL);
    assert_eq!(groups[0].models[0].source, ModelSource::Current);
    let proxy = groups.iter().find(|g| g.endpoint == "proxy").unwrap();
    assert_eq!(proxy.models[0].id, "my-alias");

    // A bare id only the proxy lists resolves to the proxy.
    rig.send(Effect::SetModel("my-alias".into()));
    rig.wait_for(|m| is_model(m, "proxy", "my-alias")).await;
    rig.wait_for(|m| matches!(m, Msg::Status(StatusUpdate::ContextWindow(Some(50_000)))))
        .await;
    rig.send(Effect::StartTurn("second".into()));
    rig.turn_finished().await;
    assert_eq!(llm.recorded()[1].model, "my-alias");

    // The identity is the session's: model and the proxy's binding.
    let meta = rig.session(&id).await.session_metadata().unwrap();
    assert_eq!(meta.model, "my-alias");
    let binding = meta.auth_binding.expect("the proxy's binding");
    assert_eq!(binding.realm.as_str(), ENDPOINT_REALM);
    assert_eq!(binding.binding.as_str(), "proxy");
    // The binding and the alias's profile live in the realm's own config
    // document: another surface loading it resolves the session.
    let document =
        meerkat_core::FileConfigStore::new(realm.join("config.toml"), meerkat_models::canonical())
            .get()
            .await
            .unwrap();
    let target =
        meerkat_core::connection::resolve_explicit_auth_binding_target(&document, &binding)
            .unwrap();
    assert_eq!(
        target.backend.base_url.as_deref(),
        Some("http://127.0.0.1:9")
    );
    let profile = &document.models.custom["my-alias"];
    assert_eq!(profile.context_window, Some(50_000));

    // The UI's cost totals land in the sidecar.
    let mut cost = SessionCost::default();
    cost.record(
        "my-alias",
        &Usage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Usage::default()
        },
    );
    rig.send(Effect::CostSnapshot(cost.clone()));
    rig.quit().await;
    assert_eq!(load_cost(&realm, &id.to_string()), Some(cost.clone()));

    // Resumed: the proxy and its model, the exact totals, and the next turn
    // runs on it.
    let llm = ScriptedLlmClient::new(vec![Step::Text("three"), Step::Text("four")]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::Resume("last".into()), options()).await;
    rig.wait_for(|m| is_model(m, "proxy", "my-alias")).await;
    let Msg::Status(StatusUpdate::CostTotals(resumed)) = rig
        .wait_for(|m| matches!(m, Msg::Status(StatusUpdate::CostTotals(_))))
        .await
    else {
        unreachable!()
    };
    assert_eq!(resumed, cost);
    rig.send(Effect::StartTurn("third".into()));
    rig.turn_finished().await;
    assert_eq!(llm.recorded()[0].model, "my-alias");

    // Back to a built-in endpoint: the explicit binding is cleared.
    rig.send(Effect::SetModel("anthropic:claude-opus-4-8".into()));
    rig.wait_for(|m| is_model(m, "anthropic", "claude-opus-4-8"))
        .await;
    rig.send(Effect::StartTurn("fourth".into()));
    rig.turn_finished().await;
    assert_eq!(llm.recorded()[1].model, "claude-opus-4-8");
    let meta = rig.session(&id).await.session_metadata().unwrap();
    assert!(meta.auth_binding.is_none(), "{meta:?}");
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unresolvable_references_leave_the_model_alone() {
    let ws = Workspace::new();
    let llm = ScriptedLlmClient::new(vec![Step::Text("ok")]);
    let mut rig = Rig::start_opts(&ws.root, &llm, Start::New, options()).await;
    rig.send(Effect::SetModel("nope:model".into()));
    let msg = rig
        .wait_for(
            |m| matches!(m, Msg::Notice { text, .. } if text.starts_with("model not changed")),
        )
        .await;
    assert!(
        matches!(&msg, Msg::Notice { text, .. } if text.contains("unknown endpoint `nope`")),
        "{msg:?}"
    );
    rig.send(Effect::StartTurn("still here".into()));
    rig.turn_finished().await;
    assert_eq!(llm.recorded()[0].model, common::MODEL);
    rig.quit().await;
}
