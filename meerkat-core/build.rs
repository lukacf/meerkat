use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const SYMBOL_ENV: &str = "MEERKAT_AGENT_FACTORY_POLICY_BUILD_SYMBOL";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={SYMBOL_ENV}");

    match build_symbol() {
        Ok(symbol) => println!("cargo:rustc-env={SYMBOL_ENV}={symbol}"),
        Err(err) => {
            eprintln!("failed to prepare factory policy bridge symbol: {err}");
            std::process::exit(1);
        }
    }
}

fn build_symbol() -> Result<String, String> {
    if let Ok(symbol) = env::var(SYMBOL_ENV) {
        validate_symbol(&symbol)?;
        return Ok(symbol);
    }

    let path = symbol_file_path()?;
    match fs::read_to_string(&path) {
        Ok(symbol) => {
            let symbol = symbol.trim().to_string();
            validate_symbol(&symbol)?;
            return Ok(symbol);
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!("read bridge symbol {}: {err}", path.display()));
        }
    }

    let symbol = fresh_symbol();
    let parent = path
        .parent()
        .ok_or_else(|| format!("bridge symbol path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("create bridge symbol directory {}: {err}", parent.display()))?;

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            writeln!(file, "{symbol}")
                .map_err(|err| format!("write bridge symbol {}: {err}", path.display()))?;
            Ok(symbol)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let symbol = fs::read_to_string(&path).map_err(|err| {
                format!("read bridge symbol after race {}: {err}", path.display())
            })?;
            let symbol = symbol.trim().to_string();
            validate_symbol(&symbol)?;
            Ok(symbol)
        }
        Err(err) => Err(format!("create bridge symbol {}: {err}", path.display())),
    }
}

fn symbol_file_path() -> Result<PathBuf, String> {
    let out_dir =
        PathBuf::from(env::var("OUT_DIR").map_err(|err| format!("OUT_DIR is not set: {err}"))?);
    let target_root = match out_dir.ancestors().nth(4) {
        Some(path) => path,
        None => out_dir.as_path(),
    }
    .to_path_buf();
    Ok(target_root
        .join(".meerkat")
        .join("agent_factory_policy_build_symbol"))
}

fn fresh_symbol() -> String {
    let mut first = DefaultHasher::new();
    hash_env("OUT_DIR", &mut first);
    hash_env("CARGO_MANIFEST_DIR", &mut first);
    std::process::id().hash(&mut first);
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().hash(&mut first),
        Err(err) => err.duration().as_nanos().hash(&mut first),
    }

    let first = first.finish();
    let mut second = DefaultHasher::new();
    first.hash(&mut second);
    hash_env("CARGO_PKG_VERSION", &mut second);
    "meerkat-agent-factory-policy".hash(&mut second);

    format!(
        "__meerkat_agent_factory_policy_bridge_{first:016x}{:016x}",
        second.finish()
    )
}

fn hash_env<H: Hasher>(key: &str, state: &mut H) {
    if let Ok(value) = env::var(key) {
        value.hash(state);
    }
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
