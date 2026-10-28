//! Paired host device-cookie custody in the current OS user's protected store.
//! This is not provider auth, a realm grant, or a host-revocation API.

use std::{
    fmt,
    sync::Arc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::paired_host::PairedHostClient;

const SERVICE: &str = "ai.rkat.meerkat-connect.paired-host-device.v1";
const MAX_RECORD: usize = 8192;
const DEVICE_COOKIE: &str = "paired_host_device";

#[cfg(any(target_os = "linux", test))]
const SECRET_SERVICE_SCHEMA_ATTRIBUTE: &str = "xdg:schema";
#[cfg(any(target_os = "linux", test))]
const SECRET_SERVICE_GENERIC_SCHEMA: &str = "org.freedesktop.Secret.Generic";

#[cfg(any(target_os = "linux", test))]
fn secret_service_attributes_match(
    found: &std::collections::HashMap<String, String>,
    origin: &str,
) -> bool {
    if found.get("service").map(String::as_str) != Some(SERVICE)
        || found.get("origin").map(String::as_str) != Some(origin)
    {
        return false;
    }
    // GNOME Keyring 46.1 materializes this canonical schema on legacy reload.
    // It is the only permitted metadata addition to the exact identity pair.
    match found.len() {
        2 => true,
        3 => {
            found
                .get(SECRET_SERVICE_SCHEMA_ATTRIBUTE)
                .map(String::as_str)
                == Some(SECRET_SERVICE_GENERIC_SCHEMA)
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeviceStoreError {
    #[error("the OS credential store is locked or access was denied")]
    Locked,
    #[error("the OS credential store is unavailable")]
    Unavailable,
    #[error("more than one matching device credential exists")]
    Ambiguous,
    #[error("the saved device credential is invalid")]
    InvalidRecord,
    #[error("the credential-store outcome is unconfirmed; do not retry automatically")]
    Unconfirmed,
}

/// Trusted custody seam for the concrete OS adapter and deterministic fixtures.
/// Implementations must not log secret bytes, fall back to files, or broaden the
/// origin/user scope. Calls are synchronous and are joined by `StoreJob`.
pub trait DeviceStore: Send + Sync + 'static {
    fn read(&self, origin: &str) -> Result<Option<Vec<u8>>, DeviceStoreError>;
    fn write(&self, origin: &str, secret: &[u8]) -> Result<(), DeviceStoreError>;
    fn delete(&self, origin: &str) -> Result<(), DeviceStoreError>;
}

/// Construction is inert. OS calls happen only inside an owned store job.
pub struct OsDeviceStore;

impl DeviceStore for OsDeviceStore {
    fn read(&self, origin: &str) -> Result<Option<Vec<u8>>, DeviceStoreError> {
        platform::read(origin)
    }
    fn write(&self, origin: &str, secret: &[u8]) -> Result<(), DeviceStoreError> {
        platform::write(origin, secret)
    }
    fn delete(&self, origin: &str) -> Result<(), DeviceStoreError> {
        platform::delete(origin)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfirmedDevice {
    version: u8,
    origin: String,
    cookie: String,
    expires_at: u64,
    secure: bool,
}

impl fmt::Debug for ConfirmedDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConfirmedDevice([redacted])")
    }
}

pub(crate) fn now() -> Result<u64, DeviceStoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| DeviceStoreError::Unavailable)
}

fn valid_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.bytes().all(|byte| {
            byte == 0x21
                || (0x23..=0x2b).contains(&byte)
                || (0x2d..=0x3a).contains(&byte)
                || (0x3c..=0x5b).contains(&byte)
                || (0x5d..=0x7e).contains(&byte)
        })
}

impl ConfirmedDevice {
    /// The client keeps this candidate only after checking the full confirmation
    /// body. Max-Age is converted once at header receipt, before body waits.
    pub(crate) fn from_cookie(origin: &str, header: &str, received_at: u64) -> Option<Self> {
        let cookie = cookie::Cookie::parse(header).ok()?;
        if cookie.name() != DEVICE_COOKIE
            || !valid_value(cookie.value())
            || cookie.path() != Some("/")
            || cookie.domain().is_some()
            || cookie.http_only() != Some(true)
            || cookie.same_site() != Some(cookie::SameSite::Lax)
            || (cookie.secure() == Some(true) && !origin.starts_with("https://"))
        {
            return None;
        }
        // Signed durations are checked without reqwest's negative-Max-Age
        // conversion, which can panic. Max-Age takes precedence over Expires.
        let expires_at = if let Some(age) = cookie.max_age() {
            let seconds = u64::try_from(age.whole_seconds()).ok()?;
            received_at.checked_add(seconds)?
        } else {
            u64::try_from(cookie.expires_datetime()?.unix_timestamp()).ok()?
        };
        if expires_at <= received_at {
            return None;
        }
        Some(Self {
            version: 1,
            origin: origin.to_owned(),
            cookie: cookie.value().to_owned(),
            expires_at,
            secure: cookie.secure() == Some(true),
        })
    }

