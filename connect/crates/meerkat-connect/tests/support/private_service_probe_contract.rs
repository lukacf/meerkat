//! Test-owned launch binding for the real private host entrypoint. This is not
//! a production authentication adapter or a Forge host policy file format.
use meerkat::governed_realm::stock::caller::StockAuthenticatedCaller;
use meerkat_authorization_contracts::{
    constraints::ResourceDomain,
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    resource::ResourceRef,
};
use meerkat_connect::service_probe::{
    ProbeHostBinding, RequiredServiceProbe, ServiceProbeSelection,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const OPERATION: &str = "get_top_hackernews_stories";
pub const REALM: &str = "cli-frontend";
pub const APP: &str = "private-service-probe-acceptance";
pub const VIEWER: &str = "fixture-human";
pub const LOGIN: &str = "fixture-login";
pub const ORIGIN: &str = "http://127.0.0.1:12345";
pub const INSTANCE: &str = "one-owned-process";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub root: PathBuf,
    pub expires_at_ms: u64,
}
impl Launch {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            expires_at_ms: now() + 900_000,
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
        json!({"protocol_version":6,"adapter":"services",
            "application":{"application_id":APP,"instance_id":INSTANCE,"origin":ORIGIN},
            "viewer":{"principal_id":VIEWER,"login_scope_id":LOGIN,"expires_at_ms":self.expires_at_ms},
            "realm":{"realm_id":REALM,"state_root":self.root.join("state"),"context_root":self.root.join("outside-checkout"),"user_config_root":self.root.join("home")},
            "model":"cli-fixture","services":{"resource_manifest_sha256":manifest,"preparation":"permitted"}})
    }
    pub fn selection(&self) -> Result<ServiceProbeSelection, Box<dyn std::error::Error>> {
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
        let mut profiles = Vec::new();
        for id in ["public-read", "after-retirement"] {
            profiles.push(RequiredServiceProbe::outbound_mcp(
                id.into(),
                "agno".into(),
                "hackernews".into(),
                "rkat-agent".into(),
                OPERATION.into(),
                json!({"num_stories":3}),
                json!({"service":"hackernews","operation":OPERATION,"count":3}),
                None,
            )?);
        }
        // Deliberately no semantic result contract: the native report must say
        // ToolCompleted, never CapabilityVerified. Parent checks public output
        // separately; this proves neither GHE/PAT nor future application access.
        Ok(ServiceProbeSelection::new(binding, profiles)?)
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
