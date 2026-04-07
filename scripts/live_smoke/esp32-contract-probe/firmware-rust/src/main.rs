use std::convert::Infallible;
use std::ffi::{c_char, CString};
use std::io::{BufRead as _, Write as _};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::ptr;
use std::ptr::NonNull;
use std::slice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
use embedded_svc::wifi::{
    AccessPointInfo, AuthMethod, ClientConfiguration, Configuration as WifiConfiguration,
    PmfConfiguration,
};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection, Response};
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::sys;
use esp_idf_svc::tls::X509;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use futures::StreamExt;
use meerkat_client::{LlmClientAdapter, OpenAiClient};
use meerkat_comms::agent::wrap_with_comms;
use meerkat_comms::{
    CommsConfig, CommsRuntime, Keypair, ResolvedCommsConfig, TrustedPeer, TrustedPeers,
};
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
    SessionError, SessionService,
};
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentBuilder, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    BindOutcome, HookCapability, HookEntryConfig, HookExecutionMode, HookId, HookPoint,
    HookRuntimeConfig, HooksConfig,
};
use meerkat_hooks::{DefaultHookEngine, RuntimeHookResponse};
use meerkat_runtime::comms_drain::spawn_comms_drain_with_observer;
use meerkat_runtime::input::{Input, InputOrigin, PromptInput};
use meerkat_runtime::{CompletionOutcome, InMemoryRuntimeStore, RuntimeSessionAdapter};
use meerkat_session::ephemeral::SessionAgentBuilder;
use meerkat_session::PersistentSessionService;
use meerkat_store::{MemoryBlobStore, MemoryStore};
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
const HOST_PROMPT_LISTEN_PORT: u16 = 4311;
const HOST_PEER_ADDR: &str = match option_env!("HOST_PEER_ADDR") {
    Some(value) => value,
    None => "tcp://127.0.0.1:4220",
};
const HOST_INPUT_MODE: &str = match option_env!("HOST_INPUT_MODE") {
    Some(value) => value,
    None => "comms",
};
const LCD_WIDTH: u16 = 172;
const LCD_HEIGHT: u16 = 320;
const LCD_OFFSET_X: u16 = 34;
const LCD_OFFSET_Y: u16 = 0;
const LCD_PIXEL_CLOCK_HZ: u32 = 12_000_000;
const LCD_PIN_SCLK: i32 = 40;
const LCD_PIN_MOSI: i32 = 45;
const LCD_PIN_DC: i32 = 41;
const LCD_PIN_RST: i32 = 39;
const LCD_PIN_CS: i32 = 42;
const LCD_PIN_BL: i32 = 48;
const MEERKAT_WORKER_STACK_BYTES: &[usize] = &[56 * 1024, 48 * 1024, 40 * 1024, 32 * 1024];
const PHASE0_HOST_SECRET: [u8; 32] = [7; 32];
const PHASE0_DEVICE_SECRET: [u8; 32] = [9; 32];
const OPENAI_WE1_PEM: &[u8] = concat!(
    include_str!("../../../../../meerkat-client/src/openai_we1.pem"),
    "\0"
)
.as_bytes();
const MARKER_BROADCAST_ADDR: &str = "255.255.255.255:42424";

static MARKER_UDP_SOCKET: OnceLock<UdpSocket> = OnceLock::new();

struct StatusDisplay {
    io: sys::esp_lcd_panel_io_handle_t,
    framebuffer: DisplayFramebuffer,
}

struct DisplayFramebuffer {
    ptr: NonNull<u16>,
    len: usize,
    storage_kind: &'static str,
}

impl DisplayFramebuffer {
    fn new(len: usize) -> anyhow::Result<Self> {
        let byte_len = len
            .checked_mul(std::mem::size_of::<u16>())
            .context("framebuffer size overflow")?;
        let (raw, storage_kind) = unsafe {
            let psram_caps = (sys::MALLOC_CAP_SPIRAM | sys::MALLOC_CAP_8BIT) as u32;
            let psram = sys::heap_caps_malloc(byte_len, psram_caps);
            if !psram.is_null() {
                (psram, "psram")
            } else {
                let heap_caps = sys::MALLOC_CAP_8BIT as u32;
                let heap = sys::heap_caps_malloc(byte_len, heap_caps);
                (heap, "heap")
            }
        };
        let ptr = NonNull::new(raw.cast::<u16>()).context("failed to allocate lcd framebuffer")?;
        let mut framebuffer = Self {
            ptr,
            len,
            storage_kind,
        };
        framebuffer.as_mut_slice().fill(0);
        Ok(framebuffer)
    }

    fn storage_kind(&self) -> &'static str {
        self.storage_kind
    }

    fn as_slice(&self) -> &[u16] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [u16] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for DisplayFramebuffer {
    fn drop(&mut self) {
        unsafe {
            sys::heap_caps_free(self.ptr.as_ptr().cast());
        }
    }
}

fn esp_ok(code: i32, context: &str) -> anyhow::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        bail!("{context} failed with esp_err={code}");
    }
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

/// Board-local UI signals. The probe firmware owns how these are rendered;
/// Meerkat only emits generic hooks, tool calls, and session events.
enum BoardUiSignal {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostInputMode {
    Comms,
    Serial,
}

enum SerialHostCommand {
    Prompt { text: String },
    Stop,
    Ping,
}

#[derive(Clone, Default)]
struct BoardUiEmitter {
    tx: Option<mpsc::UnboundedSender<BoardUiSignal>>,
}

impl BoardUiEmitter {
    fn new(tx: mpsc::UnboundedSender<BoardUiSignal>) -> Self {
        Self { tx: Some(tx) }
    }

    fn stage(
        &self,
        stage: impl Into<String>,
        headline: impl Into<String>,
        detail: Option<String>,
        background: Rgb565,
    ) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BoardUiSignal::Stage {
                stage: stage.into(),
                headline: headline.into(),
                detail,
                background,
            });
        }
    }

    fn transcript(&self, prefix: impl Into<String>, text: impl Into<String>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(BoardUiSignal::Transcript {
                prefix: prefix.into(),
                text: text.into(),
            });
        }
    }
}

