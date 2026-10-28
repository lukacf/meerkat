//! Private request locators, not restored status, approval or submit custody.

use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const LIMIT: usize = 4096;
const PAGE_READS: usize = 8;
const PAGE_VISITS: usize = 128;

pub(super) fn unavailable() -> crate::ConnectError {
    crate::ConnectError::InvalidInput("Paired host setup locators are unavailable. No previous approval or submission was restored.".into())
}

fn refused_recovery() -> ToolError {
    ToolError::execution_failed(
        "Paired host setup history is unavailable. No approval or submission was restored; do not reprepare or resubmit to recover it.",
    )
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    ChannelConfig,
    NewExtension,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::ChannelConfig => "channel_config",
            Self::NewExtension => "new_extension",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Reference {
    schema: u8,
    realm: String,
    origin: String,
    change_digest: FrontendChangeDigest,
    kind: Kind,
    review_path: Option<String>,
}

impl Reference {
    fn valid(&self) -> bool {
        self.schema == 1
            && !self.realm.is_empty()
            && self.realm.len() <= 256
            && PairedHostClient::new(&self.origin)
                .is_ok_and(|client| client.selected_origin() == self.origin)
            && self.review_path.as_ref().is_none_or(|path| {
                path == &format!(
                    "/console/extensions/frontend/review/{}",
                    self.change_digest.as_str()
                )
            })
    }

    fn filename(&self) -> String {
        let key = format!(
            "{}\n{}\n{}\n{}\n{}",
            self.realm,
            self.origin,
            self.change_digest.as_str(),
            self.kind.name(),
            self.review_path.as_deref().unwrap_or("")
        );
        format!(
            "paired-host-request-{:x}.json",
            Sha256::digest(key.as_bytes())
        )
    }

    fn matches_observation(&self, observed: &FrontendChangeRead) -> bool {
        match observed {
            FrontendChangeRead::Status(status) => {
                self.kind == Kind::ChannelConfig && status.request_id() == &self.change_digest
            }
            FrontendChangeRead::NewExtensionStatus(status) => {
                self.kind == Kind::NewExtension && status.request_id() == &self.change_digest
            }
            FrontendChangeRead::Prepared(draft) => {
                draft.change.change_digest() == &self.change_digest
                    && match &draft.change {
                        crate::paired_host::FrontendPreparedChange::Channel(_) => {
                            self.kind == Kind::ChannelConfig
                        }
                        crate::paired_host::FrontendPreparedChange::NewExtension(_) => {
                            self.kind == Kind::NewExtension
                        }
                    }
            }
            FrontendChangeRead::NotFound | FrontendChangeRead::Rejected(_) => false,
        }
    }
}

fn owned(path: &Path, directory: bool) -> Result<(), crate::ConnectError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.file_type().is_symlink()
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file() || metadata.nlink() != 1
        }
    {
        return Err(unavailable());
    }
    Ok(())
}

fn read(path: &Path) -> Result<Reference, crate::ConnectError> {
    owned(path, false)?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| unavailable())?;
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > LIMIT as u64
    {
        return Err(unavailable());
    }
    let mut bytes = String::new();
    Read::take(&mut file, (LIMIT + 1) as u64)
        .read_to_string(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > LIMIT {
        return Err(unavailable());
    }
    serde_json::from_str::<crate::forge::UniqueJson>(&bytes).map_err(|_| unavailable())?;
    let reference: Reference = serde_json::from_str(&bytes).map_err(|_| unavailable())?;
    if !reference.valid()
        || path.file_name().and_then(|name| name.to_str()) != Some(reference.filename().as_str())
    {
        return Err(unavailable());
    }
    Ok(reference)
}

struct Discovery {
    directories: fs::ReadDir,
    files: Option<fs::ReadDir>,
    seen: BTreeSet<String>,
    exhausted: bool,
}

impl Discovery {
    fn open(parent: &Path) -> Result<Self, crate::ConnectError> {
        owned(parent, true)?;
        Ok(Self {
            directories: fs::read_dir(parent).map_err(|_| unavailable())?,
            files: None,
            seen: BTreeSet::new(),
            exhausted: false,
        })
    }

