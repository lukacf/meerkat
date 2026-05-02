fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let suffix = std::env::var("DEP_MEERKAT_CORE_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
        .expect("meerkat-core must publish the AgentFactory bridge symbol suffix");
    println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
}
