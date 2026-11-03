//! Structural guard for the retired markdown agent-memory backend.
//!
//! Markdown remains a wire-recognized migration input and a crate-private
//! parser for the SQLite one-shot import. It must never become a public Rust
//! provider or builder path again.

#[test]
fn rust_surface_has_no_live_markdown_provider_or_builder() {
    let crate_root = include_str!("../src/lib.rs");
    let identity_exports = include_str!("../src/identity_first/mod.rs");
    let builder = include_str!("../src/unified_runtime/builder.rs");
    let agent_memory = include_str!("../src/identity_first/agent_memory.rs");

    for (surface, source) in [
        ("crate root", crate_root),
        ("identity exports", identity_exports),
    ] {
        assert!(
            !source.contains("MarkdownAgentMemoryStore"),
            "{surface} must not export the retired markdown provider"
        );
    }
    assert!(
        !builder.contains("pub fn persistent_agent_memory("),
        "the Rust builder must not reactivate live markdown memory"
    );
    assert!(
        !builder.contains("agent_memory_from_persistent_state"),
        "the removed builder carrier must not return under another branch"
    );
    for marker in [
        "pub struct MarkdownAgentMemoryStore",
        "impl AgentMemoryProvider for MarkdownAgentMemoryStore",
        "append_markdown_record",
        "forget_markdown_record",
    ] {
        assert!(
            !agent_memory.contains(marker),
            "legacy live-store marker must stay absent: {marker}"
        );
    }
}

#[test]
fn markdown_parser_exists_only_for_sqlite_one_shot_import() {
    let agent_memory = include_str!("../src/identity_first/agent_memory.rs");
    let sqlite = include_str!("../src/memory/sqlite_store.rs");
    // The stdin-RPC gateway lives in the library; its binary only delegates.
    let gateway = include_str!("../src/rpc_gateway.rs");

    assert!(agent_memory.contains("pub(crate) fn read_markdown_records("));
    assert!(agent_memory.contains("pub(crate) fn markdown_import_realm_dir("));
    assert!(sqlite.contains("read_markdown_records(path)"));
    assert!(sqlite.contains("fs::rename(path, PathBuf::from(imported_name))"));
    assert!(gateway.contains("AgentMemoryStoreMigration::MarkdownIsImportOnly"));
    assert!(gateway.contains("GatewayAgentMemoryStoreKind::Markdown =>"));
    assert!(!gateway.contains("\"MarkdownAgentMemoryStore\""));
}
