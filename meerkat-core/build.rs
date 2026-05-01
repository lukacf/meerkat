use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SYMBOL_ENV: &str = "MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL";
const PROOF_ENV: &str = "MEERKAT_AGENT_FACTORY_POLICY_BUILD_PROOF";

struct BridgeConfig {
    symbol: String,
    proof: String,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    match build_config() {
        Ok(config) => {
            println!("cargo:rustc-env={SYMBOL_ENV}={}", config.symbol);
            println!("cargo:rustc-env={PROOF_ENV}={}", config.proof);
        }
        Err(err) => {
            eprintln!("failed to prepare factory policy bridge config: {err}");
            std::process::exit(1);
        }
    }
}

fn build_config() -> Result<BridgeConfig, String> {
    let path = config_file_path()?;
    match fs::read_to_string(&path) {
        Ok(config) => return parse_config(&config, &path),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!("read bridge config {}: {err}", path.display()));
        }
    }

    let config = fresh_config();
    let parent = path
        .parent()
        .ok_or_else(|| format!("bridge config path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("create bridge config directory {}: {err}", parent.display()))?;

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            writeln!(file, "{}", config.symbol)
                .map_err(|err| format!("write bridge config {}: {err}", path.display()))?;
            writeln!(file, "{}", config.proof)
                .map_err(|err| format!("write bridge config {}: {err}", path.display()))?;
            Ok(config)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let config = fs::read_to_string(&path).map_err(|err| {
                format!("read bridge config after race {}: {err}", path.display())
            })?;
            parse_config(&config, &path)
        }
        Err(err) => Err(format!("create bridge config {}: {err}", path.display())),
    }
}

fn config_file_path() -> Result<PathBuf, String> {
    let out_dir =
        PathBuf::from(env::var("OUT_DIR").map_err(|err| format!("OUT_DIR is not set: {err}"))?);
    let target_root = match out_dir.ancestors().nth(4) {
        Some(path) => path,
        None => out_dir.as_path(),
    }
    .to_path_buf();
    Ok(target_root
        .join(".meerkat")
        .join("agent_factory_policy_bridge_config_v2"))
}

fn fresh_config() -> BridgeConfig {
    let first = fresh_hash("symbol-a");
    let second = fresh_hash("symbol-b");
    let proof_a = fresh_hash("proof-a");
    let proof_b = fresh_hash("proof-b");
    let proof_c = fresh_hash("proof-c");
    let proof_d = fresh_hash("proof-d");

    BridgeConfig {
        symbol: format!("__meerkat_agent_factory_policy_bridge_{first:016x}{second:016x}"),
        proof: format!("{proof_a:016x}{proof_b:016x}{proof_c:016x}{proof_d:016x}"),
    }
}

fn fresh_hash(salt: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    salt.hash(&mut hasher);
    hash_env("OUT_DIR", &mut hasher);
    hash_env("CARGO_MANIFEST_DIR", &mut hasher);
    hash_env("CARGO_PKG_VERSION", &mut hasher);
    std::process::id().hash(&mut hasher);
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().hash(&mut hasher),
        Err(err) => err.duration().as_nanos().hash(&mut hasher),
    }
    hasher.finish()
}

fn hash_env<H: Hasher>(key: &str, state: &mut H) {
    if let Ok(value) = env::var(key) {
        value.hash(state);
    }
}

fn parse_config(config: &str, path: &Path) -> Result<BridgeConfig, String> {
    let mut lines = config.lines();
    let symbol = lines
        .next()
        .ok_or_else(|| format!("bridge config {} is missing symbol", path.display()))?
        .trim()
        .to_string();
    let proof = lines
        .next()
        .ok_or_else(|| format!("bridge config {} is missing proof", path.display()))?
        .trim()
        .to_string();
    if lines.any(|line| !line.trim().is_empty()) {
        return Err(format!(
            "bridge config {} contains trailing data",
            path.display()
        ));
    }
    validate_symbol(&symbol)?;
    validate_proof(&proof)?;
    Ok(BridgeConfig { symbol, proof })
}

fn validate_symbol(symbol: &str) -> Result<(), String> {
    if symbol.starts_with("__meerkat_agent_factory_policy_bridge_")
        && symbol
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        Ok(())
    } else {
        Err("invalid factory policy bridge symbol".to_string())
    }
}

fn validate_proof(proof: &str) -> Result<(), String> {
    if proof.len() == 64 && proof.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("invalid factory policy bridge proof".to_string())
    }
}
