fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE___MEERKAT_FACADE_AGENT_FACTORY_BUILD").is_some() {
        let suffix = agent_factory_policy_bridge_symbol_suffix();
        println!("cargo:rustc-cfg=meerkat_internal_agent_factory_build");
        println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
        println!("cargo:agent_factory_policy_bridge_symbol_suffix={suffix}");
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
