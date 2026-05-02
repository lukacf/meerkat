fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Ok(suffix) = std::env::var("DEP_MEERKAT_CORE_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
    else {
        eprintln!("meerkat-core must publish the AgentFactory bridge symbol suffix");
        std::process::exit(1);
    };
    println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
}
