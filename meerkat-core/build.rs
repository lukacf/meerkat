fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE___MEERKAT_FACADE_AGENT_FACTORY_BUILD").is_some() {
        println!("cargo:rustc-cfg=meerkat_internal_agent_factory_build");
    }
}
