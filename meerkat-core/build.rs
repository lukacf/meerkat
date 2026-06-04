#![allow(clippy::redundant_feature_names)]

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE___MEERKAT_FACADE_AGENT_FACTORY_BUILD").is_some() {
        let suffix = agent_factory_policy_bridge_symbol_suffix();
        println!("cargo:rustc-cfg=meerkat_internal_agent_factory_build");
        println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
    }
    if std::env::var_os("CARGO_FEATURE___MEERKAT_GENERATED_AUTHORITY_BRIDGE").is_some() {
        let suffix = generated_authority_bridge_symbol_suffix();
        println!("cargo:rustc-cfg=meerkat_internal_generated_authority_bridge");
        println!("cargo:rustc-env=MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX={suffix}");
    }
}

fn agent_factory_policy_bridge_symbol_suffix() -> String {
    stable_bridge_symbol_suffix("meerkat-agent-factory-policy-bridge-v3")
}

fn generated_authority_bridge_symbol_suffix() -> String {
    stable_bridge_symbol_suffix("meerkat-generated-authority-bridge-v1")
}

fn stable_bridge_symbol_suffix(domain: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    domain.hash(&mut hasher);
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .ok()
        .and_then(|path| path.canonicalize().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    manifest_dir.hash(&mut hasher);
    std::env::var("TARGET")
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