    fn page(&mut self, realm: &str, origin: &str) -> (Vec<Reference>, bool) {
        let mut references = Vec::new();
        // Bound filesystem work and actual host requests independently. A
        // zero-result page can still have more traversal to do; no skipped
        // identities, filenames, origins or counts enter the model response.
        for _ in 0..PAGE_VISITS {
            if references.len() == PAGE_READS {
                break;
            }
            if let Some(files) = &mut self.files {
                match files.next() {
                    None => self.files = None,
                    Some(Err(_)) => {}
                    Some(Ok(entry)) => {
                        if !entry.file_name().to_str().is_some_and(|name| {
                            name.starts_with("paired-host-request-") && name.ends_with(".json")
                        }) {
                            continue;
                        }
                        let Ok(reference) = read(&entry.path()) else {
                            continue;
                        };
                        if reference.realm != realm || reference.origin != origin {
                            continue;
                        }
                        if self.seen.insert(reference.filename()) {
                            references.push(reference);
                        }
                    }
                }
            } else {
                match self.directories.next() {
                    None => {
                        self.exhausted = true;
                        break;
                    }
                    Some(Err(_)) => {}
                    Some(Ok(entry)) => {
                        if !entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.starts_with("setup-"))
                            || owned(&entry.path(), true).is_err()
                        {
                            continue;
                        }
                        if let Ok(files) = fs::read_dir(entry.path()) {
                            self.files = Some(files);
                        }
                    }
                }
            }
        }
        (references, !self.exhausted)
    }
}

struct ReadPage {
    origin: String,
    references: Vec<Reference>,
    more: bool,
}

pub(super) struct Store {
    directory: PathBuf,
    parent: PathBuf,
    realm: String,
    discovery: Option<Discovery>,
    // A failed or dropped read future must not consume an unreturned page.
    // This holds only locators; each attempt obtains fresh host observations.
    pending: Option<Arc<ReadPage>>,
    // These remain readback locators even after a successful observation.
    // Every redisplay requires a new host observation of that exact digest.
    located: BTreeMap<String, Reference>,
}

impl Store {
    pub(super) fn open(
        realm: &meerkat_core::RealmLocator,
        directory: &Path,
    ) -> Result<Self, crate::ConnectError> {
        if !directory.is_absolute()
            || !directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("setup-"))
        {
            return Err(unavailable());
        }
        owned(directory, true)?;
        let parent = directory.parent().ok_or_else(unavailable)?;
        owned(parent, true)?;
        owned(parent.parent().ok_or_else(unavailable)?, true)?;
        let expected = fs::canonicalize(&realm.state_root)
            .map_err(|_| unavailable())?
            .join(".meerkat-connect")
            .join(realm.realm.as_str());
        if fs::canonicalize(parent).map_err(|_| unavailable())? != expected {
            return Err(unavailable());
        }
        let directory = fs::canonicalize(directory).map_err(|_| unavailable())?;
        Ok(Self {
            directory,
            parent: expected,
            realm: realm.realm.as_str().into(),
            discovery: None,
            pending: None,
            located: BTreeMap::new(),
        })
    }

    fn save(&self, reference: &Reference) -> Result<(), crate::ConnectError> {
        if !reference.valid() || reference.realm != self.realm {
            return Err(unavailable());
        }
        owned(&self.parent, true)?;
        owned(&self.directory, true)?;
        let bytes = serde_json::to_vec(reference).map_err(|_| unavailable())?;
        if bytes.len() > LIMIT {
            return Err(unavailable());
        }
        let path = self.directory.join(reference.filename());
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                if read(&path)? != *reference {
                    return Err(unavailable());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut temporary =
                    tempfile::NamedTempFile::new_in(&self.directory).map_err(|_| unavailable())?;
                temporary
                    .write_all(&bytes)
                    .and_then(|_| temporary.as_file().sync_all())
                    .map_err(|_| unavailable())?;
                temporary
                    .persist_noclobber(&path)
                    .map_err(|_| unavailable())?;
            }
            Err(_) => return Err(unavailable()),
        }
        fs::File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(|_| unavailable())
    }

    fn page(
        &mut self,
        origin: &str,
        restart: bool,
    ) -> Result<(Vec<Reference>, bool), crate::ConnectError> {
        owned(&self.parent, true)?;
        if restart || self.discovery.is_none() {
            self.discovery = Some(Discovery::open(&self.parent)?);
        }
        Ok(self
            .discovery
            .as_mut()
            .ok_or_else(unavailable)?
            .page(&self.realm, origin))
    }

    fn read_page(
        &mut self,
        origin: &str,
        restart: bool,
    ) -> Result<Arc<ReadPage>, crate::ConnectError> {
        owned(&self.parent, true)?;
        if !restart
            && let Some(page) = &self.pending
            && page.origin == origin
        {
            return Ok(page.clone());
        }
        let restart = restart
            || self
                .pending
                .as_ref()
                .is_some_and(|page| page.origin != origin);
        let (references, more) = self.page(origin, restart)?;
        let page = Arc::new(ReadPage {
            origin: origin.into(),
            references,
            more,
        });
        self.pending = Some(page.clone());
        Ok(page)
    }

    fn finish_page(&mut self, page: &Arc<ReadPage>) {
        // An overlapping older read must not consume a newly restarted page.
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, page))
        {
            self.pending = None;
        }
    }
}

