use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use tokio::time::{Duration, timeout};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum Lane {
    Build,
    System,
    Live,
    Smoke,
    /// Auth-lane: live OAuth + refresh + dedup tests. Requires
    /// ANTHROPIC_API_KEY / OPENAI_API_KEY / GOOGLE_API_KEY env, and
    /// optional CLAUDE_CODE_OAUTH_TOKEN for interactive flow tests.
    Auth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionMode {
    Cargo,
    Prebuilt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum E2eSelection {
    Lane(Lane),
    Scenario(u16),
    Suite(String),
    SmokeTest(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactRequirement {
    RustBin(RustBinRequirement),
    RustTest(RustTestRequirement),
    NodeBuild { cwd: String },
    PythonEnv { cwd: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct RustBinRequirement {
    pub package: String,
    pub bin: String,
    pub features: BTreeSet<String>,
    pub all_features: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct RustTestRequirement {
    pub package: String,
    pub test_target: String,
    pub features: BTreeSet<String>,
    pub all_features: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedSpec {
    pub label: String,
    pub lane: Lane,
    pub id: Option<u16>,
    pub suite: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct E2ePlan {
    pub specs: Vec<PlannedSpec>,
    pub requirements: Vec<ArtifactRequirement>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ArtifactManifest {
    #[serde(default)]
    pub rust_bins: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub rust_tests: BTreeMap<String, PathBuf>,
}

#[derive(Clone, Copy)]
enum OutputPolicy {
    CargoTest,
    Pytest,
    NodeTest,
    ExitOnly,
}

#[derive(Clone, Copy)]
enum CommandSpec {
    CargoTest {
        package: &'static str,
        test_target: &'static str,
        test_name: &'static str,
        features: &'static [&'static str],
        all_features: bool,
    },
    Pytest {
        test_file: &'static str,
        test_name: &'static str,
    },
    NodeTest {
        test_file: &'static str,
        test_name: &'static str,
    },
    Raw {
        argv: &'static [&'static str],
        output_policy: OutputPolicy,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandLockMode {
    ScenarioLocal,
    Shared,
}

struct CommandEntry {
    spec: &'static Spec,
    command: Vec<String>,
}

struct BuiltCommands {
    pre_commands: Vec<Vec<String>>,
    command: Vec<String>,
    output_policy: OutputPolicy,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Spec {
    id: Option<u16>,
    lane: Lane,
    title: &'static str,
    timeout_secs: u64,
    required_env: &'static [&'static [&'static str]],
    required_bins: &'static [&'static str],
    cwd: &'static str,
    env: &'static [(&'static str, &'static str)],
    cargo_bin_env: &'static [&'static str],
    pre_commands: &'static [&'static [&'static str]],
    command: CommandSpec,
}

struct CompletedCommand {
    output: String,
}

const WEB_RUNTIME_BUILD_IF_MISSING: &[&str] = &[
    "/bin/sh",
    "-c",
    "if test -f ../../../sdks/web/wasm/meerkat_web_runtime.js && test -f ../../../sdks/web/wasm/meerkat_web_runtime_bg.wasm; then :; else npm --prefix ../../../sdks/web run build; fi",
];

const MEERKAT_E2E_EXECUTION_MODE: &str = "MEERKAT_E2E_EXECUTION_MODE";
const MEERKAT_E2E_ARTIFACT_MANIFEST: &str = "MEERKAT_E2E_ARTIFACT_MANIFEST";

#[macro_export]
macro_rules! e2e_smoke_lane_entries {
    ($receiver:ident) => {
        $receiver! {
            scenario(e2e_smoke_s16_rpc_kitchen_sink, 16);
            scenario(e2e_smoke_s21_rpc_mob_callback_tools, 21);
            scenario(e2e_smoke_s22_rpc_transport_backpressure, 22);
            scenario(e2e_smoke_s27_cli_shell_and_structured_output, 27);
            scenario(e2e_smoke_s28_cli_signed_mobpack_deploy, 28);
            scenario(e2e_smoke_s30_cli_mob_flow_probe, 30);
            scenario(e2e_smoke_s40_python_sdk_mixed_provider_swarm_probe, 40);
            scenario(e2e_smoke_s44_typescript_sdk_mixed_provider_swarm_probe, 44);
            scenario(e2e_smoke_s47_browser_mobpack_session_flow, 47);
            scenario(e2e_smoke_s48_browser_raw_mob_lifecycle, 48);
            scenario(e2e_smoke_s49_rpc_rest_shared_realm_roundtrip, 49);
            scenario(e2e_smoke_s50_rest_cli_shared_realm_roundtrip, 50);
            scenario(e2e_smoke_s51_rpc_mcp_shared_realm_parity, 51);
            scenario(e2e_smoke_s52_cli_rpc_cli_continuity, 52);
            scenario(e2e_smoke_s53_cli_rest_cli_continuity, 53);
            scenario(e2e_smoke_s54_shared_realm_mob_visibility, 54);
            scenario(e2e_smoke_s55_rpc_rest_callback_peer_storm_resume, 55);
            scenario(e2e_smoke_s57_python_sdk_realtime_channel_session_exchange, 57);
            scenario(e2e_smoke_s58_python_sdk_realtime_member_respawn_continuity, 58);
            scenario(e2e_smoke_s59_typescript_sdk_realtime_channel_session_exchange, 59);
            scenario(e2e_smoke_s60_rust_sdk_realtime_channel_session_exchange, 60);
            scenario(e2e_smoke_s61_cli_realtime_bridge_session_roundtrip, 61);
            scenario(e2e_smoke_s62_rest_bootstrap_to_rust_sdk_realtime_channel_exchange, 62);
            scenario(e2e_smoke_s63_mcp_bootstrap_to_rust_sdk_member_realtime_exchange, 63);
            scenario(e2e_smoke_s64_python_sdk_realtime_member_model_switch_continuity, 64);
            scenario(e2e_smoke_s71_rust_sdk_realtime_audio_mob_collaboration_roundtrip, 71);
            scenario(e2e_smoke_s72_rust_sdk_realtime_audio_member_model_switch_continuity, 72);
            scenario(e2e_smoke_s73_cli_generate_image_openai_default, 73);
            scenario(e2e_smoke_s74_python_sdk_gemini_image_provider_params, 74);
            scenario(e2e_smoke_s75_typescript_sdk_openai_image_provider_params, 75);
            scenario(e2e_smoke_s76_typescript_sdk_cross_provider_image_model_switch_stress, 76);
            scenario(e2e_smoke_s77_typescript_sdk_stacked_image_turn, 77);
            scenario(e2e_smoke_s78_typescript_sdk_cross_provider_image_relay, 78);
            scenario(e2e_smoke_s79_typescript_sdk_mob_image_critic, 79);
            scenario(e2e_smoke_s80_typescript_sdk_persisted_generated_image_resume, 80);
            scenario(e2e_smoke_s81_typescript_sdk_parallel_image_storm, 81);
            scenario(e2e_smoke_s82_typescript_sdk_blob_image_roundtrip, 82);
            suite(e2e_smoke_rpc_dynamic_tool_pickup, "rpc-dynamic-tool-pickup");
            suite(e2e_smoke_rpc_deferred_catalog_session, "rpc-deferred-catalog-session");
            suite(e2e_smoke_cli_background_job_active_turn, "cli-background-job-active-turn");
            suite(e2e_smoke_cli_background_job_idle_keepalive, "cli-background-job-idle-keepalive");
            suite(e2e_smoke_mob_live_smoke, "mob-live-smoke");
            suite(e2e_smoke_mob_flow_runtime_suite, "mob-flow-runtime");
            suite(e2e_smoke_mob_pictionary, "mob-pictionary");
        }
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SmokeEntry {
    Scenario {
        test_name: &'static str,
        id: u16,
    },
    Suite {
        test_name: &'static str,
        suite: &'static str,
    },
}

macro_rules! smoke_entry {
    (scenario, $test_name:ident, $id:literal) => {
        SmokeEntry::Scenario {
            test_name: stringify!($test_name),
            id: $id,
        }
    };
    (suite, $test_name:ident, $suite:literal) => {
        SmokeEntry::Suite {
            test_name: stringify!($test_name),
            suite: $suite,
        }
    };
}

macro_rules! smoke_entries {
    ($($kind:ident($test_name:ident, $value:tt);)*) => {
        &[
            $(smoke_entry!($kind, $test_name, $value),)*
        ]
    };
}

const SMOKE_ENTRIES: &[SmokeEntry] = crate::e2e_smoke_lane_entries!(smoke_entries);

#[derive(Clone, Debug)]
enum PreCommandState {
    Running,
    Done,
    Failed(String),
}

impl ExecutionMode {
    fn from_env() -> Result<Self, String> {
        match std::env::var(MEERKAT_E2E_EXECUTION_MODE) {
            Ok(value)
                if matches!(
                    value.as_str(),
                    "prebuilt" | "PREBUILT" | "Prebuilt" | "1" | "true" | "TRUE"
                ) =>
            {
                Ok(Self::Prebuilt)
            }
            Ok(value)
                if matches!(
                    value.as_str(),
                    "cargo" | "CARGO" | "Cargo" | "0" | "false" | "FALSE"
                ) =>
            {
                Ok(Self::Cargo)
            }
            Ok(value) => Err(format!(
                "unknown {MEERKAT_E2E_EXECUTION_MODE} value '{value}' (expected cargo or prebuilt)"
            )),
            Err(_) => Ok(Self::Cargo),
        }
    }
}

impl ArtifactManifest {
    pub fn from_json_str(json: &str) -> Result<Self, String> {
        serde_json::from_str(json)
            .map_err(|error| format!("invalid e2e artifact manifest: {error}"))
    }

    pub fn from_path(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "failed to read e2e artifact manifest {}: {error}",
                path.display()
            )
        })?;
        Self::from_json_str(&content)
    }

    fn from_env() -> Result<Self, String> {
        let path = std::env::var(MEERKAT_E2E_ARTIFACT_MANIFEST).map_err(|_| {
            format!(
                "{MEERKAT_E2E_EXECUTION_MODE}=prebuilt requires {MEERKAT_E2E_ARTIFACT_MANIFEST}"
            )
        })?;
        Self::from_path(Path::new(&path))
    }

    fn rust_bin(&self, bin: &str) -> Result<&Path, String> {
        self.rust_bins
            .get(bin)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                format!(
                    "e2e artifact manifest is missing rust_bins.{bin}; materialize the selected lane before running prebuilt mode"
                )
            })
    }

    fn rust_test(&self, package: &str, test_target: &str) -> Result<&Path, String> {
        let key = rust_test_manifest_key(package, test_target);
        self.rust_tests
            .get(&key)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                format!(
                    "e2e artifact manifest is missing rust_tests.{key}; materialize the selected lane before running prebuilt mode"
                )
            })
    }
}

pub fn plan_for_selection(selection: &E2eSelection) -> Result<E2ePlan, String> {
    let selected = select_specs(selection)?;
    Ok(plan_for_specs(&selected))
}

pub fn smoke_test_filter_for_selection(selection: &E2eSelection) -> Result<Option<String>, String> {
    match selection {
        E2eSelection::Lane(Lane::Smoke) => Ok(None),
        E2eSelection::Scenario(id) => {
            require_smoke_scenario_entry(*id)?;
            Ok(Some(format!("e2e_smoke_s{id}_")))
        }
        E2eSelection::Suite(name) => smoke_entry_for_suite(name)
            .map(SmokeEntry::test_name)
            .map(str::to_string)
            .map(Some)
            .ok_or_else(|| format!("unknown e2e-smoke suite '{name}'")),
        E2eSelection::SmokeTest(name) => {
            require_smoke_test_name(name)?;
            Ok(Some(name.clone()))
        }
        E2eSelection::Lane(lane) => Err(format!(
            "smoke test filters are only defined for the e2e-smoke lane, got {lane:?}"
        )),
    }
}

pub async fn run_catalog_scenario(id: u16) -> Result<(), String> {
    let Some(spec) = scenario_spec(id) else {
        return Err(format!("unknown catalog scenario id {id}"));
    };
    run_spec(spec).await
}

pub async fn run_named_suite(name: &str) -> Result<(), String> {
    let Some(spec) = suite_spec(name) else {
        return Err(format!("unknown lane suite '{name}'"));
    };
    run_spec(spec).await
}

pub async fn run_prebuilt_smoke_selection(
    selection: &E2eSelection,
    manifest_path: &Path,
) -> Result<(), String> {
    let mut selected = select_specs(selection)?;
    order_smoke_specs_for_runtime(&mut selected);
    let manifest = Arc::new(ArtifactManifest::from_path(manifest_path)?);
    let scheduler = Arc::new(SmokeScheduler::from_env(selected.len()));
    eprintln!(
        "e2e smoke scheduler: specs={}, jobs={}, realtime_jobs={}, media_jobs={}, mob_suite_jobs={}",
        selected.len(),
        scheduler.jobs,
        scheduler.realtime_jobs,
        scheduler.media_jobs,
        scheduler.mob_suite_jobs
    );

    let mut pending = FuturesUnordered::new();
    for selected_spec in selected {
        if selected_spec.spec.lane != Lane::Smoke {
            return Err(format!(
                "{} belongs to {:?}, not the e2e-smoke lane",
                run_label(selected_spec.spec),
                selected_spec.spec.lane
            ));
        }
        let manifest = Arc::clone(&manifest);
        let scheduler = Arc::clone(&scheduler);
        pending.push(run_prebuilt_smoke_spec(
            selected_spec.spec,
            scheduler,
            manifest,
        ));
    }

    let mut failures = Vec::new();
    while let Some(result) = pending.next().await {
        match result {
            Ok(()) => {}
            Err(error) => failures.push(error),
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} e2e-smoke spec(s) failed:\n{}",
            failures.len(),
            failures
                .into_iter()
                .map(|failure| format!("- {failure}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}

async fn run_prebuilt_smoke_spec(
    spec: &'static Spec,
    scheduler: Arc<SmokeScheduler>,
    manifest: Arc<ArtifactManifest>,
) -> Result<(), String> {
    let _permits = scheduler.acquire(spec).await?;
    run_spec_with_mode(spec, ExecutionMode::Prebuilt, Some(manifest.as_ref())).await
}

struct SmokeScheduler {
    jobs: usize,
    realtime_jobs: usize,
    media_jobs: usize,
    mob_suite_jobs: usize,
    global: Arc<Semaphore>,
    realtime: Arc<Semaphore>,
    media: Arc<Semaphore>,
    mob_suite: Arc<Semaphore>,
}

impl SmokeScheduler {
    fn from_env(selected_count: usize) -> Self {
        let jobs = smoke_scheduler_limit("MEERKAT_E2E_SMOKE_JOBS", 12, selected_count);
        let realtime_jobs =
            smoke_scheduler_limit("MEERKAT_E2E_SMOKE_REALTIME_JOBS", 4, selected_count);
        let media_jobs = smoke_scheduler_limit("MEERKAT_E2E_SMOKE_MEDIA_JOBS", 6, selected_count);
        let mob_suite_jobs =
            smoke_scheduler_limit("MEERKAT_E2E_SMOKE_MOB_SUITE_JOBS", 2, selected_count);
        Self {
            jobs,
            realtime_jobs,
            media_jobs,
            mob_suite_jobs,
            global: Arc::new(Semaphore::new(jobs)),
            realtime: Arc::new(Semaphore::new(realtime_jobs)),
            media: Arc::new(Semaphore::new(media_jobs)),
            mob_suite: Arc::new(Semaphore::new(mob_suite_jobs)),
        }
    }

    async fn acquire(&self, spec: &'static Spec) -> Result<SmokePermits, String> {
        let class = smoke_runtime_class(spec);
        let class_permit =
            match class {
                SmokeRuntimeClass::Realtime => {
                    Some(self.realtime.clone().acquire_owned().await.map_err(|_| {
                        format!("realtime scheduler closed for {}", run_label(spec))
                    })?)
                }
                SmokeRuntimeClass::Media => {
                    Some(
                        self.media.clone().acquire_owned().await.map_err(|_| {
                            format!("media scheduler closed for {}", run_label(spec))
                        })?,
                    )
                }
                SmokeRuntimeClass::MobSuite => {
                    Some(self.mob_suite.clone().acquire_owned().await.map_err(|_| {
                        format!("mob-suite scheduler closed for {}", run_label(spec))
                    })?)
                }
                SmokeRuntimeClass::Standard => None,
            };
        let global = self.global.clone().acquire_owned().await.map_err(|_| {
            format!(
                "e2e smoke scheduler closed before starting {}",
                run_label(spec)
            )
        })?;
        eprintln!(
            "e2e smoke scheduler dispatch: {} [{class:?}]",
            run_label(spec)
        );
        Ok(SmokePermits {
            _global: global,
            _class: class_permit,
        })
    }
}

struct SmokePermits {
    _global: tokio::sync::OwnedSemaphorePermit,
    _class: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SmokeRuntimeClass {
    Standard,
    Realtime,
    Media,
    MobSuite,
}

fn smoke_runtime_class(spec: &Spec) -> SmokeRuntimeClass {
    let label = run_label(spec).to_ascii_lowercase();
    if label.contains("mob flow runtime")
        || label.contains("pictionary")
        || label.contains("partial resume collaborative joke")
    {
        return SmokeRuntimeClass::MobSuite;
    }
    if label.contains("image") || label.contains("audio") {
        return SmokeRuntimeClass::Media;
    }
    if label.contains("realtime") {
        return SmokeRuntimeClass::Realtime;
    }
    SmokeRuntimeClass::Standard
}

fn smoke_scheduler_limit(name: &str, default: usize, selected_count: usize) -> usize {
    let parsed = std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default);
    parsed.min(selected_count.max(1))
}

fn order_smoke_specs_for_runtime(selected: &mut [SelectedSpec]) {
    selected.sort_by_key(|selected_spec| Reverse(smoke_runtime_priority(selected_spec.spec)));
}

fn smoke_runtime_priority(spec: &Spec) -> u16 {
    match spec.id {
        Some(71) => return 900,
        Some(73) => return 880,
        Some(74) => return 870,
        Some(76) => return 860,
        Some(77) => return 850,
        Some(75) => return 840,
        Some(79) => return 830,
        Some(80) => return 820,
        Some(82) => return 810,
        Some(81) => return 800,
        Some(78) => return 790,
        Some(54) => return 700,
        Some(55) => return 690,
        Some(49) | Some(51) | Some(61) => return 680,
        _ => {}
    }

    let label = run_label(spec).to_ascii_lowercase();
    if label.contains("pictionary") {
        return 1_000;
    }
    if label.contains("mob flow runtime") {
        return 760;
    }
    if label.contains("partial resume collaborative joke") {
        return 750;
    }
    match smoke_runtime_class(spec) {
        SmokeRuntimeClass::Realtime => 600,
        SmokeRuntimeClass::Media => 500,
        SmokeRuntimeClass::MobSuite => 400,
        SmokeRuntimeClass::Standard => 100,
    }
}

fn select_specs(selection: &E2eSelection) -> Result<Vec<SelectedSpec>, String> {
    match selection {
        E2eSelection::Lane(Lane::Smoke) => SMOKE_ENTRIES
            .iter()
            .copied()
            .map(selected_spec_for_smoke_entry)
            .collect(),
        E2eSelection::Lane(lane) => Err(format!("planning is not implemented for {lane:?} lane")),
        E2eSelection::Scenario(id) => {
            let spec =
                scenario_spec(*id).ok_or_else(|| format!("unknown catalog scenario id {id}"))?;
            Ok(vec![SelectedSpec { spec, suite: None }])
        }
        E2eSelection::Suite(name) => {
            let spec = suite_spec(name).ok_or_else(|| format!("unknown lane suite '{name}'"))?;
            Ok(vec![SelectedSpec {
                spec,
                suite: Some(name.clone()),
            }])
        }
        E2eSelection::SmokeTest(name) => {
            let (spec, suite) = spec_for_smoke_test_name(name)?;
            Ok(vec![SelectedSpec { spec, suite }])
        }
    }
}

#[derive(Clone)]
struct SelectedSpec {
    spec: &'static Spec,
    suite: Option<String>,
}

impl SmokeEntry {
    fn test_name(self) -> &'static str {
        match self {
            Self::Scenario { test_name, .. } | Self::Suite { test_name, .. } => test_name,
        }
    }
}

fn smoke_entry_for_scenario_id(id: u16) -> Option<SmokeEntry> {
    SMOKE_ENTRIES
        .iter()
        .copied()
        .find(|entry| matches!(*entry, SmokeEntry::Scenario { id: entry_id, .. } if entry_id == id))
}

fn smoke_entry_for_suite(name: &str) -> Option<SmokeEntry> {
    SMOKE_ENTRIES
        .iter()
        .copied()
        .find(|entry| matches!(*entry, SmokeEntry::Suite { suite, .. } if suite == name))
}

fn smoke_entry_for_test_name(name: &str) -> Option<SmokeEntry> {
    SMOKE_ENTRIES
        .iter()
        .copied()
        .find(|entry| entry.test_name() == name)
}

fn require_smoke_scenario_entry(id: u16) -> Result<SmokeEntry, String> {
    if let Some(entry) = smoke_entry_for_scenario_id(id) {
        return Ok(entry);
    }

    let Some(spec) = scenario_spec(id) else {
        return Err(format!("unknown catalog scenario id {id}"));
    };
    Err(format!(
        "scenario {id} belongs to {:?}, not the e2e-smoke lane",
        spec.lane
    ))
}

fn require_smoke_test_name(name: &str) -> Result<SmokeEntry, String> {
    smoke_entry_for_test_name(name).ok_or_else(|| format!("unknown e2e-smoke test '{name}'"))
}

fn selected_spec_for_smoke_entry(entry: SmokeEntry) -> Result<SelectedSpec, String> {
    match entry {
        SmokeEntry::Scenario { id, .. } => {
            let spec =
                scenario_spec(id).ok_or_else(|| format!("unknown e2e-smoke scenario id {id}"))?;
            if spec.lane != Lane::Smoke {
                return Err(format!(
                    "scenario {id} belongs to {:?}, not the e2e-smoke lane",
                    spec.lane
                ));
            }
            Ok(SelectedSpec { spec, suite: None })
        }
        SmokeEntry::Suite { suite, .. } => {
            let spec =
                suite_spec(suite).ok_or_else(|| format!("unknown e2e-smoke suite '{suite}'"))?;
            Ok(SelectedSpec {
                spec,
                suite: Some(suite.to_string()),
            })
        }
    }
}

fn plan_for_specs(selected: &[SelectedSpec]) -> E2ePlan {
    let mut rust_bins = BTreeMap::<String, RustBinRequirement>::new();
    let mut rust_tests = BTreeMap::<String, RustTestRequirement>::new();
    let mut node_builds = BTreeSet::<String>::new();
    let mut python_envs = BTreeSet::<String>::new();

    for selected_spec in selected {
        collect_spec_requirements(
            selected_spec.spec,
            &mut rust_bins,
            &mut rust_tests,
            &mut node_builds,
            &mut python_envs,
        );
    }

    let mut requirements = Vec::new();
    requirements.extend(rust_bins.into_values().map(ArtifactRequirement::RustBin));
    requirements.extend(rust_tests.into_values().map(ArtifactRequirement::RustTest));
    requirements.extend(
        node_builds
            .into_iter()
            .map(|cwd| ArtifactRequirement::NodeBuild { cwd }),
    );
    requirements.extend(
        python_envs
            .into_iter()
            .map(|cwd| ArtifactRequirement::PythonEnv { cwd }),
    );
    requirements.sort();

    E2ePlan {
        specs: selected
            .iter()
            .map(|selected_spec| PlannedSpec {
                label: run_label(selected_spec.spec),
                lane: selected_spec.spec.lane,
                id: selected_spec.spec.id,
                suite: selected_spec.suite.clone(),
            })
            .collect(),
        requirements,
    }
}

fn spec_for_smoke_test_name(name: &str) -> Result<(&'static Spec, Option<String>), String> {
    let selected = selected_spec_for_smoke_entry(require_smoke_test_name(name)?)?;
    Ok((selected.spec, selected.suite))
}

fn collect_spec_requirements(
    spec: &Spec,
    rust_bins: &mut BTreeMap<String, RustBinRequirement>,
    rust_tests: &mut BTreeMap<String, RustTestRequirement>,
    node_builds: &mut BTreeSet<String>,
    python_envs: &mut BTreeSet<String>,
) {
    match spec.command {
        CommandSpec::CargoTest {
            package,
            test_target,
            features,
            all_features,
            ..
        } => {
            merge_rust_test(
                rust_tests,
                RustTestRequirement::new(package, test_target, features, all_features),
            );
            if let Some(bin) = default_bin_for_package(package) {
                merge_rust_bin(
                    rust_bins,
                    RustBinRequirement::new(package, bin, features, all_features),
                );
            }
        }
        CommandSpec::Pytest { .. } => {
            python_envs.insert(spec.cwd.to_string());
        }
        CommandSpec::NodeTest { .. } => {
            node_builds.insert(spec.cwd.to_string());
        }
        CommandSpec::Raw { argv, .. } => {
            if let Some(requirement) = parse_cargo_test_requirement(argv) {
                if let Some(bin) = default_bin_for_package(&requirement.package) {
                    merge_rust_bin(
                        rust_bins,
                        RustBinRequirement {
                            package: requirement.package.clone(),
                            bin: bin.to_string(),
                            features: requirement.features.clone(),
                            all_features: requirement.all_features,
                        },
                    );
                }
                merge_rust_test(rust_tests, requirement);
            }
            if command_mentions_node_setup(argv) {
                node_builds.insert(spec.cwd.to_string());
            }
            if command_mentions_python_setup(argv) {
                python_envs.insert(spec.cwd.to_string());
            }
        }
    }

    for command in spec.pre_commands {
        if let Some(requirement) = parse_cargo_build_requirement(command) {
            merge_rust_bin(rust_bins, requirement);
        }
        if command_mentions_node_setup(command) {
            node_builds.insert(spec.cwd.to_string());
        }
        if command_mentions_python_setup(command) {
            python_envs.insert(spec.cwd.to_string());
        }
    }

    for binary in spec.cargo_bin_env {
        if let Some(package) = package_for_bin(binary) {
            merge_rust_bin(
                rust_bins,
                RustBinRequirement::new(package, *binary, &[], false),
            );
        }
    }

    for (key, value) in spec.env {
        if key.starts_with("RKAT_TEST_BIN_") {
            if let Some(binary) = bin_from_rkat_test_bin_env(key)
                && let Some(package) = package_for_bin(&binary)
            {
                merge_rust_bin(
                    rust_bins,
                    RustBinRequirement::new(package, &binary, &[], false),
                );
            }
        } else if *key == "MEERKAT_BIN_PATH"
            && let Some(binary) = binary_from_artifact_path_template(value)
            && let Some(package) = package_for_bin(&binary)
        {
            merge_rust_bin(
                rust_bins,
                RustBinRequirement::new(package, &binary, &[], false),
            );
        }
    }
}

impl RustBinRequirement {
    fn new(
        package: impl Into<String>,
        bin: impl Into<String>,
        features: &[&str],
        all_features: bool,
    ) -> Self {
        Self {
            package: package.into(),
            bin: bin.into(),
            features: features
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
            all_features,
        }
    }

    fn key(&self) -> String {
        format!("{}:{}", self.package, self.bin)
    }
}

impl RustTestRequirement {
    fn new(
        package: impl Into<String>,
        test_target: impl Into<String>,
        features: &[&str],
        all_features: bool,
    ) -> Self {
        Self {
            package: package.into(),
            test_target: test_target.into(),
            features: features
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
            all_features,
        }
    }

    fn key(&self) -> String {
        rust_test_manifest_key(&self.package, &self.test_target)
    }
}

fn merge_rust_bin(
    requirements: &mut BTreeMap<String, RustBinRequirement>,
    requirement: RustBinRequirement,
) {
    let key = requirement.key();
    requirements
        .entry(key)
        .and_modify(|existing| {
            existing.features.extend(requirement.features.clone());
            existing.all_features |= requirement.all_features;
        })
        .or_insert(requirement);
}

fn merge_rust_test(
    requirements: &mut BTreeMap<String, RustTestRequirement>,
    requirement: RustTestRequirement,
) {
    let key = requirement.key();
    requirements
        .entry(key)
        .and_modify(|existing| {
            existing.features.extend(requirement.features.clone());
            existing.all_features |= requirement.all_features;
        })
        .or_insert(requirement);
}

fn parse_cargo_build_requirement(command: &[&str]) -> Option<RustBinRequirement> {
    if !is_cargo_command(command.first().copied()?) || command.get(1) != Some(&"build") {
        return None;
    }
    let package = command_value(command, "-p", "--package")?;
    let bin = command_value(command, "--bin", "--bin")
        .map(str::to_string)
        .or_else(|| default_bin_for_package(package).map(str::to_string))?;
    Some(RustBinRequirement {
        package: package.to_string(),
        bin,
        features: command_features(command),
        all_features: command.contains(&"--all-features"),
    })
}

fn parse_cargo_test_requirement(command: &[&str]) -> Option<RustTestRequirement> {
    if !is_cargo_command(command.first().copied()?) || command.get(1) != Some(&"test") {
        return None;
    }
    let package = command_value(command, "-p", "--package")?;
    let test_target = command_value(command, "--test", "--test")?;
    Some(RustTestRequirement {
        package: package.to_string(),
        test_target: test_target.to_string(),
        features: command_features(command),
        all_features: command.contains(&"--all-features"),
    })
}

fn is_cargo_command(command: &str) -> bool {
    command == "cargo"
        || command.ends_with("/repo-cargo")
        || command == "{repo_root}/scripts/repo-cargo"
}

fn command_value<'a>(command: &'a [&str], short: &str, long: &str) -> Option<&'a str> {
    command.windows(2).find_map(|window| {
        let key = window[0];
        (key == short || key == long).then_some(window[1])
    })
}

fn command_features(command: &[&str]) -> BTreeSet<String> {
    command_value(command, "--features", "--features")
        .map(split_features)
        .unwrap_or_default()
}

fn split_features(features: &str) -> BTreeSet<String> {
    features
        .split(',')
        .map(str::trim)
        .filter(|feature| !feature.is_empty())
        .map(str::to_string)
        .collect()
}

fn command_mentions_node_setup(command: &[&str]) -> bool {
    let command_text = command.join(" ");
    command_text.contains("npm")
        || command_text.contains("playwright")
        || command_text.contains("sdks/web")
}

fn command_mentions_python_setup(command: &[&str]) -> bool {
    command.join(" ").contains("pip install")
}

fn default_bin_for_package(package: &str) -> Option<&'static str> {
    match package {
        "rkat" => Some("rkat"),
        "meerkat-rpc" => Some("rkat-rpc"),
        "meerkat-rest" => Some("rkat-rest"),
        "meerkat-mcp-server" => Some("rkat-mcp"),
        _ => None,
    }
}

fn package_for_bin(bin: &str) -> Option<&'static str> {
    match bin {
        "rkat" => Some("rkat"),
        "rkat-rpc" => Some("meerkat-rpc"),
        "rkat-rest" => Some("meerkat-rest"),
        "rkat-mcp" => Some("meerkat-mcp-server"),
        _ => None,
    }
}

fn rkat_test_bin_env_name(bin: &str) -> String {
    format!(
        "RKAT_TEST_BIN_{}",
        bin.replace('-', "_").to_ascii_uppercase()
    )
}

fn bin_from_rkat_test_bin_env(key: &str) -> Option<String> {
    key.strip_prefix("RKAT_TEST_BIN_")
        .map(|name| name.to_ascii_lowercase().replace('_', "-"))
}

fn binary_from_artifact_path_template(value: &str) -> Option<String> {
    ["rkat-rpc", "rkat-rest", "rkat-mcp", "rkat"]
        .iter()
        .find(|binary| value.contains(*binary))
        .map(|binary| (*binary).to_string())
}

fn rust_test_manifest_key(package: &str, test_target: &str) -> String {
    format!("{package}:{test_target}")
}

fn run_label(spec: &Spec) -> String {
    match spec.id {
        Some(id) => format!("{id:02} {}", spec.title),
        None => spec.title.to_string(),
    }
}

async fn run_spec(spec: &'static Spec) -> Result<(), String> {
    run_spec_with_mode(spec, ExecutionMode::from_env()?, None).await
}

async fn run_spec_with_mode(
    spec: &'static Spec,
    execution_mode: ExecutionMode,
    manifest: Option<&ArtifactManifest>,
) -> Result<(), String> {
    let spec_started = Instant::now();
    eprintln!("e2e lane start: {}", run_label(spec));
    let owned_manifest;
    let manifest = match (execution_mode, manifest) {
        (ExecutionMode::Cargo, _) => None,
        (ExecutionMode::Prebuilt, Some(manifest)) => Some(manifest),
        (ExecutionMode::Prebuilt, None) => {
            owned_manifest = ArtifactManifest::from_env()?;
            Some(&owned_manifest)
        }
    };
    if let Some(message) = prereq_failure(spec, execution_mode) {
        if strict_prereqs_enabled() {
            return Err(format!("{}: {message}", run_label(spec)));
        }
        eprintln!("skipping {}: {message}", run_label(spec));
        return Ok(());
    }

    let cwd = workspace_root().join(spec.cwd);
    let scenario_target_dir = scenario_cargo_target_dir(spec)?;
    let env_overrides = match scenario_env(spec, &scenario_target_dir, execution_mode, manifest) {
        Ok(env_overrides) => env_overrides,
        Err(error) => {
            clean_scenario_target_dir_if_requested(spec, &scenario_target_dir);
            return Err(error);
        }
    };
    let commands = match build_commands_for_mode(spec, execution_mode, manifest) {
        Ok(commands) => commands,
        Err(error) => {
            clean_scenario_target_dir_if_requested(spec, &scenario_target_dir);
            return Err(error);
        }
    };

    let result = async {
        for pre_command in commands.pre_commands {
            run_pre_command(
                CommandEntry {
                    spec,
                    command: pre_command,
                },
                &cwd,
                &env_overrides,
            )
            .await?;
        }

        // macOS 26.3.1+ `codeSigningMonitor=2` SIGKILLs adhoc/linker-signed
        // binaries whose content hash was invalidated after signing (which
        // happens when any cargo invocation re-links between scenario runs).
        // Re-sign every `CARGO_BIN_EXE_*` binary with a fresh adhoc signature
        // so the scenario can `exec` them cleanly. No-op on non-macOS.
        ensure_binary_signatures_fresh(spec, &env_overrides).await;

        let completed =
            run_command(&commands.command, &cwd, &env_overrides, spec.timeout_secs).await?;
        if let Some(problem) = analyze_success_output(commands.output_policy, &completed.output) {
            return Err(format!(
                "{}: {} ({problem})",
                run_label(spec),
                command_display(&commands.command)
            ));
        }

        Ok(())
    }
    .await;
    clean_scenario_target_dir_if_requested(spec, &scenario_target_dir);
    eprintln!(
        "e2e lane done: {} in {:.3}s",
        run_label(spec),
        spec_started.elapsed().as_secs_f64()
    );
    result
}

#[allow(clippy::await_holding_lock)] // Lock is explicitly dropped before await
async fn run_pre_command(
    entry: CommandEntry,
    cwd: &Path,
    env_overrides: &[(String, String)],
) -> Result<(), String> {
    let _shared_lock = match pre_command_lock_mode_parts(&entry.command) {
        CommandLockMode::ScenarioLocal => None,
        CommandLockMode::Shared => {
            Some(acquire_shared_pre_command_lock(&entry.command, cwd).await?)
        }
    };
    let env_signature = env_overrides
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let key = format!(
        "{}::{}::{}",
        cwd.display(),
        command_display_with_env(&entry.command, env_overrides),
        env_signature
    );
    loop {
        let mut completed = completed_pre_commands().lock().unwrap();
        match completed.get(&key) {
            Some(PreCommandState::Done) => return Ok(()),
            Some(PreCommandState::Failed(error)) => return Err(error.clone()),
            Some(PreCommandState::Running) => {
                drop(completed);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            None => {
                completed.insert(key.clone(), PreCommandState::Running);
                break;
            }
        }
    }

    let result = run_command(&entry.command, cwd, env_overrides, entry.spec.timeout_secs)
        .await
        .map(|_| ());
    let mut completed = completed_pre_commands().lock().unwrap();
    match &result {
        Ok(()) => {
            completed.insert(key, PreCommandState::Done);
        }
        Err(error) => {
            completed.insert(key, PreCommandState::Failed(error.clone()));
        }
    }
    result
}

struct SharedPreCommandLock {
    path: PathBuf,
}

impl Drop for SharedPreCommandLock {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!(
                "warning: failed to release e2e shared pre-command lock {}: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
fn pre_command_lock_mode(command: &[&str]) -> CommandLockMode {
    pre_command_lock_mode_parts(
        &command
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>(),
    )
}

fn pre_command_lock_mode_parts(command: &[String]) -> CommandLockMode {
    if command
        .iter()
        .any(|part| part.contains("{cargo_target_dir}"))
        || command.first().map(String::as_str) == Some("cargo")
    {
        return CommandLockMode::ScenarioLocal;
    }

    let command_text = command.join(" ");
    if command_text.contains("npm")
        || command_text.contains("pip install")
        || command_text.contains("playwright install")
    {
        return CommandLockMode::Shared;
    }

    CommandLockMode::ScenarioLocal
}

async fn acquire_shared_pre_command_lock(
    command: &[String],
    cwd: &Path,
) -> Result<SharedPreCommandLock, String> {
    let base = cargo_target_dir()
        .unwrap_or_else(|_| workspace_root().join("target"))
        .join("e2e-lanes")
        .join("locks")
        .join("shared-precommands");
    std::fs::create_dir_all(&base)
        .map_err(|error| format!("failed to create e2e pre-command lock root: {error}"))?;
    let lock_name = shared_pre_command_lock_name(command, cwd);
    let lock_path = base.join(format!("{lock_name}.lock"));
    let timeout_secs = std::env::var("MEERKAT_E2E_PRECOMMAND_LOCK_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(1800);
    let stale_secs = std::env::var("MEERKAT_E2E_PRECOMMAND_LOCK_STALE_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3600);
    let started = Instant::now();

    loop {
        match std::fs::create_dir(&lock_path) {
            Ok(()) => {
                let owner = format!(
                    "pid={}\ncommand={}\ncwd={}\n",
                    std::process::id(),
                    command.join(" "),
                    cwd.display()
                );
                let _ = std::fs::write(lock_path.join("owner.txt"), owner);
                return Ok(SharedPreCommandLock { path: lock_path });
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                remove_stale_shared_pre_command_lock(&lock_path, stale_secs);
                if started.elapsed().as_secs() > timeout_secs {
                    return Err(format!(
                        "timed out after {timeout_secs}s waiting for e2e shared pre-command lock {} ({})",
                        lock_path.display(),
                        command.join(" ")
                    ));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => {
                return Err(format!(
                    "failed to acquire e2e shared pre-command lock {}: {error}",
                    lock_path.display()
                ));
            }
        }
    }
}

fn shared_pre_command_lock_name(command: &[String], cwd: &Path) -> String {
    let key = format!("{} {}", cwd.display(), command.join(" "));
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let prefix = sanitize_artifact_key(&key)
        .chars()
        .take(80)
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    format!("{prefix}-{:016x}", hasher.finish())
}

fn remove_stale_shared_pre_command_lock(lock_path: &Path, stale_secs: u64) {
    let Ok(metadata) = std::fs::metadata(lock_path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    let Ok(age) = modified.elapsed() else {
        return;
    };
    if age.as_secs() <= stale_secs {
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(lock_path) {
        eprintln!(
            "warning: failed to remove stale e2e shared pre-command lock {}: {error}",
            lock_path.display()
        );
    }
}

async fn run_command(
    command: &[String],
    cwd: &Path,
    env_overrides: &[(String, String)],
    timeout_secs: u64,
) -> Result<CompletedCommand, String> {
    let argv = normalize_command_with_env_parts(command, env_overrides);
    let display = argv.join(" ");
    let started = Instant::now();
    eprintln!("e2e command start: {display}");
    let mut child = Command::new(&argv[0]);
    child
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env_overrides {
        child.env(key, value);
    }

    let output = timeout(Duration::from_secs(timeout_secs), child.output())
        .await
        .map_err(|_| {
            format!(
                "command timed out after {timeout_secs}s in {:.3}s: {display}",
                started.elapsed().as_secs_f64()
            )
        })?
        .map_err(|error| format!("failed to run {display}: {error}"))?;
    let elapsed = started.elapsed();

    let combined = combine_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(format!(
            "command failed after {:.3}s (exit {:?}): {}\n{}",
            elapsed.as_secs_f64(),
            output.status.code(),
            display,
            combined
        ));
    }
    eprintln!(
        "e2e command done: {display} in {:.3}s",
        elapsed.as_secs_f64()
    );

    Ok(CompletedCommand { output: combined })
}

fn build_commands_for_mode(
    spec: &'static Spec,
    execution_mode: ExecutionMode,
    manifest: Option<&ArtifactManifest>,
) -> Result<BuiltCommands, String> {
    let output_policy = match spec.command {
        CommandSpec::CargoTest { .. } => OutputPolicy::CargoTest,
        CommandSpec::Pytest { .. } => OutputPolicy::Pytest,
        CommandSpec::NodeTest { .. } => OutputPolicy::NodeTest,
        CommandSpec::Raw { output_policy, .. } => output_policy,
    };

    let command = match (execution_mode, spec.command) {
        (
            ExecutionMode::Prebuilt,
            CommandSpec::CargoTest {
                package,
                test_target,
                test_name,
                ..
            },
        ) => {
            let manifest = manifest.ok_or_else(|| {
                "prebuilt command generation requires an artifact manifest".to_string()
            })?;
            vec![
                manifest
                    .rust_test(package, test_target)?
                    .display()
                    .to_string(),
                test_name.to_string(),
                "--ignored".to_string(),
                "--nocapture".to_string(),
            ]
        }
        (ExecutionMode::Prebuilt, CommandSpec::Raw { argv, .. })
            if parse_cargo_test_requirement(argv).is_some() =>
        {
            let manifest = manifest.ok_or_else(|| {
                "prebuilt command generation requires an artifact manifest".to_string()
            })?;
            prebuilt_raw_cargo_test_command(argv, manifest)?
        }
        (ExecutionMode::Prebuilt, CommandSpec::Raw { argv, .. }) if is_raw_cargo_command(argv) => {
            return Err(format!(
                "prebuilt execution does not support raw cargo command for {}: {}",
                run_label(spec),
                argv.join(" ")
            ));
        }
        (
            _,
            CommandSpec::CargoTest {
                package,
                test_target,
                test_name,
                features,
                all_features,
            },
        ) => {
            let mut command = vec!["cargo", "test", "-p", package];
            if all_features {
                command.push("--all-features");
            }
            if !features.is_empty() {
                command.push("--features");
                command.push(Box::leak(features.join(",").into_boxed_str()));
            }
            command.extend([
                "--test",
                test_target,
                test_name,
                "--",
                "--ignored",
                "--nocapture",
            ]);
            command.into_iter().map(str::to_string).collect()
        }
        (
            _,
            CommandSpec::Pytest {
                test_file,
                test_name,
            },
        ) => vec![
            compatible_python_bin().unwrap_or("python3"),
            "-m",
            "pytest",
            "-v",
            Box::leak(format!("{test_file}::{test_name}").into_boxed_str()),
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        (
            _,
            CommandSpec::NodeTest {
                test_file,
                test_name,
            },
        ) => vec![
            "node".to_string(),
            "--test".to_string(),
            "--test-name-pattern".to_string(),
            test_name.to_string(),
            test_file.to_string(),
        ],
        (_, CommandSpec::Raw { argv, .. }) => argv.iter().map(|part| (*part).to_string()).collect(),
    };

    let pre_commands = spec
        .pre_commands
        .iter()
        .filter(|command| {
            !matches!(execution_mode, ExecutionMode::Prebuilt)
                || !pre_command_replaced_by_manifest_artifact(command)
        })
        .map(|command| command.iter().map(|part| (*part).to_string()).collect())
        .collect();

    Ok(BuiltCommands {
        pre_commands,
        command,
        output_policy,
    })
}

fn prebuilt_raw_cargo_test_command(
    argv: &[&str],
    manifest: &ArtifactManifest,
) -> Result<Vec<String>, String> {
    let requirement = parse_cargo_test_requirement(argv)
        .ok_or_else(|| format!("cannot derive prebuilt rust test from {}", argv.join(" ")))?;
    let mut command = vec![
        manifest
            .rust_test(&requirement.package, &requirement.test_target)?
            .display()
            .to_string(),
    ];
    let mut after_test_target = false;
    let mut after_harness_separator = false;
    let mut skip_next = false;
    for (index, part) in argv.iter().enumerate().skip(2) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if *part == "--" {
            after_harness_separator = true;
            continue;
        }
        if after_harness_separator {
            command.push((*part).to_string());
            continue;
        }
        if after_test_target {
            if !part.starts_with('-') {
                command.push((*part).to_string());
            }
            continue;
        }
        if matches!(*part, "-p" | "--package" | "--features" | "--test") {
            skip_next = true;
            if *part == "--test" {
                after_test_target = true;
            }
            continue;
        }
        if *part == "--all-features" {
            continue;
        }
        if index + 1 == argv.len() && !part.starts_with('-') {
            command.push((*part).to_string());
        }
    }
    Ok(command)
}

fn pre_command_replaced_by_manifest_artifact(command: &[&str]) -> bool {
    if parse_cargo_build_requirement(command).is_some() {
        return true;
    }
    let text = command.join(" ");
    text.contains("{cargo_target_dir}/e2e-bins")
        || (command.first() == Some(&"cp") && text.contains("{cargo_target_dir}/debug/"))
        || command_mentions_node_setup(command)
        || command_mentions_python_setup(command)
}

fn is_raw_cargo_command(argv: &[&str]) -> bool {
    argv.first().copied().is_some_and(is_cargo_command)
}

fn normalize_command(command: &[String]) -> Vec<String> {
    let cargo_target_dir = cargo_target_dir().unwrap_or_else(|_| workspace_root().join("target"));
    normalize_command_parts_with_target_dir(command, &cargo_target_dir)
}

#[cfg(test)]
fn normalize_command_with_env(command: &[&str], env_overrides: &[(String, String)]) -> Vec<String> {
    let command = command
        .iter()
        .map(|part| (*part).to_string())
        .collect::<Vec<_>>();
    normalize_command_with_env_parts(&command, env_overrides)
}

fn normalize_command_with_env_parts(
    command: &[String],
    env_overrides: &[(String, String)],
) -> Vec<String> {
    let cargo_target_dir = env_value(env_overrides, "CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| cargo_target_dir().ok())
        .unwrap_or_else(|| workspace_root().join("target"));
    normalize_command_parts_with_target_dir(command, &cargo_target_dir)
}

fn normalize_command_parts_with_target_dir(
    command: &[String],
    cargo_target_dir: &Path,
) -> Vec<String> {
    let repo_root = workspace_root().display().to_string();
    let cargo_target_dir = cargo_target_dir.display().to_string();
    let mut argv = command
        .iter()
        .map(|part| {
            part.replace("{repo_root}", &repo_root)
                .replace("{cargo_target_dir}", &cargo_target_dir)
        })
        .collect::<Vec<_>>();
    if argv.first().map(|part| part.as_str()) == Some("cargo") {
        argv[0] = repo_cargo().display().to_string();
    }
    argv
}

fn env_value<'a>(env_overrides: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env_overrides
        .iter()
        .find_map(|(env_key, value)| (env_key == key).then_some(value.as_str()))
}

fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        (_, "") => stdout.into_owned(),
        ("", _) => stderr.into_owned(),
        _ => format!("{stdout}\n--- stderr ---\n{stderr}"),
    }
}

fn analyze_success_output(output_policy: OutputPolicy, output: &str) -> Option<&'static str> {
    let lower = output.to_ascii_lowercase();
    match output_policy {
        OutputPolicy::CargoTest => {
            if lower.contains("running 0 tests") {
                return Some("cargo executed 0 tests");
            }
            let Some(summary) = lower.lines().find(|line| line.contains("test result:")) else {
                return Some("cargo output did not include a test summary");
            };
            if summary.contains(" 0 passed;") {
                return Some("cargo reported 0 passed tests");
            }
            None
        }
        OutputPolicy::Pytest => {
            if lower.contains("no tests ran") {
                return Some("pytest executed 0 tests");
            }
            if lower.contains(" skipped") || lower.contains(" deselected") {
                return Some("pytest skipped or deselected the requested scenario");
            }
            if !lower.contains(" passed") {
                return Some("pytest output did not report a passed test");
            }
            None
        }
        OutputPolicy::NodeTest => {
            let has_zero_tests =
                lower.contains("# tests 0") || lower.lines().any(|line| line.contains("tests 0"));
            if has_zero_tests {
                return Some("node executed 0 tests");
            }
            let has_pass = lower.lines().any(|line| {
                line.starts_with("# pass ")
                    || line.starts_with("ℹ pass ")
                    || line.contains(" pass 1")
                    || line.contains(" pass 2")
                    || line.contains(" pass 3")
                    || line.contains(" pass 4")
                    || line.contains(" pass 5")
            });
            if !has_pass {
                return Some("node output did not report a passed test");
            }
            None
        }
        OutputPolicy::ExitOnly => None,
    }
}

fn prereq_failure(spec: &Spec, execution_mode: ExecutionMode) -> Option<String> {
    let mut failures = Vec::new();
    for group in spec.required_env {
        if group
            .iter()
            .all(|name| std::env::var(name).map_or(true, |value| value.is_empty()))
        {
            let message = if group.len() == 1 {
                format!("missing env {}", group[0])
            } else {
                format!("missing one of {}", group.join(" / "))
            };
            failures.push(message);
        }
    }
    for binary in spec.required_bins {
        if matches!(execution_mode, ExecutionMode::Prebuilt) && *binary == "cargo" {
            continue;
        }
        if *binary == "python3" {
            if compatible_python_bin().is_none() {
                failures.push("missing compatible python >=3.10".to_string());
            }
            continue;
        }
        if *binary == "cargo" {
            if !repo_cargo().exists() {
                failures.push(format!(
                    "missing repo cargo wrapper {}",
                    repo_cargo().display()
                ));
            }
            continue;
        }
        if std::process::Command::new(binary)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            failures.push(format!("missing binary {binary}"));
        }
    }

    if failures.is_empty() {
        None
    } else {
        Some(failures.join("; "))
    }
}

fn scenario_env(
    spec: &Spec,
    cargo_target_dir: &Path,
    execution_mode: ExecutionMode,
    manifest: Option<&ArtifactManifest>,
) -> Result<Vec<(String, String)>, String> {
    let mut env = BTreeMap::new();
    std::fs::create_dir_all(cargo_target_dir).map_err(|error| {
        format!(
            "failed to create scenario cargo target dir {}: {error}",
            cargo_target_dir.display()
        )
    })?;
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        cargo_target_dir.display().to_string(),
    );
    env.insert(
        "RUST_LANE_ID".to_string(),
        format!("e2e-{}", scenario_artifact_key(spec)),
    );
    // macOS 26.3.1+ `codeSigningMonitor=2` can SIGKILL adhoc/linker-signed
    // binaries whose signature is invalidated while dyld is loading them,
    // which happens when the outer `cargo test` or sibling scenarios
    // incrementally re-link the binary between signing and scenario
    // spawn. `CARGO_INCREMENTAL=0` discourages such re-links without
    // altering the target-dir layout (scenarios with `cp` pre-commands
    // assume `{cargo_target_dir}/debug/` holds the built binaries).
    // The primary repair — stripping xattrs + re-signing — lives in
    // [`ensure_binary_signatures_fresh`] and is applied just before
    // the scenario's main command runs.
    env.entry("CARGO_INCREMENTAL".to_string())
        .or_insert_with(|| "0".to_string());
    // Live/mob-heavy tests recursively drive the runtime through many
    // layered async frames (spawn → wire → retire → destroy) which can
    // exceed the default 2 MiB main-thread stack under debug builds.
    // Default-raise to 16 MiB for every spec; scenarios can still
    // override by setting their own `RUST_MIN_STACK` in `spec.env`.
    env.entry("RUST_MIN_STACK".to_string())
        .or_insert_with(|| (16 * 1024 * 1024).to_string());
    for (key, value) in spec.env {
        env.insert((*key).to_string(), expand_template(value, cargo_target_dir));
    }
    for binary in spec.cargo_bin_env {
        env.insert(
            format!("CARGO_BIN_EXE_{binary}"),
            cargo_target_dir
                .join("debug")
                .join(platform_binary_name(binary))
                .display()
                .to_string(),
        );
    }
    if matches!(execution_mode, ExecutionMode::Prebuilt) {
        let manifest = manifest.ok_or_else(|| {
            "prebuilt scenario environment requires an artifact manifest".to_string()
        })?;
        apply_prebuilt_manifest_env(spec, &mut env, manifest)?;
    }
    if matches!(spec.command, CommandSpec::Pytest { .. })
        && let Some(python) = compatible_python_bin()
    {
        env.insert("MEERKAT_PYTHON_BIN".to_string(), python.to_string());
    }
    Ok(env.into_iter().collect())
}

fn apply_prebuilt_manifest_env(
    spec: &Spec,
    env: &mut BTreeMap<String, String>,
    manifest: &ArtifactManifest,
) -> Result<(), String> {
    for (binary, path) in &manifest.rust_bins {
        let path = path.display().to_string();
        env.insert(format!("CARGO_BIN_EXE_{binary}"), path.clone());
        env.insert(rkat_test_bin_env_name(binary), path);
    }

    for binary in spec.cargo_bin_env {
        let path = manifest.rust_bin(binary)?.display().to_string();
        env.insert(format!("CARGO_BIN_EXE_{binary}"), path.clone());
        env.insert(rkat_test_bin_env_name(binary), path);
    }

    for (key, value) in spec.env {
        if key.starts_with("RKAT_TEST_BIN_") {
            if let Some(binary) = bin_from_rkat_test_bin_env(key) {
                env.insert(
                    (*key).to_string(),
                    manifest.rust_bin(&binary)?.display().to_string(),
                );
            }
        } else if *key == "MEERKAT_BIN_PATH"
            && let Some(binary) = binary_from_artifact_path_template(value)
        {
            env.insert(
                (*key).to_string(),
                manifest.rust_bin(&binary)?.display().to_string(),
            );
        }
    }

    Ok(())
}

fn clean_scenario_target_dir_if_requested(spec: &Spec, cargo_target_dir: &Path) {
    if !clean_e2e_scenario_targets_enabled() || !cargo_target_dir.exists() {
        return;
    }
    if matches!(spec.lane, Lane::System) {
        return;
    }
    if let Err(error) = std::fs::remove_dir_all(cargo_target_dir) {
        eprintln!(
            "failed to clean e2e scenario target for {} at {}: {error}",
            run_label(spec),
            cargo_target_dir.display()
        );
    }
}

fn scenario_cargo_target_dir(spec: &Spec) -> Result<PathBuf, String> {
    if matches!(spec.lane, Lane::System) {
        // System-lane wrappers are serialized in-process, so their nested cargo
        // invocations can safely reuse the lane's normal target dir. That lets
        // CI prebuild/clippy steps feed the e2e-system lane instead of forcing
        // a second copy of the same artifacts under e2e-lanes/.
        return cargo_target_dir();
    }
    Ok(cargo_target_dir()?
        .join("e2e-lanes")
        .join(source_revision_key())
        .join(scenario_artifact_key(spec)))
}

fn source_revision_key() -> String {
    static SOURCE_REVISION: OnceLock<String> = OnceLock::new();
    SOURCE_REVISION
        .get_or_init(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .current_dir(workspace_root())
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|revision| sanitize_artifact_key(revision.trim()))
                .filter(|revision| !revision.is_empty())
                .unwrap_or_else(|| "worktree".to_string())
        })
        .clone()
}

fn scenario_artifact_key(spec: &Spec) -> String {
    let base = match spec.id {
        Some(id) => format!("scenario-{id:02}"),
        None => run_label(spec),
    };
    sanitize_artifact_key(&base)
}

fn sanitize_artifact_key(value: &str) -> String {
    let mut key = String::with_capacity(value.len());
    let mut last_was_dash = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            key.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            key.push('-');
            last_was_dash = true;
        }
    }
    key.trim_matches('-').to_string()
}

fn expand_template(value: &str, cargo_target_dir: &Path) -> String {
    value
        .replace("{repo_root}", &workspace_root().display().to_string())
        .replace(
            "{cargo_target_dir}",
            &cargo_target_dir.display().to_string(),
        )
}

fn platform_binary_name(name: &str) -> String {
    if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Sanitize every advertised test binary (`CARGO_BIN_EXE_*` and
/// `RKAT_TEST_BIN_*`) so it can be `exec`d on macOS 26.3.1+.
///
/// Background: macOS 26.3 tightens two loader gates on locally-built
/// binaries:
///
/// 1. **Code signing**: `codeSigningMonitor=2` refuses to load an
///    adhoc/linker-signed binary whose on-disk content hash no longer
///    matches the embedded signature — a situation that arises when
///    the outer `cargo test` (or a sibling scenario in the same lane)
///    has re-linked the binary between signing and scenario spawn.
/// 2. **Provenance sandbox**: `AppleSystemPolicy` refuses to apply
///    the provenance sandbox to a binary whose `com.apple.provenance`
///    extended attribute was lost or mangled — the classic case being
///    a `cp` of a cargo-built binary into a scenario-private staging
///    directory. The symptom is
///    `(AppleSystemPolicy) ASP: Unable to apply provenance sandbox`
///    in the system log and `SIGKILL (Code Signature Invalid)` to
///    the child, with empty stdout/stderr.
///
/// Both gates are repaired by stripping the extended attributes and
/// re-applying a fresh adhoc signature — `xattr -c <path>` followed by
/// `codesign --force --sign - <path>`. No-op on non-macOS platforms.
async fn ensure_binary_signatures_fresh(spec: &Spec, env_overrides: &[(String, String)]) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let _guard = codesign_refresh_lock().lock().await;
    for (key, value) in env_overrides {
        // Cover every binary the scenario could spawn: cargo-advertised
        // binaries via CARGO_BIN_EXE_* and scenario-local staged copies
        // via RKAT_TEST_BIN_*.
        if !key.starts_with("CARGO_BIN_EXE_") && !key.starts_with("RKAT_TEST_BIN_") {
            continue;
        }
        let path = Path::new(value);
        if !path.exists() {
            continue;
        }
        // Strip extended attributes first — `cp` on macOS carries
        // `com.apple.provenance` from the source, and the kernel's
        // AppleSystemPolicy rejects the exec when the provenance
        // sandbox can't be applied.
        let _ = tokio::process::Command::new("xattr")
            .arg("-c")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        // Now re-sign with a fresh adhoc signature so the signing
        // monitor accepts the current on-disk content.
        let output = tokio::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!(
                    "warning: codesign refresh failed for {} (lane scenario {}): {stderr}",
                    path.display(),
                    run_label(spec),
                );
            }
            Err(err) => {
                eprintln!(
                    "warning: failed to invoke codesign for {} (lane scenario {}): {err}",
                    path.display(),
                    run_label(spec),
                );
            }
        }
    }
}

fn codesign_refresh_lock() -> &'static AsyncMutex<()> {
    static LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| AsyncMutex::new(()))
}

fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn repo_cargo() -> &'static Path {
    static REPO_CARGO: OnceLock<PathBuf> = OnceLock::new();
    REPO_CARGO
        .get_or_init(|| workspace_root().join("scripts").join("repo-cargo"))
        .as_path()
}

