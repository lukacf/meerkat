use std::io::Write as _;
use std::path::PathBuf;
use std::ffi::{CString, c_char};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use embedded_svc::io::Write as _;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration as WifiConfiguration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{AnyInputPin, Output, PinDriver, Pins};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::{self, SpiDeviceDriver, SpiDriver, SpiDriverConfig, SPI2};
use esp_idf_svc::hal::units::*;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection, Response};
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::sys;
use esp_idf_svc::tls::X509;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use futures::StreamExt;
use meerkat_comms::agent::wrap_with_comms;
use meerkat_comms::{
    CommsConfig, CommsRuntime, Keypair, ResolvedCommsConfig, TrustedPeer, TrustedPeers,
};
use meerkat_client::{LlmClientAdapter, OpenAiClient};
use meerkat_core::event::EventEnvelope;
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, HookCapability, HookEntryConfig,
    HookExecutionMode, HookId, HookPoint, HookRuntimeConfig, HooksConfig, RetryPolicy,
};
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionError, SessionService,
    StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::SessionServiceCommsExt;
use meerkat_core::types::{HandlingMode, RenderMetadata, RunResult, SessionId, ToolCallView, ToolDef, ToolResult};
use meerkat_hooks::{DefaultHookEngine, RuntimeHookResponse};
use meerkat_runtime::{InMemoryRuntimeStore, RuntimeSessionAdapter};
use meerkat_runtime::comms_drain::spawn_comms_drain_with_observer;
use meerkat_session::PersistentSessionService;
use meerkat_session::ephemeral::{SessionAgent, SessionAgentBuilder};
use meerkat_store::{MemoryBlobStore, MemoryStore};
use meerkat_tools::{BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, ToolMode, ToolPolicyLayer};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::Builder;
use serde_json::json;
use serde_json::Value;
use tokio::sync::mpsc;

#[path = "../../runtime_support.rs"]
mod runtime_support;

use runtime_support::{
    ProbeDynAgent, ProbeNoopStore, ProbeSessionAgent, ProbeSessionRuntimeExecutor,
};

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASS: Option<&str> = option_env!("WIFI_PASS");
const OPENAI_API_KEY: Option<&str> = option_env!("OPENAI_API_KEY");
const OPENAI_MODEL: &str = match option_env!("OPENAI_MODEL") {
    Some(value) => value,
    None => "gpt-5.4-mini",
};
const OPENAI_BASE_URL: &str = match option_env!("OPENAI_BASE_URL") {
    Some(value) => value,
    None => "https://api.openai.com",
};
const PROBE_MAIN_STACK_BYTES: usize = 64 * 1024;
const COMMS_LISTEN_PORT: &str = match option_env!("COMMS_LISTEN_PORT") {
    Some(value) => value,
    None => "4210",
};
const HOST_PEER_NAME: &str = match option_env!("HOST_PEER_NAME") {
    Some(value) => value,
    None => "phase0-host",
};
const MICROPYTHON_TOOL_NAME: &str = "micropython_exec";
const HOST_PEER_ADDR: &str = match option_env!("HOST_PEER_ADDR") {
    Some(value) => value,
    None => "tcp://127.0.0.1:4220",
};
const LCD_WIDTH: u16 = 170;
const LCD_HEIGHT: u16 = 320;
const LCD_OFFSET_X: u16 = 35;
const LCD_OFFSET_Y: u16 = 0;
const MEERKAT_WORKER_STACK_BYTES: &[usize] =
    &[56 * 1024, 48 * 1024, 40 * 1024, 32 * 1024];
const PHASE0_HOST_SECRET: [u8; 32] = [7; 32];
const PHASE0_DEVICE_SECRET: [u8; 32] = [9; 32];
const OPENAI_WE1_PEM: &[u8] =
    concat!(include_str!("../../../../../meerkat-client/src/openai_we1.pem"), "\0").as_bytes();

type LcdSpi<'d> = SpiDeviceDriver<'d, SpiDriver<'d>>;
type LcdDc<'d> = PinDriver<'d, Output>;
type LcdReset<'d> = PinDriver<'d, Output>;
type LcdInterface<'d> = SpiInterface<'static, LcdSpi<'d>, LcdDc<'d>>;
type LcdPanel<'d> = mipidsi::Display<LcdInterface<'d>, ST7789, LcdReset<'d>>;

struct StatusDisplay<'d> {
    panel: LcdPanel<'d>,
    _backlight: PinDriver<'d, Output>,
}

fn openai_stream_enabled() -> bool {
    matches!(
        option_env!("OPENAI_STREAM_DEVICE").or(option_env!("OPENAI_STREAM")),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

#[derive(Debug, Clone, Copy)]
struct MemorySnapshot {
    heap_free: u32,
    heap_min_free: u32,
    internal_free: u32,
    internal_min_free: u32,
    internal_largest: u32,
    spiram_free: u32,
    spiram_min_free: u32,
    spiram_largest: u32,
}

#[derive(Debug, Default)]
struct MeerkatEvidence {
    hook_started: usize,
    hook_completed: usize,
    skill_resolved: usize,
    tool_requested: usize,
    tool_completed: usize,
    text_bytes: usize,
    saw_datetime: bool,
    saw_micropython: bool,
}

#[derive(Debug)]
struct MeerkatRunSummary {
    result_text: String,
    turns: u32,
    tool_calls: u32,
    task_count: usize,
    evidence: MeerkatEvidence,
    memory_before: MemorySnapshot,
    memory_after: MemorySnapshot,
}

#[derive(Debug)]
struct CommsRunSummary {
    incoming_messages: usize,
    outgoing_messages: usize,
    memory_before: MemorySnapshot,
    memory_after: MemorySnapshot,
}

enum DisplayEvent {
    Stage {
        stage: String,
        headline: String,
        detail: Option<String>,
        background: Rgb565,
    },
    Transcript {
        prefix: String,
        text: String,
    },
}

#[derive(Clone)]
struct DeviceProbeAgentBuilder {
    host_peer_name: String,
    model: String,
    openai_api_key: String,
    openai_base_url: String,
    comms_runtime: Arc<CommsRuntime>,
}

struct MicroPythonToolArgs {
    code: String,
    background: bool,
}

impl MicroPythonToolArgs {
    fn parse(call: ToolCallView<'_>) -> Result<Self, ToolError> {
        let raw: Value = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(MICROPYTHON_TOOL_NAME, error.to_string()))?;
        let code = raw
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::invalid_arguments(MICROPYTHON_TOOL_NAME, "missing string field 'code'"))?
            .to_string();
        let background = raw
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Self { code, background })
    }
}

#[repr(C)]
struct Phase0MpyResult {
    job_id: u32,
    background: bool,
    ok: bool,
    truncated: bool,
    status_code: i32,
    payload_len: usize,
    payload: [c_char; 2048],
}

unsafe extern "C" {
    fn phase0_mpy_init() -> i32;
    fn phase0_mpy_exec_sync(
        code: *const c_char,
        timeout_ms: u32,
        out_result: *mut Phase0MpyResult,
    ) -> i32;
}

struct EspMicroPythonToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    exec_lock: Mutex<()>,
}