impl FrontendTools {
    pub(super) fn has_recovery(&self) -> bool {
        self.surface == FrontendSurface::Interactive
            && self
                .recovery
                .lock()
                .is_ok_and(|store| matches!(&*store, Some(Ok(_))))
    }

    pub(super) fn require_recovery_available(&self) -> Result<(), ToolError> {
        match &*self.recovery.lock().map_err(|_| refused_recovery())? {
            Some(Err(())) => Err(refused_recovery()),
            _ => Ok(()),
        }
    }

    pub(super) fn retain_recovery(
        &self,
        change: &PreparedChange,
        review_path: Option<&str>,
    ) -> Result<(), ToolError> {
        let recovery = self.recovery.lock().map_err(|_| refused_recovery())?;
        let store = match &*recovery {
            None => return Ok(()),
            Some(Ok(store)) => store,
            Some(Err(())) => return Err(refused_recovery()),
        };
        let host = self.host.captured().ok_or_else(refused_recovery)?;
        let reference = Reference {
            schema: 1,
            realm: store.realm.clone(),
            origin: host.origin.clone(),
            change_digest: change.change_digest().clone(),
            kind: match change {
                PreparedChange::Channel(_) => Kind::ChannelConfig,
                PreparedChange::NewExtension(_) => Kind::NewExtension,
            },
            review_path: review_path.map(str::to_owned),
        };
        store.save(&reference).map_err(|_| refused_recovery())
    }

    pub(super) async fn previous(
        &self,
        call: ToolCallView<'_>,
        host: &crate::host_selection::CapturedHost,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Request {
            #[serde(default)]
            restart: bool,
        }
        serde_json::from_str::<crate::forge::UniqueJson>(call.args.get())
            .map_err(|_| refused_recovery())?;
        let request: Request = call.parse_args().map_err(|_| refused_recovery())?;
        let page = {
            let mut recovery = self.recovery.lock().map_err(|_| refused_recovery())?;
            let Some(Ok(store)) = &mut *recovery else {
                return Err(refused_recovery());
            };
            store
                .read_page(&host.origin, request.restart)
                .map_err(|_| refused_recovery())?
        };
        let mut observations = Vec::new();
        let mut admitted = Vec::new();
        for reference in &page.references {
            let current = match host
                .client
                .lock()
                .await
                .frontend_change_status(&reference.change_digest)
                .await
            {
                Ok(current) => current,
                // This exact point-read refusal exposes nothing about its
                // locator or authority over any other request in the page.
                Err(crate::paired_host::PairedHostError::Refused {
                    operation: crate::paired_host::PairedHostOperation::FrontendChangeStatus,
                    reason: crate::paired_host::PairedHostRefusal::ConsoleOwnerRequired,
                }) => continue,
                Err(_) => return Err(refused_recovery()),
            };
            // An operation rejection is not a request's rejected status. It
            // leaves this page pending without exposing the rejected locator.
            if matches!(&current, FrontendChangeRead::Rejected(_)) {
                return Err(refused_recovery());
            }
            if reference.matches_observation(&current) {
                // Serialize only newly admitted host data, never a saved title,
                // status, review path, principal or historical host response.
                observations.push(Observed {
                    change_digest: reference.change_digest.clone(),
                    current,
                });
                admitted.push(reference.clone());
            }
        }
        // Serialize directly to preserve opaque owner JSON, before consuming
        // the page. No response contains saved outcome or approval data.
        let response = Previous {
            requests: observations,
            more: page.more,
            scope: "current_host_observations_only",
            notice: "These current host observations restore no approval or permission to submit. A prepared draft does not establish that an earlier submission failed. Ask for a selected request's review link to inspect it privately; never reprepare or resubmit to recover status.",
        };
        let content = serde_json::to_string(&response).map_err(|_| refused_recovery())?;
        {
            let mut recovery = self.recovery.lock().map_err(|_| refused_recovery())?;
            let Some(Ok(store)) = &mut *recovery else {
                return Err(refused_recovery());
            };
            for reference in admitted {
                let key = reference.change_digest.as_str().to_owned();
                if !store
                    .located
                    .get(&key)
                    .is_some_and(|old| old.review_path.is_some() && reference.review_path.is_none())
                {
                    store.located.insert(key, reference);
                }
            }
            store.finish_page(&page);
        }
        Ok(ToolResult::new(call.id.into(), content, false).into())
    }

