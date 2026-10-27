//! Explicit opt-in against a disposable Linux Secret Service only.
//! The fixture owns collection setup and backend lifetime. This test never
//! contacts PairedHost or supplies a real cookie, account, or desktop bus.
#![cfg(target_os = "linux")]

use dbus_secret_service::{EncryptionType, SecretService};
use meerkat_connect::paired_host_device::{DeviceStore, DeviceStoreError, OsDeviceStore};
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};

const ROOT: &str = "/fixture";
const BUS: &str = "unix:path=/fixture/run/bus";
const OPT_IN: &str = "disposable-linux-secret-service-v1";
const SERVICE: &str = "ai.rkat.meerkat-connect.paired-host-device.v1";
const A: &str = "https://paired-host-a.invalid";
const B: &str = "https://paired-host-b.invalid";
const A1: &[u8] = br#"{"synthetic_record":"A1"}"#;
const A2: &[u8] = br#"{"synthetic_record":"A2"}"#;
const B1: &[u8] = br#"{"synthetic_record":"B1"}"#;

// Keep backend diagnostics and record bytes out of libtest panic output.
trait FixtureResult<T> {
    fn fixture(self, message: &'static str) -> T;
}
impl<T, E> FixtureResult<T> for Result<T, E> {
    fn fixture(self, message: &'static str) -> T {
        self.unwrap_or_else(|_| panic!("{message}"))
    }
}

fn fixture_guard() {
    for (name, expected) in [
        ("MEERKAT_CONNECT_SECRET_SERVICE_FIXTURE", OPT_IN),
        ("DBUS_SESSION_BUS_ADDRESS", BUS),
        ("HOME", "/fixture/home"),
        ("XDG_RUNTIME_DIR", "/fixture/run"),
        ("XDG_DATA_HOME", "/fixture/home/.local/share"),
        ("XDG_CONFIG_HOME", "/fixture/home/.config"),
    ] {
        assert!(
            env::var(name).as_deref() == Ok(expected),
            "fixture environment guard refused"
        );
    }
    assert!(
        Path::new("/.dockerenv").is_file(),
        "fixture container required"
    );
    let status = fs::read_to_string("/proc/self/status").fixture("fixture process status");
    let ids: Vec<_> = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .expect("fixture uid")
        .split_whitespace()
        .collect();
    assert!(
        ids == ["10001", "10001", "10001", "10001"],
        "fixture uid required"
    );
    for path in [ROOT, "/fixture/home", "/fixture/run"] {
        let metadata = fs::symlink_metadata(path).fixture("fixture directory");
        assert!(metadata.is_dir() && metadata.uid() == 10001);
        assert_eq!(
            metadata.mode() & 0o077,
            0,
            "private fixture directory required"
        );
        assert_eq!(
            fs::canonicalize(path).fixture("fixture real path"),
            Path::new(path)
        );
    }
    let marker = fs::symlink_metadata("/fixture/OPT_IN").fixture("fixture marker");
    assert!(marker.is_file() && marker.uid() == 10001 && marker.mode() & 0o077 == 0);
    assert_eq!(
        fs::read_to_string("/fixture/OPT_IN").fixture("fixture marker content"),
        OPT_IN
    );
    let socket = fs::symlink_metadata("/fixture/run/bus").fixture("private bus socket");
    assert!(socket.file_type().is_socket() && socket.uid() == 10001);
}

fn fixture_service() -> SecretService {
    SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0)
        .unwrap_or_else(|_| panic!("fixture Secret Service connection failed"))
}

fn attributes(origin: &str) -> HashMap<&str, &str> {
    HashMap::from([("service", SERVICE), ("origin", origin)])
}

