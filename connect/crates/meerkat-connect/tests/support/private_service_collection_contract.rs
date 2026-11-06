//! Test-owned launch binding for the real private host entrypoint. This is not
//! a production authentication adapter or a Forge host policy file format.
use meerkat::governed_realm::stock::caller::StockAuthenticatedCaller;
use meerkat_authorization_contracts::{
    constraints::ResourceDomain,
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    resource::ResourceRef,
};
use meerkat_connect::service_collection::{
    CollectionCall, CollectionCursor, CollectionLimits, CollectionProgram, CollectionStep,
    CollectionTarget, CommittedCollectionResult, RequiredCollectionOperation,
    RequiredServiceCollection, ServiceCollectionSelection,
};
use meerkat_connect::service_probe::{ProbeHostBinding, ResourceAssessment};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::sync::Arc;
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const OPERATION: &str = "get_top_hackernews_stories";
pub const USER_OPERATION: &str = "get_user_details";
pub const SOURCE: &str = "public-top-author";
pub const SECOND_SOURCE: &str = "second-public-top-author";
pub const FIRST_SERVICE: &str = "first-hackernews";
pub const SECOND_SERVICE: &str = "second-hackernews";
pub const REQUIRED: &str = "top-story-author-details";
pub const REALM: &str = "cli-frontend";
pub const APP: &str = "private-service-collection-acceptance";
pub const VIEWER: &str = "fixture-human";
pub const LOGIN: &str = "fixture-login";
pub const ORIGIN: &str = "http://127.0.0.1:12345";
pub const INSTANCE: &str = "one-owned-process";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub root: PathBuf,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub multiple_services: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Launch {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            expires_at_ms: now() + 900_000,
            multiple_services: false,
        }
    }
    pub fn for_services(root: &Path) -> Self {
        Self {
            multiple_services: true,
            ..Self::new(root)
        }
    }
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::File::open(path.parent().unwrap())?.sync_all()
    }
    pub fn read(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !path.is_absolute()
            || !metadata.is_file()
            || metadata.len() > 4096
            || metadata.mode() & 0o777 != 0o600
            || metadata.uid() != nix::unistd::geteuid().as_raw()
        {
            return Err(io::Error::other("invalid acceptance binding"));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(path)?;
        let opened = file.metadata()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(io::Error::other("changed acceptance binding"));
        }
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        let value: Self = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::other("invalid acceptance binding"))?;
        if !value.root.is_absolute()
            || !value.root.is_dir()
            || value.expires_at_ms <= now()
            || value.expires_at_ms > now() + 900_000
            || path.parent() != Some(value.root.as_path())
        {
            return Err(io::Error::other("invalid acceptance binding"));
        }
        Ok(value)
    }
    pub fn initialize(&self, manifest: &str) -> Value {
        json!({"protocol_version":7,"adapter":"services",
            "application":{"application_id":APP,"instance_id":INSTANCE,"origin":ORIGIN},
            "viewer":{"principal_id":VIEWER,"login_scope_id":LOGIN,"expires_at_ms":self.expires_at_ms},
            "realm":{"realm_id":REALM,"state_root":self.root.join("state"),"context_root":self.root.join("outside-checkout"),"user_config_root":self.root.join("home")},
            "model":"cli-fixture","services":{"resource_manifest_sha256":manifest,"preparation":"permitted"}})
    }
    pub fn selection(&self) -> Result<ServiceCollectionSelection, Box<dyn std::error::Error>> {
        let storage = meerkat_connect::RealmOptions {
            context_root: self.root.join("outside-checkout"),
            state_root: Some(self.root.join("state")),
            user_config_root: Some(self.root.join("home")),
        }
        .resolve(REALM)?;
        let domain = TrustDomainId::new("private-service-acceptance")?;
        let caller = StockAuthenticatedCaller::new(
            PrincipalRef::in_domain(PrincipalKind::Human, VIEWER, domain.clone())?,
            HistoricalEvidenceRef {
                resource: ResourceRef {
                    domain: ResourceDomain {
                        authority: PrincipalRef::in_domain(
                            PrincipalKind::RuntimeHost,
                            "test-owner",
                            domain.clone(),
                        )?,
                        namespace: "test-login".into(),
                    },
                    resource_id: LOGIN.into(),
                },
                revision: EvidenceId::new("one")?,
                digest: EvidenceDigest::of_bytes(&serde_json::to_vec(self)?),
            },
        )?;
        let binding = ProbeHostBinding {
            application_id: APP.into(),
            instance_id: INSTANCE.into(),
            origin: ORIGIN.into(),
            viewer_id: VIEWER.into(),
            login_scope_id: LOGIN.into(),
            expires_at_ms: self.expires_at_ms,
            realm: storage.locator,
            context_root: self.root.join("outside-checkout"),
            user_config_root: self.root.join("home"),
            caller,
            ingress: PrincipalRef::in_domain(PrincipalKind::RuntimeHost, "private-pipe", domain)?,
        };
        if self.multiple_services {
            let observed = Arc::new(std::sync::Mutex::new(None));
            let mut services = Vec::new();
            for (service, source, first) in [
                (FIRST_SERVICE, SOURCE, true),
                (SECOND_SERVICE, SECOND_SOURCE, false),
            ] {
                let program = Arc::new(SameInvocationAuthor {
                    first,
                    observed: observed.clone(),
                });
                let row = RequiredCollectionOperation::new(
                    REQUIRED.into(),
                    resource(),
                    vec![OPERATION.into(), USER_OPERATION.into()],
                    program,
                )?;
                let collection = RequiredServiceCollection::new(
                    source.into(),
                    "agno".into(),
                    "hackernews".into(),
                    "rkat-agent".into(),
                    vec![row],
                    CollectionLimits::new(2, 8192, 2 * 1024 * 1024)?,
                )?;
                services.push((service.into(), vec![collection]));
            }
            return Ok(ServiceCollectionSelection::for_services(binding, services)?);
        }
        let mut collections = Vec::new();
        for id in [SOURCE, "after-retirement"] {
            let row = RequiredCollectionOperation::new(
                REQUIRED.into(),
                resource(),
                vec![OPERATION.into(), USER_OPERATION.into()],
                Arc::new(TopAuthor),
            )?;
            collections.push(RequiredServiceCollection::new(
                id.into(),
                "agno".into(),
                "hackernews".into(),
                "rkat-agent".into(),
                vec![row],
                CollectionLimits::new(2, 8192, 2 * 1024 * 1024)?,
            )?);
        }
        Ok(ServiceCollectionSelection::new(binding, collections)?)
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// This public-service resource is deliberately not a GHE repository or Slack
/// conversation. Verification is historical and only for this exact sequence.
pub fn resource() -> Value {
    json!({"service":"hackernews","requirement":"details_of_first_top_story_author","top_stories":3})
}
pub fn decode_pair(content: &str) -> Result<Value, ()> {
    let rows: Vec<Value> = serde_json::Deserializer::from_str(content)
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|_| ())?;
    if rows.len() != 2 || rows[1].as_object().is_none_or(|m| m.len() != 1) {
        return Err(());
    }
    let mirrored: Value =
        serde_json::from_str(rows[1]["value"].as_str().ok_or(())?).map_err(|_| ())?;
    if mirrored != rows[0] {
        return Err(());
    }
    Ok(mirrored)
}
pub fn author_from_stories(value: &Value) -> Result<String, ()> {
    let rows = value.as_array().ok_or(())?;
    let mut ids = std::collections::BTreeSet::new();
    if rows.len() != 3 {
        return Err(());
    }
    for row in rows {
        let id = row["id"].as_u64().filter(|id| *id > 0).ok_or(())?;
        if !ids.insert(id) || row["title"].as_str().is_none_or(|s| s.trim().is_empty()) {
            return Err(());
        }
    }
    let author = rows[0]["username"].as_str().ok_or(())?;
    if author.is_empty()
        || author.len() > 64
        || !author
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || rows[0]["by"] != author
    {
        return Err(());
    }
    Ok(author.into())
}
pub fn exact_user(value: &Value, author: &str) -> bool {
    value["id"] == author
        && value["karma"].as_i64().is_some()
        && value["total_items_submitted"].as_u64().is_some()
        && (value["about"].is_null() || value["about"].is_string())
        && value.as_object().is_some_and(|m| m.len() == 4)
}
struct TopAuthor;
struct Cursor {
    author: Option<String>,
    finished: bool,
}
impl CollectionProgram for TopAuthor {
    fn accepts_target(&self, target: &CollectionTarget<'_>) -> bool {
        // Ready independently validates the real checked Agno/hackernews job,
        // exact configuration bytes, grant and selected raw operation on every
        // call. That selected package owns the fixed public Firebase URLs.
        matches!(
            target.source_id(),
            SOURCE | SECOND_SOURCE | "after-retirement"
        ) && target.required_operation_id() == REQUIRED
            && target.resource() == &resource()
            && target.raw_operation_ceiling()
                == &[OPERATION.to_owned(), USER_OPERATION.to_owned()]
                    .into_iter()
                    .collect()
            && matches!(
                &target.installed_connection().transport,
                meerkat_core::mcp_config::McpTransportConfig::Stdio(_)
            )
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        Box::new(Cursor {
            author: None,
            finished: false,
        })
    }
}
impl CollectionCursor for Cursor {
    fn begin(&mut self) -> CollectionStep {
        if self.author.is_some() || self.finished {
            return CollectionStep::Refuse;
        }
        CollectionStep::Call(
            CollectionCall::new(OPERATION.into(), json!({"num_stories":3}))
                .expect("fixed nonsecret arguments"),
        )
    }
    fn observe(&mut self, observed: &CommittedCollectionResult<'_>) -> CollectionStep {
        if self.finished || !TopAuthor.accepts_target(observed.target()) {
            return CollectionStep::Refuse;
        }
        let Ok(value) = decode_pair(&observed.result().text_content()) else {
            return CollectionStep::Finish(ResourceAssessment::Unknown);
        };
        match &self.author {
            None => {
                if observed.operation() != OPERATION
                    || observed.arguments() != &json!({"num_stories":3})
                {
                    return CollectionStep::Refuse;
                }
                let Ok(author) = author_from_stories(&value) else {
                    return CollectionStep::Finish(ResourceAssessment::NotVerified);
                };
                self.author = Some(author.clone());
                CollectionStep::Call(
                    CollectionCall::new(USER_OPERATION.into(), json!({"username":author}))
                        .expect("schema-validated nonsecret author"),
                )
            }
            Some(author) => {
                self.finished = true;
                if observed.operation() != USER_OPERATION
                    || observed.arguments() != &json!({"username":author})
                {
                    return CollectionStep::Refuse;
                }
                CollectionStep::Finish(if exact_user(&value, author) {
                    ResourceAssessment::Verified
                } else {
                    ResourceAssessment::NotVerified
                })
            }
        }
    }
}

