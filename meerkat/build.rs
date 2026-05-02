fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Some(suffix) = agent_factory_policy_bridge_symbol_suffix() else {
        eprintln!("meerkat build could not locate the AgentFactory bridge symbol suffix");
        std::process::exit(1);
    };
    println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
}

fn agent_factory_policy_bridge_symbol_suffix() -> Option<String> {
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR")?);
    let build_dir = out_dir.parent()?.parent()?.to_path_buf();
    let target = std::env::var("TARGET").unwrap_or_default();
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let expected_core_manifest_dir = manifest_dir
        .parent()?
        .join("meerkat-core")
        .canonicalize()
        .ok();

    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(build_dir).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let marker = entry
            .path()
            .join("out")
            .join("agent_factory_policy_bridge_symbol_suffix");
        let Ok(contents) = std::fs::read_to_string(&marker) else {
            continue;
        };
        let Some(marker_suffix) = marker_value(&contents, "suffix") else {
            continue;
        };
        if marker_value(&contents, "target").is_some_and(|marker_target| marker_target != target) {
            continue;
        }
        if let Some(expected_core_manifest_dir) = expected_core_manifest_dir.as_ref() {
            let Some(marker_manifest_dir) = marker_value(&contents, "manifest_dir") else {
                continue;
            };
            let marker_manifest_dir = std::path::PathBuf::from(marker_manifest_dir);
            if marker_manifest_dir.canonicalize().ok().as_ref() != Some(expected_core_manifest_dir)
            {
                continue;
            }
        }
        let modified = std::fs::metadata(&marker)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best
            .as_ref()
            .is_none_or(|(best_modified, _)| modified > *best_modified)
        {
            best = Some((modified, marker_suffix.to_string()));
        }
    }
    best.map(|(_, suffix)| suffix)
}

fn marker_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}