fn read_failure_diagnostic(origin: &str, error: DeviceStoreError) {
    let origin_label = match origin {
        A => "A",
        B => "B",
        _ => "other",
    };
    let error_label = match error {
        DeviceStoreError::Locked => "Locked",
        DeviceStoreError::Unavailable => "Unavailable",
        DeviceStoreError::Ambiguous => "Ambiguous",
        DeviceStoreError::InvalidRecord => "InvalidRecord",
        DeviceStoreError::Unconfirmed => "Unconfirmed",
    };
    // Emit the closed adapter outcome before any diagnostic call can block.
    eprintln!(
        "FIXTURE_READ_FAILURE {}",
        json!({"origin": origin_label, "error": error_label})
    );
    let mut state = json!({
        "connected": false,
        "default_present": null,
        "collection_locked": null,
        "matching_item_count": null,
        "attributes_match": null,
    });
    // One observation, bounded by the existing 25-second fixture process owner.
    // Null means unknown, not absent. Never read a secret or repeat the adapter.
    'observe: {
        let Ok(service) =
            SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0)
        else {
            break 'observe;
        };
        state["connected"] = json!(true);
        let collection = match service.get_default_collection() {
            Ok(collection) => {
                state["default_present"] = json!(true);
                collection
            }
            Err(dbus_secret_service::Error::NoResult) => {
                state["default_present"] = json!(false);
                break 'observe;
            }
            Err(_) => break 'observe,
        };
        state["collection_locked"] = json!(collection.is_locked().ok());
        let Ok(items) = collection.search_items(attributes(origin)) else {
            break 'observe;
        };
        state["matching_item_count"] = json!(items.len());
        if let [item] = items.as_slice() {
            state["attributes_match"] = json!(item.get_attributes().ok().map(|found| {
                found.len() == 2
                    && found.get("service").map(String::as_str) == Some(SERVICE)
                    && found.get("origin").map(String::as_str) == Some(origin)
            }));
        }
    }
    eprintln!("FIXTURE_COLLECTION_STATE {state}");
}

fn equal(origin: &str, expected: Option<&[u8]>) {
    let actual = OsDeviceStore.read(origin).unwrap_or_else(|error| {
        read_failure_diagnostic(origin, error);
        panic!("typed store read failed")
    });
    assert!(
        actual.as_deref() == expected,
        "record equality or absence check failed"
    );
}

fn refused(origin: &str, expected: DeviceStoreError) {
    assert!(matches!(OsDeviceStore.read(origin), Err(error) if error == expected));
    assert!(matches!(OsDeviceStore.write(origin, A2), Err(error) if error == expected));
    assert!(matches!(OsDeviceStore.delete(origin), Err(error) if error == expected));
}