impl EspMicroPythonToolDispatcher {
    fn new() -> Self {
        let tool = Arc::new(ToolDef {
            name: MICROPYTHON_TOOL_NAME.to_string(),
            description: "Execute Python code using the embedded MicroPython interpreter on the ESP32 device.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code to execute on the embedded device."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Reserved for long-running jobs; sync execution is the proven baseline."
                    }
                },
                "required": ["code"],
                "additionalProperties": false
            }),
        });
        Self {
            tools: vec![tool].into(),
            exec_lock: Mutex::new(()),
        }
    }

    fn ensure_ready() -> Result<(), ToolError> {
        static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
        let init = INIT_RESULT.get_or_init(|| {
            emit_marker("MKT:MICROPY:INIT_START", &[]);
            let err = unsafe { phase0_mpy_init() };
            emit_marker("MKT:MICROPY:INIT_RESULT", &[("esp_err", &err.to_string())]);
            if err == 0 {
                emit_marker("MKT:MICROPY:INIT_OK", &[]);
                Ok(())
            } else {
                Err(format!("embedded MicroPython init failed with esp_err={err}"))
            }
        });
        init.clone().map_err(ToolError::execution_failed)
    }

    fn exec_sync(&self, call: ToolCallView<'_>, args: MicroPythonToolArgs) -> Result<ToolDispatchOutcome, ToolError> {
        if args.background {
            return Err(ToolError::Unavailable {
                name: MICROPYTHON_TOOL_NAME.to_string(),
                reason: "background micropython execution is not enabled in this phase-0 firmware".to_string(),
            });
        }
        Self::ensure_ready()?;
        let _guard = self
            .exec_lock
            .lock()
            .map_err(|_| ToolError::execution_failed("micropython execution lock poisoned"))?;
        let code = CString::new(args.code.clone())
            .map_err(|_| ToolError::invalid_arguments(MICROPYTHON_TOOL_NAME, "code contains interior NUL byte"))?;
        let mut result = Phase0MpyResult {
            job_id: 0,
            background: false,
            ok: false,
            truncated: false,
            status_code: 0,
            payload_len: 0,
            payload: [0; 2048],
        };
        emit_marker("MKT:MICROPY:REQUESTED", &[("bytes", &args.code.len().to_string())]);
        let err = unsafe { phase0_mpy_exec_sync(code.as_ptr(), 15_000, &mut result) };
        if err != 0 {
            emit_marker("MKT:MICROPY:FAILED", &[("esp_err", &err.to_string())]);
            return Err(ToolError::execution_failed(format!(
                "embedded MicroPython exec failed with esp_err={err}"
            )));
        }

        let payload_len = result.payload_len.min(result.payload.len().saturating_sub(1));
        let payload = unsafe {
            let bytes = std::slice::from_raw_parts(result.payload.as_ptr() as *const u8, payload_len);
            String::from_utf8_lossy(bytes).to_string()
        };
        emit_marker(
            "MKT:MICROPY:COMPLETED",
            &[
                ("is_error", if result.ok { "false" } else { "true" }),
                ("status_code", &result.status_code.to_string()),
                ("payload_len", &result.payload_len.to_string()),
            ],
        );
        Ok(ToolResult::new(call.id.to_string(), payload, !result.ok).into())
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl AgentToolDispatcher for EspMicroPythonToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let args = MicroPythonToolArgs::parse(call)?;
        self.exec_sync(call, args)
    }
}

fn main() {
    sys::link_patches();
    rom_marker("MKT:ROM:MAIN_ENTER");
    println!("MKT:EARLY:MAIN_ENTER");
    let _ = std::io::stdout().flush();
    EspLogger::initialize_default();
    rom_marker("MKT:ROM:LOGGER_READY");
    println!("MKT:EARLY:LOGGER_READY");
    let _ = std::io::stdout().flush();
    std::panic::set_hook(Box::new(|panic_info| {
        let payload = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
            *message
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
            message.as_str()
        } else {
            "unknown panic payload"
        };
        emit_marker(
            "MKT:PANIC",
            &[("message", &json_quote(&short_display_line(payload, 120)))],
        );
    }));

    #[cfg(target_os = "espidf")]
    {
        rom_marker("MKT:ROM:BEFORE_RUN_PROBE");
        if let Err(error) = run_probe() {
            rom_marker("MKT:ROM:RUN_PROBE_ERR");
            emit_marker(
                "MKT:RUST_STACK:FAIL",
                &[("error", &json_quote(&error.to_string()))],
            );
            emit_marker(
                "MKT:SINGLE_NODE:FAIL",
                &[("error", &json_quote(&error.to_string()))],
            );
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        rom_marker("MKT:ROM:RUN_PROBE_OK");
    }

    #[cfg(not(target_os = "espidf"))]
    {
        let worker = thread::Builder::new()
            .name("probe-main".to_string())
            .stack_size(PROBE_MAIN_STACK_BYTES)
            .spawn(|| {
                if let Err(error) = run_probe() {
                    emit_marker(
                        "MKT:RUST_STACK:FAIL",
                        &[("error", &json_quote(&error.to_string()))],
                    );
                    emit_marker(
                        "MKT:SINGLE_NODE:FAIL",
                        &[("error", &json_quote(&error.to_string()))],
                    );
                    eprintln!("{error:#}");
                    std::process::exit(1);
                }
            })
            .expect("failed to spawn probe-main worker");

        if let Err(error) = worker.join() {
            emit_marker(
                "MKT:RUST_STACK:FAIL",
                &[("error", &json_quote(&format!("probe-main panicked: {error:?}")))],
            );
            std::process::exit(1);
        }
    }
}

fn rom_marker(message: &str) {
    let mut bytes = Vec::with_capacity(message.len() + 2);
    bytes.extend_from_slice(message.as_bytes());
    bytes.push(b'\n');
    bytes.push(0);
    unsafe {
        sys::esp_rom_printf(bytes.as_ptr() as *const c_char);
    }
}

