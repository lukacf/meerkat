fn main() {
    for index in 0..4 {
        let dep_key = format!("DEP_MEERKAT_AGENT_BUILD_AUTHORITY_WORD_{index}");
        let word = match std::env::var(&dep_key) {
            Ok(word) => word,
            Err(_) => {
                eprintln!("missing authority build metadata {dep_key}");
                std::process::exit(1);
            }
        };
        println!("cargo:rustc-env=MEERKAT_AGENT_BUILD_AUTHORITY_WORD_{index}={word}");
        println!("cargo:rerun-if-env-changed={dep_key}");
    }
}
