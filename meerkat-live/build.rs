#![allow(clippy::redundant_feature_names)]

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(meerkat_internal_generated_authority_bridge)");
    if std::env::var_os("CARGO_FEATURE___MEERKAT_GENERATED_AUTHORITY_BRIDGE").is_some() {
        let Some(suffix) = generated_authority_bridge_symbol_suffix() else {
            eprintln!("meerkat-live build could not locate the generated authority bridge suffix");
            std::process::exit(1);
        };
        println!("cargo:rustc-cfg=meerkat_internal_generated_authority_bridge");
        println!("cargo:rustc-env=MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX={suffix}");
    }
}

fn generated_authority_bridge_symbol_suffix() -> Option<String> {
    use std::hash::{Hash, Hasher};

    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR")?);
    let build_dir = out_dir.parent()?.parent()?.to_path_buf();
    let target = std::env::var("TARGET").unwrap_or_default();
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let core_manifest_dirs = core_manifest_dir_candidates(&manifest_dir, &build_dir);

    core_manifest_dirs.first().map(|core_manifest_dir| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "meerkat-generated-authority-bridge-v1".hash(&mut hasher);
        core_manifest_dir.to_string_lossy().hash(&mut hasher);
        target.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    })
}

fn core_manifest_dir_candidates(
    manifest_dir: &std::path::Path,
    build_dir: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let Some(parent) = manifest_dir.parent() else {
        return Vec::new();
    };
    let package_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    let mut candidates = Vec::new();
    let direct = parent.join("meerkat-core");
    if let Ok(path) = direct.canonicalize() {
        candidates.push(path);
    }

    if !package_version.is_empty() {
        let exact = parent.join(format!("meerkat-core-{package_version}"));
        if let Ok(path) = exact.canonicalize()
            && !candidates.contains(&path)
        {
            candidates.push(path);
        }
    }

    if let Ok(entries) = std::fs::read_dir(parent) {
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
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
