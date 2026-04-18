use oai_rt_rs::realtime_tool;
use oai_rt_rs::sdk::{Realtime, ToolRegistry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
struct EchoArgs {
    text: String,
}

#[derive(Debug, Serialize)]
struct EchoResp {
    echoed: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SumArgs {
    a: i32,
    b: i32,
}

#[derive(Debug, Serialize)]
pub struct SumResp {
    sum: i32,
}

realtime_tool!(SumTool: SumArgs => SumResp {
    name: "sum",
    description: "Add two integers.",
    |args: SumArgs| async move {
        Ok(SumResp { sum: args.a + args.b })
    }
});

#[test]
fn builder_chain_compiles() {
    let _ = Realtime::builder()
        .api_key("k")
        .model("gpt-realtime")
        .voice("marin")
        .output_audio();
}

#[test]
fn voice_session_builder_compiles() {
    let _ = Realtime::builder()
        .voice_session()
        .voice("alloy")
        .vad_server_default();
}

#[test]
fn tool_registry_collects_definitions() {
    let mut registry = ToolRegistry::new();
    registry.tool("echo", |args: EchoArgs| async move {
        Ok(EchoResp { echoed: args.text })
    });

    assert_eq!(registry.definitions().len(), 1);
    assert_eq!(registry.definitions()[0].name, "echo");
}

#[test]
fn tool_registry_registers_tool_spec() {
    let mut registry = ToolRegistry::new();
    registry.register(SumTool);
    assert_eq!(registry.definitions().len(), 1);
    assert_eq!(registry.definitions()[0].name, "sum");
}