fn build_probe_hook_engine(board_ui: BoardUiEmitter, scope: &'static str) -> DefaultHookEngine {
    let hooks = vec![
        ("run-started", HookPoint::RunStarted),
        ("run-completed", HookPoint::RunCompleted),
        ("run-failed", HookPoint::RunFailed),
        ("pre-tool", HookPoint::PreToolExecution),
        ("post-tool", HookPoint::PostToolExecution),
    ];

    let config = HooksConfig {
        entries: hooks
            .iter()
            .map(|(name, point)| HookEntryConfig {
                id: HookId::new(format!("phase0-{scope}-{name}")),
                point: *point,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "in_process",
                    Some(json!({ "name": format!("phase0-{scope}-{name}") })),
                )
                .unwrap_or_default(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    hooks
        .into_iter()
        .fold(DefaultHookEngine::new(config), |engine, (name, point)| {
            let handler_name = format!("phase0-{scope}-{name}");
            let board_ui = board_ui.clone();
            engine.with_in_process_handler(
                handler_name,
                Arc::new(move |invocation| {
                    let board_ui = board_ui.clone();
                    Box::pin(async move {
                        emit_marker(
                            "MKT:BOARD_HOOK",
                            &[
                                ("scope", &json_quote(scope)),
                                ("point", &json_quote(&format!("{point:?}").to_lowercase())),
                            ],
                        );
                        match point {
                            HookPoint::RunStarted => board_ui.stage(
                                "run",
                                "agent started",
                                invocation
                                    .prompt
                                    .as_deref()
                                    .map(|prompt| short_display_line(prompt, 26)),
                                Rgb565::new(0, 32, 64),
                            ),
                            HookPoint::RunCompleted => board_ui.stage(
                                "run",
                                "agent complete",
                                Some("awaiting next input".to_string()),
                                Rgb565::new(0, 72, 0),
                            ),
                            HookPoint::RunFailed => board_ui.stage(
                                "fail",
                                "agent failed",
                                invocation
                                    .error
                                    .as_deref()
                                    .map(|error| short_display_line(error, 26)),
                                Rgb565::new(88, 0, 0),
                            ),
                            HookPoint::PreToolExecution => board_ui.stage(
                                "tool",
                                invocation
                                    .tool_call
                                    .as_ref()
                                    .map(|tool| tool.name.as_str())
                                    .unwrap_or("tool"),
                                Some("running".to_string()),
                                Rgb565::new(0, 24, 72),
                            ),
                            HookPoint::PostToolExecution => board_ui.stage(
                                "tool",
                                invocation
                                    .tool_result
                                    .as_ref()
                                    .map(|tool| tool.name.as_str())
                                    .unwrap_or("tool"),
                                invocation.tool_result.as_ref().map(|tool| {
                                    if tool.is_error {
                                        short_display_line("error", 26)
                                    } else {
                                        short_display_line("completed", 26)
                                    }
                                }),
                                if invocation
                                    .tool_result
                                    .as_ref()
                                    .map(|tool| tool.is_error)
                                    .unwrap_or(false)
                                {
                                    Rgb565::new(88, 0, 0)
                                } else {
                                    Rgb565::new(0, 56, 40)
                                },
                            ),
                            _ => {}
                        }
                        Ok(RuntimeHookResponse::default())
                    })
                }),
            )
        })
}

#[derive(Clone)]
struct DeviceProbeAgentBuilder {
    host_peer_name: String,
    model: String,
    openai_api_key: String,
    openai_base_url: String,
    comms_runtime: Arc<CommsRuntime>,
    inbound_epoch: Arc<AtomicUsize>,
    board_ui: BoardUiEmitter,
}

#[derive(Clone)]
struct SerialProbeAgentBuilder {
    model: String,
    openai_api_key: String,
    openai_base_url: String,
    board_ui: BoardUiEmitter,
}

struct DeviceSendGuardDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    inbound_epoch: Arc<AtomicUsize>,
    sent_epoch: Arc<AtomicUsize>,
}

impl DeviceSendGuardDispatcher {
    fn new(
        inner: Arc<dyn AgentToolDispatcher>,
        inbound_epoch: Arc<AtomicUsize>,
        sent_epoch: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            inner,
            inbound_epoch,
            sent_epoch,
        }
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl AgentToolDispatcher for DeviceSendGuardDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == "send" {
            let epoch = self.inbound_epoch.load(Ordering::SeqCst);
            let previous = self.sent_epoch.swap(epoch, Ordering::SeqCst);
            if epoch == previous {
                emit_marker(
                    "MKT:COMMS:SEND_SUPPRESSED",
                    &[("epoch", &epoch.to_string())],
                );
                return Ok(ToolDispatchOutcome::from(ToolResult::new(
                    call.id.to_string(),
                    "{\"kind\":\"peer_message\",\"status\":\"sent\"}".to_string(),
                    false,
                )));
            }
            let outcome = self.inner.dispatch(call).await;
            if outcome.is_err() {
                self.sent_epoch.store(previous, Ordering::SeqCst);
            }
            return outcome;
        }
        self.inner.dispatch(call).await
    }

    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn bind_wait_interrupt(
        self: Arc<Self>,
        rx: meerkat_core::wait_interrupt::WaitInterruptReceiver,
    ) -> Result<BindOutcome, meerkat_core::wait_interrupt::WaitInterruptBindError> {
        let outcome = Arc::clone(&self.inner).bind_wait_interrupt(rx)?;
        let bound = outcome.was_bound();
        let inner = outcome.into_dispatcher();
        let wrapped: Arc<dyn AgentToolDispatcher> = Arc::new(DeviceSendGuardDispatcher::new(
            inner,
            Arc::clone(&self.inbound_epoch),
            Arc::clone(&self.sent_epoch),
        ));
        Ok(if bound {
            BindOutcome::Bound(wrapped)
        } else {
            BindOutcome::Skipped(wrapped)
        })
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner_session_id: SessionId,
    ) -> Result<BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        let outcome = Arc::clone(&self.inner).bind_ops_lifecycle(registry, owner_session_id)?;
        let bound = outcome.was_bound();
        let inner = outcome.into_dispatcher();
        let wrapped: Arc<dyn AgentToolDispatcher> = Arc::new(DeviceSendGuardDispatcher::new(
            inner,
            Arc::clone(&self.inbound_epoch),
            Arc::clone(&self.sent_epoch),
        ));
        Ok(if bound {
            BindOutcome::Bound(wrapped)
        } else {
            BindOutcome::Skipped(wrapped)
        })
    }
}

struct MicroPythonToolArgs {
    code: String,
    background: bool,
}

#[derive(Debug)]
struct SerialRunSummary {
    prompts_completed: usize,
    memory_before: MemorySnapshot,
    memory_after: MemorySnapshot,
}

impl MicroPythonToolArgs {
    fn parse(call: ToolCallView<'_>) -> Result<Self, ToolError> {
        let raw: Value = call.parse_args().map_err(|error| {
            ToolError::invalid_arguments(MICROPYTHON_TOOL_NAME, error.to_string())
        })?;
        let code = raw
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::invalid_arguments(MICROPYTHON_TOOL_NAME, "missing string field 'code'")
            })?
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
                Err(format!(
                    "embedded MicroPython init failed with esp_err={err}"
                ))
            }
        });
        init.clone().map_err(ToolError::execution_failed)
    }

    fn exec_sync(
        &self,
        call: ToolCallView<'_>,
        args: MicroPythonToolArgs,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if args.background {
            return Err(ToolError::Unavailable {
                name: MICROPYTHON_TOOL_NAME.to_string(),
                reason: "background micropython execution is not enabled in this phase-0 firmware"
                    .to_string(),
            });
        }
        Self::ensure_ready()?;
        let _guard = self
            .exec_lock
            .lock()
            .map_err(|_| ToolError::execution_failed("micropython execution lock poisoned"))?;
        let code = CString::new(args.code.clone()).map_err(|_| {
            ToolError::invalid_arguments(MICROPYTHON_TOOL_NAME, "code contains interior NUL byte")
        })?;
        let mut result = Phase0MpyResult {
            job_id: 0,
            background: false,
            ok: false,
            truncated: false,
            status_code: 0,
            payload_len: 0,
            payload: [0; 2048],
        };
        emit_marker(
            "MKT:MICROPY:REQUESTED",
            &[("bytes", &args.code.len().to_string())],
        );
        let err = unsafe { phase0_mpy_exec_sync(code.as_ptr(), 15_000, &mut result) };
        if err != 0 {
            emit_marker("MKT:MICROPY:FAILED", &[("esp_err", &err.to_string())]);
            return Err(ToolError::execution_failed(format!(
                "embedded MicroPython exec failed with esp_err={err}"
            )));
        }

        let payload_len = result
            .payload_len
            .min(result.payload.len().saturating_sub(1));
        let payload = unsafe {
            let bytes =
                std::slice::from_raw_parts(result.payload.as_ptr() as *const u8, payload_len);
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
                &[(
                    "error",
                    &json_quote(&format!("probe-main panicked: {error:?}")),
                )],
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
            (
                "heap_free",
                &unsafe { sys::esp_get_free_heap_size() }.to_string(),
            ),
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
    let sys_loop = EspSystemEventLoop::take().context("failed to take system event loop")?;
    let nvs = EspDefaultNvsPartition::take().context("failed to take default nvs partition")?;

    let mut display_boot_status = String::from("deferred");
    let mut status_display: Option<StatusDisplay> = None;

    if park_only {
        emit_marker("MKT:PARK:ENABLED", &[]);
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs)).context("failed to create esp wifi")?,
        sys_loop,
    )
    .context("failed to wrap blocking wifi")?;

    let wifi_ip = connect_wifi(&mut wifi, wifi_ssid, wifi_pass, None)?;
    if let Err(error) = init_marker_broadcast() {
        emit_marker(
            "MKT:NETMARKER:INIT_FAIL",
            &[(
                "error",
                &json_quote(&short_display_line(&error.to_string(), 80)),
            )],
        );
    } else {
        emit_marker(
            "MKT:NETMARKER:READY",
            &[
                ("wifi_ip", &json_quote(&wifi_ip)),
                ("display_status", &json_quote(&display_boot_status)),
            ],
        );
    }

    status_display = match StatusDisplay::try_new() {
        Ok(mut display) => {
            emit_marker("MKT:DISPLAY:OK", &[("mode", "\"waveshare-147-status\"")]);
            display_boot_status = String::from("init_ok");
            match display.show_stage(
                "boot",
                "phase 0 contract probe",
                Some("bringing up board"),
                Rgb565::new(0, 32, 72),
            ) {
                Ok(()) => {
                    display_boot_status = String::from("stage_ok");
                }
                Err(error) => {
                    display_boot_status =
                        format!("stage_fail:{}", short_display_line(&error.to_string(), 80));
                    emit_marker(
                        "MKT:DISPLAY:STAGE_FAIL",
                        &[("error", &json_quote(&error.to_string()))],
                    );
                }
            }
            emit_marker(
                "MKT:DISPLAY:BOOT_STATUS",
                &[("status", &json_quote(&display_boot_status))],
            );
            Some(display)
        }
        Err(error) => {
            display_boot_status =
                format!("init_fail:{}", short_display_line(&error.to_string(), 80));
            emit_marker(
                "MKT:DISPLAY:FAIL",
                &[(
                    "error",
                    &json_quote(&short_display_line(&error.to_string(), 40)),
                )],
            );
            emit_marker(
                "MKT:DISPLAY:BOOT_STATUS",
                &[("status", &json_quote(&display_boot_status))],
            );
            eprintln!("display init failed: {error:#}");
            None
        }
    };

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
                (
                    "meerkat_tool_calls",
                    &meerkat_summary.tool_calls.to_string(),
                ),
                ("meerkat_tasks", &meerkat_summary.task_count.to_string()),
                (
                    "meerkat_heap_before",
                    &meerkat_summary.memory_before.heap_free.to_string(),
                ),
                (
                    "meerkat_heap_after",
                    &meerkat_summary.memory_after.heap_free.to_string(),
                ),
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

    match host_input_mode() {
        HostInputMode::Comms if enable_comms() => {
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
        HostInputMode::Serial => {
            if let Some(display) = status_display.as_mut() {
                let _ = display.show_stage(
                    "serial",
                    "arming usb host",
                    Some("awaiting prompt"),
                    Rgb565::new(0, 24, 72),
                );
            }
            run_serial_probe(openai_api_key, status_display.as_mut())?;
        }
        HostInputMode::Comms => {}
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
        let external_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(EspMicroPythonToolDispatcher::new());
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(DeviceSendGuardDispatcher::new(
            wrap_with_comms(external_tools, Arc::clone(&self.comms_runtime)),
            Arc::clone(&self.inbound_epoch),
            Arc::new(AtomicUsize::new(0)),
        ));
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

        let comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = self.comms_runtime.clone();
        let hooks = build_probe_hook_engine(self.board_ui.clone(), "comms");

        let agent: ProbeDynAgent = AgentBuilder::new()
            .model(req.model.clone())
            .system_prompt(
                req.system_prompt
                    .clone()
                    .unwrap_or_else(|| device_system_prompt(&self.host_peer_name)),
            )
            .max_tokens_per_turn(req.max_tokens.unwrap_or(256))
            .max_turns(2)
            .with_comms_runtime(comms_runtime)
            .with_hook_engine(Arc::new(hooks))
            .build(llm, tools, store)
            .await;

        Ok(ProbeSessionAgent::new(agent))
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait::async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait::async_trait
)]
impl SessionAgentBuilder for SerialProbeAgentBuilder {
    type Agent = ProbeSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(EspMicroPythonToolDispatcher::new());
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
        let hooks = build_probe_hook_engine(self.board_ui.clone(), "serial");