fn run_probe() -> anyhow::Result<()> {
    rom_marker("MKT:ROM:RUN_PROBE_ENTER");
    let park_only = option_env!("PARK_ONLY") == Some("1");
    let wifi_ssid = required_build_input(WIFI_SSID, "WIFI_SSID")?;
    let wifi_pass = required_build_input(WIFI_PASS, "WIFI_PASS")?;
    let openai_api_key = required_build_input(OPENAI_API_KEY, "OPENAI_API_KEY")?;

    emit_marker(
        "MKT:BOOT:OK",
        &[
            ("heap_free", &unsafe { sys::esp_get_free_heap_size() }.to_string()),
            (
                "internal_free",
                &heap_caps_get_free_size(sys::MALLOC_CAP_INTERNAL).to_string(),
            ),
            (
                "spiram_free",
                &heap_caps_get_free_size(sys::MALLOC_CAP_SPIRAM).to_string(),
            ),
            (
                "reset_reason",
                &unsafe { sys::esp_reset_reason() }.to_string(),
            ),
        ],
    );

    let peripherals = Peripherals::take().context("failed to take peripherals")?;
    let modem = peripherals.modem;
    let spi2 = peripherals.spi2;
    let pins = peripherals.pins;
    let sys_loop = EspSystemEventLoop::take().context("failed to take system event loop")?;
    let nvs = EspDefaultNvsPartition::take().context("failed to take default nvs partition")?;

    let mut status_display = match StatusDisplay::try_new(spi2, pins) {
        Ok(mut display) => {
            emit_marker("MKT:DISPLAY:OK", &[("mode", "\"st7789-status\"")]);
            let _ = display.show_stage(
                "boot",
                "phase 0 contract probe",
                Some("bringing up board"),
                Rgb565::new(0, 32, 72),
            );
            Some(display)
        }
        Err(error) => {
            emit_marker(
                "MKT:DISPLAY:FAIL",
                &[(
                    "error",
                    &json_quote(&short_display_line(&error.to_string(), 40)),
                )],
            );
            eprintln!("display init failed: {error:#}");
            None
        }
    };

    if park_only {
        emit_marker("MKT:PARK:ENABLED", &[]);
        if let Some(display) = status_display.as_mut() {
            let _ = display.show_stage(
                "parked",
                "provider loop off",
                Some("phase 0 board parked"),
                Rgb565::new(0, 24, 0),
            );
        }
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs)).context("failed to create esp wifi")?,
        sys_loop,
    )
    .context("failed to wrap blocking wifi")?;

    if let Some(display) = status_display.as_mut() {
        let _ = display.show_stage("wifi", "connecting", Some(wifi_ssid), Rgb565::new(0, 0, 80));
    }
    let wifi_ip = connect_wifi(&mut wifi, wifi_ssid, wifi_pass, status_display.as_mut())?;

    let _sntp = EspSntp::new_default().context("failed to start SNTP")?;
    if let Some(display) = status_display.as_mut() {
        let _ = display.show_stage("time", "syncing sntp", None, Rgb565::new(0, 48, 48));
    }
    wait_for_time_sync(status_display.as_mut())?;

    if !skip_single_node() {
        let stream_started = Instant::now();
        if let Some(display) = status_display.as_mut() {
            let _ = display.show_stage(
                "tls",
                "opening https",
                Some("streaming openai"),
                Rgb565::new(56, 28, 0),
            );
        }
        let stream_events = match run_openai_stream(openai_api_key) {
            Ok(events) => events,
            Err(error) => {
                if let Some(display) = status_display.as_mut() {
                    let _ = display.show_stage(
                        "fail",
                        "stream error",
                        Some(&short_display_line(&error.to_string(), 28)),
                        Rgb565::new(88, 0, 0),
                    );
                }
                return Err(error);
            }
        };
        let elapsed_ms = stream_started.elapsed().as_millis();

        if let Some(display) = status_display.as_mut() {
            let _ = display.show_stage(
                "meerkat",
                "bootstrapping agent",
                Some("hooks skills tools"),
                Rgb565::new(0, 32, 64),
            );
        }
        let meerkat_started = Instant::now();
        let meerkat_summary = match run_meerkat_turn(openai_api_key) {
            Ok(summary) => summary,
            Err(error) => {
                if let Some(display) = status_display.as_mut() {
                    let _ = display.show_stage(
                        "fail",
                        "meerkat error",
                        Some(&short_display_line(&error.to_string(), 28)),
                        Rgb565::new(88, 0, 0),
                    );
                }
                return Err(error);
            }
        };
        let meerkat_elapsed_ms = meerkat_started.elapsed().as_millis();

        emit_marker(
            "MKT:RUST_STACK:OK",
            &[
                ("lane", "\"std-blocking\""),
                ("stream_events", &stream_events.to_string()),
                ("elapsed_ms", &elapsed_ms.to_string()),
                ("meerkat_elapsed_ms", &meerkat_elapsed_ms.to_string()),
                ("meerkat_turns", &meerkat_summary.turns.to_string()),
                ("meerkat_tool_calls", &meerkat_summary.tool_calls.to_string()),
                ("meerkat_tasks", &meerkat_summary.task_count.to_string()),
                ("meerkat_heap_before", &meerkat_summary.memory_before.heap_free.to_string()),
                ("meerkat_heap_after", &meerkat_summary.memory_after.heap_free.to_string()),
                (
                    "meerkat_heap_min_after",
                    &meerkat_summary.memory_after.heap_min_free.to_string(),
                ),
                (
                    "meerkat_internal_after",
                    &meerkat_summary.memory_after.internal_free.to_string(),
                ),
                (
                    "meerkat_spiram_after",
                    &meerkat_summary.memory_after.spiram_free.to_string(),
                ),
                (
                    "heap_free",
                    &unsafe { sys::esp_get_free_heap_size() }.to_string(),
                ),
            ],
        );
        emit_marker(
            "MKT:SINGLE_NODE:PASS",
            &[
                ("lane", "\"std-blocking\""),
                ("elapsed_ms", &elapsed_ms.to_string()),
            ],
        );

        if let Some(display) = status_display.as_mut() {
            let detail = format!(
                "{} ev {}ms {} tools",
                stream_events, meerkat_elapsed_ms, meerkat_summary.tool_calls
            );
            let _ = display.show_stage(
                "pass",
                "meerkat on esp ok",
                Some(&detail),
                Rgb565::new(0, 72, 0),
            );
        }
    } else {
        emit_marker("MKT:SINGLE_NODE:SKIPPED", &[]);
    }

    if enable_comms() {
        if let Some(display) = status_display.as_mut() {
            let _ = display.show_stage(
                "comms",
                "starting listener",
                Some("awaiting peer"),
                Rgb565::new(0, 24, 72),
            );
        }
        run_comms_probe(openai_api_key, &wifi_ip, status_display.as_mut())?;
    }

    thread::sleep(Duration::from_secs(2));
    Ok(())
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl SessionAgentBuilder for DeviceProbeAgentBuilder {
    type Agent = ProbeSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let task_store = Arc::new(MemoryTaskStore::new());
        let external_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(EspMicroPythonToolDispatcher::new());
        let inner = CompositeDispatcher::new_wasm(
            task_store,
            &BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .with_mode(ToolMode::DenyAll)
                    .enable_tool("datetime"),
                ..Default::default()
            },
            Some(external_tools),
            Some("esp32-phase0-runtime".to_string()),
        )
        .context("failed to construct ESP32 comms dispatcher")
        .map_err(|error| SessionError::Agent(AgentError::ConfigError(error.to_string())))?;

        let tools: Arc<dyn AgentToolDispatcher> =
            wrap_with_comms(Arc::new(inner), Arc::clone(&self.comms_runtime));
        let llm: Arc<dyn AgentLlmClient> = Arc::new(
            LlmClientAdapter::new(
                Arc::new(OpenAiClient::new_with_base_url(
                    self.openai_api_key.clone(),
                    self.openai_base_url.clone(),
                )),
                self.model.clone(),
            )
            .with_provider_params(Some(json!({
                "reasoning_effort": "low",
                "stream": openai_stream_enabled()
            }))),
        );
        let store: Arc<dyn AgentSessionStore> = Arc::new(ProbeNoopStore);

        let comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> =
            self.comms_runtime.clone();

        let agent: ProbeDynAgent = AgentBuilder::new()
            .model(req.model.clone())
            .system_prompt(
                req.system_prompt
                    .clone()
                    .unwrap_or_else(|| device_system_prompt(&self.host_peer_name)),
            )
            .max_tokens_per_turn(req.max_tokens.unwrap_or(256))
            .with_comms_runtime(comms_runtime)
            .build(llm, tools, store)
            .await;

        Ok(ProbeSessionAgent::new(agent))
    }
}

fn connect_wifi(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    wifi_ssid: &str,
    wifi_pass: &str,
    status_display: Option<&mut StatusDisplay<'_>>,
) -> anyhow::Result<String> {
    let configuration = WifiConfiguration::Client(ClientConfiguration {
        ssid: wifi_ssid
            .try_into()
            .map_err(|_| anyhow!("wifi ssid is too long"))?,
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: wifi_pass
            .try_into()
            .map_err(|_| anyhow!("wifi password is too long"))?,
        channel: None,
        ..Default::default()
    });

    wifi.set_configuration(&configuration)
        .context("failed to set wifi configuration")?;
    wifi.start().context("failed to start wifi")?;
    wifi.connect().context("failed to connect wifi")?;
    wifi.wait_netif_up().context("wifi netif did not come up")?;

    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .context("failed to read station ip info")?;
    emit_marker(
        "MKT:WIFI:OK",
        &[("ip", &json_quote(&ip_info.ip.to_string()))],
    );
    if let Some(display) = status_display {
        let detail = format!("ip {}", ip_info.ip);
        let _ = display.show_stage("wifi", "connected", Some(&detail), Rgb565::new(0, 72, 0));
    }
    Ok(ip_info.ip.to_string())
}

