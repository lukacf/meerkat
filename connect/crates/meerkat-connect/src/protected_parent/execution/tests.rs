//! Kernel-status parser controls. They do not model an isolated launch or run
//! process-mutating prctl operations inside the test harness.
use super::*;

fn selection() -> ExecutionMapping {
    ExecutionMapping {
        uid: 12001,
        gid: 12002,
        supplementary_gids: vec![12003],
    }
}
fn status() -> String {
    "Name:\tforge-service-host\nUid:\t12001\t12001\t12001\t12001\nGid:\t12002\t12002\t12002\t12002\nCapInh:\t0000000000000000\nCapPrm:\t0000000000000000\nCapEff:\t0000000000000000\nCapAmb:\t0000000000000000\nNoNewPrivs:\t1\nThreads:\t1\n".into()
}

#[test]
fn complete_unprivileged_status_and_foreign_filesystem_ids() {
    assert!(verify_status(&status(), &selection()).is_ok());
    for (from, to) in [
        ("12001\t12001\t12001\t12001", "12001\t12001\t12001\t0"),
        ("12002\t12002\t12002\t12002", "12002\t12002\t12002\t0"),
        ("12001\t12001\t12001\t12001", "12001\t0\t12001\t12001"),
        ("12002\t12002\t12002\t12002", "12002\t12002\t0\t12002"),
    ] {
        assert!(verify_status(&status().replace(from, to), &selection()).is_err());
    }
}

#[test]
fn residual_capabilities_are_refused_even_with_no_new_privileges() {
    for field in ["CapInh", "CapPrm", "CapEff", "CapAmb"] {
        for value in [
            "0000000000000001",
            "0000000000200000",
            "ffffffffffffffff",
            "0",
            "not-hexadecimal!",
        ] {
            let raw = status().replace(
                &format!("{field}:\t0000000000000000"),
                &format!("{field}:\t{value}"),
            );
            assert!(verify_status(&raw, &selection()).is_err(), "{field}");
        }
    }
}

#[test]
fn status_requires_all_fields_once_and_single_threaded_adoption() {
    for field in [
        "Uid",
        "Gid",
        "CapInh",
        "CapPrm",
        "CapEff",
        "CapAmb",
        "NoNewPrivs",
        "Threads",
    ] {
        let raw = status();
        let line = raw
            .lines()
            .find(|line| line.starts_with(&format!("{field}:")))
            .unwrap();
        assert!(verify_status(&raw.replace(&format!("{line}\n"), ""), &selection()).is_err());
        assert!(verify_status(&format!("{raw}{line}\n"), &selection()).is_err());
    }
    for (from, to) in [
        ("NoNewPrivs:\t1", "NoNewPrivs:\t0"),
        ("Threads:\t1", "Threads:\t2"),
    ] {
        assert!(verify_status(&status().replace(from, to), &selection()).is_err());
    }
    assert!(
        verify_status(
            &format!("{}Extra:{}\n", status(), "x".repeat(65536)),
            &selection()
        )
        .is_err()
    );
}

#[test]
fn filesystem_admission_rejects_replaced_and_linked_private_roots() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("private");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let uid = nix::unistd::getuid().as_raw();
    assert!(private_component(&fs::symlink_metadata(&root).unwrap(), uid, true).is_ok());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(private_component(&fs::symlink_metadata(&root).unwrap(), uid, true).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        private_component(
            &fs::symlink_metadata(&root).unwrap(),
            uid.wrapping_add(1),
            true
        )
        .is_err()
    );
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    assert!(ancestors(&alias).is_err());
    assert!(private_component(&fs::symlink_metadata(&alias).unwrap(), uid, true).is_err());
    fs::remove_dir(&root).unwrap();
    fs::write(&root, b"replacement file").unwrap();
    assert!(private_component(&fs::symlink_metadata(&root).unwrap(), uid, true).is_err());
    assert!(ancestors(&temp.path().join("missing")).is_err());
}

#[test]
fn child_owned_readable_resource_is_refused_despite_safe_mode_bits() {
    use std::os::unix::fs::PermissionsExt;
    assert!(
        !nix::unistd::getuid().is_root(),
        "lifecycle controls run as an unprivileged user"
    );
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("helper.py");
    fs::write(&file, b"fixed installed bytes").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let info = fs::symlink_metadata(&file).unwrap();
    assert_eq!(info.mode() & 0o022, 0);
    assert!(root_resource_metadata(&info).is_err());
    // A real root-owned directory supplies the positive metadata control.
    let root = fs::symlink_metadata("/").unwrap();
    assert_eq!(root.uid(), 0);
    assert!(root_resource_metadata(&root).is_ok());
}

#[test]
fn native_default_resolution_matches_explicit_home_xdg_and_root() {
    let temp = tempfile::tempdir().unwrap();
    for xdg in [false, true] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "protected_parent::execution::tests::native_default_resolution_child",
                "--nocapture",
            ])
            .env("CONNECT_PROTECTED_STORE_CONTROL", "1")
            .env("HOME", temp.path());
        if xdg {
            command.env("XDG_CONFIG_HOME", temp.path().join("selected-config"));
        } else {
            command.env_remove("XDG_CONFIG_HOME");
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "isolated native default resolver control failed"
        );
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("CONNECT_PROTECTED_STORE_CONTROL_PASSED"),
            "the exact child control must actually execute"
        );
    }
}

#[test]
fn native_default_resolution_child() {
    if std::env::var("CONNECT_PROTECTED_STORE_CONTROL").as_deref() != Ok("1") {
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let root = xdg
        .clone()
        .unwrap_or_else(|| home.join(".config"))
        .join("meerkat/credentials");
    let exact = || ProviderStore::NativeDefault {
        home: home.clone(),
        xdg_config_home: xdg.clone(),
        root: root.clone(),
    };
    assert_eq!(selected_provider_root(&exact()).unwrap(), root);
    assert!(
        !root.exists(),
        "native resolver must not create the credential directory"
    );
    assert!(
        selected_provider_root(&ProviderStore::NativeDefault {
            home: home.join("foreign"),
            xdg_config_home: xdg.clone(),
            root: root.clone(),
        })
        .is_err()
    );
    assert!(
        selected_provider_root(&ProviderStore::NativeDefault {
            home: home.clone(),
            xdg_config_home: if xdg.is_some() {
                None
            } else {
                Some(home.join(".config"))
            },
            root: root.clone(),
        })
        .is_err(),
        "explicit absence must differ from an equivalent-looking path"
    );
    assert!(
        selected_provider_root(&ProviderStore::NativeDefault {
            home,
            xdg_config_home: xdg,
            root: root.join("foreign"),
        })
        .is_err()
    );
    let explicit = root.join("explicit");
    assert_eq!(
        selected_provider_root(&ProviderStore::File {
            root: explicit.clone()
        })
        .unwrap(),
        explicit
    );
    println!("CONNECT_PROTECTED_STORE_CONTROL_PASSED");
}
