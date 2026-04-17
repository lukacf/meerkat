use std::fs;
use std::path::Path;
use std::path::PathBuf;

use meerkat_contracts::{
    RpcMethodCatalogOptions, rest_documented_paths, rpc_method_catalog, rpc_method_names,
};

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir)
}

#[test]
fn public_realtime_catalog_uses_converged_route_names() {
    let methods = rpc_method_names(RpcMethodCatalogOptions::documented_surface());
    assert!(
        methods.contains(&"runtime/realtime_attachment_status".to_string()),
        "rpc catalog should expose runtime/realtime_attachment_status"
    );
    assert!(
        methods.contains(&"mob/realtime_attach".to_string()),
        "rpc catalog should expose mob/realtime_attach"
    );
    assert!(
        methods.contains(&"mob/realtime_detach".to_string()),
        "rpc catalog should expose mob/realtime_detach"
    );
    assert!(
        !methods.contains(&"runtime/live_attachment_status".to_string()),
        "rpc catalog should not expose the legacy runtime/live_attachment_status route"
    );
    assert!(
        !methods.contains(&"mob/live_attach".to_string()),
        "rpc catalog should not expose the legacy mob/live_attach route"
    );
    assert!(
        !methods.contains(&"mob/live_detach".to_string()),
        "rpc catalog should not expose the legacy mob/live_detach route"
    );

    let descriptors = rpc_method_catalog(RpcMethodCatalogOptions::documented_surface());
    let attach = descriptors
        .iter()
        .find(|descriptor| descriptor.name == "mob/realtime_attach");
    assert_eq!(
        attach.and_then(|descriptor| descriptor.params_type),
        Some("MobRealtimeAttachParams"),
        "mob/realtime_attach should expose converged param type names"
    );
    assert_eq!(
        attach.and_then(|descriptor| descriptor.result_type),
        Some("MobRealtimeAttachResult"),
        "mob/realtime_attach should expose converged result type names"
    );
    let detach = descriptors
        .iter()
        .find(|descriptor| descriptor.name == "mob/realtime_detach");
    assert_eq!(
        detach.and_then(|descriptor| descriptor.params_type),
        Some("MobRealtimeDetachParams"),
        "mob/realtime_detach should expose converged param type names"
    );
    assert_eq!(
        detach.and_then(|descriptor| descriptor.result_type),
        Some("MobRealtimeDetachResult"),
        "mob/realtime_detach should expose converged result type names"
    );

    let rest_paths = rest_documented_paths();
    assert!(
        rest_paths.contains(&"/runtime/{id}/realtime_attachment_status"),
        "rest catalog should expose /runtime/{{id}}/realtime_attachment_status"
    );
    assert!(
        rest_paths.contains(&"/mob/{id}/members/{agent_identity}/realtime/attach"),
        "rest catalog should expose /mob/{{id}}/members/{{agent_identity}}/realtime/attach"
    );
    assert!(
        rest_paths.contains(&"/mob/{id}/members/{agent_identity}/realtime/detach"),
        "rest catalog should expose /mob/{{id}}/members/{{agent_identity}}/realtime/detach"
    );
    assert!(
        !rest_paths.contains(&"/runtime/{id}/live_attachment_status"),
        "rest catalog should not expose /runtime/{{id}}/live_attachment_status"
    );
    assert!(
        !rest_paths.contains(&"/mob/{id}/members/{agent_identity}/live/attach"),
        "rest catalog should not expose /mob/{{id}}/members/{{agent_identity}}/live/attach"
    );
    assert!(
        !rest_paths.contains(&"/mob/{id}/members/{agent_identity}/live/detach"),
        "rest catalog should not expose /mob/{{id}}/members/{{agent_identity}}/live/detach"
    );
}

#[test]
fn public_docs_and_skills_do_not_expose_legacy_live_attachment_terms() {
    let root = workspace_root();
    let files = [
        "docs/api/rpc.mdx",
        "docs/architecture/identity-first-live-voice-proposal.md",
        ".claude/skills/meerkat-platform/SKILL.md",
        ".claude/skills/meerkat-architecture/SKILL.md",
    ];
    let banned_terms = [
        "runtime/live_attachment_status",
        "mob/live_attach",
        "mob/live_detach",
        "live_attachment_status",
    ];
    let required_terms = [
        "runtime/realtime_attachment_status",
        "mob/realtime_attach",
        "mob/realtime_detach",
        "realtime_attachment_status",
    ];

    for rel_path in files {
        let path = root.join(rel_path);
        let contents = fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !contents.is_empty(),
            "{rel_path} should be readable from {}",
            path.display()
        );
        for banned in banned_terms {
            assert!(
                !contents.contains(banned),
                "{rel_path} should not expose legacy term `{banned}`"
            );
        }
        for required in required_terms {
            assert!(
                contents.contains(required),
                "{rel_path} should expose converged realtime term `{required}`"
            );
        }
    }
}