    fn decode(
        bytes: &[u8],
        origin: &str,
        current_time: u64,
    ) -> Result<LoadedDevice, DeviceStoreError> {
        if bytes.len() > MAX_RECORD {
            return Err(DeviceStoreError::InvalidRecord);
        }
        let record: Self =
            serde_json::from_slice(bytes).map_err(|_| DeviceStoreError::InvalidRecord)?;
        if record.version != 1
            || record.origin != origin
            || !valid_value(&record.cookie)
            || (record.secure && !origin.starts_with("https://"))
            || record.expires_at > i64::MAX as u64
        {
            return Err(DeviceStoreError::InvalidRecord);
        }
        if record.expires_at <= current_time {
            return Ok(LoadedDevice::Expired);
        }
        Ok(LoadedDevice::Ready(record))
    }

    pub(crate) fn header(
        &self,
        origin: &str,
        current_time: u64,
    ) -> Result<String, DeviceStoreError> {
        if self.origin != origin || self.expires_at <= current_time {
            return Err(DeviceStoreError::InvalidRecord);
        }
        let expires = cookie::time::OffsetDateTime::from_unix_timestamp(
            i64::try_from(self.expires_at).map_err(|_| DeviceStoreError::InvalidRecord)?,
        )
        .map_err(|_| DeviceStoreError::InvalidRecord)?;
        Ok(cookie::Cookie::build((DEVICE_COOKIE, self.cookie.clone()))
            .path("/")
            .http_only(true)
            .same_site(cookie::SameSite::Lax)
            .secure(self.secure)
            .expires(expires)
            .build()
            .to_string())
    }
}

#[derive(Debug)]
pub(crate) enum LoadedDevice {
    Missing,
    Expired,
    Ready(ConfirmedDevice),
}

/// There is no detachable blocking task: normal completion, cancellation and
/// Drop all join the exact thread. An OS call may block; no connection-time or
/// bounded-cancellation claim is made. Cancellation cannot undo a started write.
pub(crate) struct StoreJob<T> {
    receiver: oneshot::Receiver<Result<T, DeviceStoreError>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl<T: Send + 'static> StoreJob<T> {
    fn start(
        call: impl FnOnce() -> Result<T, DeviceStoreError> + Send + 'static,
    ) -> Result<Self, DeviceStoreError> {
        let (sender, receiver) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("paired-host-device-store".into())
            .spawn(move || {
                let _ = sender.send(call());
            })
            .map_err(|_| DeviceStoreError::Unavailable)?;
        Ok(Self {
            receiver,
            thread: Some(thread),
        })
    }
    pub(crate) async fn join(&mut self) -> Result<T, DeviceStoreError> {
        let result = (&mut self.receiver)
            .await
            .map_err(|_| DeviceStoreError::Unconfirmed)?;
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| DeviceStoreError::Unconfirmed)?;
        }
        result
    }
}