// Only a real committed result can create this handle. The second service's
// program refuses before evaluating a body unless it shares the first owner.
struct SameInvocationAuthor {
    first: bool,
    observed: Arc<
        std::sync::Mutex<
            Option<meerkat_connect::service_collection::CollectionInvocationObservation>,
        >,
    >,
}
struct SameInvocationCursor {
    program: SameInvocationAuthor,
    inner: Box<dyn CollectionCursor>,
}
impl CollectionProgram for SameInvocationAuthor {
    fn accepts_target(&self, target: &CollectionTarget<'_>) -> bool {
        target.source_id() == (if self.first { SOURCE } else { SECOND_SOURCE })
            && TopAuthor.accepts_target(target)
    }
    fn start(&self, target: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        Box::new(SameInvocationCursor {
            program: Self {
                first: self.first,
                observed: self.observed.clone(),
            },
            inner: TopAuthor.start(target),
        })
    }
}
impl CollectionCursor for SameInvocationCursor {
    fn begin(&mut self) -> CollectionStep {
        self.inner.begin()
    }
    fn observe(&mut self, result: &CommittedCollectionResult<'_>) -> CollectionStep {
        if !self.program.accepts_target(result.target()) {
            return CollectionStep::Refuse;
        }
        let Ok(mut previous) = self.program.observed.lock() else {
            return CollectionStep::Refuse;
        };
        let current = result.invocation_observation();
        match previous.as_ref() {
            Some(first) if !first.same_invocation(&current) => return CollectionStep::Refuse,
            None if !self.program.first => return CollectionStep::Refuse,
            None => *previous = Some(current),
            Some(_) => {}
        }
        drop(previous);
        self.inner.observe(result)
    }
}