#[test]
#[ignore = "requires the isolated GNOME Keyring fixture; never run against a desktop bus"]
fn isolated_secret_service() {
    // This guard runs before even connecting to D-Bus, including fixture calls.
    fixture_guard();
    let phase = env::var("MEERKAT_CONNECT_SECRET_SERVICE_PHASE").fixture("explicit fixture phase");
    let checks: &[&str] = match phase.as_str() {
        "empty" => {
            let service = fixture_service();
            let collection = service
                .get_default_collection()
                .fixture("fixture default collection");
            assert!(!collection.is_locked().fixture("fixture lock state"));
            assert!(
                collection
                    .get_all_items()
                    .fixture("fixture items")
                    .is_empty()
            );
            &["existing_default_unlocked", "initially_empty"]
        }
        "write" => {
            OsDeviceStore
                .write(A, A1)
                .fixture("typed store write A failed");
            OsDeviceStore
                .write(B, B1)
                .fixture("typed store write B failed");
            &["write_a", "write_b"]
        }
        "read" => {
            equal(A, Some(A1));
            equal(B, Some(B1));
            equal("http://paired-host-a.invalid", None);
            equal("https://paired-host-a.invalid:8443", None);
            &[
                "a_equal",
                "b_equal",
                "different_scheme_absent",
                "different_port_absent",
            ]
        }
        "update" => {
            OsDeviceStore
                .write(A, A2)
                .fixture("typed store update failed");
            equal(B, Some(B1));
            &["update_a", "b_unchanged"]
        }
        "read_updated" | "read_after_restart" => {
            equal(A, Some(A2));
            equal(B, Some(B1));
            &["a_updated_equal", "b_unchanged"]
        }
        "delete" => {
            OsDeviceStore.delete(A).fixture("typed store delete failed");
            equal(B, Some(B1));
            &["delete_a", "b_unchanged"]
        }
        "read_deleted" => {
            equal(A, None);
            equal(B, Some(B1));
            &["a_absent", "b_unchanged"]
        }
        "ambiguous" => {
            let service = fixture_service();
            let collection = service
                .get_default_collection()
                .fixture("fixture default collection");
            for value in [A1, A2] {
                collection
                    .create_item(
                        "Synthetic fixture duplicate",
                        attributes(A),
                        value,
                        false,
                        "application/json",
                    )
                    .fixture("fixture duplicate creation");
            }
            assert_eq!(
                collection
                    .search_items(attributes(A))
                    .fixture("fixture duplicate search")
                    .len(),
                2
            );
            refused(A, DeviceStoreError::Ambiguous);
            let items = collection
                .search_items(attributes(A))
                .fixture("fixture duplicate readback");
            assert_eq!(items.len(), 2);
            let mut values: Vec<_> = items
                .iter()
                .map(|item| item.get_secret().fixture("fixture value readback"))
                .collect();
            values.sort();
            let mut expected = vec![A1.to_vec(), A2.to_vec()];
            expected.sort();
            assert!(values == expected, "ambiguous items changed");
            for item in items {
                item.delete().fixture("fixture duplicate cleanup");
            }
            equal(B, Some(B1));
            &[
                "duplicate_fixture_present",
                "read_write_delete_ambiguous",
                "duplicates_unchanged",
                "b_unchanged",
            ]
        }
        "extra_attribute" => {
            let service = fixture_service();
            let collection = service
                .get_default_collection()
                .fixture("fixture default collection");
            let mut attrs = attributes(A);
            attrs.insert("fixture_extra", "present");
            let item = collection
                .create_item(
                    "Synthetic wider fixture",
                    attrs,
                    A1,
                    false,
                    "application/json",
                )
                .fixture("fixture wider item creation");
            refused(A, DeviceStoreError::Ambiguous);
            assert!(
                item.get_secret().fixture("fixture wider item readback") == A1,
                "wider item changed"
            );
            item.delete().fixture("fixture wider item cleanup");
            equal(B, Some(B1));
            &[
                "extra_attribute_refused",
                "wider_item_unchanged",
                "b_unchanged",
            ]
        }
        "locked" => {
            let service = fixture_service();
            let collection = service
                .get_default_collection()
                .fixture("fixture default collection");
            collection.lock().fixture("fixture lock");
            assert!(
                collection
                    .is_locked()
                    .fixture("fixture locked precondition")
            );
            refused(B, DeviceStoreError::Locked);
            assert!(collection.is_locked().fixture("fixture lock readback"));
            &["read_write_delete_locked", "collection_remains_locked"]
        }
        "read_after_unlock" => {
            equal(A, None);
            equal(B, Some(B1));
            &["fixture_unlock_only", "a_still_absent", "b_unchanged"]
        }
        "missing_default" => {
            let service = fixture_service();
            service
                .get_default_collection()
                .fixture("fixture default collection")
                .delete()
                .fixture("fixture default deletion");
            assert!(matches!(
                service.get_default_collection(),
                Err(dbus_secret_service::Error::NoResult)
            ));
            let mut before: Vec<_> = service
                .get_all_collections()
                .fixture("fixture collections")
                .into_iter()
                .map(|collection| collection.path.to_string())
                .collect();
            refused(B, DeviceStoreError::Unavailable);
            assert!(matches!(
                service.get_default_collection(),
                Err(dbus_secret_service::Error::NoResult)
            ));
            let mut after: Vec<_> = service
                .get_all_collections()
                .fixture("fixture collections readback")
                .into_iter()
                .map(|collection| collection.path.to_string())
                .collect();
            before.sort();
            after.sort();
            assert_eq!(before, after, "refused operation created a collection");
            &[
                "read_write_delete_unavailable",
                "default_still_missing",
                "collections_unchanged",
            ]
        }
        "unavailable" => {
            refused(B, DeviceStoreError::Unavailable);
            &["read_write_delete_unavailable"]
        }
        _ => panic!("unknown fixture phase"),
    };
    println!(
        "FIXTURE_RESULT {}",
        json!({"phase": phase, "pid": std::process::id(),
        "result": "passed", "checks": checks})
    );
}
