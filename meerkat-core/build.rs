fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE___MEERKAT_FACADE_AGENT_FACTORY_BUILD").is_some() {
        let suffix = agent_factory_policy_bridge_symbol_suffix();
        println!("cargo:rustc-cfg=meerkat_internal_agent_factory_build");
        println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
        write_agent_factory_policy_bridge_symbol_suffix(&suffix);
    }
}

fn agent_factory_policy_bridge_symbol_suffix() -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_default()
        .hash(&mut hasher);
    std::env::var("OUT_DIR")
        .unwrap_or_default()
        .hash(&mut hasher);
    std::env::var("TARGET")
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn write_agent_factory_policy_bridge_symbol_suffix(suffix: &str) {
    let Some(out_dir) = std::env::var_os("OUT_DIR").map(std::path::PathBuf::from) else {
        eprintln!("OUT_DIR must be set for meerkat-core build script");
        std::process::exit(1);
    };
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_default();
    let marker = out_dir.join("agent_factory_policy_bridge_symbol_suffix");
    let contents = format!("suffix={suffix}\nmanifest_dir={manifest_dir}\ntarget={target}\n");
    if let Err(error) = std::fs::write(&marker, contents) {
        eprintln!(
            "failed to write AgentFactory bridge symbol suffix marker {}: {error}",
            marker.display()
        );
        std::process::exit(1);
    }
}
