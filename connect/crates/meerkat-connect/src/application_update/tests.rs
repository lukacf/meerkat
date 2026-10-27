//! Component controls for the owner actually called by the native hook. The
//! identity projection is synthetic; these are not native prepare/finish runs.
use super::*;
use std::os::unix::fs::PermissionsExt;
use serde_json::json;
fn tool(name: &str) -> ToolDef { ToolDef::new(name, "actual fixture definition", json!({"type":"object"})) }
fn member(name: &str) -> meerkat_core::MobMemberBinding {
    meerkat_core::MobMemberBinding { mob_id: "application".into(), member: name.into(), role: "worker".into() }
}
fn identity(name: &str) -> serde_json::Value { json!({"realm":"realm", "member":member(name), "session_document":"session"}) }
fn owner(path: &Path) -> UpdateAssociations {
    UpdateAssociations {
        directory: AssociationDirectory::open(path).unwrap(),
        selection: SelectedHostArtifact { path: "/selected/update.json".into(), sha256: "a".repeat(64) },
        prior: SelectedHostArtifact { path: "/selected/prior.json".into(), sha256: "b".repeat(64) },
        update_id: "existing-update".into(), helper: member("helper"), observed: Mutex::new(BTreeMap::new()),
    }
}
fn surface(tools: &[ToolDef]) -> PreparedToolSurface {
    PreparedToolSurface { tools: tools.iter().map(ToolDef::identity).collect(),
        projected: tools.iter().map(ToolDef::identity).collect(), catalog: PreparedToolCatalog::VisibleOnly }
}
#[test]
fn repeated_preparations_are_independent_and_visible_rows_never_union_or_replay_disk() {
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().canonicalize().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let selected = owner(&path); let id = identity("worker"); let a = tool("a"); let b = tool("b");
    assert!(selected.visible_observation(id.clone(), &[a.clone()]).is_err());
    selected.prepared_observation(id.clone(), &member("worker"), &surface(&[a.clone()])).unwrap();
    selected.prepared_observation(id.clone(), &member("worker"), &surface(&[a.clone()])).unwrap();
    selected.prepared_observation(id.clone(), &member("worker"), &surface(&[b.clone()])).unwrap();
    selected.visible_observation(id.clone(), &[a.clone()]).unwrap();
    selected.visible_observation(id.clone(), &[b.clone()]).unwrap();
    assert!(selected.visible_observation(id.clone(), &[a.clone(), b.clone()]).is_err());
    assert!(selected.visible_observation(identity("another-member"), &[a.clone()]).is_err());
    let mut other_session = id.clone(); other_session["session_document"] = json!("another-session");
    assert!(selected.visible_observation(other_session, &[a.clone()]).is_err());
    let restarted = owner(&path);
    assert!(restarted.visible_observation(id.clone(), &[a.clone()]).is_err(), "disk observations cannot certify a new process's construction");
    restarted.prepared_observation(id.clone(), &member("worker"), &surface(&[a.clone()])).unwrap();
    restarted.visible_observation(id, &[a]).unwrap();
    assert_eq!(selected.observed.lock().unwrap().values().next().unwrap().len(), 2);
    for file in std::fs::read_dir(&path).unwrap() {
        let path = file.unwrap().path(); let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["update_id"], "existing-update"); assert_eq!(value["selection"]["sha256"], "a".repeat(64));
        assert!(value.get("finished").is_none() && value.get("adopted").is_none());
    }
}
#[test]
fn helper_requires_exact_report_and_full_identity_conflicts_remain_refusals() {
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().canonicalize().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let selected = owner(&path); let report = toolkit_mobkit::installed_host::acquisition::setup_report_definition();
    assert!(selected.prepared_observation(identity("helper"), &member("helper"), &surface(&[tool(&report.name)])).is_err());
    selected.prepared_observation(identity("helper"), &member("helper"), &surface(&[report.clone()])).unwrap();
    selected.visible_observation(identity("helper"), &[report]).unwrap();
    let mut malformed = surface(&[tool("a")]); malformed.projected.push(tool("foreign").identity());
    assert!(selected.prepared_observation(identity("worker"), &member("worker"), &malformed).is_err());
    malformed = surface(&[tool("a"), tool("a")]);
    assert!(selected.prepared_observation(identity("worker"), &member("worker"), &malformed).is_err());
}
#[test]
fn uncommitted_partial_files_do_not_poison_content_names_and_directory_replacement_refuses() {
    let dir = tempfile::tempdir().unwrap(); let root = dir.path().canonicalize().unwrap();
    let path = root.join("selected"); std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let selected = owner(&path);
    let partial = path.join("uncommitted-observation-previous-crash"); std::fs::write(&partial, b"partial").unwrap();
    selected.prepared_observation(identity("worker"), &member("worker"), &surface(&[tool("a")])).unwrap();
    assert_eq!(std::fs::read(&partial).unwrap(), b"partial");
    let prior = root.join("retained"); std::fs::rename(&path, &prior).unwrap(); std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(selected.prepared_observation(identity("worker"), &member("worker"), &surface(&[tool("b")])).is_err());
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0, "a replaced pathname receives no observations");
    assert_eq!(std::fs::read(prior.join("uncommitted-observation-previous-crash")).unwrap(), b"partial");
}
#[test]
fn concurrent_identical_preparations_share_facts_without_mixed_visibility() {
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().canonicalize().unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let selected = Arc::new(owner(&path));
    std::thread::scope(|scope| {
        for name in ["a", "b", "a", "b"] {
            let selected = selected.clone();
            scope.spawn(move || selected.prepared_observation(identity("worker"), &member("worker"), &surface(&[tool(name)])).unwrap());
        }
    });
    selected.visible_observation(identity("worker"), &[tool("a")]).unwrap();
    assert!(selected.visible_observation(identity("worker"), &[tool("a"), tool("b")]).is_err());
    assert_eq!(selected.observed.lock().unwrap().values().next().unwrap().len(), 2);
}