fn wait_for_time_sync(status_display: Option<&mut StatusDisplay<'_>>) -> anyhow::Result<()> {
    let mut status_display = status_display;
    for _ in 0..30 {
        let now = unix_time_secs();
        if now > 1_700_000_000 {
            emit_marker("MKT:TIME:OK", &[("unix", &now.to_string())]);
            if let Some(display) = status_display.as_mut() {
                let detail = format!("unix {now}");
                let _ = display.show_stage(
                    "time",
                    "clock synced",
                    Some(&detail),
                    Rgb565::new(0, 56, 40),
                );
            }
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }

    bail!("timed out waiting for SNTP time sync")
}

fn run_openai_stream(openai_api_key: &str) -> anyhow::Result<usize> {
    let body = json!({
        "model": OPENAI_MODEL,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "Write twelve short lowercase words about embedded streaming, separated by spaces."
            }
        ],
        "max_output_tokens": 64,
        "stream": true
    });
    let payload = serde_json::to_vec(&body).context("failed to serialize request body")?;
    let auth_header = format!("Bearer {openai_api_key}");
    let content_length = payload.len().to_string();
    let url = format!("{OPENAI_BASE_URL}/v1/responses");
    let headers = [
        ("content-type", "application/json"),
        ("authorization", auth_header.as_str()),
        ("content-length", content_length.as_str()),
    ];

    let http_config = HttpConfiguration {
        timeout: Some(Duration::from_secs(60)),
        buffer_size: Some(2048),
        buffer_size_tx: Some(2048),
        server_certificate: Some(X509::pem_until_nul(OPENAI_WE1_PEM)),
        crt_bundle_attach: None,
        keep_alive_enable: true,
        ..Default::default()
    };
    let mut client = HttpClient::wrap(
        EspHttpConnection::new(&http_config).context("failed to create HTTPS client")?,
    );

    let mut request = client
        .request(Method::Post, &url, &headers)
        .context("failed to create request")?;
    request
        .write_all(&payload)
        .context("failed to write request payload")?;
    request.flush().context("failed to flush request payload")?;

    let mut response = request.submit().context("failed to submit request")?;
    let status = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("unknown")
        .to_string();

    if !(200..=299).contains(&status) {
        let body = read_response_body(&mut response)?;
        bail!("provider status {status}: {body}");
    }

    emit_marker(
        "MKT:TLS:OK",
        &[
            ("status", &status.to_string()),
            ("content_type", &json_quote(&content_type)),
        ],
    );
    emit_marker(
        "MKT:PROVIDER:STREAM_START",
        &[("model", &json_quote(OPENAI_MODEL))],
    );

    let stream_events = read_sse_stream(&mut response)?;
    emit_marker(
        "MKT:PROVIDER:STREAM_DONE",
        &[("events", &stream_events.to_string())],
    );

    Ok(stream_events)
}

fn run_meerkat_turn(openai_api_key: &str) -> anyhow::Result<MeerkatRunSummary> {
    let memory_before = capture_memory_snapshot();
    emit_marker(
        "MKT:MEERKAT:BOOTSTRAP_OK",
        &[
            ("heap_free", &memory_before.heap_free.to_string()),
            ("heap_min_free", &memory_before.heap_min_free.to_string()),
            ("surface", "\"phase0-mini-surface\""),
        ],
    );

    emit_meerkat_step("before_runtime_build");
    let mut last_spawn_error: Option<anyhow::Error> = None;

    for stack_bytes in MEERKAT_WORKER_STACK_BYTES {
        emit_marker(
            "MKT:MEERKAT:THREAD_SPAWN",
            &[("stack_bytes", &stack_bytes.to_string())],
        );
        let openai_api_key = openai_api_key.to_string();
        match std::thread::Builder::new()
            .name(format!("meerkat-phase0-{stack_bytes}"))
            .stack_size(*stack_bytes)
            .spawn(move || run_meerkat_turn_inner(openai_api_key, memory_before))
        {
            Ok(worker) => {
                return worker
                    .join()
                    .map_err(|_| anyhow!("Meerkat worker thread panicked"))?;
            }
            Err(error) => {
                emit_marker(
                    "MKT:MEERKAT:THREAD_SPAWN_FAIL",
                    &[
                        ("stack_bytes", &stack_bytes.to_string()),
                        ("error", &json_quote(&error.to_string())),
                    ],
                );
                last_spawn_error = Some(anyhow!(error));
            }
        }
    }

    emit_marker(
        "MKT:MEERKAT:THREAD_FALLBACK",
        &[
            ("mode", "\"main_task\""),
            (
                "last_error",
                &json_quote(
                    &last_spawn_error
                        .as_ref()
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                ),
            ),
        ],
    );
    run_meerkat_turn_inner(openai_api_key.to_string(), memory_before)
}

