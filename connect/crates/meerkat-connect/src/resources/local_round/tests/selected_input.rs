//! Actual owner/copy/custody controls using the existing inert resource fixture.
//! No production helper is launched and this does not authenticate a root host.
use super::*;
use crate::protected_parent::selected_input::inert_test_input;
use crate::resources::PrivateLocalRoundAttempts;

pub(in crate::resources::local_round) fn preparation()
-> (impl Sized, LocalRoundOwner, meerkat_core::McpServerConfig) {
    let mut case = Case::new("normal");
    let mut owner = case.owner.take().unwrap();
    let prepared = owner.artifacts.join("prepared");
    let config = prepared.join("config");
    let staging = prepared.join("staging");
    for path in [&prepared, &config, &staging] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let profile = serde_json::to_vec(&json!({"schema_version":1,
        "scope":"standing_local_software_installation_only", "staging_root":staging,
        "validators":{"python":{"path":"/inert/validator","sha256":"a".repeat(64)}}}))
    .unwrap();
    retain(&config.join("selected-install.json"), &profile).unwrap();
    let backend = serde_json::to_vec(&json!({"mcpServers":{"toolkit-setup":{"command":"/inert/python","args":[
        "--selected-install-profile",config.join("selected-install.json"),"--selected-install-profile-sha256",digest(&profile)]}}})).unwrap();
    retain(&config.join("mcp.json"), &backend).unwrap();
    let descriptor = serde_json::to_vec(
        &json!({"mcpServers":{"toolkit-setup":{"command":"/inert/python","args":[
        "--configuration",config.join("mcp.json"),"--configuration-sha256",digest(&backend)]}}}),
    )
    .unwrap();
    let receipt = serde_json::to_vec(&json!({"mcp_sha256":digest(&backend)})).unwrap();
    retain(&prepared.join("product-mcp.json"), &descriptor).unwrap();
    retain(&prepared.join("product-prepared.json"), &receipt).unwrap();
    owner.request = json!({"prepared":prepared,"descriptor_sha256":digest(&descriptor),"preparation_sha256":digest(&receipt)});
    let server = crate::setup::PreparedSetupConnection::from_preparer_output(&descriptor)
        .unwrap()
        .into_server();
    (case, owner, server)
}

#[tokio::test]
async fn original_owner_stages_before_native_entry_and_fence_removes_projection() {
    let (_case, owner, server) = preparation();
    let staging = owner.artifacts.join("prepared/staging");
    let attempts =
        PrivateLocalRoundAttempts::for_native_construction(meerkat_core::SessionId::new());
    attempts.retain(server, owner).unwrap();
    let input = inert_test_input("repository");
    attempts
        .stage_selected(input.clone(), std::future::pending())
        .await
        .unwrap();
    let result = attempts.staged_input().unwrap();
    assert_eq!(
        fs::read(staging.join(&result.staged_id).join("artifact.whl")).unwrap(),
        b"wheel"
    );
    assert!(
        attempts
            .stage_selected(input, std::future::pending())
            .await
            .is_err()
    );
    attempts.fence();
    attempts.join_intake().await.unwrap();
    attempts.settled().unwrap();
    assert!(attempts.staged_input().is_none());
}

#[tokio::test]
async fn cancelled_intake_is_joined_and_sticky_without_projection() {
    let (_case, owner, server) = preparation();
    let attempts =
        PrivateLocalRoundAttempts::for_native_construction(meerkat_core::SessionId::new());
    attempts.retain(server, owner).unwrap();
    let input = inert_test_input("repository");
    assert!(
        attempts
            .stage_selected(input.clone(), async { Ok(()) })
            .await
            .is_err()
    );
    assert!(input.begin().is_err());
    assert!(attempts.staged_input().is_none());
    attempts.fence();
    attempts.join_intake().await.unwrap();
    assert!(attempts.settled().is_err());
}

#[test]
fn dropped_copy_wait_is_still_joined_by_original_custody() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_case, owner, server) = preparation();
        let attempts =
            PrivateLocalRoundAttempts::for_native_construction(meerkat_core::SessionId::new());
        attempts.retain(server, owner).unwrap();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.await.unwrap();
        let input = inert_test_input("repository");
        let mut copy = Box::pin(attempts.stage_selected(input.clone(), std::future::pending()));
        assert!(futures_util::poll!(copy.as_mut()).is_pending());
        drop(copy);
        attempts.fence();
        release_tx.send(()).unwrap();
        blocker.await.unwrap();
        assert!(attempts.join_intake().await.is_err());
        assert!(attempts.staged_input().is_none());
        assert!(input.begin().is_err());
        assert!(attempts.settled().is_err());
    });
}