        let agent: ProbeDynAgent = AgentBuilder::new()
            .model(req.model.clone())
            .system_prompt(
                req.system_prompt
                    .clone()
                    .unwrap_or_else(serial_system_prompt),
            )
            .max_tokens_per_turn(req.max_tokens.unwrap_or(256))
            .max_turns(2)
            .with_hook_engine(Arc::new(hooks))
            .build(llm, tools, store)
            .await;

        Ok(ProbeSessionAgent::new(agent))
    }
}

fn connect_wifi(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    wifi_ssid: &str,
    wifi_pass: &str,
    status_display: Option<&mut StatusDisplay>,
) -> anyhow::Result<String> {
    let scan_configuration = WifiConfiguration::Client(ClientConfiguration::default());
    wifi.set_configuration(&scan_configuration)
        .context("failed to set initial wifi configuration")?;
    wifi.start().context("failed to start wifi")?;

    let scan_results = wifi.scan().context("failed to scan wifi access points")?;
    let matching_aps: Vec<AccessPointInfo> = scan_results
        .into_iter()
        .filter(|ap| ap.ssid.as_str() == wifi_ssid)
        .collect();
    emit_marker(
        "MKT:WIFI:SCAN",
        &[("matches", &matching_aps.len().to_string())],
    );
    for ap in matching_aps.iter().take(4) {
        emit_marker(
            "MKT:WIFI:AP",
            &[
                ("channel", &ap.channel.to_string()),
                ("signal", &ap.signal_strength.to_string()),
                (
                    "auth",
                    &json_quote(&format!("{:?}", ap.auth_method.unwrap_or(AuthMethod::None))),
                ),
            ],
        );
    }

    let selected_ap = matching_aps
        .iter()
        .filter(|ap| ap.channel <= 14)
        .max_by_key(|ap| ap.signal_strength)
        .cloned()
        .or_else(|| matching_aps.iter().max_by_key(|ap| ap.signal_strength).cloned())
        .ok_or_else(|| anyhow!("wifi ssid {wifi_ssid:?} not found during scan"))?;

    if selected_ap.channel > 14 {
        bail!(
            "wifi ssid {wifi_ssid:?} is only visible on unsupported 5GHz channel {}",
            selected_ap.channel
        );
    }

    let selected_auth = selected_ap.auth_method.unwrap_or(AuthMethod::WPA2Personal);
    emit_marker(
        "MKT:WIFI:SELECTED",
        &[
            ("channel", &selected_ap.channel.to_string()),
            ("signal", &selected_ap.signal_strength.to_string()),
            ("auth", &json_quote(&format!("{selected_auth:?}"))),
        ],
    );

    let mut attempts: Vec<(AuthMethod, PmfConfiguration)> = Vec::new();
    match selected_auth {
        AuthMethod::WPA3Personal => {
            attempts.push((AuthMethod::WPA3Personal, PmfConfiguration::Capable { required: true }));
        }
        AuthMethod::WPA2WPA3Personal => {
            attempts.push((
                AuthMethod::WPA2WPA3Personal,
                PmfConfiguration::Capable { required: false },
            ));
            attempts.push((AuthMethod::WPA2Personal, PmfConfiguration::NotCapable));
        }
        other => {
            attempts.push((other, PmfConfiguration::NotCapable));
        }
    }

    let mut last_error: Option<anyhow::Error> = None;
    for (attempt_index, (auth_method, pmf_cfg)) in attempts.into_iter().enumerate() {
        emit_marker(
            "MKT:WIFI:ATTEMPT",
            &[
                ("index", &(attempt_index + 1).to_string()),
                ("auth", &json_quote(&format!("{auth_method:?}"))),
                (
                    "pmf",
                    &json_quote(match pmf_cfg {
                        PmfConfiguration::NotCapable => "NotCapable",
                        PmfConfiguration::Capable { required: true } => "CapableRequired",
                        PmfConfiguration::Capable { required: false } => "CapableOptional",
                    }),
                ),
            ],
        );

        let configuration = WifiConfiguration::Client(ClientConfiguration {
            ssid: wifi_ssid
                .try_into()
                .map_err(|_| anyhow!("wifi ssid is too long"))?,
            bssid: Some(selected_ap.bssid),
            auth_method,
            password: wifi_pass
                .try_into()
                .map_err(|_| anyhow!("wifi password is too long"))?,
            channel: Some(selected_ap.channel),
            pmf_cfg,
            ..Default::default()
        });

        wifi.set_configuration(&configuration)
            .context("failed to set wifi client configuration")?;
        let _ = wifi.disconnect();
        wifi.connect().context("failed to connect wifi")?;

        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let connected = wifi
                .is_connected()
                .context("failed to query wifi connection state")?;
            let ip_info = wifi
                .wifi()
                .sta_netif()
                .get_ip_info()
                .context("failed to read station ip info")?;
            if connected && ip_info.ip.to_string() != "0.0.0.0" {
                emit_marker(
                    "MKT:WIFI:OK",
                    &[("ip", &json_quote(&ip_info.ip.to_string()))],
                );
                if let Some(display) = status_display {
                    let detail = format!("ip {}", ip_info.ip);
                    let _ = display.show_stage(
                        "wifi",
                        "connected",
                        Some(&detail),
                        Rgb565::new(0, 72, 0),
                    );
                }
                return Ok(ip_info.ip.to_string());
            }
            thread::sleep(Duration::from_millis(250));
        }

        emit_marker(
            "MKT:WIFI:ATTEMPT_FAIL",
            &[("index", &(attempt_index + 1).to_string())],
        );
        last_error = Some(anyhow!(
            "wifi attempt {} timed out before netif ip came up",
            attempt_index + 1
        ));
    }

    Err(last_error.unwrap_or_else(|| anyhow!("wifi did not come up")))
}