fn run_meerkat_turn_inner(
    openai_api_key: String,
    memory_before: MemorySnapshot,
) -> anyhow::Result<MeerkatRunSummary> {
    emit_meerkat_step("thread_enter");
    emit_marker(
        "MKT:MEERKAT:RUNTIME_MODE",
        &[
            ("mode", "\"current_thread_no_time\""),
        ],
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("failed to build tokio runtime for meerkat probe")?;
    emit_meerkat_step("after_runtime_build");

    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        emit_meerkat_step("inside_block_on");
        let task_store = Arc::new(MemoryTaskStore::new());
        emit_meerkat_step("after_task_store");
        let builtin_config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new()
                .with_mode(ToolMode::DenyAll)
                .enable_tool("datetime"),
            ..Default::default()
        };
        let external_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(EspMicroPythonToolDispatcher::new());
        let dispatcher = CompositeDispatcher::new_wasm(
            task_store.clone(),
            &builtin_config,
            Some(external_tools),
            Some("esp32-phase0".to_string()),
        )
        .context("failed to construct embedded builtin dispatcher")?;
        emit_meerkat_step("after_dispatcher");
        emit_marker(
            "MKT:MEERKAT:TOOLS_EXPOSED",
            &[
                ("count", &dispatcher.tools().len().to_string()),
                ("names", &json_quote("datetime,micropython_exec")),
            ],
        );

        let hooks = DefaultHookEngine::new(HooksConfig {
            entries: vec![
                HookEntryConfig {
                    id: HookId::new("phase0-pre-llm"),
                    point: HookPoint::PreLlmRequest,
                    mode: HookExecutionMode::Foreground,
                    capability: HookCapability::Observe,
                    runtime: HookRuntimeConfig::new(
                        "in_process",
                        Some(json!({ "name": "phase0-pre-llm" })),
                    )
                    .unwrap_or_default(),
                    ..Default::default()
                },
                HookEntryConfig {
                    id: HookId::new("phase0-pre-tool"),
                    point: HookPoint::PreToolExecution,
                    mode: HookExecutionMode::Foreground,
                    capability: HookCapability::Observe,
                    runtime: HookRuntimeConfig::new(
                        "in_process",
                        Some(json!({ "name": "phase0-pre-tool" })),
                    )
                    .unwrap_or_default(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .with_in_process_handler(
            "phase0-pre-llm",
            Arc::new(|invocation| {
                Box::pin(async move {
                    emit_marker(
                        "MKT:MEERKAT:HOOK_PRE_LLM",
                        &[("turn", &invocation.turn_number.unwrap_or(0).to_string())],
                    );
                    Ok(RuntimeHookResponse::default())
                })
            }),
        )
        .with_in_process_handler(
            "phase0-pre-tool",
            Arc::new(|invocation| {
                Box::pin(async move {
                    let tool_name = invocation
                        .tool_call
                        .as_ref()
                        .map(|tool| tool.name.as_str())
                        .unwrap_or("unknown");
                    emit_marker(
                        "MKT:MEERKAT:HOOK_PRE_TOOL",
                        &[("tool", &json_quote(tool_name))],
                    );
                    Ok(RuntimeHookResponse::default())
                })
            }),
        );
        emit_meerkat_step("after_hooks");

        let client = Arc::new(OpenAiClient::new_with_base_url(
            openai_api_key,
            OPENAI_BASE_URL.to_string(),
        ));
        emit_meerkat_step("after_openai_client");
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(64);
        emit_meerkat_step("after_event_channel");
        let llm = Arc::new(
            LlmClientAdapter::with_event_channel(client, OPENAI_MODEL.to_string(), event_tx.clone())
                .with_provider_params(Some(json!({
                    "reasoning_effort": "low",
                    "stream": openai_stream_enabled()
                }))),
        );
        emit_meerkat_step("after_llm_adapter");

        let collector = tokio::spawn(async move {
            let mut evidence = MeerkatEvidence::default();
            while let Some(event) = event_rx.recv().await {
                match event {
                    AgentEvent::HookStarted { .. } => evidence.hook_started += 1,
                    AgentEvent::HookCompleted { .. } => evidence.hook_completed += 1,
                    AgentEvent::SkillsResolved { .. } => evidence.skill_resolved += 1,
                    AgentEvent::ToolCallRequested { name, .. } => {
                        evidence.tool_requested += 1;
                        emit_marker(
                            "MKT:MEERKAT:EVENT_TOOL_REQUESTED",
                            &[("name", &json_quote(&name))],
                        );
                        if name == "datetime" {
                            evidence.saw_datetime = true;
                        } else if name == MICROPYTHON_TOOL_NAME {
                            evidence.saw_micropython = true;
                        }
                    }
                    AgentEvent::ToolExecutionCompleted { .. } => {
                        evidence.tool_completed += 1;
                        emit_marker(
                            "MKT:MEERKAT:EVENT_TOOL_COMPLETED",
                            &[("count", &evidence.tool_completed.to_string())],
                        );
                    }
                    AgentEvent::TextDelta { delta } => {
                        evidence.text_bytes += delta.len();
                        if evidence.text_bytes == delta.len() {
                            emit_marker(
                                "MKT:MEERKAT:EVENT_TEXT_DELTA",
                                &[("bytes", &delta.len().to_string())],
                            );
                        }
                    }
                    _ => {}
                }
            }
            evidence
        });
        emit_meerkat_step("after_collector_spawn");

        let mut agent = AgentBuilder::new()
            .model(OPENAI_MODEL)
            .system_prompt(
                "You are the Meerkat Phase 0 embedded probe. \
                 You must call the datetime tool exactly once and the micropython_exec tool exactly once. \
                 Use micropython_exec to run short Python that prints 'hello from micropython' and sets result = 6 * 7. \
                 After both tools complete, answer in one short sentence that includes the observed date or time and the python result.",
            )
            .max_tokens_per_turn(96)
            .with_hook_engine(Arc::new(hooks))
            .build(llm, Arc::new(dispatcher), Arc::new(ProbeNoopStore))
            .await;
        emit_meerkat_step("after_agent_build");

        emit_marker(
            "MKT:MEERKAT:RUN_START",
            &[("model", &json_quote(OPENAI_MODEL))],
        );
        let result = agent
            .run_with_events(
                "Call datetime exactly once. Then call micropython_exec exactly once with code that prints 'hello from micropython' and sets result = 6 * 7. Finally answer in one short sentence confirming embedded Meerkat is alive, include the observed date or time, and include the python result."
                    .into(),
                event_tx.clone(),
            )
            .await
            .context("embedded Meerkat turn failed")?;
        emit_meerkat_step("after_run_with_events");

        drop(agent);
        drop(event_tx);
        emit_meerkat_step("after_event_tx_drop");
        let evidence = collector
            .await
            .map_err(|error| anyhow!("failed joining event collector: {error}"))?;
        emit_meerkat_step("after_collector_join");
        let task_count = task_store.len();

        emit_marker(
            "MKT:MEERKAT:SKILLS_OK",
            &[("resolved", &evidence.skill_resolved.to_string())],
        );

        if !evidence.saw_datetime || !evidence.saw_micropython {
            bail!(
                "meerkat run did not exercise required tools (datetime={}, micropython={}, tasks={task_count})",
                evidence.saw_datetime,
                evidence.saw_micropython,
            );
        }
        emit_marker(
            "MKT:MEERKAT:TOOLS_OK",
            &[
                ("tool_requested", &evidence.tool_requested.to_string()),
                ("tool_completed", &evidence.tool_completed.to_string()),
                ("saw_micropython", if evidence.saw_micropython { "true" } else { "false" }),
                ("task_count", &task_count.to_string()),
            ],
        );

        if evidence.hook_started == 0 || evidence.hook_completed == 0 {
            bail!(
                "meerkat run did not exercise hooks (started={}, completed={})",
                evidence.hook_started,
                evidence.hook_completed,
            );
        }
        emit_marker(
            "MKT:MEERKAT:HOOKS_OK",
            &[
                ("started", &evidence.hook_started.to_string()),
                ("completed", &evidence.hook_completed.to_string()),
            ],
        );

        let memory_after = capture_memory_snapshot();
        emit_marker(
            "MKT:MEERKAT:RUN_DONE",
            &[
                ("turns", &result.turns.to_string()),
                ("tool_calls", &result.tool_calls.to_string()),
                ("task_count", &task_count.to_string()),
                ("text_bytes", &evidence.text_bytes.to_string()),
                ("heap_free", &memory_after.heap_free.to_string()),
                ("heap_min_free", &memory_after.heap_min_free.to_string()),
                ("internal_free", &memory_after.internal_free.to_string()),
                ("internal_min_free", &memory_after.internal_min_free.to_string()),
                ("internal_largest", &memory_after.internal_largest.to_string()),
                ("spiram_free", &memory_after.spiram_free.to_string()),
                ("spiram_min_free", &memory_after.spiram_min_free.to_string()),
                ("spiram_largest", &memory_after.spiram_largest.to_string()),
            ],
        );

        Ok(MeerkatRunSummary {
            result_text: result.text,
            turns: result.turns,
            tool_calls: result.tool_calls,
            task_count,
            evidence,
            memory_before,
            memory_after,
        })
    })
}

fn run_comms_probe(
    openai_api_key: &str,
    wifi_ip: &str,
    mut status_display: Option<&mut StatusDisplay<'_>>,
) -> anyhow::Result<()> {
    let memory_before = capture_memory_snapshot();
    emit_marker(
        "MKT:COMMS:BOOTSTRAP_OK",
        &[
            ("heap_free", &memory_before.heap_free.to_string()),
            ("internal_free", &memory_before.internal_free.to_string()),
            ("spiram_free", &memory_before.spiram_free.to_string()),
        ],
    );

    if let Some(display) = status_display.as_mut() {
        let detail = format!("{wifi_ip}:{COMMS_LISTEN_PORT}");
        let _ = display.show_stage(
            "comms",
            "arming keep-alive",
            Some(&detail),
            Rgb565::new(0, 20, 64),
        );
    }

    let summary = run_comms_probe_inner(
        openai_api_key.to_string(),
        wifi_ip.to_string(),
        memory_before,
        status_display.as_deref_mut(),
    )?;

    emit_marker(
        "MKT:COMMS:SUMMARY",
        &[
            ("incoming", &summary.incoming_messages.to_string()),
            ("outgoing", &summary.outgoing_messages.to_string()),
            ("heap_after", &summary.memory_after.heap_free.to_string()),
            ("internal_after", &summary.memory_after.internal_free.to_string()),
            ("spiram_after", &summary.memory_after.spiram_free.to_string()),
        ],
    );

    Ok(())
}

fn run_comms_probe_inner(
    openai_api_key: String,
    wifi_ip: String,
    memory_before: MemorySnapshot,
    mut status_display: Option<&mut StatusDisplay<'_>>,
) -> anyhow::Result<CommsRunSummary> {
    emit_marker(
        "MKT:COMMS:RUNTIME_MODE",
        &[("mode", "\"current_thread_runtime_backed_keep_alive_inline\"")],
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("failed to build tokio runtime for comms probe")?;
    let public_addr = format!("tcp://{wifi_ip}:{COMMS_LISTEN_PORT}");

    emit_marker(
        "MKT:COMMS:LISTENING",
        &[
            ("peer_name", &json_quote("esp32-probe")),
            ("peer_id", &json_quote(&phase0_device_keypair().public_key().to_peer_id())),
            ("addr", &json_quote(&public_addr)),
            ("trusted_host", &json_quote(HOST_PEER_NAME)),
        ],
    );
    emit_marker(
        "MKT:COMMS:WAITING",
        &[("port", &json_quote(COMMS_LISTEN_PORT))],
    );

    if let Some(display) = status_display.as_deref_mut() {
        let _ = display.show_stage(
            "comms",
            "keep-alive ready",
            Some(&short_display_line(&public_addr, 26)),
            Rgb565::new(0, 24, 72),
        );
    }

    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let (display_tx, mut display_rx) = tokio::sync::mpsc::unbounded_channel::<DisplayEvent>();
        let mut transcript = Vec::new();
        let trusted = TrustedPeers {
            peers: vec![phase0_host_trusted_peer()],
        };
        let mut comms_runtime = CommsRuntime::new_with_state(
            device_comms_config(),
            phase0_device_keypair(),
            trusted,
        )
        .await
        .context("failed to build ESP32 comms runtime")?;
        comms_runtime
            .start_listeners()
            .await
            .context("failed to start ESP32 comms listeners")?;
        let comms_runtime = Arc::new(comms_runtime);
        let session_store: Arc<dyn meerkat_core::SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::new());
        let service = Arc::new(PersistentSessionService::new(
            DeviceProbeAgentBuilder {
                host_peer_name: HOST_PEER_NAME.to_string(),
                model: OPENAI_MODEL.to_string(),
                openai_api_key,
                openai_base_url: OPENAI_BASE_URL.to_string(),
                comms_runtime: Arc::clone(&comms_runtime),
            },
            1,
            session_store,
            Some(runtime_store.clone()),
            blob_store.clone(),
        ));
        let runtime_adapter = Arc::new(RuntimeSessionAdapter::persistent(
            runtime_store,
            blob_store,
        ));
        let session_id = create_device_keep_alive_session(&service).await?;

        runtime_adapter
            .ensure_session_with_executor(
                session_id.clone(),
                Box::new(ProbeSessionRuntimeExecutor::new(
                    Arc::clone(&service),
                    session_id.clone(),
                )),
            )
            .await;

        let session_comms = service
            .comms_runtime(&session_id)
            .await
            .context("device session missing comms runtime")?;

        let incoming_messages = Arc::new(AtomicUsize::new(0));
        let outgoing_messages = Arc::new(AtomicUsize::new(0));
        let incoming_counter = Arc::clone(&incoming_messages);
        let incoming_display = display_tx.clone();
        let drain = spawn_comms_drain_with_observer(
            Arc::clone(&runtime_adapter),
            session_id.clone(),
            session_comms.clone(),
            None,
            Some(Arc::new(move |ci| {
                if let Some(text) = inbound_text(&ci) {
                    incoming_counter.fetch_add(1, Ordering::SeqCst);
                    emit_marker(
                        "MKT:COMMS:INCOMING",
                        &[("text", &json_quote(&short_display_line(&text, 120)))],
                    );
                    let _ = incoming_display.send(DisplayEvent::Transcript {
                        prefix: "host".to_string(),
                        text,
                    });
                }
            })),
        );

        let mut events = service
            .subscribe_session_events(&session_id)
            .await
            .context("failed to subscribe to device session events")?;
        let outgoing_counter = Arc::clone(&outgoing_messages);
        let outgoing_display = display_tx.clone();
        tokio::task::spawn_local(async move {
            while let Some(envelope) = events.next().await {
                match &envelope.payload {
                    AgentEvent::ToolCallRequested { name, args, .. } if name == "send" => {
                        if let Some(text) = args
                            .get("body")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                        {
                            emit_marker(
                                "MKT:COMMS:SEND_REQUESTED",
                                &[("text", &json_quote(&short_display_line(&text, 120)))],
                            );
                            outgoing_counter.fetch_add(1, Ordering::SeqCst);
                            emit_marker(
                                "MKT:COMMS:OUTGOING",
                                &[("text", &json_quote(&short_display_line(&text, 120)))],
                            );
                            let _ = outgoing_display.send(DisplayEvent::Transcript {
                                prefix: "esp32".to_string(),
                                text,
                            });
                        }
                    }
                    AgentEvent::ToolExecutionCompleted {
                        name,
                        result,
                        is_error,
                        ..
                    } if name == "send" => {
                        emit_marker(
                            "MKT:COMMS:SEND_COMPLETED",
                            &[
                                ("is_error", &is_error.to_string()),
                                ("result", &json_quote(&short_display_line(&result, 120))),
                            ],
                        );
                    }
                    AgentEvent::ToolResultReceived { name, is_error, .. } if name == "send" => {
                        emit_marker(
                            "MKT:COMMS:SEND_RESULT_RECEIVED",
                            &[("is_error", &is_error.to_string())],
                        );
                    }
                    AgentEvent::InteractionComplete { interaction_id, result } => {
                        emit_marker(
                            "MKT:COMMS:INTERACTION_COMPLETE",
                            &[
                                ("id", &json_quote(&interaction_id.0.to_string())),
                                ("result", &json_quote(&short_display_line(&result, 120))),
                            ],
                        );
                    }
                    AgentEvent::InteractionFailed { interaction_id, error } => {
                        emit_marker(
                            "MKT:COMMS:INTERACTION_FAILED",
                            &[
                                ("id", &json_quote(&interaction_id.0.to_string())),
                                ("error", &json_quote(&short_display_line(&error, 120))),
                            ],
                        );
                    }
                    _ => {}
                }
            }
        });

        emit_marker(
            "MKT:COMMS:READY",
            &[
                ("session_id", &json_quote(&session_id.to_string())),
                ("peer_name", &json_quote("esp32-probe")),
                ("addr", &json_quote(&public_addr)),
            ],
        );

        let mut drain = Box::pin(drain);
        loop {
            tokio::select! {
                event = display_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Some(display) = status_display.as_deref_mut() {
                                let _ = handle_display_event(display, &mut transcript, event);
                            }
                        }
                        None => {
                            let _ = (&mut drain).await;
                            break;
                        }
                    }
                }
                result = &mut drain => {
                    let _ = result;
                    break;
                }
            }
        }

        while let Ok(event) = display_rx.try_recv() {
            if let Some(display) = status_display.as_deref_mut() {
                let _ = handle_display_event(display, &mut transcript, event);
            }
        }

        let memory_after = capture_memory_snapshot();
        emit_marker(
            "MKT:COMMS:PASS",
            &[
                ("incoming", &incoming_messages.load(Ordering::SeqCst).to_string()),
                ("outgoing", &outgoing_messages.load(Ordering::SeqCst).to_string()),
                ("heap_before", &memory_before.heap_free.to_string()),
                ("heap_after", &memory_after.heap_free.to_string()),
                ("internal_after", &memory_after.internal_free.to_string()),
                ("spiram_after", &memory_after.spiram_free.to_string()),
            ],
        );

        let _ = service.archive(&session_id).await;
        runtime_adapter.unregister_session(&session_id).await;

        Ok(CommsRunSummary {
            incoming_messages: incoming_messages.load(Ordering::SeqCst),
            outgoing_messages: outgoing_messages.load(Ordering::SeqCst),
            memory_before,
            memory_after,
        })
    })
}