impl<T> Drop for StoreJob<T> {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(crate) fn load(
    store: Arc<dyn DeviceStore>,
    client: &PairedHostClient,
) -> Result<StoreJob<LoadedDevice>, DeviceStoreError> {
    let origin = client.canonical_origin();
    StoreJob::start(move || match store.read(&origin)? {
        None => Ok(LoadedDevice::Missing),
        Some(bytes) => ConfirmedDevice::decode(&bytes, &origin, now()?),
    })
}

pub(crate) fn save(
    store: Arc<dyn DeviceStore>,
    client: &mut PairedHostClient,
) -> Result<StoreJob<()>, DeviceStoreError> {
    let record = client
        .take_confirmed_device()
        .ok_or(DeviceStoreError::InvalidRecord)?;
    let origin = client.canonical_origin();
    // Do not persist a confirmation which expired while its body was pending.
    record.header(&origin, now()?)?;
    let bytes = serde_json::to_vec(&record).map_err(|_| DeviceStoreError::InvalidRecord)?;
    if bytes.len() > MAX_RECORD {
        return Err(DeviceStoreError::InvalidRecord);
    }
    StoreJob::start(move || {
        store
            .write(&origin, &bytes)
            .map_err(|_| DeviceStoreError::Unconfirmed)?;
        match store.read(&origin) {
            Ok(Some(stored)) if stored == bytes => Ok(()),
            _ => Err(DeviceStoreError::Unconfirmed),
        }
    })
}

pub(crate) fn forget(
    store: Arc<dyn DeviceStore>,
    client: &PairedHostClient,
) -> Result<StoreJob<()>, DeviceStoreError> {
    let origin = client.canonical_origin();
    StoreJob::start(move || {
        store
            .delete(&origin)
            .map_err(|_| DeviceStoreError::Unconfirmed)?;
        // In particular, keyring's Mac delete ignores the native OSStatus.
        // Only this exact subsequent NoEntry confirms local absence.
        match store.read(&origin) {
            Ok(None) => Ok(()),
            _ => Err(DeviceStoreError::Unconfirmed),
        }
    })
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use sha2::{Digest, Sha256};
    fn entry(origin: &str) -> Result<keyring::Entry, DeviceStoreError> {
        let account = format!("origin-v1:{:x}", Sha256::digest(origin.as_bytes()));
        let credential =
            keyring::macos::MacCredential::new_with_target(Some("User"), SERVICE, &account)
                .map_err(error)?;
        Ok(keyring::Entry::new_with_credential(Box::new(credential)))
    }
    fn error(error: keyring::Error) -> DeviceStoreError {
        match error {
            keyring::Error::NoStorageAccess(_) => DeviceStoreError::Locked,
            keyring::Error::Ambiguous(_) => DeviceStoreError::Ambiguous,
            _ => DeviceStoreError::Unavailable,
        }
    }
    pub(super) fn read(origin: &str) -> Result<Option<Vec<u8>>, DeviceStoreError> {
        match entry(origin)?.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(other) => Err(error(other)),
        }
    }
    pub(super) fn write(origin: &str, bytes: &[u8]) -> Result<(), DeviceStoreError> {
        entry(origin)?.set_secret(bytes).map_err(error)
    }
    pub(super) fn delete(origin: &str) -> Result<(), DeviceStoreError> {
        match entry(origin)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(other) => Err(error(other)),
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use dbus_secret_service::{Collection, EncryptionType, Item, SecretService};
    use std::collections::HashMap;

    fn service() -> Result<SecretService, DeviceStoreError> {
        SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0)
            .map_err(|_| DeviceStoreError::Unavailable)
    }
    fn collection(service: &SecretService) -> Result<Collection<'_>, DeviceStoreError> {
        let collection = service
            .get_default_collection()
            .map_err(|_| DeviceStoreError::Unavailable)?;
        if collection
            .is_locked()
            .map_err(|_| DeviceStoreError::Unavailable)?
        {
            return Err(DeviceStoreError::Locked);
        }
        Ok(collection)
    }
    fn attributes(origin: &str) -> HashMap<&str, &str> {
        HashMap::from([("service", SERVICE), ("origin", origin)])
    }
    fn item<'a>(
        collection: &'a Collection<'_>,
        origin: &str,
    ) -> Result<Option<Item<'a>>, DeviceStoreError> {
        let mut items = collection
            .search_items(attributes(origin))
            .map_err(|_| DeviceStoreError::Unavailable)?;
        if items.len() > 1 {
            return Err(DeviceStoreError::Ambiguous);
        }
        let Some(item) = items.pop() else {
            return Ok(None);
        };
        let found = item
            .get_attributes()
            .map_err(|_| DeviceStoreError::Unavailable)?;
        if !secret_service_attributes_match(&found, origin) {
            return Err(DeviceStoreError::Ambiguous);
        }
        if item
            .is_locked()
            .map_err(|_| DeviceStoreError::Unavailable)?
        {
            return Err(DeviceStoreError::Locked);
        }
        Ok(Some(item))
    }
    pub(super) fn read(origin: &str) -> Result<Option<Vec<u8>>, DeviceStoreError> {
        let service = service()?;
        let collection = collection(&service)?;

        item(&collection, origin)?
            .map(|item| item.get_secret().map_err(|_| DeviceStoreError::Unavailable))
            .transpose()
    }
    pub(super) fn write(origin: &str, bytes: &[u8]) -> Result<(), DeviceStoreError> {
        let service = service()?;
        let collection = collection(&service)?;
        if let Some(item) = item(&collection, origin)? {
            item.set_secret(bytes, "application/json")
                .map_err(|_| DeviceStoreError::Unconfirmed)?;
        } else {
            // Never replace a broader search result; no unlock/create-collection
            // or get-any fallback. Concurrent duplicates are refused on readback.
            collection
                .create_item(
                    "Meerkat Connect paired host device",
                    attributes(origin),
                    bytes,
                    false,
                    "application/json",
                )
                .map_err(|_| DeviceStoreError::Unconfirmed)?;
        }
        Ok(())
    }
    pub(super) fn delete(origin: &str) -> Result<(), DeviceStoreError> {
        let service = service()?;
        let collection = collection(&service)?;
        if let Some(item) = item(&collection, origin)? {
            item.delete().map_err(|_| DeviceStoreError::Unconfirmed)?;
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::*;
    pub(super) fn read(_: &str) -> Result<Option<Vec<u8>>, DeviceStoreError> {
        Err(DeviceStoreError::Unavailable)
    }
    pub(super) fn write(_: &str, _: &[u8]) -> Result<(), DeviceStoreError> {
        Err(DeviceStoreError::Unavailable)
    }
    pub(super) fn delete(_: &str) -> Result<(), DeviceStoreError> {
        Err(DeviceStoreError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc;

    const ORIGIN: &str = "https://home.example:8443";
    const MARKER: &str = "synthetic-device-only";

    fn header(attributes: &str) -> String {
        format!("paired_host_device={MARKER}; Path=/; HttpOnly; SameSite=Lax; {attributes}")
    }

    fn secret_service_metadata() -> HashMap<String, String> {
        HashMap::from([
            ("service".to_owned(), SERVICE.to_owned()),
            ("origin".to_owned(), ORIGIN.to_owned()),
        ])
    }

    #[test]
    fn secret_service_metadata_accepts_plain_and_canonical_reload_schema() {
        let mut found = secret_service_metadata();
        assert!(secret_service_attributes_match(&found, ORIGIN));
        found.insert(
            "xdg:schema".to_owned(),
            "org.freedesktop.Secret.Generic".to_owned(),
        );
        assert!(secret_service_attributes_match(&found, ORIGIN));
    }

    #[test]
    fn secret_service_metadata_preserves_exact_service_and_origin() {
        for with_schema in [false, true] {
            let mut found = secret_service_metadata();
            if with_schema {
                found.insert(
                    "xdg:schema".to_owned(),
                    "org.freedesktop.Secret.Generic".to_owned(),
                );
            }
            for other in [
                "https://home.example",
                "http://home.example:8443",
                "https://alias.example:8443",
            ] {
                assert!(!secret_service_attributes_match(&found, other));
            }
            for key in ["service", "origin"] {
                let mut missing = found.clone();
                missing.remove(key);
                assert!(!secret_service_attributes_match(&missing, ORIGIN));
                for value in ["", "different"] {
                    let mut changed = found.clone();
                    changed.insert(key.to_owned(), value.to_owned());
                    assert!(!secret_service_attributes_match(&changed, ORIGIN));
                }
            }
        }
    }

    #[test]
    fn secret_service_metadata_rejects_other_schema_or_extra_fields() {
        for schema in [
            "",
            "org.gnome.keyring.Note",
            "org.freedesktop.Secret.Generic ",
            "org.freedesktop.secret.Generic",
        ] {
            let mut found = secret_service_metadata();
            found.insert("xdg:schema".to_owned(), schema.to_owned());
            assert!(!secret_service_attributes_match(&found, ORIGIN));
        }
        for key in ["extra", "XDG:schema", "gkr:compat:hashed:service"] {
            let mut found = secret_service_metadata();
            found.insert(key.to_owned(), "org.freedesktop.Secret.Generic".to_owned());
            assert!(!secret_service_attributes_match(&found, ORIGIN));
            found.insert(
                "xdg:schema".to_owned(),
                "org.freedesktop.Secret.Generic".to_owned(),
            );
            assert!(!secret_service_attributes_match(&found, ORIGIN));
        }
    }

    #[test]
    fn expiry_is_absolute_and_max_age_has_precedence_without_negative_panics() {
        let record = ConfirmedDevice::from_cookie(
            ORIGIN,
            &header("Max-Age=90; Expires=Wed, 09 Jun 2027 10:18:14 GMT; Secure"),
            1000,
        )
        .unwrap();
        assert_eq!(record.expires_at, 1090);
        let restored = record.header(ORIGIN, 1080).unwrap();
        let parsed = cookie::Cookie::parse(restored).unwrap();
        assert_eq!(parsed.max_age(), None);
        assert_eq!(parsed.expires_datetime().unwrap().unix_timestamp(), 1090);
        assert_eq!(parsed.secure(), Some(true));
        assert!(record.header(ORIGIN, 1090).is_err());
        assert!(record.header("http://home.example:8443", 1080).is_err());
        assert!(record.header("https://home.example:8444", 1080).is_err());
        assert_eq!(format!("{record:?}"), "ConfirmedDevice([redacted])");
        for attributes in [
            "",
            "Max-Age=0",
            "Max-Age=-1",
            "Max-Age=-1; Expires=Wed, 09 Jun 2027 10:18:14 GMT",
        ] {
            assert!(ConfirmedDevice::from_cookie(ORIGIN, &header(attributes), 1000).is_none());
        }
        let expires = ConfirmedDevice::from_cookie(
            ORIGIN,
            &header("Expires=Wed, 09 Jun 2027 10:18:14 GMT"),
            1000,
        )
        .unwrap();
        assert!(expires.expires_at > 1000);
        assert!(
            ConfirmedDevice::from_cookie(
                "http://home.example",
                &header("Max-Age=90; Secure"),
                1000
            )
            .is_none()
        );
        assert!(
            ConfirmedDevice::from_cookie(ORIGIN, &header("Max-Age=90; Domain=home.example"), 1000)
                .is_none()
        );
        assert!(
            ConfirmedDevice::from_cookie(
                ORIGIN,
                "paired_host_pair=synthetic; Path=/; HttpOnly; SameSite=Lax; Max-Age=90",
                1000
            )
            .is_none()
        );
    }

    #[test]
    fn saved_records_are_closed_bounded_and_exact_origin_scoped() {
        let record = ConfirmedDevice::from_cookie(ORIGIN, &header("Max-Age=90"), 1000).unwrap();
        let encoded = serde_json::to_vec(&record).unwrap();
        assert!(matches!(
            ConfirmedDevice::decode(&encoded, ORIGIN, 1000).unwrap(),
            LoadedDevice::Ready(_)
        ));
        assert!(matches!(
            ConfirmedDevice::decode(&encoded, ORIGIN, 1090).unwrap(),
            LoadedDevice::Expired
        ));
        for other in [
            "https://home.example",
            "http://home.example:8443",
            "https://alias.example:8443",
        ] {
            assert_eq!(
                ConfirmedDevice::decode(&encoded, other, 1000).unwrap_err(),
                DeviceStoreError::InvalidRecord
            );
        }
        let mut unknown: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        unknown["extra"] = serde_json::json!(MARKER);
        let duplicate = String::from_utf8(encoded.clone()).unwrap().replacen(
            "\"version\":1",
            "\"version\":1,\"version\":1",
            1,
        );
        for bytes in [
            serde_json::to_vec(&unknown).unwrap(),
            duplicate.into_bytes(),
            vec![b'x'; MAX_RECORD + 1],
            b"not json".to_vec(),
        ] {
            let error = ConfirmedDevice::decode(&bytes, ORIGIN, 1000).unwrap_err();
            assert_eq!(error, DeviceStoreError::InvalidRecord);
            assert!(!error.to_string().contains(MARKER));
        }
    }

    #[test]
    fn dropping_store_job_joins_the_actual_blocking_operation() {
        let (entered, did_enter) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let job = StoreJob::start(move || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .unwrap();
        did_enter.recv().unwrap();
        let (dropping, did_drop_start) = mpsc::channel();
        let (done, did_finish) = mpsc::channel();
        let dropper = thread::spawn(move || {
            dropping.send(()).unwrap();
            drop(job);
            done.send(()).unwrap();
        });
        did_drop_start.recv().unwrap();
        let early = did_finish.recv_timeout(std::time::Duration::from_millis(30));
        release.send(()).unwrap();
        dropper.join().unwrap();
        assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)));
        did_finish.recv().unwrap();
    }
}