fn wait_for_time_sync(status_display: Option<&mut StatusDisplay>) -> anyhow::Result<()> {
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

fn host_input_mode() -> HostInputMode {
    match HOST_INPUT_MODE {
        "hostlink" | "HOSTLINK" | "host_link" | "HOST_LINK" | "serial" | "SERIAL" => {
            HostInputMode::Serial
        }
        _ => HostInputMode::Comms,
    }
}

fn serial_system_prompt() -> String {
    "You are the ESP32 embedded probe. \
     Respond directly to the latest host prompt. \
     If the prompt asks you to run Python, micropython, code, or a snippet, you must call micropython_exec exactly once before you answer. \
     When the prompt includes a snippet after `with:` or on following lines, run that exact snippet unchanged. \
     After micropython_exec completes, answer exactly in this format and nothing else: `Printed output: <printed text>\\nResult: <result>`. \
     Never call micropython_exec more than once per prompt. \
     If the prompt does not ask for code, do not call micropython_exec.".to_string()
}

fn parse_serial_host_command(line: &str) -> anyhow::Result<SerialHostCommand> {
    let raw: Value =
        serde_json::from_str(line).context("host-link command must be valid JSON object")?;
    let command = raw
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("host-link command missing string field 'cmd'"))?;
    match command {
        "prompt" => {
            let text = raw
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("host-link prompt missing string field 'text'"))?;
            Ok(SerialHostCommand::Prompt {
                text: text.to_string(),
            })
        }
        "stop" => Ok(SerialHostCommand::Stop),
        "ping" => Ok(SerialHostCommand::Ping),
        other => bail!("unsupported host-link command {other:?}"),
    }
}