fn phase0_host_trusted_peer() -> TrustedPeer {
    TrustedPeer {
        name: HOST_PEER_NAME.to_string(),
        pubkey: phase0_host_keypair().public_key(),
        addr: HOST_PEER_ADDR.to_string(),
        meta: meerkat_comms::PeerMeta::default(),
    }
}

fn phase0_host_keypair() -> Keypair {
    Keypair::from_secret(PHASE0_HOST_SECRET)
}

fn phase0_device_keypair() -> Keypair {
    Keypair::from_secret(PHASE0_DEVICE_SECRET)
}

async fn create_device_keep_alive_session(
    service: &Arc<impl SessionService>,
) -> anyhow::Result<SessionId> {
    let result = service
        .create_session(CreateSessionRequest {
            model: OPENAI_MODEL.to_string(),
            prompt: "".into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: Some(256),
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            build: Some(SessionBuildOptions {
                keep_alive: true,
                comms_name: Some("esp32-probe".to_string()),
                ..Default::default()
            }),
            labels: None,
        })
        .await
        .context("failed to create device keep-alive session")?;
    Ok(result.session_id)
}

fn device_comms_config() -> ResolvedCommsConfig {
    ResolvedCommsConfig {
        enabled: true,
        name: "esp32-probe".to_string(),
        inproc_namespace: None,
        identity_dir: PathBuf::from("/tmp/meerkat-phase0-esp32-identity"),
        trusted_peers_path: PathBuf::from("/tmp/meerkat-phase0-esp32-trusted.json"),
        listen_uds: None,
        listen_tcp: Some(
            format!("0.0.0.0:{COMMS_LISTEN_PORT}")
                .parse()
                .expect("valid comms listen addr"),
        ),
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        comms_config: CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: true,
        allow_external_unauthenticated: false,
    }
}

