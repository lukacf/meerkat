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
    let core_manifest_dirs = core_manifest_dir_candidates(&manifest_dir, &build_dir);

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
        let emitted_suffix = std::fs::read_to_string(package_dir.join("output"))
            .ok()
            .and_then(|output| {
                output.lines().find_map(|line| {
                    line.strip_prefix(
                        "cargo:rustc-env=MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX=",
                    )
                    .map(str::to_owned)
                })
            });
        if emitted_suffix.is_none() {
            continue;
        }
        let suffix = core_manifest_dirs.iter().find_map(|core_manifest_dir| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            core_manifest_dir.to_string_lossy().hash(&mut hasher);
            core_out_dir.to_string_lossy().hash(&mut hasher);
            target.hash(&mut hasher);
            let suffix = format!("{:016x}", hasher.finish());
            (emitted_suffix.as_deref() == Some(suffix.as_str())).then_some(suffix)
        });
        let Some(suffix) = suffix else {
            continue;
        };
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

fn core_manifest_dir_candidates(
    manifest_dir: &std::path::Path,
    build_dir: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let Some(parent) = manifest_dir.parent() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    let direct = parent.join("meerkat-core");
    if let Ok(path) = direct.canonicalize() {
        candidates.push(path);
    }

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.starts_with("meerkat-core-") {
                continue;
            }
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }

    if let Some(profile_dir) = build_dir.parent() {
        let deps_dir = profile_dir.join("deps");
        if let Ok(entries) = std::fs::read_dir(deps_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !name.starts_with("meerkat_core-") || !name.ends_with(".d") {
                    continue;
                }
                let Ok(dep_info) = std::fs::read_to_string(path) else {
                    continue;
                };
                for source in dep_info.split_whitespace() {
                    let Some(core_dir) = source.strip_suffix("/src/lib.rs") else {
                        continue;
                    };
                    if !core_dir.ends_with("/meerkat-core") && !core_dir.contains("/meerkat-core-")
                    {
                        continue;
                    }
                    let Ok(path) = std::path::PathBuf::from(core_dir).canonicalize() else {
                        continue;
                    };
                    if !candidates.contains(&path) {
                        candidates.push(path);
                    }
                }
            }
        }
    }

    candidates
}
