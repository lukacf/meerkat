use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path
}

fn repo_file(relative: &str) -> String {
    let mut path = repo_root();
    path.push(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

fn rust_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in
        fs::read_dir(dir).unwrap_or_else(|err| panic!("failed to read {}: {err}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let path = entry.path();
        if path.is_dir() {
            rust_files_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_test_source(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "tests")
        || path.file_name().is_some_and(|name| name == "tests.rs")
}

#[test]
fn core_agent_builder_does_not_expose_public_build_bypass() {
    let builder = repo_file("meerkat-core/src/agent/builder.rs");

    assert!(
        !builder.contains("pub async fn build<"),
        "meerkat_core::AgentBuilder must not expose a public unqualified \
         build(client, tools, store) seam; production-facing construction must \
         route through AgentFactory policy or an explicitly named low-level seam"
    );
    assert!(
        builder.contains("build_with_factory_policy"),
        "the canonical factory path should call a typed AgentBuilder factory-policy seam"
    );
}

#[test]
fn production_crates_do_not_adopt_standalone_builder_seam() {
    let root = repo_root();
    let allowed_factory = root.join("meerkat/src/factory.rs");
    let mut production_files = Vec::new();

    for entry in
        fs::read_dir(&root).unwrap_or_else(|err| panic!("failed to read {}: {err}", root.display()))
    {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read repo entry: {err}"));
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("meerkat-") || name == "meerkat-core" {
            continue;
        }
        let src = path.join("src");
        if src.is_dir() {
            rust_files_under(&src, &mut production_files);
        }
    }
    rust_files_under(&root.join("meerkat/src"), &mut production_files);

    for path in production_files {
        if is_test_source(&path) {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        assert!(
            !source.contains(".build_standalone("),
            "production source must not bypass AgentFactory via build_standalone: {}",
            path.display()
        );

        if path != allowed_factory {
            assert!(
                !source.contains("build_with_factory_policy(")
                    && !source.contains("new_unchecked_for_canonical_factory"),
                "only AgentFactory may enter the core factory-policy seam: {}",
                path.display()
            );
        }
    }
}

#[test]
fn production_like_callers_do_not_call_core_builder_build_directly() {
    let factory = repo_file("meerkat/src/factory.rs");
    let comms_agent = repo_file("meerkat-comms/src/agent/mod.rs");

    assert!(
        !factory.contains("builder.build(llm_adapter, tools, store_adapter)"),
        "AgentFactory must use the typed factory-policy seam when entering meerkat_core"
    );
    assert!(
        !comms_agent.contains("pub struct CommsAgentBuilder"),
        "meerkat-comms must not expose a public builder that can bypass AgentFactory policy"
    );
    assert!(
        !comms_agent.contains(".build(client, tools, store)"),
        "meerkat-comms must not construct core agents through the unqualified build seam"
    );
}