fn device_system_prompt(host_peer_name: &str) -> String {
    format!(
        "You are the ESP32 peer talking to {host_peer_name}. \
         Each turn, read the latest peer message and respond by calling the send tool exactly once with kind=peer_message and to={host_peer_name}. \
         Do not finish a turn with plain assistant text and do not skip the send call. \
         If the latest peer message asks you to run Python, micropython, code, or a snippet, you must call micropython_exec exactly once before your send call. \
         Use short safe embedded Python only, then summarize the observed print output or result in your outgoing send message. \
         Call the datetime tool only when it materially helps. \
         If the latest peer message contains STOP or DISMISS, send one short farewell via send and then stop sending further messages."
    )
}

fn inbound_text(
    ci: &meerkat_core::interaction::ClassifiedInboxInteraction,
) -> Option<String> {
    match &ci.interaction.content {
        meerkat_core::interaction::InteractionContent::Message { body, .. } => Some(body.clone()),
        _ => None,
    }
}

fn outgoing_send_text(envelope: &EventEnvelope<AgentEvent>) -> Option<String> {
    match &envelope.payload {
        AgentEvent::ToolCallRequested { name, args, .. } if name == "send" => args
            .get("body")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

fn handle_display_event(
    display: &mut StatusDisplay<'_>,
    transcript: &mut Vec<String>,
    event: DisplayEvent,
) -> anyhow::Result<()> {
    let result = match event {
        DisplayEvent::Stage {
            stage,
            headline,
            detail,
            background,
        } => display.show_stage(&stage, &headline, detail.as_deref(), background),
        DisplayEvent::Transcript { prefix, text } => {
            for line in wrap_display_text(&format!("{prefix}: {text}"), 26) {
                transcript.push(line);
            }
            if transcript.len() > 18 {
                let excess = transcript.len() - 18;
                transcript.drain(..excess);
            }
            display.show_chat("peer chat", transcript)
        }
    };
    #[cfg(target_os = "espidf")]
    {
        FreeRtos::delay_ms(1);
    }
    result
}

fn wrap_display_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(short_display_line(&current, width));
            current.clear();
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        out.push(short_display_line(&current, width));
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn enable_comms() -> bool {
    matches!(option_env!("ENABLE_COMMS"), Some("1"))
}

fn skip_single_node() -> bool {
    matches!(option_env!("SKIP_SINGLE_NODE"), Some("1"))
}

fn emit_meerkat_step(step: &str) {
    let memory = capture_memory_snapshot();
    emit_marker(
        "MKT:MEERKAT:STEP",
        &[
            ("name", &json_quote(step)),
            ("heap_free", &memory.heap_free.to_string()),
            ("heap_min_free", &memory.heap_min_free.to_string()),
            ("internal_free", &memory.internal_free.to_string()),
            ("spiram_free", &memory.spiram_free.to_string()),
        ],
    );
}

fn read_sse_stream(response: &mut Response<&mut EspHttpConnection>) -> anyhow::Result<usize> {
    let mut buf = [0_u8; 512];
    let mut pending = String::new();
    let mut saw_done = false;
    let mut saw_output_event = false;
    let mut event_seq = 0_usize;

    'outer: loop {
        let read = response
            .read(&mut buf)
            .map_err(|error| anyhow!("failed to read response body: {error}"))?;
        if read == 0 {
            break;
        }

        pending.push_str(&String::from_utf8_lossy(&buf[..read]));

        while let Some(newline_index) = pending.find('\n') {
            let line = pending[..newline_index].trim_end_matches('\r').to_string();
            pending.replace_range(..=newline_index, "");

            let should_stop =
                handle_sse_line(&line, &mut saw_done, &mut saw_output_event, &mut event_seq)?;
            if should_stop {
                break 'outer;
            }
        }
    }

    if !saw_done && !pending.trim().is_empty() {
        handle_sse_line(
            pending.trim_end_matches('\r'),
            &mut saw_done,
            &mut saw_output_event,
            &mut event_seq,
        )?;
    }

    if !saw_output_event {
        bail!("stream completed without an output event");
    }
    if !saw_done {
        bail!("stream completed without a done marker");
    }

    Ok(event_seq)
}

fn handle_sse_line(
    line: &str,
    saw_done: &mut bool,
    saw_output_event: &mut bool,
    event_seq: &mut usize,
) -> anyhow::Result<bool> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let payload = data.trim_start();

    if payload.is_empty() {
        return Ok(false);
    }
    if payload == "[DONE]" {
        *saw_done = true;
        return Ok(true);
    }

    let event: Value = serde_json::from_str(payload)
        .with_context(|| format!("failed to parse SSE payload: {payload}"))?;
    if let Some(error) = event.get("error") {
        bail!("provider stream error: {error}");
    }

    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if event_type.ends_with(".delta")
        || (!*saw_output_event && response_completed_contains_text(&event))
    {
        *saw_output_event = true;
        *event_seq += 1;
        emit_marker(
            "MKT:PROVIDER:CHUNK",
            &[
                ("seq", &event_seq.to_string()),
                ("type", &json_quote(event_type)),
                ("bytes", &payload.len().to_string()),
            ],
        );
    }

    if matches!(event_type, "response.done" | "response.completed") {
        *saw_done = true;
        return Ok(true);
    }

    Ok(false)
}