fn compatible_python_bin() -> Option<&'static str> {
    static PYTHON_BIN: OnceLock<Option<String>> = OnceLock::new();
    PYTHON_BIN
        .get_or_init(|| {
            for candidate in ["python3.11", "python3.10", "python3", "python"] {
                let output = std::process::Command::new(candidate)
                    .args([
                        "-c",
                        "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output();
                let Ok(output) = output else {
                    continue;
                };
                if !output.status.success() {
                    continue;
                }
                let version = String::from_utf8_lossy(&output.stdout);
                let mut parts = version.trim().split('.');
                let major = parts.next().and_then(|part| part.parse::<u32>().ok());
                let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
                if matches!((major, minor), (Some(major), Some(minor)) if (major, minor) >= (3, 10))
                {
                    return Some(candidate.to_string());
                }
            }
            None
        })
        .as_deref()
}

fn cargo_target_dir() -> Result<PathBuf, String> {
    if let Ok(value) = std::env::var("CARGO_TARGET_DIR") {
        return Ok(PathBuf::from(value));
    }

    static TARGET_DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    TARGET_DIR
        .get_or_init(|| {
            let output = std::process::Command::new(repo_cargo())
                .arg("--print-env")
                .output()
                .map_err(|error| format!("failed to query repo-cargo env: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "repo-cargo --print-env failed: {}",
                    combine_output(&output.stdout, &output.stderr)
                ));
            }
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(value) = line.strip_prefix("CARGO_TARGET_DIR=") {
                    return Ok(PathBuf::from(value));
                }
            }
            Err("repo-cargo --print-env did not report CARGO_TARGET_DIR".to_string())
        })
        .clone()
}

