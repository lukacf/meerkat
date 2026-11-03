#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::time::Duration;

use meerkat_meta::{
    DiscoverySpec, EventEnvelope, LifecycleStage, MetaConfig, ModuleConfig, RestartPolicy,
    UnifiedEvent, start_meta_runtime,
};

fn module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn main() {
    // Minimal skeleton: real module subprocesses + merged event stream.
    let config = MetaConfig {
        modules: vec![
            module(
                "scheduler",
                r#"printf '%s\n' '{"event_id":"evt-scheduler","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"scheduler","event_type":"ready","payload":{"ok":true,"subsystem":"scheduling"}}}'; exec sleep 20"#,
            ),
            module(
                "router",
                r#"printf '%s\n' '{"event_id":"evt-router","source":"module","timestamp_ms":30,"event":{"kind":"module","module":"router","event_type":"ready","payload":{"ok":true,"subsystem":"routing"}}}'; exec sleep 20"#,
            ),
        ],
        discovery: DiscoverySpec {
            namespace: "demo".to_string(),
            modules: vec!["scheduler".to_string(), "router".to_string()],
        },
        pre_spawn: vec![],
    };

    let agent_events = vec![EventEnvelope {
        event_id: "evt-agent-bootstrap".to_string(),
        source: "agent".to_string(),
        timestamp_ms: 10,
        event: UnifiedEvent::Agent {
            agent_id: "bootstrap-agent".to_string(),
            event_type: "ready".to_string(),
            payload: None,
        },
    }];

    let mut runtime = start_meta_runtime(config, agent_events, Duration::from_secs(2))
        .expect("mob runtime should start");

    println!("mob running: {}", runtime.is_running());
    println!("loaded modules: {:?}", runtime.loaded_modules());
    println!("lifecycle stages:");
    for event in runtime.lifecycle_events() {
        let stage = match event.stage {
            LifecycleStage::MobStarted => "MobStarted",
            LifecycleStage::ModulesStarted => "ModulesStarted",
            LifecycleStage::MergedStreamStarted => "MergedStreamStarted",
            LifecycleStage::ShutdownRequested => "ShutdownRequested",
            LifecycleStage::ShutdownComplete => "ShutdownComplete",
        };
        println!("  - seq={} stage={}", event.seq, stage);
    }

    println!("merged events:");
    for event in runtime.merged_events() {
        println!(
            "  - id={} source={} ts={} event={:?}",
            event.event_id, event.source, event.timestamp_ms, event.event
        );
    }

    let shutdown = runtime.shutdown();
    println!("shutdown report: {:?}", shutdown);
    println!("mob running after shutdown: {}", runtime.is_running());
}