fn response_completed_contains_text(event: &Value) -> bool {
    event
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("message")
                    && item
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|content| {
                            content.iter().any(|block| {
                                block.get("type").and_then(Value::as_str) == Some("output_text")
                            })
                        })
            })
        })
}

fn read_response_body(response: &mut Response<&mut EspHttpConnection>) -> anyhow::Result<String> {
    let mut buf = [0_u8; 512];
    let mut body = String::new();
    loop {
        let read = response
            .read(&mut buf)
            .map_err(|error| anyhow!("failed to read error body: {error}"))?;
        if read == 0 {
            break;
        }
        body.push_str(&String::from_utf8_lossy(&buf[..read]));
    }
    Ok(body)
}

fn capture_memory_snapshot() -> MemorySnapshot {
    MemorySnapshot {
        heap_free: unsafe { sys::esp_get_free_heap_size() },
        heap_min_free: unsafe { sys::esp_get_minimum_free_heap_size() },
        internal_free: heap_caps_get_free_size(sys::MALLOC_CAP_INTERNAL),
        internal_min_free: heap_caps_get_minimum_free_size(sys::MALLOC_CAP_INTERNAL),
        internal_largest: heap_caps_get_largest_free_block(sys::MALLOC_CAP_INTERNAL),
        spiram_free: heap_caps_get_free_size(sys::MALLOC_CAP_SPIRAM),
        spiram_min_free: heap_caps_get_minimum_free_size(sys::MALLOC_CAP_SPIRAM),
        spiram_largest: heap_caps_get_largest_free_block(sys::MALLOC_CAP_SPIRAM),
    }
}

fn heap_caps_get_free_size(caps: u32) -> u32 {
    usize_to_u32(unsafe { sys::heap_caps_get_free_size(caps) })
}

fn heap_caps_get_minimum_free_size(caps: u32) -> u32 {
    usize_to_u32(unsafe { sys::heap_caps_get_minimum_free_size(caps) })
}

fn heap_caps_get_largest_free_block(caps: u32) -> u32 {
    usize_to_u32(unsafe { sys::heap_caps_get_largest_free_block(caps) })
}

fn usize_to_u32(value: usize) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

fn unix_time_secs() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn emit_marker(marker: &str, fields: &[(&str, &str)]) {
    if fields.is_empty() {
        println!("{marker}");
        log::info!("{marker}");
    } else {
        let suffix = fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{marker} {suffix}");
        log::info!("{marker} {suffix}");
    }
    let _ = std::io::stdout().flush();
}

fn json_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<encoding-error>\"".to_string())
}

impl<'d> StatusDisplay<'d> {
    fn try_new(spi: SPI2<'d>, pins: Pins) -> anyhow::Result<Self> {
        let dc = PinDriver::output(pins.gpio11).context("failed to configure lcd dc")?;
        let rst = PinDriver::output(pins.gpio9).context("failed to configure lcd rst")?;
        let mut backlight =
            PinDriver::output(pins.gpio14).context("failed to configure lcd backlight")?;
        let spi_device = SpiDeviceDriver::new_single(
            spi,
            pins.gpio10,
            pins.gpio13,
            None::<AnyInputPin>,
            Some(pins.gpio12),
            &SpiDriverConfig::new(),
            &spi::config::Config::new().baudrate(40.MHz().into()),
        )
        .context("failed to configure lcd spi")?;

        let buffer = Box::leak(Box::new([0_u8; 1024]));
        let interface = SpiInterface::new(spi_device, dc, buffer);
        let mut delay = FreeRtos;
        let mut panel = Builder::new(ST7789, interface)
            .reset_pin(rst)
            .display_size(LCD_WIDTH, LCD_HEIGHT)
            .display_offset(LCD_OFFSET_X, LCD_OFFSET_Y)
            .init(&mut delay)
            .map_err(|error| anyhow!("failed to init st7789 panel: {error:?}"))?;

        panel
            .clear(Rgb565::BLACK)
            .map_err(|error| anyhow!("failed to clear lcd: {error:?}"))?;
        backlight
            .set_low()
            .map_err(|error| anyhow!("failed to enable lcd backlight: {error:?}"))?;

        Ok(Self {
            panel,
            _backlight: backlight,
        })
    }

    fn show_stage(
        &mut self,
        stage: &str,
        headline: &str,
        detail: Option<&str>,
        background: Rgb565,
    ) -> anyhow::Result<()> {
        Rectangle::new(
            Point::zero(),
            Size::new(LCD_WIDTH.into(), LCD_HEIGHT.into()),
        )
        .into_styled(PrimitiveStyle::with_fill(background))
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to paint lcd background: {error:?}"))?;

        let header_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let body_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

        Text::with_baseline(
            &stage.to_uppercase(),
            Point::new(8, 14),
            header_style,
            Baseline::Top,
        )
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to draw lcd stage: {error:?}"))?;

        Text::with_baseline(
            &short_display_line(headline, 24),
            Point::new(8, 48),
            body_style,
            Baseline::Top,
        )
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to draw lcd headline: {error:?}"))?;

        if let Some(detail) = detail {
            Text::with_baseline(
                &short_display_line(detail, 28),
                Point::new(8, 66),
                body_style,
                Baseline::Top,
            )
            .draw(&mut self.panel)
            .map_err(|error| anyhow!("failed to draw lcd detail: {error:?}"))?;
        }

        Ok(())
    }

    fn show_chat(&mut self, title: &str, lines: &[String]) -> anyhow::Result<()> {
        Rectangle::new(
            Point::zero(),
            Size::new(LCD_WIDTH.into(), LCD_HEIGHT.into()),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(0, 8, 20)))
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to paint lcd chat background: {error:?}"))?;

        let header_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let body_style = MonoTextStyle::new(&FONT_6X10, Rgb565::new(180, 220, 255));

        Text::with_baseline(title, Point::new(8, 12), header_style, Baseline::Top)
            .draw(&mut self.panel)
            .map_err(|error| anyhow!("failed to draw lcd chat title: {error:?}"))?;

        let mut y = 42;
        for line in lines.iter().rev().take(18).collect::<Vec<_>>().into_iter().rev() {
            Text::with_baseline(line, Point::new(8, y), body_style, Baseline::Top)
                .draw(&mut self.panel)
                .map_err(|error| anyhow!("failed to draw lcd chat line: {error:?}"))?;
            y += 14;
        }

        Ok(())
    }

}

fn short_display_line(input: &str, max_chars: usize) -> String {
    let mut out = input.replace('\n', " ");
    if out.chars().count() <= max_chars {
        return out;
    }
    out = out
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn required_build_input<'a>(value: Option<&'a str>, name: &str) -> anyhow::Result<&'a str> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => bail!("missing required build input {name}"),
    }
}