pub fn materialize_local_cargo_plan(
    selection: &E2eSelection,
    manifest_path: &Path,
) -> Result<E2ePlan, String> {
    let plan = plan_for_selection(selection)?;
    let mut manifest = ArtifactManifest::default();

    for requirement in &plan.requirements {
        match requirement {
            ArtifactRequirement::RustBin(requirement) => {
                let path = materialize_rust_bin(requirement)?;
                manifest.rust_bins.insert(requirement.bin.clone(), path);
            }
            ArtifactRequirement::RustTest(requirement) => {
                let path = materialize_rust_test(requirement)?;
                manifest.rust_tests.insert(requirement.key(), path);
            }
            ArtifactRequirement::NodeBuild { cwd } => materialize_node_build(cwd)?,
            ArtifactRequirement::PythonEnv { cwd } => materialize_python_env(cwd)?,
        }
    }

    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create e2e artifact manifest directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("failed to serialize e2e artifact manifest: {error}"))?;
    std::fs::write(manifest_path, format!("{json}\n")).map_err(|error| {
        format!(
            "failed to write e2e artifact manifest {}: {error}",
            manifest_path.display()
        )
    })?;

    Ok(plan)
}

fn materialize_rust_bin(requirement: &RustBinRequirement) -> Result<PathBuf, String> {
    let mut args = vec![
        "build".to_string(),
        "-p".to_string(),
        requirement.package.clone(),
        "--bin".to_string(),
        requirement.bin.clone(),
    ];
    push_feature_args(&mut args, &requirement.features, requirement.all_features);
    args.push("--message-format=json".to_string());
    let messages = run_repo_cargo_json(&args)?;
    executable_from_cargo_messages(&messages, &requirement.bin, "bin")?.ok_or_else(|| {
        format!(
            "cargo did not report executable path for bin {} ({})",
            requirement.bin,
            requirement.key()
        )
    })
}

