fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Some(suffix) = agent_factory_policy_bridge_symbol_suffix() else {
        eprintln!("meerkat build could not locate the AgentFactory bridge symbol suffix");
        std::process::exit(1);
    };
    println!("cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX={suffix}");
}

fn agent_factory_policy_bridge_symbol_suffix() -> Option<String> {
    use std::hash::{Hash, Hasher};

    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR")?);
    let build_dir = out_dir.parent()?.parent()?.to_path_buf();
    let target = std::env::var("TARGET").unwrap_or_default();
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let core_manifest_dir = manifest_dir
        .parent()?
        .join("meerkat-core")
        .canonicalize()
        .ok();

    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(build_dir).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let package_dir = entry.path();
        let package_name = package_dir.file_name()?.to_string_lossy();
        if !package_name.starts_with("meerkat-core-") {
            continue;
        }
        let core_out_dir = package_dir.join("out");
        if !core_out_dir.is_dir() {
            continue;
        }
        let Some(core_manifest_dir) = core_manifest_dir.as_ref() else {
            continue;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        core_manifest_dir.to_string_lossy().hash(&mut hasher);
        core_out_dir.to_string_lossy().hash(&mut hasher);
        target.hash(&mut hasher);
        let suffix = format!("{:016x}", hasher.finish());
        let modified = std::fs::metadata(&core_out_dir)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best
            .as_ref()
            .is_none_or(|(best_modified, _)| modified > *best_modified)
        {
            best = Some((modified, suffix));
        }
    }
    best.map(|(_, suffix)| suffix)
}