fn spawn_host_command_reader(
    tx: mpsc::UnboundedSender<SerialHostCommand>,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("phase0-hostlink".to_string())
        .spawn(move || {
            let socket = match UdpSocket::bind(("0.0.0.0", HOST_PROMPT_LISTEN_PORT)) {
                Ok(socket) => socket,
                Err(error) => {
                    emit_marker(
                        "MKT:HOSTLINK:LISTEN_FAIL",
                        &[("error", &json_quote(&short_display_line(&error.to_string(), 80)))],
                    );
                    return;
                }
            };
            emit_marker(
                "MKT:HOSTLINK:LISTENING",
                &[
                    ("port", &HOST_PROMPT_LISTEN_PORT.to_string()),
                    ("transport", "\"udp-broadcast\""),
                ],
            );
            let mut buffer = [0u8; 2048];
            loop {
                let (len, peer_addr) = match socket.recv_from(&mut buffer) {
                    Ok(result) => result,
                    Err(error) => {
                        emit_marker(
                            "MKT:HOSTLINK:READ_FAIL",
                            &[("error", &json_quote(&short_display_line(&error.to_string(), 80)))],
                        );
                        thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                };
                emit_marker(
                    "MKT:HOSTLINK:CLIENT_CONNECTED",
                    &[("peer", &json_quote(&peer_addr.to_string()))],
                );
                let payload = match std::str::from_utf8(&buffer[..len]) {
                    Ok(payload) => payload.trim(),
                    Err(error) => {
                        emit_marker(
                            "MKT:HOSTLINK:CMD_PARSE_FAIL",
                            &[("error", &json_quote(&short_display_line(&error.to_string(), 80)))],
                        );
                        continue;
                    }
                };
                if payload.is_empty() {
                    continue;
                }
                match parse_serial_host_command(payload) {
                    Ok(command) => {
                        match &command {
                            SerialHostCommand::Prompt { text } => emit_marker(
                                "MKT:HOSTLINK:CMD_RX",
                                &[("kind", "\"prompt\""), ("bytes", &text.len().to_string())],
                            ),
                            SerialHostCommand::Stop => {
                                emit_marker("MKT:HOSTLINK:CMD_RX", &[("kind", "\"stop\"")])
                            }
                            SerialHostCommand::Ping => {
                                emit_marker("MKT:HOSTLINK:CMD_RX", &[("kind", "\"ping\"")])
                            }
                        }
                        if tx.send(command).is_err() {
                            return;
                        }
                    }
                    Err(error) => emit_marker(
                        "MKT:HOSTLINK:CMD_PARSE_FAIL",
                        &[("error", &json_quote(&short_display_line(&error.to_string(), 80)))],
                    ),
                }
            }
        })
        .context("failed to spawn host command reader")
}

fn make_serial_prompt_input(text: impl Into<String>) -> Input {
    let mut prompt = PromptInput::new(text.into(), None);
    prompt.header.source = InputOrigin::External {
        source_name: "host_link".to_string(),
    };
    Input::Prompt(prompt)
}

fn drain_board_ui_queue(
    board_ui_rx: &mut mpsc::UnboundedReceiver<BoardUiSignal>,
    mut status_display: Option<&mut StatusDisplay>,
    transcript: &mut Vec<String>,
) -> anyhow::Result<()> {
    while let Ok(event) = board_ui_rx.try_recv() {
        if let Some(display) = status_display.as_deref_mut() {
            handle_board_ui_signal(display, transcript, event)?;
        }
    }
    Ok(())
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
    emit_marker(
        "MKT:MEERKAT:THREAD_FALLBACK",
        &[
            ("mode", "\"main_task_first\""),
            ("reason", "\"waveshare-147-inline-runtime-probe\""),
        ],
    );
    return run_meerkat_turn_inner(openai_api_key.to_string(), memory_before);

    #[allow(unreachable_code)]
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
        &[("mode", "\"current_thread_no_time\"")],
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("failed to build tokio runtime for meerkat probe")?;
    emit_meerkat_step("after_runtime_build");

    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        emit_meerkat_step("inside_block_on");
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(EspMicroPythonToolDispatcher::new());
        emit_meerkat_step("after_dispatcher");
        emit_marker(
            "MKT:MEERKAT:TOOLS_EXPOSED",
            &[
                ("count", &dispatcher.tools().len().to_string()),
                ("names", &json_quote("micropython_exec")),
            ],
        );

        let hooks = build_probe_hook_engine(BoardUiEmitter::default(), "single-node");
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
                        if name == MICROPYTHON_TOOL_NAME {
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
                 You must call the micropython_exec tool exactly once. \
                 Use micropython_exec to run short Python that prints 'hello from micropython' and sets result = 6 * 7. \
                 After the tool completes, answer in one short sentence that includes the python result.",
            )
            .max_tokens_per_turn(96)
            .with_hook_engine(Arc::new(hooks))
            .build(llm, dispatcher, Arc::new(ProbeNoopStore))
            .await;
        emit_meerkat_step("after_agent_build");

        emit_marker(
            "MKT:MEERKAT:RUN_START",
            &[("model", &json_quote(OPENAI_MODEL))],
        );
        let result = agent
            .run_with_events(
                "Call micropython_exec exactly once with code that prints 'hello from micropython' and sets result = 6 * 7. Then answer in one short sentence confirming embedded Meerkat is alive and include the python result."
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
        let task_count = 0usize;

        emit_marker(
            "MKT:MEERKAT:SKILLS_OK",
            &[("resolved", &evidence.skill_resolved.to_string())],
        );

        if !evidence.saw_micropython {
            bail!(
                "meerkat run did not exercise required tools (micropython={}, tasks={task_count})",
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
    mut status_display: Option<&mut StatusDisplay>,
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

    #[cfg(target_os = "espidf")]
    let summary = {
        emit_marker(
            "MKT:COMMS:THREAD_FALLBACK",
            &[("mode", "\"main_task_local_runtime\"")],
        );
        run_comms_probe_inner(
            openai_api_key.to_string(),
            wifi_ip.to_string(),
            memory_before,
            status_display.as_deref_mut(),
        )?
    };

    #[cfg(not(target_os = "espidf"))]
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
            (
                "internal_after",
                &summary.memory_after.internal_free.to_string(),
            ),
            (
                "spiram_after",
                &summary.memory_after.spiram_free.to_string(),
            ),
        ],
    );

    Ok(())
}

#[inline(never)]
fn run_comms_probe_inner(
    openai_api_key: String,
    wifi_ip: String,
    memory_before: MemorySnapshot,
    mut status_display: Option<&mut StatusDisplay>,
) -> anyhow::Result<CommsRunSummary> {
    emit_marker(
        "MKT:COMMS:RUNTIME_MODE",
        &[(
            "mode",
            "\"current_thread_runtime_backed_keep_alive_inline\"",
        )],
    );
    #[cfg(target_os = "espidf")]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build_local(tokio::runtime::LocalOptions::default())
        .context("failed to build tokio local runtime for comms probe")?;
    #[cfg(not(target_os = "espidf"))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("failed to build tokio runtime for comms probe")?;
    emit_marker("MKT:COMMS:STEP", &[("name", "\"runtime_built\"")]);
    let public_addr = format!("tcp://{wifi_ip}:{COMMS_LISTEN_PORT}");
    emit_marker(
        "MKT:COMMS:STEP",
        &[
            ("name", "\"public_addr_ready\""),
            ("addr", &json_quote(&public_addr)),
        ],
    );

    emit_marker(
        "MKT:COMMS:LISTENING",
        &[
            ("peer_name", &json_quote("esp32-probe")),
            (
                "peer_id",
                &json_quote(&phase0_device_keypair().public_key().to_peer_id()),
            ),
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

    emit_marker("MKT:COMMS:STEP", &[("name", "\"before_local_block_on\"")]);
    let comms_future = Box::pin(run_comms_probe_async(
        openai_api_key,
        public_addr,
        memory_before,
        status_display.take(),
    ));
    #[cfg(target_os = "espidf")]
    {
        runtime.block_on(comms_future)
    }
    #[cfg(not(target_os = "espidf"))]
    {
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, comms_future)
    }
}

#[inline(never)]
async fn run_comms_probe_async(
    openai_api_key: String,
    public_addr: String,
    memory_before: MemorySnapshot,
    mut status_display: Option<&mut StatusDisplay>,
) -> anyhow::Result<CommsRunSummary> {
    emit_marker("MKT:COMMS:STEP", &[("name", "\"inside_local_block_on\"")]);
    let (board_ui_tx, mut board_ui_rx) = tokio::sync::mpsc::unbounded_channel::<BoardUiSignal>();
    let board_ui = BoardUiEmitter::new(board_ui_tx);
    emit_marker("MKT:COMMS:STEP", &[("name", "\"display_channel_ready\"")]);
    let mut transcript = Vec::new();
    let trusted = TrustedPeers {
        peers: vec![phase0_host_trusted_peer()],
    };
    emit_marker(
        "MKT:COMMS:STEP",
        &[("name", "\"before_comms_runtime_new\"")],
    );
    let mut comms_runtime =
        CommsRuntime::new_with_state(device_comms_config(), phase0_device_keypair(), trusted)
            .await
            .context("failed to build ESP32 comms runtime")?;
    emit_marker("MKT:COMMS:STEP", &[("name", "\"after_comms_runtime_new\"")]);
    comms_runtime
        .start_listeners()
        .await
        .context("failed to start ESP32 comms listeners")?;
    emit_marker("MKT:COMMS:STEP", &[("name", "\"after_start_listeners\"")]);
    let comms_runtime = Arc::new(comms_runtime);
    let inbound_epoch = Arc::new(AtomicUsize::new(1));
    let session_store: Arc<dyn meerkat_core::SessionStore> = Arc::new(MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::new());
    emit_marker(
        "MKT:COMMS:STEP",
        &[("name", "\"before_persistent_service_new\"")],
    );
    let service = Arc::new(PersistentSessionService::new(
        DeviceProbeAgentBuilder {
            host_peer_name: HOST_PEER_NAME.to_string(),
            model: OPENAI_MODEL.to_string(),
            openai_api_key,
            openai_base_url: OPENAI_BASE_URL.to_string(),
            comms_runtime: Arc::clone(&comms_runtime),
            inbound_epoch: Arc::clone(&inbound_epoch),
            board_ui: board_ui.clone(),
        },
        1,
        session_store,
        Some(runtime_store.clone()),
        blob_store.clone(),
    ));
    emit_marker(
        "MKT:COMMS:STEP",
        &[("name", "\"after_persistent_service_new\"")],
    );
    let runtime_adapter = Arc::new(RuntimeSessionAdapter::persistent(runtime_store, blob_store));
    emit_marker("MKT:COMMS:STEP", &[("name", "\"after_runtime_adapter\"")]);
    let session_id = create_device_keep_alive_session(&service).await?;
    emit_marker(
        "MKT:COMMS:STEP",
        &[
            ("name", "\"after_create_keep_alive_session\""),
            ("session_id", &json_quote(&session_id.to_string())),
        ],
    );

    runtime_adapter
        .ensure_session_with_executor(
            session_id.clone(),
            Box::new(ProbeSessionRuntimeExecutor::new(
                Arc::clone(&service),
                session_id.clone(),
            )),
        )
        .await;
    emit_marker(
        "MKT:COMMS:STEP",
        &[("name", "\"after_ensure_session_with_executor\"")],
    );

    let session_comms = service
        .comms_runtime(&session_id)
        .await
        .context("device session missing comms runtime")?;
    emit_marker(
        "MKT:COMMS:STEP",
        &[("name", "\"after_session_comms_lookup\"")],
    );

    let incoming_messages = Arc::new(AtomicUsize::new(0));
    let outgoing_messages = Arc::new(AtomicUsize::new(0));
    let incoming_counter = Arc::clone(&incoming_messages);
    let inbound_epoch_for_observer = Arc::clone(&inbound_epoch);
    let incoming_display = board_ui.clone();
    let drain = spawn_comms_drain_with_observer(
        Arc::clone(&runtime_adapter),
        session_id.clone(),
        session_comms.clone(),
        None,
        Some(Arc::new(move |ci| {
            if let Some(text) = inbound_text(&ci) {
                inbound_epoch_for_observer.fetch_add(1, Ordering::SeqCst);
                incoming_counter.fetch_add(1, Ordering::SeqCst);
                emit_marker(
                    "MKT:COMMS:INCOMING",
                    &[("text", &json_quote(&short_display_line(&text, 120)))],
                );
                incoming_display.transcript("host", text);
            }
        })),
    );
    emit_marker("MKT:COMMS:STEP", &[("name", "\"after_spawn_comms_drain\"")]);

    let mut events = service
        .subscribe_session_events(&session_id)
        .await
        .context("failed to subscribe to device session events")?;
    // Lifecycle/status can ride generic hooks, but transcript mirroring still
    // needs plain session events because hook payloads do not expose peer
    // message bodies.
    emit_marker(
        "MKT:COMMS:STEP",
        &[("name", "\"after_subscribe_session_events\"")],
    );
    let outgoing_counter = Arc::clone(&outgoing_messages);
    let outgoing_display = board_ui.clone();
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
                        outgoing_display.transcript("esp32", text);
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
                AgentEvent::InteractionComplete {
                    interaction_id,
                    result,
                } => {
                    emit_marker(
                        "MKT:COMMS:INTERACTION_COMPLETE",
                        &[
                            ("id", &json_quote(&interaction_id.0.to_string())),
                            ("result", &json_quote(&short_display_line(&result, 120))),
                        ],
                    );
                }
                AgentEvent::InteractionFailed {
                    interaction_id,
                    error,
                } => {
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
        "MKT:COMMS:STEP",
        &[("name", "\"after_spawn_local_events\"")],
    );

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
            event = board_ui_rx.recv() => {
                match event {
                    Some(event) => {
                        if let Some(display) = status_display.as_deref_mut() {
                            let _ = handle_board_ui_signal(display, &mut transcript, event);
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

    while let Ok(event) = board_ui_rx.try_recv() {
        if let Some(display) = status_display.as_deref_mut() {
            let _ = handle_board_ui_signal(display, &mut transcript, event);
        }
    }

    let memory_after = capture_memory_snapshot();
    emit_marker(
        "MKT:COMMS:PASS",
        &[
            (
                "incoming",
                &incoming_messages.load(Ordering::SeqCst).to_string(),
            ),
            (
                "outgoing",
                &outgoing_messages.load(Ordering::SeqCst).to_string(),
            ),
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
}

fn run_serial_probe(
    openai_api_key: &str,
    mut status_display: Option<&mut StatusDisplay>,
) -> anyhow::Result<()> {
    let memory_before = capture_memory_snapshot();
    emit_marker(
        "MKT:HOSTLINK:BOOTSTRAP_OK",
        &[
            ("heap_free", &memory_before.heap_free.to_string()),
            ("internal_free", &memory_before.internal_free.to_string()),
            ("spiram_free", &memory_before.spiram_free.to_string()),
        ],
    );

    if let Some(display) = status_display.as_deref_mut() {
        let _ = display.show_stage(
            "hostlink",
            "arming host link",
            Some("awaiting udp prompt"),
            Rgb565::new(0, 20, 64),
        );
    }

    #[cfg(target_os = "espidf")]
    let summary = run_serial_probe_inner(
        openai_api_key.to_string(),
        memory_before,
        status_display.as_deref_mut(),
    )?;

    #[cfg(not(target_os = "espidf"))]
    let summary = run_serial_probe_inner(
        openai_api_key.to_string(),
        memory_before,
        status_display.as_deref_mut(),
    )?;

    emit_marker(
        "MKT:HOSTLINK:SUMMARY",
        &[
            ("prompts", &summary.prompts_completed.to_string()),
            ("heap_after", &summary.memory_after.heap_free.to_string()),
            ("internal_after", &summary.memory_after.internal_free.to_string()),
            ("spiram_after", &summary.memory_after.spiram_free.to_string()),
        ],
    );
    Ok(())
}

#[inline(never)]
fn run_serial_probe_inner(
    openai_api_key: String,
    memory_before: MemorySnapshot,
    mut status_display: Option<&mut StatusDisplay>,
) -> anyhow::Result<SerialRunSummary> {
    emit_marker(
        "MKT:HOSTLINK:RUNTIME_MODE",
        &[("mode", "\"current_thread_runtime_backed_host_link\"")],
    );
    #[cfg(target_os = "espidf")]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build_local(tokio::runtime::LocalOptions::default())
        .context("failed to build tokio local runtime for serial probe")?;
    #[cfg(not(target_os = "espidf"))]
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("failed to build tokio runtime for serial probe")?;

    let serial_future = Box::pin(run_serial_probe_async(
        openai_api_key,
        memory_before,
        status_display.take(),
    ));
    #[cfg(target_os = "espidf")]
    {
        runtime.block_on(serial_future)
    }
    #[cfg(not(target_os = "espidf"))]
    {
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, serial_future)
    }
}

#[inline(never)]
async fn run_serial_probe_async(
    openai_api_key: String,
    memory_before: MemorySnapshot,
    mut status_display: Option<&mut StatusDisplay>,
) -> anyhow::Result<SerialRunSummary> {
    let (board_ui_tx, mut board_ui_rx) = mpsc::unbounded_channel::<BoardUiSignal>();
    let board_ui = BoardUiEmitter::new(board_ui_tx);
    let mut transcript = Vec::new();

    let session_store: Arc<dyn meerkat_core::SessionStore> = Arc::new(MemoryStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> = Arc::new(MemoryBlobStore::new());
    let service = Arc::new(PersistentSessionService::new(
        SerialProbeAgentBuilder {
            model: OPENAI_MODEL.to_string(),
            openai_api_key,
            openai_base_url: OPENAI_BASE_URL.to_string(),
            board_ui: board_ui.clone(),
        },
        1,
        session_store,
        Some(runtime_store.clone()),
        blob_store.clone(),
    ));
    let runtime_adapter = Arc::new(RuntimeSessionAdapter::persistent(runtime_store, blob_store));
    let session_id = create_serial_keep_alive_session(&service).await?;

    runtime_adapter
        .ensure_session_with_executor(
            session_id.clone(),
            Box::new(ProbeSessionRuntimeExecutor::new(
                Arc::clone(&service),
                session_id.clone(),
            )),
        )
        .await;

    let mut events = service
        .subscribe_session_events(&session_id)
        .await
        .context("failed to subscribe to serial probe session events")?;
    tokio::task::spawn_local(async move {
        while let Some(envelope) = events.next().await {
            match &envelope.payload {
                AgentEvent::ToolCallRequested { name, .. } if name == MICROPYTHON_TOOL_NAME => {
                    emit_marker(
                        "MKT:HOSTLINK:TOOL_REQUESTED",
                        &[("name", &json_quote(name))],
                    );
                }
                AgentEvent::InteractionComplete {
                    interaction_id,
                    result,
                } => emit_marker(
                    "MKT:HOSTLINK:INTERACTION_COMPLETE",
                    &[
                        ("id", &json_quote(&interaction_id.0.to_string())),
                        ("result", &json_quote(&short_display_line(result, 120))),
                    ],
                ),
                AgentEvent::InteractionFailed {
                    interaction_id,
                    error,
                } => emit_marker(
                    "MKT:HOSTLINK:INTERACTION_FAILED",
                    &[
                        ("id", &json_quote(&interaction_id.0.to_string())),
                        ("error", &json_quote(&short_display_line(error, 120))),
                    ],
                ),
                _ => {}
            }
        }
    });

    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<SerialHostCommand>();
    let _reader = spawn_host_command_reader(command_tx)?;
    board_ui.stage(
        "hostlink",
        "host ready",
        Some(format!("udp {HOST_PROMPT_LISTEN_PORT} json")),
        Rgb565::new(0, 24, 72),
    );
    drain_board_ui_queue(
        &mut board_ui_rx,
        status_display.as_deref_mut(),
        &mut transcript,
    )?;
    emit_marker(
        "MKT:HOSTLINK:READY",
        &[
            ("session_id", &json_quote(&session_id.to_string())),
            ("transport", "\"udp-broadcast\""),
            ("port", &HOST_PROMPT_LISTEN_PORT.to_string()),
        ],
    );

    let mut prompts_completed = 0usize;
    loop {
        tokio::select! {
            event = board_ui_rx.recv() => {
                if let Some(event) = event {
                    if let Some(display) = status_display.as_deref_mut() {
                        let _ = handle_board_ui_signal(display, &mut transcript, event);
                    }
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    SerialHostCommand::Prompt { text } => {
                        board_ui.transcript("host", text.clone());
                        drain_board_ui_queue(
                            &mut board_ui_rx,
                            status_display.as_deref_mut(),
                            &mut transcript,
                        )?;
                        let (accept_outcome, completion) = runtime_adapter
                            .accept_input_with_completion(&session_id, make_serial_prompt_input(text))
                            .await
                            .context("failed to accept host-link prompt")?;
                        match accept_outcome {
                            meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } => emit_marker(
                                "MKT:HOSTLINK:PROMPT_ACCEPTED",
                                &[("input_id", &json_quote(&input_id.to_string()))],
                            ),
                            meerkat_runtime::accept::AcceptOutcome::Deduplicated { input_id, existing_id } => emit_marker(
                                "MKT:HOSTLINK:PROMPT_DEDUP",
                                &[
                                    ("input_id", &json_quote(&input_id.to_string())),
                                    ("existing_id", &json_quote(&existing_id.to_string())),
                                ],
                            ),
                            meerkat_runtime::accept::AcceptOutcome::Rejected { reason } => {
                                bail!("host-link prompt rejected: {reason}");
                            }
                            _ => {
                                bail!("host-link prompt reached unsupported accept outcome");
                            }
                        }

                        let outcome = match completion {
                            Some(handle) => handle.wait().await,
                            None => CompletionOutcome::CompletedWithoutResult,
                        };
                        drain_board_ui_queue(
                            &mut board_ui_rx,
                            status_display.as_deref_mut(),
                            &mut transcript,
                        )?;
                        match outcome {
                            CompletionOutcome::Completed(result) => {
                                prompts_completed += 1;
                                board_ui.transcript("esp32", result.text.clone());
                                drain_board_ui_queue(
                                    &mut board_ui_rx,
                                    status_display.as_deref_mut(),
                                    &mut transcript,
                                )?;
                                emit_marker(
                                    "MKT:HOSTLINK:RUN_RESULT",
                                    &[
                                        ("turns", &result.turns.to_string()),
                                        ("tool_calls", &result.tool_calls.to_string()),
                                        ("text", &json_quote(&short_display_line(&result.text, 120))),
                                    ],
                                );
                            }
                            CompletionOutcome::CompletedWithoutResult => {
                                bail!("host-link prompt completed without result");
                            }
                            CompletionOutcome::CallbackPending { tool_name, .. } => {
                                bail!("host-link prompt hit callback boundary at tool {tool_name}");
                            }
                            CompletionOutcome::Abandoned(reason)
                            | CompletionOutcome::RuntimeTerminated(reason) => {
                                bail!("host-link prompt did not complete: {reason}");
                            }
                        }
                    }
                    SerialHostCommand::Stop => {
                        emit_marker("MKT:HOSTLINK:STOP", &[]);
                        break;
                    }
                    SerialHostCommand::Ping => {
                        emit_marker("MKT:HOSTLINK:PONG", &[]);
                    }
                }
            }
        }
    }

    drain_board_ui_queue(
        &mut board_ui_rx,
        status_display.as_deref_mut(),
        &mut transcript,
    )?;

    let memory_after = capture_memory_snapshot();
    emit_marker(
        "MKT:HOSTLINK:PASS",
        &[
            ("prompts", &prompts_completed.to_string()),
            ("heap_before", &memory_before.heap_free.to_string()),
            ("heap_after", &memory_after.heap_free.to_string()),
            ("internal_after", &memory_after.internal_free.to_string()),
            ("spiram_after", &memory_after.spiram_free.to_string()),
        ],
    );

    let _ = service.archive(&session_id).await;
    runtime_adapter.unregister_session(&session_id).await;

    Ok(SerialRunSummary {
        prompts_completed,
        memory_before,
        memory_after,
    })
}

async fn create_serial_keep_alive_session(
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
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                keep_alive: true,
                ..Default::default()
            }),
            labels: None,
        })
        .await
        .context("failed to create serial keep-alive session")?;
    Ok(result.session_id)
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
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
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
         Treat STOP or DISMISS as a stop command only when the latest peer message itself starts with `STOP` or `DISMISS`. \
         Only in that exact case, send one short farewell via send and then stop. \
         For every other peer message, never send `Farewell.`, `Goodbye.`, or any other closing message. \
         If the latest peer message asks you to run Python, micropython, code, or a snippet, you must call micropython_exec exactly once before your send call. \
         When the peer message includes an explicit snippet after `with:` or on the following lines, run that exact snippet unchanged. \
         After micropython_exec completes, send exactly one reply in this format and nothing else: `Printed output: <printed text>\\nResult: <result>`. \
         Never send `Acknowledged.`, `Working on it.`, or any other prelude, placeholder, or follow-up. \
         Never call send before micropython_exec for a snippet request, and never call send more than once for the same incoming peer message. \
         After the single normal reply for a snippet request, the interaction is complete; do not send any second reply, summary, or farewell for that same peer message. \
         Never infer a stop command from your own instructions or from prior conversation context."
    )
}

fn inbound_text(ci: &meerkat_core::interaction::ClassifiedInboxInteraction) -> Option<String> {
    match &ci.interaction.content {
        meerkat_core::interaction::InteractionContent::Message { body, .. } => Some(body.clone()),
        _ => None,
    }
}

fn handle_board_ui_signal(
    display: &mut StatusDisplay,
    transcript: &mut Vec<String>,
    event: BoardUiSignal,
) -> anyhow::Result<()> {
    let result = match event {
        BoardUiSignal::Stage {
            stage,
            headline,
            detail,
            background,
        } => display.show_stage(&stage, &headline, detail.as_deref(), background),
        BoardUiSignal::Transcript { prefix, text } => {
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
    let line = if fields.is_empty() {
        marker.to_string()
    } else {
        let suffix = fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{marker} {suffix}")
    };
    println!("{line}");
    log::info!("{line}");
    let _ = std::io::stdout().flush();
    if let Some(socket) = MARKER_UDP_SOCKET.get() {
        let _ = socket.send_to(line.as_bytes(), MARKER_BROADCAST_ADDR);
    }
}

fn json_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<encoding-error>\"".to_string())
}

fn init_marker_broadcast() -> anyhow::Result<()> {
    if MARKER_UDP_SOCKET.get().is_some() {
        return Ok(());
    }
    let socket = UdpSocket::bind("0.0.0.0:0").context("failed to bind marker udp socket")?;
    socket
        .set_broadcast(true)
        .context("failed to enable udp broadcast")?;
    let _ = MARKER_UDP_SOCKET.set(socket);
    Ok(())
}

impl StatusDisplay {
    fn try_new() -> anyhow::Result<Self> {
        emit_marker("MKT:DISPLAY:TRY_NEW_START", &[]);
        unsafe {
            let mut cfg = sys::gpio_config_t::default();
            cfg.pin_bit_mask = (1u64 << LCD_PIN_BL) | (1u64 << LCD_PIN_RST);
            cfg.mode = sys::gpio_mode_t_GPIO_MODE_OUTPUT;
            esp_ok(sys::gpio_config(&cfg), "lcd gpio_config")
                .context("failed to configure lcd control pins")?;
            emit_marker("MKT:DISPLAY:GPIO_OK", &[]);

            esp_ok(
                sys::gpio_set_level(LCD_PIN_BL as sys::gpio_num_t, 1),
                "lcd backlight on",
            )
            .context("failed to enable lcd backlight")?;
            emit_marker("MKT:DISPLAY:BACKLIGHT_OK", &[]);

            esp_ok(
                sys::gpio_set_level(LCD_PIN_RST as sys::gpio_num_t, 0),
                "lcd reset low",
            )
            .context("failed to drive lcd reset low")?;
            thread::sleep(Duration::from_millis(20));
            esp_ok(
                sys::gpio_set_level(LCD_PIN_RST as sys::gpio_num_t, 1),
                "lcd reset high",
            )
            .context("failed to drive lcd reset high")?;
            thread::sleep(Duration::from_millis(20));
            emit_marker("MKT:DISPLAY:RESET_OK", &[]);

            let mut buscfg = sys::spi_bus_config_t::default();
            buscfg.__bindgen_anon_1.mosi_io_num = LCD_PIN_MOSI;
            buscfg.__bindgen_anon_2.miso_io_num = -1;
            buscfg.sclk_io_num = LCD_PIN_SCLK;
            buscfg.__bindgen_anon_3.quadwp_io_num = -1;
            buscfg.__bindgen_anon_4.quadhd_io_num = -1;
            buscfg.data4_io_num = -1;
            buscfg.data5_io_num = -1;
            buscfg.data6_io_num = -1;
            buscfg.data7_io_num = -1;
            buscfg.max_transfer_sz = (usize::from(LCD_WIDTH) * usize::from(LCD_HEIGHT) * 2) as i32;

            esp_ok(
                sys::spi_bus_initialize(
                    sys::spi_host_device_t_SPI3_HOST,
                    &buscfg,
                    sys::spi_common_dma_t_SPI_DMA_CH_AUTO,
                ),
                "lcd spi_bus_initialize",
            )
            .context("failed to initialize lcd spi bus")?;
            emit_marker("MKT:DISPLAY:SPI_OK", &[]);

            let mut io_cfg = sys::esp_lcd_panel_io_spi_config_t::default();
            io_cfg.cs_gpio_num = LCD_PIN_CS;
            io_cfg.dc_gpio_num = LCD_PIN_DC;
            io_cfg.spi_mode = 0;
            io_cfg.pclk_hz = LCD_PIXEL_CLOCK_HZ;
            io_cfg.trans_queue_depth = 10;
            io_cfg.lcd_cmd_bits = 8;
            io_cfg.lcd_param_bits = 8;

            let mut io: sys::esp_lcd_panel_io_handle_t = ptr::null_mut();
            esp_ok(
                sys::esp_lcd_new_panel_io_spi(
                    sys::spi_host_device_t_SPI3_HOST as sys::esp_lcd_spi_bus_handle_t,
                    &io_cfg,
                    &mut io,
                ),
                "esp_lcd_new_panel_io_spi",
            )
            .context("failed to create lcd panel io")?;
            emit_marker("MKT:DISPLAY:PANEL_IO_OK", &[]);

            if io.is_null() {
                bail!("lcd panel io handle is null");
            }

            let memory_before_fb = capture_memory_snapshot();
            emit_marker(
                "MKT:DISPLAY:FB_ALLOC_BEGIN",
                &[
                    ("heap_free", &memory_before_fb.heap_free.to_string()),
                    ("internal_free", &memory_before_fb.internal_free.to_string()),
                    ("spiram_free", &memory_before_fb.spiram_free.to_string()),
                ],
            );

            let mut display = Self {
                io,
                framebuffer: DisplayFramebuffer::new(
                    usize::from(LCD_WIDTH) * usize::from(LCD_HEIGHT),
                )?,
            };
            let memory_after_fb = capture_memory_snapshot();
            emit_marker(
                "MKT:DISPLAY:FB_ALLOC_OK",
                &[
                    ("heap_free", &memory_after_fb.heap_free.to_string()),
                    ("internal_free", &memory_after_fb.internal_free.to_string()),
                    ("spiram_free", &memory_after_fb.spiram_free.to_string()),
                    ("storage", display.framebuffer.storage_kind()),
                ],
            );
            display.init_panel()?;
            emit_marker("MKT:DISPLAY:PANEL_INIT_OK", &[]);
            display.clear_framebuffer(Rgb565::BLACK);
            display.flush()?;
            emit_marker("MKT:DISPLAY:FLUSH_OK", &[]);
            Ok(display)
        }
    }

    fn tx(&self, cmd: u32, params: &[u8]) -> anyhow::Result<()> {
        unsafe {
            esp_ok(
                sys::esp_lcd_panel_io_tx_param(
                    self.io,
                    cmd as i32,
                    if params.is_empty() {
                        ptr::null()
                    } else {
                        params.as_ptr().cast()
                    },
                    params.len(),
                ),
                "esp_lcd_panel_io_tx_param",
            )
        }
    }

    fn init_panel(&mut self) -> anyhow::Result<()> {
        self.tx(sys::LCD_CMD_SLPOUT, &[])?;
        thread::sleep(Duration::from_millis(100));
        // Waveshare's reference driver applies mirror_x=true, mirror_y=false
        // after init, which maps to MADCTL MX on this panel.
        self.tx(0x36, &[0x40])?;
        self.tx(0x3A, &[0x55])?;
        self.tx(0xB0, &[0x00, 0xE8])?;
        self.tx(0xB2, &[0x0c, 0x0c, 0x00, 0x33, 0x33])?;
        self.tx(0xB7, &[0x75])?;
        self.tx(0xBB, &[0x1A])?;
        self.tx(0xC0, &[0x80])?;
        self.tx(0xC2, &[0x01, 0xff])?;
        self.tx(0xC3, &[0x13])?;
        self.tx(0xC4, &[0x20])?;
        self.tx(0xC6, &[0x0F])?;
        self.tx(0xD0, &[0xA4, 0xA1])?;
        self.tx(
            0xE0,
            &[
                0xD0, 0x0D, 0x14, 0x0D, 0x0D, 0x09, 0x38, 0x44, 0x4E, 0x3A, 0x17, 0x18, 0x2F, 0x30,
            ],
        )?;
        self.tx(
            0xE1,
            &[
                0xD0, 0x09, 0x0F, 0x08, 0x07, 0x14, 0x37, 0x44, 0x4D, 0x38, 0x15, 0x16, 0x2C, 0x2E,
            ],
        )?;
        self.tx(0x21, &[])?;
        self.tx(0x29, &[])?;
        self.tx(0x2C, &[])?;
        Ok(())
    }

    fn clear_framebuffer(&mut self, color: Rgb565) {
        self.framebuffer.as_mut_slice().fill(color.into_storage());
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        let x_start = LCD_OFFSET_X;
        let x_end = LCD_OFFSET_X + LCD_WIDTH - 1;
        let case = [
            (x_start >> 8) as u8,
            (x_start & 0xFF) as u8,
            (x_end >> 8) as u8,
            (x_end & 0xFF) as u8,
        ];
        self.tx(sys::LCD_CMD_CASET, &case)?;

        let mut line = vec![0u8; usize::from(LCD_WIDTH) * 2];
        for y in 0..usize::from(LCD_HEIGHT) {
            let y_u16 = y as u16 + LCD_OFFSET_Y;
            let ras = [
                (y_u16 >> 8) as u8,
                (y_u16 & 0xFF) as u8,
                (y_u16 >> 8) as u8,
                (y_u16 & 0xFF) as u8,
            ];
            self.tx(sys::LCD_CMD_RASET, &ras)?;

            let row = &self.framebuffer.as_slice()
                [y * usize::from(LCD_WIDTH)..(y + 1) * usize::from(LCD_WIDTH)];
            for (i, px) in row.iter().enumerate() {
                line[i * 2] = (px >> 8) as u8;
                line[i * 2 + 1] = (px & 0xFF) as u8;
            }
            unsafe {
                esp_ok(
                    sys::esp_lcd_panel_io_tx_color(
                        self.io,
                        sys::LCD_CMD_RAMWR as i32,
                        line.as_ptr().cast(),
                        line.len(),
                    ),
                    "esp_lcd_panel_io_tx_color",
                )?;
            }
        }
        Ok(())
    }

    fn show_stage(
        &mut self,
        stage: &str,
        headline: &str,
        detail: Option<&str>,
        background: Rgb565,
    ) -> anyhow::Result<()> {
        Rectangle::new(Point::zero(), self.size())
            .into_styled(PrimitiveStyle::with_fill(background))
            .draw(self)
            .map_err(|error| anyhow!("failed to paint lcd background: {error:?}"))?;

        let header_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let body_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

        Text::with_baseline(
            &stage.to_uppercase(),
            Point::new(8, 14),
            header_style,
            Baseline::Top,
        )
        .draw(self)
        .map_err(|error| anyhow!("failed to draw lcd stage: {error:?}"))?;

        Text::with_baseline(
            &short_display_line(headline, 24),
            Point::new(8, 48),
            body_style,
            Baseline::Top,
        )
        .draw(self)
        .map_err(|error| anyhow!("failed to draw lcd headline: {error:?}"))?;

        if let Some(detail) = detail {
            Text::with_baseline(
                &short_display_line(detail, 28),
                Point::new(8, 66),
                body_style,
                Baseline::Top,
            )
            .draw(self)
            .map_err(|error| anyhow!("failed to draw lcd detail: {error:?}"))?;
        }

        self.flush()
    }

    fn show_chat(&mut self, title: &str, lines: &[String]) -> anyhow::Result<()> {
        Rectangle::new(Point::zero(), self.size())
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(0, 8, 20)))
            .draw(self)
            .map_err(|error| anyhow!("failed to paint lcd chat background: {error:?}"))?;

        let header_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let body_style = MonoTextStyle::new(&FONT_6X10, Rgb565::new(180, 220, 255));

        Text::with_baseline(title, Point::new(8, 12), header_style, Baseline::Top)
            .draw(self)
            .map_err(|error| anyhow!("failed to draw lcd chat title: {error:?}"))?;

        let mut y = 42;
        for line in lines
            .iter()
            .rev()
            .take(18)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            Text::with_baseline(line, Point::new(8, y), body_style, Baseline::Top)
                .draw(self)
                .map_err(|error| anyhow!("failed to draw lcd chat line: {error:?}"))?;
            y += 14;
        }

        self.flush()
    }
}

impl OriginDimensions for StatusDisplay {
    fn size(&self) -> Size {
        Size::new(LCD_WIDTH.into(), LCD_HEIGHT.into())
    }
}

impl DrawTarget for StatusDisplay {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x < 0 || point.y < 0 {
                continue;
            }
            let x = point.x as u16;
            let y = point.y as u16;
            if x >= LCD_WIDTH || y >= LCD_HEIGHT {
                continue;
            }
            let idx = usize::from(y) * usize::from(LCD_WIDTH) + usize::from(x);
            self.framebuffer.as_mut_slice()[idx] = color.into_storage();
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