fn materialize_rust_test(requirement: &RustTestRequirement) -> Result<PathBuf, String> {
    let mut args = vec![
        "test".to_string(),
        "-p".to_string(),
        requirement.package.clone(),
        "--test".to_string(),
        requirement.test_target.clone(),
        "--no-run".to_string(),
    ];
    push_feature_args(&mut args, &requirement.features, requirement.all_features);
    args.push("--message-format=json".to_string());
    let messages = run_repo_cargo_json(&args)?;
    executable_from_cargo_messages(&messages, &requirement.test_target, "test")?.ok_or_else(|| {
        format!(
            "cargo did not report executable path for test {} ({})",
            requirement.test_target,
            requirement.key()
        )
    })
}

fn push_feature_args(args: &mut Vec<String>, features: &BTreeSet<String>, all_features: bool) {
    if all_features {
        args.push("--all-features".to_string());
    }
    if !features.is_empty() {
        args.push("--features".to_string());
        args.push(features.iter().cloned().collect::<Vec<_>>().join(","));
    }
}

fn run_repo_cargo_json(args: &[String]) -> Result<Vec<serde_json::Value>, String> {
    eprintln!("e2e materialize cargo: {}", args.join(" "));
    let output = StdCommand::new(repo_cargo())
        .args(args)
        .output()
        .map_err(|error| format!("failed to run repo-cargo {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "repo-cargo {} failed (exit {:?}):\n{}",
            args.join(" "),
            output.status.code(),
            combine_output(&output.stdout, &output.stderr)
        ));
    }

    let mut messages = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str(line)
            .map_err(|error| format!("failed to parse cargo JSON message '{line}': {error}"))?;
        messages.push(value);
    }
    Ok(messages)
}