#[test]
fn changed_pinned_profile_and_cancellation_refuse_without_new_projection() {
    let (_case, mut owner, server) = preparation();
    let input = inert_test_input("repository");
    fs::write(
        owner
            .artifacts
            .join("prepared/config/selected-install.json"),
        b"{}",
    )
    .unwrap();
    assert!(
        owner
            .stage_selected_input(&server, &input, &|| false)
            .is_err()
    );
    assert!(owner.poisoned);
    let (_case, mut owner, server) = preparation();
    assert!(
        owner
            .stage_selected_input(&server, &input, &|| true)
            .is_err()
    );
    assert!(owner.poisoned);
}

// These behavior controls use only APIs present at both 01128033 and 7821f4af.
// Overlay this test file alone for the RED run; no WrittenFile helper is used.
fn copied_target(staging: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut entries = fs::read_dir(staging).unwrap();
    let entry = entries.next()?.unwrap();
    assert!(entries.next().is_none(), "one original staging attempt");
    assert!(entry.file_type().unwrap().is_dir());
    Some(entry.path())
}

fn replace_same_bytes(path: &std::path::Path) {
    use std::os::unix::fs::MetadataExt;
    let bytes = fs::read(path).unwrap();
    let before = fs::metadata(path).unwrap();
    fs::rename(path, path.with_extension("original-held")).unwrap();
    retain(path, &bytes).unwrap();
    let after = fs::metadata(path).unwrap();
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(after.mode() & 0o777, 0o600);
    assert_eq!(after.nlink(), 1);
    assert_eq!(after.uid(), before.uid());
    assert_ne!((before.dev(), before.ino()), (after.dev(), after.ino()));
}

fn replacement_through_original_owner(later_member: bool) {
    let (_case, mut owner, server) = preparation();
    let staging = owner.artifacts.join("prepared/staging");
    let replaced = std::cell::Cell::new(false);
    let input = inert_test_input("repository");
    let result = owner.stage_selected_input(&server, &input, &|| {
        if !replaced.get()
            && let Some(target) = copied_target(&staging)
        {
            let artifact = target.join("artifact.whl");
            let ready = if later_member {
                target.join("package.json").exists()
            } else {
                fs::metadata(&artifact).is_ok_and(|m| m.len() == 5)
            };
            if ready {
                replace_same_bytes(&artifact);
                replaced.set(true);
            }
        }
        false
    });
    assert!(
        replaced.get(),
        "must reach the copy/readback injection boundary"
    );
    assert!(result.is_err(), "same-byte replacement must refuse staging");
    assert!(owner.poisoned, "refusal must poison the original owner");
    let attempts =
        PrivateLocalRoundAttempts::for_native_construction(meerkat_core::SessionId::new());
    assert!(
        attempts.retain(server, owner).is_err(),
        "poisoned original cannot be adopted"
    );
    assert!(attempts.staged_input().is_none());
    attempts.fence();
    assert!(attempts.settled().is_err());
}

#[test]
fn stage_selected_refuses_earlier_member_replacement_during_later_copy() {
    replacement_through_original_owner(true);
}

#[test]
fn stage_selected_refuses_member_replacement_during_own_readback() {
    replacement_through_original_owner(false);
}

#[test]
fn stage_selected_refuses_pinned_profile_changed_during_copy() {
    let (_case, mut owner, server) = preparation();
    let staging = owner.artifacts.join("prepared/staging");
    let profile = owner
        .artifacts
        .join("prepared/config/selected-install.json");
    let changed = std::cell::Cell::new(false);
    let result = owner.stage_selected_input(&server, &inert_test_input("repository"), &|| {
        if !changed.get()
            && let Some(target) = copied_target(&staging)
            && target.join("artifact.whl").exists()
        {
            fs::write(&profile, b"{}").unwrap();
            changed.set(true);
        }
        false
    });
    assert!(changed.get(), "pin must change during the actual copy");
    assert!(result.is_err());
    assert!(owner.poisoned);
    let attempts =
        PrivateLocalRoundAttempts::for_native_construction(meerkat_core::SessionId::new());
    assert!(attempts.retain(server, owner).is_err());
    assert!(attempts.staged_input().is_none());
    attempts.fence();
    assert!(attempts.settled().is_err());
}