    pub(super) fn requeue_observed_review(&self, observed: &FrontendChangeRead) -> &'static str {
        let digest = match observed {
            FrontendChangeRead::Status(status) => status.request_id(),
            FrontendChangeRead::NewExtensionStatus(status) => status.request_id(),
            FrontendChangeRead::Prepared(draft) => draft.change.change_digest(),
            _ => return "unavailable",
        };
        // Preserve the existing current-invocation path, including its external
        // uncertain-effect gate. Recovered records never enter retained.prepared.
        let original = self.requeue_review(digest);
        if original != "unavailable" {
            return original;
        }
        let reference = self.recovery.lock().ok().and_then(|recovery| {
            let Some(Ok(store)) = &*recovery else {
                return None;
            };
            store.located.get(digest.as_str()).cloned()
        });
        let Some(reference) = reference else {
            return "unavailable";
        };
        if !self
            .host
            .captured()
            .is_some_and(|host| reference.origin == host.origin)
            || !reference.matches_observation(observed)
        {
            return "unavailable";
        }
        match reference.review_path.as_deref() {
            Some(path) => self.queue_review(digest, reference.kind.name(), path),
            None => "unavailable",
        }
    }
}

#[derive(Serialize)]
struct Observed {
    change_digest: FrontendChangeDigest,
    current: FrontendChangeRead,
}