fn materialize_node_build(cwd: &str) -> Result<(), String> {
    match cwd {
        "sdks/typescript" => {
            run_materialize_command(cwd, &["npm", "install", "--no-audit", "--no-fund"])?;
            run_materialize_command(cwd, &["npm", "run", "build"])?;
        }
        "tests/live_smoke/browser" => {
            run_materialize_command(cwd, &["/bin/sh", "-c", "test -d node_modules || npm ci"])?;
            run_materialize_command(
                cwd,
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
            )?;
            run_materialize_command_with_env(
                cwd,
                WEB_RUNTIME_BUILD_IF_MISSING,
                &[
                    ("CARGO_BUILD_JOBS", "1"),
                    ("MEERKAT_WEB_WASM_PROFILE", "dev"),
                ],
            )?;
            run_materialize_command(cwd, &["npx", "playwright", "install", "chromium"])?;
        }
        other => {
            return Err(format!(
                "no e2e materializer is defined for node build cwd '{other}'"
            ));
        }
    }
    Ok(())
}

fn materialize_python_env(cwd: &str) -> Result<(), String> {
    let python = compatible_python_bin().ok_or_else(|| {
        format!("cannot materialize python env for {cwd}: missing compatible python >=3.10")
    })?;
    let script = format!(
        "{python} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || {python} -m pip install -e \".[dev]\""
    );
    run_materialize_command(cwd, &["/bin/sh", "-c", script.as_str()])
}

