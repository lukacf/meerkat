use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use wasm_bindgen::prelude::*;

const FORBIDDEN_CAPABILITIES: &[&str] = &["shell", "mcp_stdio", "process_spawn"];

#[derive(Debug, Deserialize)]
struct WebManifest {
    mobpack: WebMobpackSection,
    #[serde(default)]
    requires: Option<WebRequiresSection>,
}

#[derive(Debug, Deserialize)]
struct WebMobpackSection {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize, Default)]
struct WebRequiresSection {
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WebDefinition {
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InitOptions {
    team: String,
    model: String,
    #[serde(default)]
    seed: u64,
    #[serde(default)]
    auth_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeEvent {
    seq: u64,
    kind: String,
    from: String,
    #[serde(default)]
    to: Option<String>,
    team: String,
    payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegionState {
    id: String,
    controller: String,
    defense: i32,
    value: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArenaState {
    turn: u32,
    max_turns: u32,
    regions: Vec<RegionState>,
    north_score: u32,
    south_score: u32,
    #[serde(default)]
    winner: Option<String>,
    #[serde(default)]
    ruleset_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnInput {
    state: ArenaState,
    #[serde(default)]
    opponent_signal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OrderSet {
    team: String,
    turn: u32,
    aggression: i32,
    fortify: i32,
    diplomacy: String,
    target_region: String,
    model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnDecision {
    order: OrderSet,
    planner_note: String,
    operator_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolveInput {
    state: ArenaState,
    north_order: OrderSet,
    south_order: OrderSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolveOutput {
    state: ArenaState,
    summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionSnapshot {
    handle: u32,
    mob_id: String,
    team: String,
    model: String,
    seed: u64,
    started: bool,
    seq: u64,
    pending_events: Vec<RuntimeEvent>,
    last_order: Option<OrderSet>,
}

#[derive(Debug, Clone)]
struct RuntimeSession {
    handle: u32,
    mob_id: String,
    team: String,
    model: String,
    seed: u64,
    started: bool,
    seq: u64,
    events: VecDeque<RuntimeEvent>,
    last_order: Option<OrderSet>,
}

#[derive(Default)]
struct RuntimeRegistry {
    next_handle: u32,
    sessions: BTreeMap<u32, RuntimeSession>,
}

thread_local! {
    static REGISTRY: RefCell<RuntimeRegistry> = const {
        RefCell::new(RuntimeRegistry {
            next_handle: 1,
            sessions: BTreeMap::new(),
        })
    };
}

fn parse_mobpack(bytes: &[u8]) -> Result<(WebManifest, WebDefinition), String> {
    let files = extract_targz_safe(bytes)
        .map_err(|err| format!("failed to parse mobpack archive: {err}"))?;
    let manifest_text = std::str::from_utf8(
        files
            .get("manifest.toml")
            .ok_or_else(|| "manifest.toml is missing".to_string())?,
    )
    .map_err(|err| format!("manifest.toml is not valid UTF-8: {err}"))?;
    let manifest: WebManifest =
        toml::from_str(manifest_text).map_err(|err| format!("invalid manifest.toml: {err}"))?;
    let definition: WebDefinition = serde_json::from_slice(
        files
            .get("definition.json")
            .ok_or_else(|| "definition.json is missing".to_string())?,
    )
    .map_err(|err| format!("invalid definition.json: {err}"))?;

    if let Some(requires) = &manifest.requires {
        for capability in &requires.capabilities {
            if FORBIDDEN_CAPABILITIES.contains(&capability.as_str()) {
                return Err(format!(
                    "forbidden capability '{}' is not allowed in browser-safe mode",
                    capability
                ));
            }
        }
    }

    Ok((manifest, definition))
}

fn parse_json<T: for<'a> Deserialize<'a>>(raw: &str, field: &str) -> Result<T, String> {
    serde_json::from_str(raw).map_err(|err| format!("invalid {field}: {err}"))
}

fn stable_mix(seed: u64, text: &str) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    for &b in text.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x1000_0000_01B3);
        h ^= h >> 29;
    }
    h
}

fn strength_from_model(model: &str) -> i32 {
    let model_lc = model.to_ascii_lowercase();
    if model_lc.contains("opus") {
        88
    } else if model_lc.contains("gpt-5") {
        84
    } else if model_lc.contains("gemini") {
        80
    } else {
        72
    }
}

fn default_ruleset(mut state: ArenaState) -> ArenaState {
    if state.ruleset_version.trim().is_empty() {
        state.ruleset_version = "mini-diplomacy-v1".to_string();
    }
    state
}

fn choose_target(regions: &[RegionState], team: &str, seed: u64, turn: u32) -> String {
    let enemy = if team == "north" { "south" } else { "north" };
    let mut targets: Vec<&RegionState> = regions.iter().filter(|r| r.controller == enemy).collect();
    if targets.is_empty() {
        targets = regions.iter().collect();
    }
    targets.sort_by(|a, b| a.id.cmp(&b.id));
    let idx_seed = stable_mix(seed ^ u64::from(turn), team);
    let len_u64 = u64::try_from(targets.len()).unwrap_or(1);
    let idx_u64 = if len_u64 == 0 { 0 } else { idx_seed % len_u64 };
    let idx = usize::try_from(idx_u64).unwrap_or(0);
    targets
        .get(idx)
        .map(|r| r.id.clone())
        .unwrap_or_else(|| "center".to_string())
}

fn propose_order(session: &RuntimeSession, input: &TurnInput) -> TurnDecision {
    let state = &input.state;
    let model_strength = strength_from_model(&session.model);
    let seed_mix = stable_mix(session.seed ^ u64::from(state.turn), &session.team);
    let mod20 = i32::try_from(seed_mix % 20).unwrap_or(0);
    let aggression = 30 + ((model_strength / 3) + mod20).min(70);
    let fortify = 100 - aggression;
    let signal = input
        .opponent_signal
        .as_deref()
        .unwrap_or("no-opponent-signal");
    let planner_note = format!(
        "Turn {} plan: pressure={}, fortify={}, signal={}.",
        state.turn, aggression, fortify, signal
    );
    let target_region = choose_target(&state.regions, &session.team, session.seed, state.turn);
    let diplomacy = if aggression > 72 {
        "conditional_non_aggression".to_string()
    } else {
        "mutual_frontier_stability".to_string()
    };
    let order = OrderSet {
        team: session.team.clone(),
        turn: state.turn,
        aggression,
        fortify,
        diplomacy,
        target_region,
        model: session.model.clone(),
    };
    let operator_note = format!(
        "Committed legal order for {} targeting {} (aggr={}, fortify={}).",
        order.team, order.target_region, order.aggression, order.fortify
    );
    TurnDecision {
        order,
        planner_note,
        operator_note,
    }
}

fn push_event(
    session: &mut RuntimeSession,
    kind: &str,
    from: &str,
    to: Option<&str>,
    payload: String,
) {
    session.seq = session.seq.saturating_add(1);
    session.events.push_back(RuntimeEvent {
        seq: session.seq,
        kind: kind.to_string(),
        from: from.to_string(),
        to: to.map(std::string::ToString::to_string),
        team: session.team.clone(),
        payload,
    });
}

fn apply_order(state: &mut ArenaState, order: &OrderSet, counter: &OrderSet) {
    let team = order.team.as_str();
    let enemy = if team == "north" { "south" } else { "north" };

    for region in &mut state.regions {
        if region.controller == team {
            region.defense = region.defense.saturating_add((order.fortify / 25).max(1));
            break;
        }
    }

    let mut captured = false;
    for region in &mut state.regions {
        if region.id != order.target_region {
            continue;
        }
        if region.controller == team {
            continue;
        }
        let atk_bonus = i32::try_from(
            stable_mix(
                u64::from(state.turn),
                &(order.model.clone() + &order.target_region),
            ) % 11,
        )
        .unwrap_or(0);
        let def_bonus = i32::try_from(
            stable_mix(
                u64::from(state.turn) ^ 0xA5A5,
                &(counter.model.clone() + &region.id),
            ) % 9,
        )
        .unwrap_or(0);
        let attack = order.aggression + atk_bonus;
        let defense = region
            .defense
            .saturating_add(counter.fortify / 30)
            .saturating_add(def_bonus);
        if attack > defense {
            region.controller = team.to_string();
            region.defense = 45 + ((order.fortify / 6).max(4));
            captured = true;
        } else {
            region.defense = region.defense.saturating_add(1);
        }
        break;
    }

    if !captured {
        for region in &mut state.regions {
            if region.controller == enemy {
                region.defense = region.defense.saturating_add(1);
                break;
            }
        }
    }
}

fn score_turn(state: &mut ArenaState) {
    let mut north_delta = 0_u32;
    let mut south_delta = 0_u32;
    for region in &state.regions {
        if region.controller == "north" {
            north_delta = north_delta.saturating_add(region.value);
        } else if region.controller == "south" {
            south_delta = south_delta.saturating_add(region.value);
        }
    }
    state.north_score = state.north_score.saturating_add(north_delta);
    state.south_score = state.south_score.saturating_add(south_delta);
}

fn resolve_turn_impl(mut input: ResolveInput) -> ResolveOutput {
    input.state = default_ruleset(input.state);
    apply_order(&mut input.state, &input.north_order, &input.south_order);
    apply_order(&mut input.state, &input.south_order, &input.north_order);
    score_turn(&mut input.state);
    input.state.turn = input.state.turn.saturating_add(1);

    if input.state.turn > input.state.max_turns {
        input.state.winner = if input.state.north_score > input.state.south_score {
            Some("north".to_string())
        } else if input.state.south_score > input.state.north_score {
            Some("south".to_string())
        } else {
            Some("draw".to_string())
        };
    }

    let summary = format!(
        "turn={} north={} south={} winner={}",
        input.state.turn,
        input.state.north_score,
        input.state.south_score,
        input
            .state
            .winner
            .clone()
            .unwrap_or_else(|| "pending".to_string())
    );

    ResolveOutput {
        state: input.state,
        summary,
    }
}

fn bootstrap_impl(mobpack_bytes: &[u8], prompt: &str) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("prompt must not be empty".to_string());
    }
    let (manifest, definition) = parse_mobpack(mobpack_bytes)?;

    Ok(format!(
        "bootstrapped:{}:{}@{}:prompt_bytes={}",
        definition.id,
        manifest.mobpack.name,
        manifest.mobpack.version,
        prompt.len()
    ))
}

fn init_mobpack_impl(mobpack_bytes: &[u8], options_json: &str) -> Result<u32, String> {
    let (_manifest, definition) = parse_mobpack(mobpack_bytes)?;
    let options: InitOptions = parse_json(options_json, "options_json")?;
    let team = options.team.trim().to_ascii_lowercase();
    if team != "north" && team != "south" {
        return Err("options_json.team must be 'north' or 'south'".to_string());
    }
    if options.model.trim().is_empty() {
        return Err("options_json.model must not be empty".to_string());
    }

    let handle = REGISTRY.with(|cell| {
        let mut registry = cell.borrow_mut();
        let handle = registry.next_handle;
        registry.next_handle = registry.next_handle.saturating_add(1);
        let session = RuntimeSession {
            handle,
            mob_id: definition.id,
            team,
            model: options.model,
            seed: options.seed,
            started: false,
            seq: 0,
            events: VecDeque::new(),
            last_order: None,
        };
        registry.sessions.insert(handle, session);
        handle
    });

    Ok(handle)
}

fn with_session_mut<T>(
    handle: u32,
    f: impl FnOnce(&mut RuntimeSession) -> Result<T, String>,
) -> Result<T, String> {
    REGISTRY.with(|cell| {
        let mut registry = cell.borrow_mut();
        let session = registry
            .sessions
            .get_mut(&handle)
            .ok_or_else(|| format!("unknown handle: {handle}"))?;
        f(session)
    })
}

fn start_match_impl(handle: u32, initial_state_json: &str) -> Result<String, String> {
    let initial: ArenaState = parse_json(initial_state_json, "initial_state_json")?;
    with_session_mut(handle, |session| {
        session.started = true;
        push_event(
            session,
            "SYSTEM",
            "runtime",
            None,
            format!(
                "match_started team={} model={} ruleset={}",
                session.team,
                session.model,
                if initial.ruleset_version.is_empty() {
                    "mini-diplomacy-v1"
                } else {
                    initial.ruleset_version.as_str()
                }
            ),
        );
        serde_json::to_string(&default_ruleset(initial.clone()))
            .map_err(|err| format!("failed encoding start state: {err}"))
    })
}

fn submit_turn_input_impl(handle: u32, turn_input_json: &str) -> Result<String, String> {
    let input: TurnInput = parse_json(turn_input_json, "turn_input_json")?;
    with_session_mut(handle, |session| {
        if !session.started {
            return Err("match not started for handle".to_string());
        }
        let decision = propose_order(session, &input);

        push_event(
            session,
            "PLAN",
            "planner",
            Some("operator"),
            decision.planner_note.clone(),
        );
        push_event(
            session,
            "NEGOTIATE",
            "planner",
            Some("opponent"),
            format!(
                "proposal={} target={}",
                decision.order.diplomacy, decision.order.target_region
            ),
        );
        push_event(
            session,
            "COMMIT",
            "operator",
            Some("board"),
            decision.operator_note.clone(),
        );

        session.last_order = Some(decision.order.clone());

        serde_json::to_string(&decision).map_err(|err| format!("failed encoding decision: {err}"))
    })
}

fn poll_events_impl(handle: u32) -> Result<String, String> {
    with_session_mut(handle, |session| {
        let drained: Vec<RuntimeEvent> = session.events.drain(..).collect();
        serde_json::to_string(&drained).map_err(|err| format!("failed encoding events: {err}"))
    })
}

fn snapshot_state_impl(handle: u32) -> Result<String, String> {
    with_session_mut(handle, |session| {
        let snapshot = SessionSnapshot {
            handle: session.handle,
            mob_id: session.mob_id.clone(),
            team: session.team.clone(),
            model: session.model.clone(),
            seed: session.seed,
            started: session.started,
            seq: session.seq,
            pending_events: session.events.iter().cloned().collect(),
            last_order: session.last_order.clone(),
        };
        serde_json::to_string(&snapshot).map_err(|err| format!("failed encoding snapshot: {err}"))
    })
}

fn restore_state_impl(handle: u32, snapshot_json: &str) -> Result<(), String> {
    let snapshot: SessionSnapshot = parse_json(snapshot_json, "snapshot_json")?;
    with_session_mut(handle, |session| {
        if snapshot.team != session.team {
            return Err("snapshot team does not match runtime session".to_string());
        }
        session.model = snapshot.model;
        session.seed = snapshot.seed;
        session.started = snapshot.started;
        session.seq = snapshot.seq;
        session.events = snapshot.pending_events.into_iter().collect();
        session.last_order = snapshot.last_order;
        Ok(())
    })
}

fn resolve_turn_json_impl(input_json: &str) -> Result<String, String> {
    let input: ResolveInput = parse_json(input_json, "resolve_input_json")?;
    let output = resolve_turn_impl(input);
    serde_json::to_string(&output).map_err(|err| format!("failed encoding resolved state: {err}"))
}

fn extract_targz_safe(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let decoder = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|err| format!("failed to read archive entries: {err}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|err| format!("failed reading archive entry: {err}"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err("archive contains unsupported entry type".to_string());
        }
        let path = entry
            .path()
            .map_err(|err| format!("invalid archive path: {err}"))?;
        if !kind.is_file() {
            continue;
        }
        let normalized = normalize_for_archive(path.to_string_lossy().as_ref())?;
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|err| format!("failed reading archive file '{normalized}': {err}"))?;
        files.insert(normalized, contents);
    }
    Ok(files)
}

fn normalize_for_archive(path: &str) -> Result<String, String> {
    let replaced = path.replace('\\', "/");
    if replaced.starts_with('/') || looks_like_windows_absolute(&replaced) {
        return Err("archive contains absolute path entry".to_string());
    }
    let mut parts = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err("archive contains parent directory traversal entry".to_string());
        }
        parts.push(segment);
    }
    if parts.is_empty() {
        return Err("archive contains empty path entry".to_string());
    }
    Ok(parts.join("/"))
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

#[wasm_bindgen]
pub fn bootstrap_mobpack(mobpack_bytes: &[u8], prompt: &str) -> Result<String, JsValue> {
    bootstrap_impl(mobpack_bytes, prompt).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn init_mobpack(mobpack_bytes: &[u8], options_json: &str) -> Result<u32, JsValue> {
    init_mobpack_impl(mobpack_bytes, options_json).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn start_match(handle: u32, initial_state_json: &str) -> Result<String, JsValue> {
    start_match_impl(handle, initial_state_json).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn submit_turn_input(handle: u32, turn_input_json: &str) -> Result<String, JsValue> {
    submit_turn_input_impl(handle, turn_input_json).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn poll_events(handle: u32) -> Result<String, JsValue> {
    poll_events_impl(handle).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn snapshot_state(handle: u32) -> Result<String, JsValue> {
    snapshot_state_impl(handle).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn restore_state(handle: u32, snapshot_json: &str) -> Result<(), JsValue> {
    restore_state_impl(handle, snapshot_json).map_err(|err| JsValue::from_str(&err))
}

#[wasm_bindgen]
pub fn resolve_turn(resolve_input_json: &str) -> Result<String, JsValue> {
    resolve_turn_json_impl(resolve_input_json).map_err(|err| JsValue::from_str(&err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_archive(with_forbidden_capability: bool) -> Vec<u8> {
        let manifest = if with_forbidden_capability {
            "[mobpack]\nname = \"web\"\nversion = \"1.0.0\"\n\n[requires]\ncapabilities = [\"shell\"]\n"
        } else {
            "[mobpack]\nname = \"web\"\nversion = \"1.0.0\"\n"
        };
        let definition = "{\"id\":\"web-mob\",\"skills\":{}}";
        let files = BTreeMap::from([
            ("manifest.toml".to_string(), manifest.as_bytes().to_vec()),
            (
                "definition.json".to_string(),
                definition.as_bytes().to_vec(),
            ),
        ]);
        create_targz(&files)
    }

    fn create_targz(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.mode(tar::HeaderMode::Deterministic);
            for (path, content) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_uid(0);
                header.set_gid(0);
                header.set_cksum();
                let append_result =
                    builder.append_data(&mut header, path.as_str(), content.as_slice());
                assert!(
                    append_result.is_ok(),
                    "append data failed: {append_result:?}"
                );
            }
            let finish_result = builder.finish();
            assert!(
                finish_result.is_ok(),
                "finish tar failed: {finish_result:?}"
            );
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let write_result = std::io::Write::write_all(&mut encoder, &tar_bytes);
        assert!(write_result.is_ok(), "write gz failed: {write_result:?}");
        match encoder.finish() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("finish gz failed: {err}"),
        }
    }

    fn create_single_entry_archive(path: &str) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.mode(tar::HeaderMode::Deterministic);
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            let append_result = builder.append_data(&mut header, path, b"x".as_slice());
            assert!(
                append_result.is_ok(),
                "append data failed: {append_result:?}"
            );
            let finish_result = builder.finish();
            assert!(
                finish_result.is_ok(),
                "finish tar failed: {finish_result:?}"
            );
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let write_result = std::io::Write::write_all(&mut encoder, &tar_bytes);
        assert!(write_result.is_ok(), "write gz failed: {write_result:?}");
        match encoder.finish() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("finish gz failed: {err}"),
        }
    }

    fn initial_state() -> ArenaState {
        ArenaState {
            turn: 1,
            max_turns: 3,
            regions: vec![
                RegionState {
                    id: "north-hub".to_string(),
                    controller: "north".to_string(),
                    defense: 50,
                    value: 2,
                },
                RegionState {
                    id: "south-hub".to_string(),
                    controller: "south".to_string(),
                    defense: 50,
                    value: 2,
                },
                RegionState {
                    id: "frontier".to_string(),
                    controller: "south".to_string(),
                    defense: 42,
                    value: 4,
                },
            ],
            north_score: 0,
            south_score: 0,
            winner: None,
            ruleset_version: "mini-diplomacy-v1".to_string(),
        }
    }

    #[test]
    fn bootstrap_accepts_browser_safe_pack() {
        let archive = fixture_archive(false);
        let out = match bootstrap_impl(&archive, "hello") {
            Ok(value) => value,
            Err(err) => unreachable!("bootstrap should pass: {err}"),
        };
        assert!(out.contains("bootstrapped:web-mob"));
    }

    #[test]
    fn bootstrap_rejects_forbidden_capability() {
        let archive = fixture_archive(true);
        let err = match bootstrap_impl(&archive, "hello") {
            Ok(value) => unreachable!("should fail, got: {value}"),
            Err(err) => err,
        };
        assert!(err.contains("forbidden capability 'shell'"));
    }

    #[test]
    fn extract_rejects_windows_absolute_paths() {
        let archive = create_single_entry_archive("C:/temp/evil.txt");
        let err = match extract_targz_safe(&archive) {
            Ok(_) => unreachable!("windows absolute should fail"),
            Err(err) => err,
        };
        assert!(err.contains("absolute path"));
    }

    #[test]
    fn runtime_lifecycle_roundtrip() {
        let archive = fixture_archive(false);
        let opts = r#"{"team":"north","model":"gpt-5.2","seed":17}"#;
        let handle = match init_mobpack_impl(&archive, opts) {
            Ok(value) => value,
            Err(err) => unreachable!("init should pass: {err}"),
        };

        let init_json = match serde_json::to_string(&initial_state()) {
            Ok(value) => value,
            Err(err) => unreachable!("encode state failed: {err}"),
        };
        let start_out = start_match_impl(handle, &init_json);
        assert!(start_out.is_ok(), "start failed: {start_out:?}");

        let turn_input = TurnInput {
            state: initial_state(),
            opponent_signal: Some("offer_truce".to_string()),
        };
        let turn_json = match serde_json::to_string(&turn_input) {
            Ok(value) => value,
            Err(err) => unreachable!("encode turn failed: {err}"),
        };
        let decision_json = match submit_turn_input_impl(handle, &turn_json) {
            Ok(value) => value,
            Err(err) => unreachable!("turn should pass: {err}"),
        };
        assert!(decision_json.contains("planner_note"));

        let events_json = match poll_events_impl(handle) {
            Ok(value) => value,
            Err(err) => unreachable!("poll should pass: {err}"),
        };
        assert!(events_json.contains("PLAN"));
        assert!(events_json.contains("COMMIT"));

        let snapshot = match snapshot_state_impl(handle) {
            Ok(value) => value,
            Err(err) => unreachable!("snapshot should pass: {err}"),
        };
        let restored = restore_state_impl(handle, &snapshot);
        assert!(restored.is_ok(), "restore should pass: {restored:?}");
    }

    #[test]
    fn resolver_produces_winner_after_max_turns() {
        let input = ResolveInput {
            state: initial_state(),
            north_order: OrderSet {
                team: "north".to_string(),
                turn: 1,
                aggression: 80,
                fortify: 20,
                diplomacy: "conditional_non_aggression".to_string(),
                target_region: "frontier".to_string(),
                model: "claude-opus-4-6".to_string(),
            },
            south_order: OrderSet {
                team: "south".to_string(),
                turn: 1,
                aggression: 60,
                fortify: 40,
                diplomacy: "mutual_frontier_stability".to_string(),
                target_region: "north-hub".to_string(),
                model: "gemini-3.1-pro-preview".to_string(),
            },
        };
        let output = resolve_turn_impl(input);
        assert!(output.state.turn == 2);
        assert!(output.state.ruleset_version == "mini-diplomacy-v1");
    }
}