#[derive(Serialize)]
struct Previous {
    requests: Vec<Observed>,
    more: bool,
    scope: &'static str,
    notice: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
        time::timeout,
    };

    const BOUND: Duration = Duration::from_secs(5);
    const CREDENTIAL: &str = "fixture-return-presenter";

    fn storage(root: &Path, realm: &str) -> meerkat_core::ResolvedStorage {
        crate::RealmOptions {
            context_root: root.join("context"),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("user")),
        }
        .resolve(realm)
        .unwrap()
    }

    fn digest(index: usize) -> FrontendChangeDigest {
        FrontendChangeDigest::new(&format!("paired-host-change-v1:sha256:{index:064x}")).unwrap()
    }

    fn reference(realm: &str, origin: &str, index: usize) -> Reference {
        let change_digest = digest(index);
        Reference {
            schema: 1,
            realm: realm.into(),
            origin: origin.into(),
            kind: Kind::ChannelConfig,
            review_path: Some(format!(
                "/console/extensions/frontend/review/{}",
                change_digest.as_str()
            )),
            change_digest,
        }
    }

    fn prepared(index: usize) -> String {
        format!(
            r#"{{"state":"prepared","base_current":false,"change":{{"change_digest":"{}","base":{{"activation_id":"1","policy_digest":"sha256:fixture","ledger_digest":"sha256:fixture"}},"ledger_patch":{{"path":"deployment/extensions.toml","extension_id":"fixture","channel_id":"fixture:1","before":null,"after":{{"threshold":1e-06}}}},"unified_diff":"fixture"}}}}"#,
            digest(index).as_str()
        )
    }

    fn opened(root: &Path, realm: &str) -> (meerkat_core::ResolvedStorage, Store) {
        let storage = storage(root, realm);
        let directory = crate::resources::reserve_setup_artifacts(&storage).unwrap();
        let store = Store::open(&storage.locator, &directory).unwrap();
        (storage, store)
    }

    #[test]
    fn private_locators_reopen_without_status_credentials_or_submission_objects() {
        let root = tempfile::tempdir().unwrap();
        let (storage, first) = opened(root.path(), "return-fixture");
        let reference = reference("return-fixture", "https://host.invalid", 1);
        first.save(&reference).unwrap();
        first.save(&reference).unwrap();
        let saved = fs::read_to_string(first.directory.join(reference.filename())).unwrap();
        let fields: Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            fields
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            [
                "schema",
                "realm",
                "origin",
                "change_digest",
                "kind",
                "review_path"
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(
            fs::metadata(first.directory.join(reference.filename()))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        drop(first);
        let directory = crate::resources::reserve_setup_artifacts(&storage).unwrap();
        let mut next = Store::open(&storage.locator, &directory).unwrap();
        let (found, _) = next.page("https://host.invalid", false).unwrap();
        assert!(found == vec![reference]);
        assert!(
            next.located.is_empty(),
            "disk locators are not admitted review references"
        );
    }

    #[test]
    fn recovery_refuses_wrong_realm_permissions_symlinks_and_secret_paths() {
        let root = tempfile::tempdir().unwrap();
        let (_, store) = opened(root.path(), "return-fixture");
        let other = storage(root.path(), "other-realm");
        assert!(Store::open(&other.locator, &store.directory).is_err());
        let reference = reference("return-fixture", "https://host.invalid", 1);
        store.save(&reference).unwrap();
        let path = store.directory.join(reference.filename());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let linked = store.directory.join("paired-host-request-linked.json");
        symlink(&path, &linked).unwrap();
        assert!(read(&linked).is_err());
        let mut secret = reference.clone();
        secret.review_path = Some("/console/extensions/secrets/entry/private-fixture".into());
        assert!(store.save(&secret).is_err());
        let mut foreign = reference.clone();
        foreign.realm = "other-realm".into();
        assert!(store.save(&foreign).is_err());
        let mut extra = serde_json::to_value(&reference).unwrap();
        extra["approval"] = json!("approved");
        fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(read(&path).is_err());
        fs::write(
            &path,
            format!(
                "{{\"schema\":1,{}",
                serde_json::to_string(&reference)
                    .unwrap()
                    .trim_start_matches('{')
            ),
        )
        .unwrap();
        assert!(
            read(&path).is_err(),
            "duplicate fields must not be normalized"
        );
    }

    #[test]
    fn previous_pages_bound_work_and_ignore_foreign_host_and_realm_locators() {
        let root = tempfile::tempdir().unwrap();
        let (_, mut store) = opened(root.path(), "return-fixture");
        for index in 0..19 {
            store
                .save(&reference("return-fixture", "https://host.invalid", index))
                .unwrap();
        }
        store
            .save(&reference("return-fixture", "https://other.invalid", 91))
            .unwrap();
        let (_, other) = opened(root.path(), "other-realm");
        other
            .save(&reference("other-realm", "https://host.invalid", 92))
            .unwrap();
        let mut found = BTreeSet::new();
        for page_number in 0..10 {
            let (page, more) = store.page("https://host.invalid", false).unwrap();
            assert!(page.len() <= PAGE_READS);
            found.extend(
                page.into_iter()
                    .map(|row| row.change_digest.as_str().to_owned()),
            );
            if !more {
                break;
            }
            assert!(page_number < 9);
        }
        assert_eq!(
            found,
            (0..19)
                .map(|index| digest(index).as_str().to_owned())
                .collect()
        );
    }

    struct Host {
        origin: String,
        allowed: Arc<AtomicBool>,
        replies: Arc<StateMutex<BTreeMap<String, (u16, String)>>>,
        calls: Arc<StateMutex<Vec<String>>>,
        fail_at: Arc<StateMutex<Option<usize>>>,
        stop: Option<oneshot::Sender<()>>,
        task: Option<tokio::task::JoinHandle<()>>,
    }

    impl Host {
        async fn open() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let expected_origin = origin.clone();
            let allowed = Arc::new(AtomicBool::new(true));
            let admission = allowed.clone();
            let replies: Arc<StateMutex<BTreeMap<String, (u16, String)>>> =
                Arc::new(StateMutex::new(BTreeMap::new()));
            let responses = replies.clone();
            let calls = Arc::new(StateMutex::new(Vec::new()));
            let observed = calls.clone();
            let fail_at = Arc::new(StateMutex::new(None));
            let failure = fail_at.clone();
            let (stop, mut stopped) = oneshot::channel();
            let task = tokio::spawn(async move {
                loop {
                    let accepted = tokio::select! { biased;
                        _ = &mut stopped => break,
                        value = listener.accept() => value.unwrap(),
                    };
                    let (mut stream, _) = accepted;
                    let mut bytes = Vec::new();
                    timeout(BOUND, async {
                        while !bytes.ends_with(b"\r\n\r\n") {
                            assert!(bytes.len() < 8192);
                            let mut byte = [0u8; 1];
                            stream.read_exact(&mut byte).await.unwrap();
                            bytes.push(byte[0]);
                        }
                    })
                    .await
                    .unwrap();
                    let request = String::from_utf8(bytes).unwrap();
                    let line = request.lines().next().unwrap();
                    assert!(line.starts_with("GET /console/extensions/frontend/change-requests/"));
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains(&format!("origin: {expected_origin}"))
                    );
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("cache-control: no-store")
                    );
                    let digest = line
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .to_owned();
                    let call_number = {
                        let mut calls = observed.lock().unwrap();
                        calls.push(digest.clone());
                        calls.len()
                    };
                    let transient = {
                        let mut fail_at = failure.lock().unwrap();
                        if *fail_at == Some(call_number) {
                            *fail_at = None;
                            true
                        } else {
                            false
                        }
                    };
                    let authorized = admission.load(Ordering::SeqCst)
                        && request
                            .contains(&format!("x-paired-host-presenter-credential: {CREDENTIAL}"));
                    let (status, body) = if transient {
                        (503, json!({"error":"unavailable","problems":[{"code":"current_inputs_invalid","path":"","message":"The current ledger or compiled family contacts cannot be read."}]}).to_string())
                    } else if authorized {
                        responses
                            .lock()
                            .unwrap()
                            .get(&digest)
                            .cloned()
                            .unwrap_or((404, "{\"state\":\"not_found\"}".into()))
                    } else {
                        (
                            403,
                            "{\"error\":\"forbidden\",\"code\":\"console_owner_required\"}".into(),
                        )
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    timeout(BOUND, stream.write_all(reply.as_bytes()))
                        .await
                        .unwrap()
                        .unwrap();
                }
            });
            Self {
                origin,
                allowed,
                replies,
                calls,
                fail_at,
                stop: Some(stop),
                task: Some(task),
            }
        }

        fn frontend(&self) -> PairedHostFrontend {
            PairedHostFrontend::new(
                PairedHostClient::for_presenter(&self.origin, CREDENTIAL).unwrap(),
            )
        }

        fn status(&self, index: usize) {
            let digest = digest(index);
            self.replies.lock().unwrap().insert(digest.as_str().into(), (200,
                json!({"state":"applied","request_id":digest,"submitted_at":"fixture","activation_id":"7"}).to_string()));
        }

        async fn finish(mut self) {
            let _ = self.stop.take().unwrap().send(());
            timeout(BOUND, self.task.take().unwrap())
                .await
                .unwrap()
                .unwrap();
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                task.abort();
            }
        }
    }

    async fn call(
        frontend: &PairedHostFrontend,
        name: &str,
        args: Value,
    ) -> Result<String, ToolError> {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        timeout(
            BOUND,
            frontend.dispatcher().dispatch(ToolCallView {
                id: "return-fixture",
                name,
                args: &raw,
            }),
        )
        .await
        .unwrap()
        .map(|result| result.result.text_content())
    }

    #[tokio::test]
    async fn reopened_history_uses_fresh_host_admission_and_never_restores_submit() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        host.status(1);
        let (storage, first) = opened(root.path(), "return-fixture");
        let saved_reference = reference("return-fixture", &host.origin, 1);
        first.save(&saved_reference).unwrap();
        first
            .save(&reference("return-fixture", &host.origin, 2))
            .unwrap();
        drop(first);
        let frontend = host.frontend();
        let mut presenter = frontend.take_secret_entry_presenter().unwrap();
        let directory = crate::resources::reserve_setup_artifacts(&storage).unwrap();
        frontend
            .bind_recovery_artifacts(&storage.locator, &directory)
            .unwrap();
        let result = call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
            .await
            .unwrap();
        assert!(
            !result.contains(&host.origin)
                && !result.contains(saved_reference.review_path.as_ref().unwrap())
        );
        assert!(
            !result.contains(digest(2).as_str()),
            "not-found locators are not exposed"
        );
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["requests"].as_array().unwrap().len(), 1);
        assert_eq!(
            result["requests"][0]["current"]["result"]["state"],
            "applied"
        );
        assert!(frontend.tool.retained.lock().unwrap().prepared.is_empty());
        let raw = serde_json::value::RawValue::from_string(
            json!({"change_digest":digest(1)}).to_string(),
        )
        .unwrap();
        assert!(
            frontend
                .retain_action(ToolCallView {
                    id: "no-restored-approval",
                    name: PAIRED_HOST_FRONTEND_SUBMIT,
                    args: &raw
                })
                .is_err()
        );
        assert!(
            call(
                &frontend,
                PAIRED_HOST_FRONTEND_SUBMIT,
                json!({"change_digest":digest(1)})
            )
            .await
            .is_err()
        );
        let review = call(
            &frontend,
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":digest(1),"show_review":true}),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&review).unwrap()["review_presentation"],
            "queued"
        );
        let mut private = Vec::new();
        presenter.present_pending(&mut private).unwrap();
        assert!(
            String::from_utf8(private)
                .unwrap()
                .contains(saved_reference.review_path.as_ref().unwrap())
        );
        host.allowed.store(false, Ordering::SeqCst);
        assert!(
            call(
                &frontend,
                PAIRED_HOST_FRONTEND_STATUS,
                json!({"change_digest":digest(1),"show_review":true})
            )
            .await
            .is_err()
        );
        let mut refused_output = Vec::new();
        presenter.present_pending(&mut refused_output).unwrap();
        assert!(
            refused_output.is_empty(),
            "saved admission cannot bypass current host refusal"
        );
        let previous = call(
            &frontend,
            PAIRED_HOST_FRONTEND_PREVIOUS,
            json!({"restart":true}),
        )
        .await
        .unwrap();
        assert!(!previous.contains(digest(1).as_str()) && !previous.contains(&host.origin));
        assert!(
            serde_json::from_str::<Value>(&previous).unwrap()["requests"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        host.finish().await;
    }

    #[tokio::test]
    async fn denied_locator_does_not_hide_other_currently_admitted_requests() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        host.replies.lock().unwrap().insert(
            digest(1).as_str().into(),
            (
                403,
                "{\"error\":\"forbidden\",\"code\":\"console_owner_required\"}".into(),
            ),
        );
        host.status(2);
        let (storage, store) = opened(root.path(), "return-fixture");
        for index in 1..=2 {
            store
                .save(&reference("return-fixture", &host.origin, index))
                .unwrap();
        }
        let frontend = host.frontend();
        frontend
            .bind_recovery_artifacts(&storage.locator, &store.directory)
            .unwrap();
        let result = call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
            .await
            .unwrap();
        assert!(!result.contains(digest(1).as_str()) && !result.contains(&host.origin));
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["requests"].as_array().unwrap().len(), 1);
        assert_eq!(result["requests"][0]["change_digest"], digest(2).as_str());
        assert_eq!(host.calls.lock().unwrap().len(), 2);
        assert!(frontend.tool.retained.lock().unwrap().prepared.is_empty());
        host.finish().await;
    }

    #[tokio::test]
    async fn previous_call_never_performs_more_than_eight_status_reads() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        let (storage, store) = opened(root.path(), "return-fixture");
        for index in 0..19 {
            store
                .save(&reference("return-fixture", &host.origin, index))
                .unwrap();
            host.status(index);
        }
        let frontend = host.frontend();
        frontend
            .bind_recovery_artifacts(&storage.locator, &store.directory)
            .unwrap();
        let mut total = 0;
        for _ in 0..10 {
            let before = host.calls.lock().unwrap().len();
            let result: Value = serde_json::from_str(
                &call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(host.calls.lock().unwrap().len() - before <= PAGE_READS);
            total += result["requests"].as_array().unwrap().len();
            if result["more"] == false {
                break;
            }
        }
        assert_eq!(total, 19);
        assert_eq!(host.calls.lock().unwrap().len(), 19);
        host.finish().await;
    }

    #[tokio::test]
    async fn current_prepared_draft_can_redisplay_without_reviving_submission() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        host.replies
            .lock()
            .unwrap()
            .insert(digest(1).as_str().into(), (200, prepared(1)));
        let (storage, store) = opened(root.path(), "return-fixture");
        let saved = reference("return-fixture", &host.origin, 1);
        store.save(&saved).unwrap();
        let frontend = host.frontend();
        let mut presenter = frontend.take_secret_entry_presenter().unwrap();
        frontend
            .bind_recovery_artifacts(&storage.locator, &store.directory)
            .unwrap();
        let current = call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
            .await
            .unwrap();
        assert!(current.contains("\"threshold\":1e-06"));
        assert_eq!(
            serde_json::from_str::<Value>(&current).unwrap()["requests"][0]["current"]["outcome"],
            "prepared"
        );
        let status = call(
            &frontend,
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":digest(1),"show_review":true}),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&status).unwrap()["review_presentation"],
            "queued"
        );
        assert_eq!(
            host.calls.lock().unwrap().len(),
            2,
            "redisplay needs another exact current status read"
        );
        let mut private = Vec::new();
        presenter.present_pending(&mut private).unwrap();
        assert!(
            String::from_utf8(private)
                .unwrap()
                .contains(saved.review_path.as_ref().unwrap())
        );
        assert!(frontend.tool.retained.lock().unwrap().prepared.is_empty());
        assert!(
            call(
                &frontend,
                PAIRED_HOST_FRONTEND_SUBMIT,
                json!({"change_digest":digest(1)})
            )
            .await
            .is_err()
        );
        assert_eq!(
            host.calls.lock().unwrap().len(),
            2,
            "a disk reference cannot dispatch a submission"
        );
        host.finish().await;
    }

    #[tokio::test]
    async fn unavailable_artifacts_refuse_new_preparation_before_any_host_effect() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        let (storage, store) = opened(root.path(), "return-fixture");
        fs::set_permissions(&store.directory, fs::Permissions::from_mode(0o755)).unwrap();
        let frontend = host.frontend();
        assert!(
            frontend
                .bind_recovery_artifacts(&storage.locator, &store.directory)
                .is_err()
        );
        assert!(
            !frontend
                .allowed_tools()
                .contains(&PAIRED_HOST_FRONTEND_PREVIOUS)
        );
        let refused = call(&frontend, PAIRED_HOST_FRONTEND_PREPARE, json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("setup history"),
            "storage refusal must precede request parsing or host work"
        );
        assert!(host.calls.lock().unwrap().is_empty());
        assert!(frontend.tool.retained.lock().unwrap().prepared.is_empty());
        host.finish().await;
    }

    #[tokio::test]
    async fn failed_page_read_retries_every_unreturned_reference_with_fresh_admission() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        let (storage, store) = opened(root.path(), "return-fixture");
        for index in 0..11 {
            store
                .save(&reference("return-fixture", &host.origin, index))
                .unwrap();
            host.status(index);
        }
        let frontend = host.frontend();
        frontend
            .bind_recovery_artifacts(&storage.locator, &store.directory)
            .unwrap();
        *host.fail_at.lock().unwrap() = Some(2);
        assert!(
            call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
                .await
                .is_err()
        );
        let first_attempt = host.calls.lock().unwrap().clone();
        assert_eq!(first_attempt.len(), 2, "there is no hidden read retry");
        let mut found = BTreeSet::new();
        for _ in 0..10 {
            let before = host.calls.lock().unwrap().len();
            let result: Value = serde_json::from_str(
                &call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(host.calls.lock().unwrap().len() - before <= PAGE_READS);
            found.extend(
                result["requests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["change_digest"].as_str().unwrap().to_owned()),
            );
            if result["more"] == false {
                break;
            }
        }
        assert_eq!(
            found,
            (0..11)
                .map(|index| digest(index).as_str().to_owned())
                .collect()
        );
        {
            let calls = host.calls.lock().unwrap();
            assert_eq!(
                &calls[2..4],
                first_attempt.as_slice(),
                "even earlier admitted rows are freshly reread"
            );
            assert_eq!(calls.len(), 13);
        }
        host.finish().await;
    }

    #[tokio::test]
    async fn dropped_read_future_leaves_the_page_available_without_restart() {
        let root = tempfile::tempdir().unwrap();
        let host = Host::open().await;
        host.status(1);
        let (storage, store) = opened(root.path(), "return-fixture");
        store
            .save(&reference("return-fixture", &host.origin, 1))
            .unwrap();
        let frontend = host.frontend();
        frontend
            .bind_recovery_artifacts(&storage.locator, &store.directory)
            .unwrap();
        let captured = frontend.tool.host.captured().unwrap();
        let held_client = captured.client.lock().await;
        let raw = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let dispatcher = frontend.dispatcher();
        assert!(
            timeout(
                Duration::from_millis(20),
                dispatcher.dispatch(ToolCallView {
                    id: "dropped-return-read",
                    name: PAIRED_HOST_FRONTEND_PREVIOUS,
                    args: &raw,
                })
            )
            .await
            .is_err()
        );
        assert!(host.calls.lock().unwrap().is_empty());
        drop(held_client);
        let result: Value = serde_json::from_str(
            &call(&frontend, PAIRED_HOST_FRONTEND_PREVIOUS, json!({}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["requests"].as_array().unwrap().len(), 1);
        assert_eq!(host.calls.lock().unwrap().len(), 1);
        host.finish().await;
    }

    #[test]
    fn an_older_read_cannot_consume_a_restarted_page() {
        let root = tempfile::tempdir().unwrap();
        let (_, mut store) = opened(root.path(), "return-fixture");
        store
            .save(&reference("return-fixture", "https://host.invalid", 1))
            .unwrap();
        let older = store.read_page("https://host.invalid", false).unwrap();
        let restarted = store.read_page("https://host.invalid", true).unwrap();
        assert!(!Arc::ptr_eq(&older, &restarted));
        store.finish_page(&older);
        let still_pending = store.read_page("https://host.invalid", false).unwrap();
        assert!(Arc::ptr_eq(&restarted, &still_pending));
    }

    #[test]
    fn optional_recovery_does_not_extend_other_compositions_or_restore_a_draft() {
        let frontend =
            PairedHostFrontend::new(PairedHostClient::new("https://host.invalid").unwrap());
        assert!(
            !frontend
                .allowed_tools()
                .contains(&PAIRED_HOST_FRONTEND_PREVIOUS)
        );
        assert!(
            !frontend
                .dispatcher()
                .tools()
                .iter()
                .any(|tool| tool.name == PAIRED_HOST_FRONTEND_PREVIOUS)
        );
        let root = tempfile::tempdir().unwrap();
        let (storage, store) = opened(root.path(), "return-fixture");
        let preparation = PairedHostFrontend::private_preparation(
            PairedHostClient::new("https://host.invalid").unwrap(),
        );
        assert!(
            preparation
                .bind_recovery_artifacts(&storage.locator, &store.directory)
                .is_err()
        );
        assert!(
            !preparation
                .allowed_tools()
                .contains(&PAIRED_HOST_FRONTEND_PREVIOUS)
        );
        let reference = reference("return-fixture", "https://host.invalid", 1);
        let raw = prepared(1);
        let draft =
            serde_json::from_str::<crate::paired_host::FrontendPreparedDraft>(&raw).unwrap();
        let observed = FrontendChangeRead::Prepared(draft);
        assert!(reference.matches_observation(&observed));
        let serialized = serde_json::to_string(&Observed {
            change_digest: reference.change_digest.clone(),
            current: observed,
        })
        .unwrap();
        assert!(
            serialized.contains("\"threshold\":1e-06"),
            "opaque host numbers must remain exact"
        );
        assert!(frontend.tool.retained.lock().unwrap().prepared.is_empty());
        assert!(!reference.matches_observation(&FrontendChangeRead::NotFound));
        let crossed = FrontendChangeRead::Status(
            serde_json::from_value(
                json!({"state":"submitted","request_id":digest(2),"submitted_at":"fixture"}),
            )
            .unwrap(),
        );
        assert!(!reference.matches_observation(&crossed));
    }
}