fn run_materialize_command(cwd: &str, command: &[&str]) -> Result<(), String> {
    run_materialize_command_with_env(cwd, command, &[])
}

fn run_materialize_command_with_env(
    cwd: &str,
    command: &[&str],
    env: &[(&str, &str)],
) -> Result<(), String> {
    let Some((program, args)) = command.split_first() else {
        return Err(format!("empty e2e materialize command for {cwd}"));
    };
    eprintln!("e2e materialize setup ({cwd}): {}", command.join(" "));
    let mut child = StdCommand::new(program);
    child.args(args).current_dir(workspace_root().join(cwd));
    for (key, value) in env {
        child.env(key, value);
    }
    let output = child.output().map_err(|error| {
        format!(
            "failed to run e2e materialize setup in {cwd}: {}: {error}",
            command.join(" ")
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "e2e materialize setup failed in {cwd} (exit {:?}): {}\n{}",
            output.status.code(),
            command.join(" "),
            combine_output(&output.stdout, &output.stderr)
        ))
    }
}

fn executable_from_cargo_messages(
    messages: &[serde_json::Value],
    target_name: &str,
    target_kind: &str,
) -> Result<Option<PathBuf>, String> {
    for message in messages {
        if message.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let Some(target) = message.get("target") else {
            continue;
        };
        if target.get("name").and_then(serde_json::Value::as_str) != Some(target_name) {
            continue;
        }
        let has_kind = target
            .get("kind")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some(target_kind)));
        if !has_kind {
            continue;
        }
        if let Some(executable) = message
            .get("executable")
            .and_then(serde_json::Value::as_str)
        {
            return Ok(Some(PathBuf::from(executable)));
        }
    }
    Ok(None)
}

fn strict_prereqs_enabled() -> bool {
    matches!(
        std::env::var("MEERKAT_STRICT_E2E_PREREQS").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn clean_e2e_scenario_targets_enabled() -> bool {
    matches!(
        std::env::var("MEERKAT_CLEAN_E2E_SCENARIO_TARGETS").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn completed_pre_commands() -> &'static Mutex<HashMap<String, PreCommandState>> {
    static PRE_COMMANDS: OnceLock<Mutex<HashMap<String, PreCommandState>>> = OnceLock::new();
    PRE_COMMANDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn command_display(command: &[String]) -> String {
    normalize_command(command).join(" ")
}

fn command_display_with_env(command: &[String], env_overrides: &[(String, String)]) -> String {
    normalize_command_with_env_parts(command, env_overrides).join(" ")
}

fn scenario_spec(id: u16) -> Option<&'static Spec> {
    match id {
        15 => Some(&Spec {
            id: Some(15),
            lane: Lane::Live,
            title: "RPC full lifecycle and recall",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_15_full_rpc_conversation_flow",
                features: &[],
                all_features: false,
            },
        }),
        16 => Some(&Spec {
            id: Some(16),
            lane: Lane::Smoke,
            title: "RPC kitchen sink",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_16_kitchen_sink",
                features: &[],
                all_features: false,
            },
        }),
        17 => Some(&Spec {
            id: Some(17),
            lane: Lane::Live,
            title: "RPC multi-turn event streaming",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_17_multi_turn_event_streaming",
                features: &[],
                all_features: false,
            },
        }),
        18 => Some(&Spec {
            id: Some(18),
            lane: Lane::Live,
            title: "RPC config, capabilities, and errors",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_rpc_regression",
                test_name: "e2e_scenario_18_config_capabilities_errors",
                features: &[],
                all_features: false,
            },
        }),
        19 => Some(&Spec {
            id: Some(19),
            lane: Lane::Live,
            title: "RPC context injection recall",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_rpc_regression",
                test_name: "e2e_scenario_19_inject_context_recall",
                features: &[],
                all_features: false,
            },
        }),
        20 => Some(&Spec {
            id: Some(20),
            lane: Lane::Live,
            title: "RPC dedicated event stream roundtrip",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_rpc_regression",
                test_name: "e2e_scenario_20_streaming_events",
                features: &[],
                all_features: false,
            },
        }),
        21 => Some(&Spec {
            id: Some(21),
            lane: Lane::Smoke,
            title: "RPC mob deferred callback tools",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_21_mob_callback_tools",
                features: &[],
                all_features: false,
            },
        }),
        22 => Some(&Spec {
            id: Some(22),
            lane: Lane::Smoke,
            title: "RPC transport backpressure",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_22_transport_backpressure",
                features: &[],
                all_features: false,
            },
        }),
        23 => Some(&Spec {
            id: Some(23),
            lane: Lane::Live,
            title: "REST SSE follow-up event stream",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_23_rest_sse_events_follow_continue_turn",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        24 => Some(&Spec {
            id: Some(24),
            lane: Lane::Live,
            title: "REST config, capabilities, health, and skills",
            timeout_secs: 600,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_24_rest_config_capabilities_health_and_skills",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        25 => Some(&Spec {
            id: Some(25),
            lane: Lane::Live,
            title: "REST reload and resume on same realm root",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_25_rest_reload_and_resume_on_same_realm_root",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        26 => Some(&Spec {
            id: Some(26),
            lane: Lane::Live,
            title: "CLI run and resume persistence",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_26_cli_run_resume_persistence",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        27 => Some(&Spec {
            id: Some(27),
            lane: Lane::Smoke,
            title: "CLI shell and structured output",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_27_cli_shell_and_structured_output",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        28 => Some(&Spec {
            id: Some(28),
            lane: Lane::Smoke,
            title: "CLI signed mobpack deploy",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_scenario_28_cli_mobpack_deploy_signed_strict_live",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        29 => Some(&Spec {
            id: Some(29),
            lane: Lane::Live,
            title: "CLI mob member turn probe",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_scenario_29_cli_mob_rpc_member_turn_probe",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        30 => Some(&Spec {
            id: Some(30),
            lane: Lane::Smoke,
            title: "CLI mob flow probe",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_scenario_30_cli_mob_rpc_flow_probe",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        31 => Some(&Spec {
            id: Some(31),
            lane: Lane::Live,
            title: "MCP stdio run and resume lifecycle",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-mcp"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-mcp-server",
                "--bin",
                "rkat-mcp",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_31_mcp_stdio_run_resume_lifecycle",
                features: &[],
                all_features: false,
            },
        }),
        32 => Some(&Spec {
            id: Some(32),
            lane: Lane::Live,
            title: "MCP stdio config, capabilities, and skills",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-mcp"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-mcp-server",
                "--bin",
                "rkat-mcp",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_32_mcp_stdio_config_capabilities_and_skills",
                features: &[],
                all_features: false,
            },
        }),
        33 => Some(&Spec {
            id: Some(33),
            lane: Lane::Live,
            title: "MCP stdio event stream roundtrip",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-mcp"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-mcp-server",
                "--bin",
                "rkat-mcp",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_33_mcp_stdio_event_stream_read_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        34 => Some(&Spec {
            id: Some(34),
            lane: Lane::Live,
            title: "MCP streamable HTTP run and resume lifecycle",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_34_mcp_streamable_http_run_resume_lifecycle",
                features: &[],
                all_features: false,
            },
        }),
        35 => Some(&Spec {
            id: Some(35),
            lane: Lane::Live,
            title: "MCP streamable HTTP config, capabilities, and skills",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_35_mcp_streamable_http_config_capabilities_and_skills",
                features: &[],
                all_features: false,
            },
        }),
        36 => Some(&Spec {
            id: Some(36),
            lane: Lane::Live,
            title: "MCP streamable HTTP event stream and archive",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_36_mcp_streamable_http_event_stream_and_archive_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        37 => Some(&Spec {
            id: Some(37),
            lane: Lane::Live,
            title: "Python SDK full lifecycle and capabilities",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_37_full_lifecycle_and_capabilities",
            },
        }),
        38 => Some(&Spec {
            id: Some(38),
            lane: Lane::Live,
            title: "Python SDK context injection and streaming",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_38_inject_context_and_streaming",
            },
        }),
        39 => Some(&Spec {
            id: Some(39),
            lane: Lane::Live,
            title: "Python SDK persistent reconnect and runtime accept",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_39_persistent_reconnect_and_session_submit",
            },
        }),
        40 => Some(&Spec {
            id: Some(40),
            lane: Lane::Smoke,
            title: "Python SDK mixed-provider swarm probe",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_40_mixed_provider_swarm_probe",
            },
        }),
        41 => Some(&Spec {
            id: Some(41),
            lane: Lane::Live,
            title: "TypeScript SDK full lifecycle and recall",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 41",
            },
        }),
        42 => Some(&Spec {
            id: Some(42),
            lane: Lane::Live,
            title: "TypeScript SDK deferred context and streaming",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 42",
            },
        }),
        43 => Some(&Spec {
            id: Some(43),
            lane: Lane::Live,
            title: "TypeScript SDK persistent reconnect and resume",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 43",
            },
        }),
        44 => Some(&Spec {
            id: Some(44),
            lane: Lane::Smoke,
            title: "TypeScript SDK mixed-provider swarm probe",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 44",
            },
        }),
        45 => Some(&Spec {
            id: Some(45),
            lane: Lane::Live,
            title: "Browser raw session lifecycle",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                WEB_RUNTIME_BUILD_IF_MISSING,
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-RAW-SESSION-001",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        46 => Some(&Spec {
            id: Some(46),
            lane: Lane::Live,
            title: "Browser raw session recall",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                WEB_RUNTIME_BUILD_IF_MISSING,
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-RAW-RECALL-002",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        47 => Some(&Spec {
            id: Some(47),
            lane: Lane::Smoke,
            title: "Browser mobpack session flow",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                WEB_RUNTIME_BUILD_IF_MISSING,
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-MOBPACK-SESSION-003",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        48 => Some(&Spec {
            id: Some(48),
            lane: Lane::Smoke,
            title: "Browser raw mob lifecycle",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                WEB_RUNTIME_BUILD_IF_MISSING,
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-RAW-MOB-004",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        49 => Some(&Spec {
            id: Some(49),
            lane: Lane::Smoke,
            title: "Cross-surface RPC to REST shared-realm roundtrip",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_49_rpc_rest_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        50 => Some(&Spec {
            id: Some(50),
            lane: Lane::Smoke,
            title: "Cross-surface REST to CLI shared-realm roundtrip",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_50_rest_cli_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        51 => Some(&Spec {
            id: Some(51),
            lane: Lane::Smoke,
            title: "Cross-surface RPC to MCP shared-realm parity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_51_rpc_mcp_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        52 => Some(&Spec {
            id: Some(52),
            lane: Lane::Smoke,
            title: "Cross-surface CLI to RPC to CLI continuity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_52_cli_rpc_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        53 => Some(&Spec {
            id: Some(53),
            lane: Lane::Smoke,
            title: "Cross-surface CLI to REST to CLI continuity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_53_cli_rest_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        54 => Some(&Spec {
            id: Some(54),
            lane: Lane::Smoke,
            title: "Cross-surface shared-realm mob visibility",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat-rpc",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["mkdir", "-p", "{cargo_target_dir}/e2e-bins/scenario-54"],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat-rpc",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_54_shared_realm_mob_sessions_visible_to_cli",
                features: &[],
                all_features: false,
            },
        }),
        55 => Some(&Spec {
            id: Some(55),
            lane: Lane::Smoke,
            title: "RPC+REST callback peer storm resume",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rpc",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rest",
                    "--bin",
                    "rkat-rest",
                    "--features",
                    "mob",
                ],
                &["mkdir", "-p", "{cargo_target_dir}/e2e-bins/scenario-55"],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_55_rpc_rest_callback_peer_storm_resume",
                features: &[],
                all_features: false,
            },
        }),
        56 => Some(&Spec {
            id: Some(56),
            lane: Lane::System,
            title: "RPC+REST explicit mob registry restores",
            timeout_secs: 300,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rpc",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rest",
                    "--bin",
                    "rkat-rest",
                    "--features",
                    "mob",
                ],
                &["mkdir", "-p", "{cargo_target_dir}/e2e-bins/scenario-56"],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "rpc_rest_explicit_mob_registry_restores_without_live_api",
                features: &[],
                all_features: false,
            },
        }),
        57 => Some(&Spec {
            id: Some(57),
            lane: Lane::Smoke,
            title: "Python SDK realtime channel session exchange",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_57_realtime_channel_session_exchange",
            },
        }),
        58 => Some(&Spec {
            id: Some(58),
            lane: Lane::Smoke,
            title: "Python SDK realtime member respawn continuity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_58_realtime_member_channel_respawn_continuity",
            },
        }),
        59 => Some(&Spec {
            id: Some(59),
            lane: Lane::Smoke,
            title: "TypeScript SDK realtime channel session exchange",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 59",
            },
        }),
        60 => Some(&Spec {
            id: Some(60),
            lane: Lane::Smoke,
            title: "Rust SDK realtime channel session exchange",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-rpc"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-rpc",
                "--bin",
                "rkat-rpc",
                "--features",
                "mob",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_60_rust_sdk_realtime_channel_session_exchange",
                features: &[],
                all_features: false,
            },
        }),
        61 => Some(&Spec {
            id: Some(61),
            lane: Lane::Smoke,
            title: "CLI realtime bridge session roundtrip",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat", "rkat-rpc"],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_61_cli_realtime_bridge_session_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        62 => Some(&Spec {
            id: Some(62),
            lane: Lane::Smoke,
            title: "REST bootstrap to Rust SDK realtime channel exchange",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-rpc", "rkat-rest"],
            pre_commands: &[
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rest",
                    "--bin",
                    "rkat-rest",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_62_rest_bootstrap_to_rust_sdk_realtime_channel_exchange",
                features: &[],
                all_features: false,
            },
        }),
        63 => Some(&Spec {
            id: Some(63),
            lane: Lane::Smoke,
            title: "MCP bootstrap to Rust SDK member realtime exchange",
            timeout_secs: 1500,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-rpc", "rkat-rest", "rkat-mcp"],
            pre_commands: &[
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rest",
                    "--bin",
                    "rkat-rest",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-mcp-server",
                    "--bin",
                    "rkat-mcp",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_63_mcp_bootstrap_to_rust_sdk_member_realtime_exchange",
                features: &[],
                all_features: false,
            },
        }),
        64 => Some(&Spec {
            id: Some(64),
            lane: Lane::Smoke,
            title: "Python SDK realtime member model-switch continuity",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_64_realtime_member_channel_model_switch_continuity",
            },
        }),
        65 => Some(&Spec {
            id: Some(65),
            lane: Lane::Live,
            title: "Realtime channel rejects stale attachment authority",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-runtime",
                    "realtime_attachment_signal_rejects_stale_authority",
                    "--lib",
                    "--",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        66 => Some(&Spec {
            id: Some(66),
            lane: Lane::Live,
            title: "Realtime reconnect retry machine exhausts within bounded budget",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-rpc",
                    "reconnect_retry_exhausts_after_attempt_budget",
                    "--lib",
                    "--",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        67 => Some(&Spec {
            id: Some(67),
            lane: Lane::Live,
            title: "Realtime websocket rejects unsupported explicit_commit mode",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-rpc",
                    "--test",
                    "realtime_ws_protocol",
                    "channel_open_rejects_unsupported_explicit_commit_turning_mode",
                    "--",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        68 => Some(&Spec {
            id: Some(68),
            lane: Lane::Live,
            title: "Realtime observer channels fan out primary events and stay read-only",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-rpc",
                    "--test",
                    "realtime_ws_protocol",
                    "observer_channels_receive_primary_events_and_remain_read_only",
                    "--",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        69 => Some(&Spec {
            id: Some(69),
            lane: Lane::Live,
            title: "Realtime explicit_commit disconnect discards staged transcript",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-rpc",
                    "--test",
                    "realtime_ws_protocol",
                    "explicit_commit_disconnect_discards_uncommitted_transcript",
                    "--",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        70 => Some(&Spec {
            id: Some(70),
            lane: Lane::Live,
            title: "Realtime tool continuation failure emits failed lifecycle and provider error",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-rpc",
                    "--test",
                    "realtime_ws_protocol",
                    "product_session_tool_call_failures_emit_failed_event_and_submit_provider_error",
                    "--",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        71 => Some(&Spec {
            id: Some(71),
            lane: Lane::Smoke,
            title: "Rust SDK realtime audio mob collaboration roundtrip",
            timeout_secs: 2400,
            required_env: &[&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-rpc"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-rpc",
                "--bin",
                "rkat-rpc",
                "--features",
                "mob",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_71_rust_sdk_realtime_audio_mob_collaboration_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        72 => Some(&Spec {
            id: Some(72),
            lane: Lane::Smoke,
            title: "Rust SDK realtime audio member model-switch continuity",
            timeout_secs: 1200,
            required_env: &[&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-rpc"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-rpc",
                "--bin",
                "rkat-rpc",
                "--features",
                "mob",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_72_rust_sdk_realtime_audio_member_model_switch_continuity",
                features: &[],
                all_features: false,
            },
        }),
        73 => Some(&Spec {
            id: Some(73),
            lane: Lane::Smoke,
            title: "CLI generate_image OpenAI default",
            timeout_secs: 900,
            required_env: &[&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "rkat",
                "--features",
                "session-store,openai,gemini,skills,comms,mcp,schedule,mob",
            ]],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_73_cli_generate_image_openai_default",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        74 => Some(&Spec {
            id: Some(74),
            lane: Lane::Smoke,
            title: "Python SDK generate_image Gemini provider params",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"],
            ],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_74_python_sdk_gemini_image_provider_params",
            },
        }),
        75 => Some(&Spec {
            id: Some(75),
            lane: Lane::Smoke,
            title: "TypeScript SDK generate_image OpenAI provider params",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 75",
            },
        }),
        76 => Some(&Spec {
            id: Some(76),
            lane: Lane::Smoke,
            title: "TypeScript SDK cross-provider image model-switch stress",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 76",
            },
        }),
        77 => Some(&Spec {
            id: Some(77),
            lane: Lane::Smoke,
            title: "TypeScript SDK stacked image generate/edit turn",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 77",
            },
        }),
        78 => Some(&Spec {
            id: Some(78),
            lane: Lane::Smoke,
            title: "TypeScript SDK cross-provider image relay readback",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 78",
            },
        }),
        79 => Some(&Spec {
            id: Some(79),
            lane: Lane::Smoke,
            title: "TypeScript SDK mob image critic handoff",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 79",
            },
        }),
        80 => Some(&Spec {
            id: Some(80),
            lane: Lane::Smoke,
            title: "TypeScript SDK persisted generated image resume",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 80",
            },
        }),
        81 => Some(&Spec {
            id: Some(81),
            lane: Lane::Smoke,
            title: "TypeScript SDK parallel image storm",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 81",
            },
        }),
        82 => Some(&Spec {
            id: Some(82),
            lane: Lane::Smoke,
            title: "TypeScript SDK blob image roundtrip",
            timeout_secs: 2400,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["npm", "install", "--no-audit", "--no-fund"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 82",
            },
        }),
        _ => None,
    }
}

fn suite_spec(name: &str) -> Option<&'static Spec> {
    match name {
        "fixture-embedded-min" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: embedded fixture roundtrip",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "{repo_root}/scripts/repo-cargo",
                    "run",
                    "-p",
                    "surface-build-fixtures",
                    "--bin",
                    "embedded_min",
                    "--",
                    "basic-roundtrip",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "fixture-runtime-backed-min" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: runtime-backed fixture roundtrip",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "{repo_root}/scripts/repo-cargo",
                    "run",
                    "-p",
                    "surface-build-fixtures",
                    "--bin",
                    "runtime_backed_min",
                    "--",
                    "basic-roundtrip",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-deterministic" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: rkat deterministic run/resume",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "{repo_root}/scripts/repo-cargo",
                    "test",
                    "-p",
                    "rkat",
                    "--features",
                    "integration-real-tests",
                    "--test",
                    "system_cli_resume",
                    "integration_real_cli_resume_tools",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-mini-deterministic" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: rkat-mini deterministic run/help surface",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "{repo_root}/scripts/repo-cargo",
                    "test",
                    "-p",
                    "rkat",
                    "--no-default-features",
                    "--features",
                    "anthropic,openai,gemini,jsonl-store,session-store,skills,integration-real-tests",
                    "--test",
                    "system_cli_mini",
                    "integration_real_rkat_mini_surface",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-rpc-deterministic" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: rkat-rpc built-binary deterministic roundtrip",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "tcp_e2e",
                test_name: "tcp_e2e_session_create_and_turn_start_with_test_client",
                features: &[],
                all_features: false,
            },
        }),
        "surface-rkat-rpc-mini-deterministic" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: rkat-rpc-mini built-binary deterministic roundtrip",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "{repo_root}/scripts/repo-cargo",
                    "test",
                    "-p",
                    "meerkat-rpc",
                    "--no-default-features",
                    "--features",
                    "mini-surface",
                    "--test",
                    "tcp_e2e",
                    "tcp_e2e_rkat_rpc_mini_initialize_and_roundtrip",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-rest-deterministic" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: rkat-rest built-binary deterministic roundtrip",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "binary_http_e2e",
                test_name: "http_e2e_create_and_continue_with_test_client",
                features: &[],
                all_features: false,
            },
        }),
        "surface-rkat-mcp-deterministic" => Some(&Spec {
            id: None,
            lane: Lane::Build,
            title: "Build lane: rkat-mcp built-binary deterministic run/resume",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[("RKAT_TEST_CLIENT", "1")],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "binary_stdio_e2e",
                test_name: "stdio_e2e_run_and_resume",
                features: &[],
                all_features: false,
            },
        }),
        "cli-init-snapshot" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI init snapshot",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "system_cli_init",
                test_name: "integration_real_rkat_init_snapshot",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-resume-tools" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI resume tools",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "system_cli_resume",
                test_name: "integration_real_cli_resume_tools",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "rest-resume-metadata" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "REST resume metadata",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "system_rest_resume",
                test_name: "integration_real_rest_resume_metadata",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-capabilities-and-config" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI capabilities and config",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_28_cli_capabilities_and_config",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-mobpack-pack-inspect-validate" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI mobpack pack/inspect/validate",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_smoke_mobpack_pack_inspect_validate",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-wasm-surface-gate" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI wasm surface gate",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_smoke_wasm_surface_gate",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-wasm-forbidden-capability" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI wasm forbidden capability rejection",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_smoke_wasm_forbidden_capability_rejected",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-mob-rpc-state-machine-probe" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "CLI mob RPC state machine probe",
            timeout_secs: 600,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_cli_mob_rpc_state_machine_probe",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "sqlite-shared-realm-rpc-rest-rpc" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "SQLite shared realm RPC -> REST -> RPC",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rpc",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "meerkat-rpc", "--bin", "rkat-rpc"],
                &["cargo", "build", "-p", "meerkat-rest", "--bin", "rkat-rest"],
                &[
                    "mkdir",
                    "-p",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "system_shared_realm",
                test_name: "rpc_rest_rpc_default_sqlite_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        "sqlite-shared-realm-cli-rpc-cli" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "SQLite shared realm CLI -> RPC -> CLI",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat-rpc",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &["cargo", "build", "-p", "meerkat-rpc", "--bin", "rkat-rpc"],
                &[
                    "mkdir",
                    "-p",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat-rpc",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "system_shared_realm",
                test_name: "cli_rpc_cli_default_sqlite_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        "sqlite-shared-realm-cli-rest-cli" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "SQLite shared realm CLI -> REST -> CLI",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &["cargo", "build", "-p", "meerkat-rest", "--bin", "rkat-rest"],
                &[
                    "mkdir",
                    "-p",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "system_shared_realm",
                test_name: "cli_rest_cli_default_sqlite_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        "surface-rkat-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "rkat",
                    "--no-default-features",
                    "--features",
                    "session-store",
                    "--bin",
                    "rkat",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-tests" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat no-default-features tests",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "nextest",
                    "run",
                    "-p",
                    "rkat",
                    "--no-default-features",
                    "--features",
                    "session-store,mcp",
                    "--no-capture",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-rpc-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat-rpc --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "meerkat-rpc",
                    "--no-default-features",
                    "--bin",
                    "rkat-rpc",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-rest-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat-rest --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "meerkat-rest",
                    "--no-default-features",
                    "--bin",
                    "rkat-rest",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-mcp-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat-mcp --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "meerkat-mcp-server",
                    "--no-default-features",
                    "--bin",
                    "rkat-mcp",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "agent-mob-tools" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "Agent mob tools live suite",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-integration-tests",
                    "--features",
                    "integration-real-tests",
                    "--test",
                    "live_mob_tools",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        "cli-structured-output" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "CLI structured output live regression",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_cli_structured_output",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "surface-build-fixtures-live" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "Surface fixture binaries live suite",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "surface-build-fixtures",
                    "--test",
                    "live_roundtrip",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        "rpc-mob-callback-tools" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC mob deferred callback tools smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_21_mob_callback_tools",
                features: &[],
                all_features: false,
            },
        }),
        "rpc-transport-backpressure" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC transport backpressure smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_22_transport_backpressure",
                features: &[],
                all_features: false,
            },
        }),
        "rpc-dynamic-tool-pickup" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC deferred dynamic tool pickup smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_23_late_register_on_existing_member",
                features: &[],
                all_features: false,
            },
        }),
        "rpc-deferred-catalog-session" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC deferred catalog direct-session smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_direct_session_deferred_callback_tool_flow",
                features: &[],
                all_features: false,
            },
        }),
        "cli-background-job-active-turn" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "CLI background job active-turn smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_001_background_job_active_turn_completion",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-background-job-idle-keepalive" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "CLI background job idle keepalive smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_002_background_job_idle_keepalive_completion",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "mob-live-smoke" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "Mob partial resume collaborative joke smoke",
            timeout_secs: 300,
            required_env: &[&[
                "RKAT_OPENAI_API_KEY",
                "OPENAI_API_KEY",
                "RKAT_GEMINI_API_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "RKAT_ANTHROPIC_API_KEY",
                "ANTHROPIC_API_KEY",
            ]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mob",
                test_target: "smoke_mob_resume",
                test_name: "e2e_smoke_mob_partial_resume_collaborative_joke",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "mob-flow-runtime" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "Mob flow runtime smoke suite",
            timeout_secs: 2400,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-mob",
                    "--features",
                    "integration-real-tests",
                    "--test",
                    "smoke_mob_flow_runtime",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        "mob-pictionary" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "Mob pictionary multimodal smoke",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY"],
            ],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mob",
                test_target: "smoke_mob_pictionary",
                test_name: "e2e_pictionary_multimodal_comms_stress",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactManifest, ArtifactRequirement, CommandLockMode, E2eSelection, ExecutionMode, Lane,
        SMOKE_ENTRIES, SmokeRuntimeClass, SmokeScheduler, build_commands_for_mode,
        normalize_command_with_env, order_smoke_specs_for_runtime, plan_for_selection,
        pre_command_lock_mode, repo_cargo, sanitize_artifact_key, scenario_spec,
        smoke_runtime_class, smoke_test_filter_for_selection, source_revision_key, suite_spec,
    };
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::time::{Duration, timeout};

    #[test]
    fn artifact_keys_are_stable_for_scenario_targets() {
        assert_eq!(
            sanitize_artifact_key("Surface smoke: rkat-rpc --help"),
            "surface-smoke-rkat-rpc-help"
        );
        assert_eq!(sanitize_artifact_key("56 RPC/rest"), "56-rpc-rest");
    }

    #[test]
    fn command_normalization_uses_scenario_target_dir_from_env() {
        let env = vec![(
            "CARGO_TARGET_DIR".to_string(),
            "/tmp/meerkat-e2e-scenario-target".to_string(),
        )];
        let argv =
            normalize_command_with_env(&["cargo", "run", "{cargo_target_dir}/debug/rkat"], &env);
        assert_eq!(argv[0], repo_cargo().display().to_string());
        assert_eq!(argv[2], "/tmp/meerkat-e2e-scenario-target/debug/rkat");
    }

    #[test]
    fn source_revision_key_is_path_safe() {
        let key = source_revision_key();
        assert!(!key.is_empty());
        assert!(
            key.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        );
    }

    #[test]
    fn numbered_catalog_covers_matrix_ids() {
        let ids = (15..=56)
            .filter(|id| scenario_spec(*id).is_some())
            .collect::<Vec<_>>();
        assert_eq!(ids, (15..=56).collect::<Vec<_>>());
        assert_eq!(ids.len(), 42);
    }

    #[test]
    fn live_and_smoke_counts_match_expected_split() {
        let live = (15..=56)
            .filter(|id| scenario_spec(*id).map(|spec| spec.lane) == Some(Lane::Live))
            .count();
        let smoke = (15..=56)
            .filter(|id| scenario_spec(*id).map(|spec| spec.lane) == Some(Lane::Smoke))
            .count();
        let system = (15..=56)
            .filter(|id| scenario_spec(*id).map(|spec| spec.lane) == Some(Lane::System))
            .count();
        assert_eq!(live, 24);
        assert_eq!(smoke, 17);
        assert_eq!(system, 1);
    }

    #[test]
    fn shared_setup_pre_commands_use_cross_process_locks() {
        assert_eq!(
            pre_command_lock_mode(&["/bin/sh", "-c", "test -d node_modules || npm ci"]),
            CommandLockMode::Shared
        );
        assert_eq!(
            pre_command_lock_mode(&["npm", "install", "--no-audit", "--no-fund"]),
            CommandLockMode::Shared
        );
        assert_eq!(
            pre_command_lock_mode(&["npm", "run", "build"]),
            CommandLockMode::Shared
        );
        assert_eq!(
            pre_command_lock_mode(&[
                "/bin/sh",
                "-c",
                "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
            ]),
            CommandLockMode::Shared
        );
    }

    #[test]
    fn scenario_local_cargo_builds_remain_parallelizable() {
        assert_eq!(
            pre_command_lock_mode(&["cargo", "build", "-p", "meerkat-rpc"]),
            CommandLockMode::ScenarioLocal
        );
        assert_eq!(
            pre_command_lock_mode(&["/bin/sh", "-c", "echo {cargo_target_dir}"]),
            CommandLockMode::ScenarioLocal
        );
    }

    #[test]
    fn smoke_selection_supports_scenario_suite_and_test_name() {
        assert_eq!(
            smoke_test_filter_for_selection(&E2eSelection::Scenario(62)).unwrap(),
            Some("e2e_smoke_s62_".to_string())
        );
        assert_eq!(
            smoke_test_filter_for_selection(&E2eSelection::Suite("mob-live-smoke".to_string()))
                .unwrap(),
            Some("e2e_smoke_mob_live_smoke".to_string())
        );

        let plan = plan_for_selection(&E2eSelection::SmokeTest(
            "e2e_smoke_mob_live_smoke".to_string(),
        ))
        .unwrap();
        assert_eq!(plan.specs.len(), 1);
        assert_eq!(plan.specs[0].suite.as_deref(), Some("mob-live-smoke"));
    }

    #[test]
    fn smoke_filter_rejects_invalid_selectors() {
        let non_smoke = smoke_test_filter_for_selection(&E2eSelection::Scenario(15)).unwrap_err();
        assert!(non_smoke.contains("not the e2e-smoke lane"), "{non_smoke}");

        let unknown = smoke_test_filter_for_selection(&E2eSelection::Scenario(9999)).unwrap_err();
        assert!(
            unknown.contains("unknown catalog scenario id 9999"),
            "{unknown}"
        );

        let typo = smoke_test_filter_for_selection(&E2eSelection::SmokeTest(
            "e2e_smoke_s62_typo".to_string(),
        ))
        .unwrap_err();
        assert!(typo.contains("unknown e2e-smoke test"), "{typo}");
    }

    #[test]
    fn smoke_lane_plan_uses_single_smoke_entry_catalog() {
        let plan = plan_for_selection(&E2eSelection::Lane(Lane::Smoke)).unwrap();
        assert_eq!(plan.specs.len(), SMOKE_ENTRIES.len());

        let planned_ids = plan
            .specs
            .iter()
            .filter_map(|spec| spec.id)
            .collect::<BTreeSet<_>>();
        let planned_suites = plan
            .specs
            .iter()
            .filter_map(|spec| spec.suite.as_deref())
            .collect::<BTreeSet<_>>();
        let mut test_names = BTreeSet::new();

        for entry in SMOKE_ENTRIES {
            assert!(
                test_names.insert(entry.test_name()),
                "duplicate smoke test name {}",
                entry.test_name()
            );
            match entry {
                super::SmokeEntry::Scenario { id, .. } => {
                    assert!(planned_ids.contains(id), "missing smoke scenario {id}");
                }
                super::SmokeEntry::Suite { suite, .. } => {
                    assert!(
                        planned_suites.contains(suite),
                        "missing smoke suite {suite}"
                    );
                }
            }
        }
    }

    #[test]
    fn smoke_scheduler_classifies_high_pressure_specs() {
        assert_eq!(
            smoke_runtime_class(suite_spec("mob-flow-runtime").unwrap()),
            SmokeRuntimeClass::MobSuite
        );
        assert_eq!(
            smoke_runtime_class(scenario_spec(62).unwrap()),
            SmokeRuntimeClass::Realtime
        );
        assert_eq!(
            smoke_runtime_class(scenario_spec(81).unwrap()),
            SmokeRuntimeClass::Media
        );
        assert_eq!(
            smoke_runtime_class(suite_spec("rpc-dynamic-tool-pickup").unwrap()),
            SmokeRuntimeClass::Standard
        );
    }

    #[test]
    fn smoke_scheduler_prioritizes_long_runtime_specs() {
        let mut selected = vec![
            super::SelectedSpec {
                spec: scenario_spec(16).unwrap(),
                suite: None,
            },
            super::SelectedSpec {
                spec: suite_spec("mob-pictionary").unwrap(),
                suite: Some("mob-pictionary".to_string()),
            },
            super::SelectedSpec {
                spec: scenario_spec(73).unwrap(),
                suite: None,
            },
        ];
        order_smoke_specs_for_runtime(&mut selected);
        assert_eq!(selected[0].suite.as_deref(), Some("mob-pictionary"));
        assert_eq!(selected[1].spec.id, Some(73));
        assert_eq!(selected[2].spec.id, Some(16));
    }

    #[tokio::test]
    async fn class_limited_specs_do_not_occupy_global_slots_while_waiting() {
        let scheduler = Arc::new(SmokeScheduler {
            jobs: 1,
            realtime_jobs: 1,
            media_jobs: 0,
            mob_suite_jobs: 1,
            global: Arc::new(Semaphore::new(1)),
            realtime: Arc::new(Semaphore::new(1)),
            media: Arc::new(Semaphore::new(0)),
            mob_suite: Arc::new(Semaphore::new(1)),
        });
        let blocked_media = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move { scheduler.acquire(scenario_spec(73).unwrap()).await })
        };

        tokio::task::yield_now().await;

        let standard = timeout(
            Duration::from_millis(100),
            scheduler.acquire(scenario_spec(16).unwrap()),
        )
        .await
        .expect("standard spec should not wait behind a media class backlog")
        .expect("standard spec should acquire scheduler permits");
        drop(standard);
        blocked_media.abort();
    }

    #[test]
    fn single_scenario_plan_avoids_unrelated_service_bins() {
        let plan = plan_for_selection(&E2eSelection::Scenario(62)).unwrap();
        let rust_bins = plan
            .requirements
            .iter()
            .filter_map(|requirement| match requirement {
                ArtifactRequirement::RustBin(requirement) => Some(requirement.bin.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(rust_bins, vec!["rkat-rest", "rkat-rpc"]);
        assert!(plan.requirements.iter().any(|requirement| matches!(
            requirement,
            ArtifactRequirement::RustTest(requirement)
                if requirement.package == "meerkat-integration-tests"
                    && requirement.test_target == "smoke_shared_realm"
        )));
    }

    #[test]
    fn manifest_parses_stable_rust_artifact_keys() {
        let manifest = ArtifactManifest::from_json_str(
            r#"{
              "rust_bins": {
                "rkat-rpc": "/tmp/rkat-rpc"
              },
              "rust_tests": {
                "meerkat-integration-tests:smoke_shared_realm": "/tmp/smoke_shared_realm"
              }
            }"#,
        )
        .unwrap();
        assert_eq!(
            manifest.rust_bins["rkat-rpc"],
            PathBuf::from("/tmp/rkat-rpc")
        );
        assert_eq!(
            manifest.rust_tests["meerkat-integration-tests:smoke_shared_realm"],
            PathBuf::from("/tmp/smoke_shared_realm")
        );
    }

    #[test]
    fn command_generation_switches_cargo_test_to_prebuilt_executable() {
        let spec = scenario_spec(62).unwrap();
        let manifest = ArtifactManifest::from_json_str(
            r#"{
              "rust_bins": {
                "rkat-rpc": "/tmp/rkat-rpc",
                "rkat-rest": "/tmp/rkat-rest"
              },
              "rust_tests": {
                "meerkat-integration-tests:smoke_shared_realm": "/tmp/smoke_shared_realm"
              }
            }"#,
        )
        .unwrap();

        let cargo = build_commands_for_mode(spec, ExecutionMode::Cargo, None).unwrap();
        assert_eq!(cargo.command[0], "cargo");
        assert!(cargo.command.contains(&"test".to_string()));

        let prebuilt =
            build_commands_for_mode(spec, ExecutionMode::Prebuilt, Some(&manifest)).unwrap();
        assert_eq!(
            prebuilt.command,
            vec![
                "/tmp/smoke_shared_realm",
                "e2e_scenario_62_rest_bootstrap_to_rust_sdk_realtime_channel_exchange",
                "--ignored",
                "--nocapture"
            ]
        );
        assert!(
            prebuilt
                .pre_commands
                .iter()
                .all(|command| command.first().map(String::as_str) != Some("cargo"))
        );
    }

    #[test]
    fn prebuilt_node_specs_do_not_repeat_setup_precommands() {
        let spec = scenario_spec(44).unwrap();
        let manifest = ArtifactManifest::from_json_str(
            r#"{
              "rust_bins": {
                "rkat-rpc": "/tmp/rkat-rpc"
              },
              "rust_tests": {}
            }"#,
        )
        .unwrap();

        let prebuilt =
            build_commands_for_mode(spec, ExecutionMode::Prebuilt, Some(&manifest)).unwrap();
        assert!(
            prebuilt.pre_commands.is_empty(),
            "node setup should move to materialization in prebuilt mode: {:?}",
            prebuilt.pre_commands
        );
    }
}
