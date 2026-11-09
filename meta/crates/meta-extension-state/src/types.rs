use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotFound,
    Conflict,
    RequestIdReused,
    AuthorityUnavailable,
    Invalid(String),
    Storage(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("document unavailable"),
            Self::Conflict => f.write_str("document revision changed"),
            Self::RequestIdReused => f.write_str("request id already used for a different request"),
            Self::AuthorityUnavailable => f.write_str("caller authority unavailable"),
            Self::Invalid(s) => write!(f, "invalid document request: {s}"),
            Self::Storage(s) => write!(f, "document storage failed: {s}"),
        }
    }
}
impl std::error::Error for Error {}

/// Maximum UTF-8 bytes in any identifier, including provider-issued document
/// IDs and revisions. Providers must enforce this before committing so tools
/// can reserve a finite receipt size when preflighting bounded responses.
pub const MAX_IDENTIFIER_BYTES: usize = 512;

pub(crate) fn identifier(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        Err(Error::Invalid("invalid identifier".into()))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Principal {
    Agent(String),
    Worker {
        mob_id: String,
        member_id: String,
        creation_id: String,
    },
}
impl Principal {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Agent(identity) => identifier(identity),
            Self::Worker {
                mob_id,
                member_id,
                creation_id,
            } => {
                identifier(mob_id)?;
                identifier(member_id)?;
                identifier(creation_id)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Reader,
    Editor,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    SelfOnly,
    #[default]
    Forks,
    Descendants,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Fork,
    Spawn,
}

/// Host-attested immutable creation edge, ordered from caller toward root.
/// Deliberately not deserializable from tool input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageLink {
    pub child: Principal,
    pub parent: Principal,
    pub kind: EdgeKind,
    pub ceiling: Option<Role>,
}

/// Construction requires a complete path to a runtime-attested root.
/// Legacy unknown ancestry must fail in the resolver instead of using root().
#[derive(Debug, Clone)]
pub struct VerifiedLineage {
    pub(crate) links: Vec<LineageLink>,
}
impl VerifiedLineage {
    pub fn root() -> Self {
        Self { links: Vec::new() }
    }
    pub fn derived(links: Vec<LineageLink>) -> Result<Self> {
        if links.is_empty() || links.len() > 128 {
            return Err(Error::AuthorityUnavailable);
        }
        let mut seen = BTreeSet::new();
        for (index, link) in links.iter().enumerate() {
            link.child.validate()?;
            link.parent.validate()?;
            if !seen.insert(link.child.clone()) || seen.contains(&link.parent) {
                return Err(Error::AuthorityUnavailable);
            }
            if index > 0 && links[index - 1].parent != link.child {
                return Err(Error::AuthorityUnavailable);
            }
        }
        Ok(Self { links })
    }
}

/// Current host ABAC and tool policy, already evaluated by the runtime adapter.
#[derive(Debug, Clone, Copy)]
pub struct HostPolicy {
    pub read: bool,
    pub edit: bool,
    pub manage: bool,
}
impl HostPolicy {
    pub const ALLOW: Self = Self {
        read: true,
        edit: true,
        manage: true,
    };
    pub const READ_ONLY: Self = Self {
        read: true,
        edit: false,
        manage: false,
    };
    pub const DENY: Self = Self {
        read: false,
        edit: false,
        manage: false,
    };
}

/// Trusted host input, intentionally without Deserialize. Host callers must
/// refresh these facts for every tool dispatch, including restored forks.
#[derive(Debug, Clone)]
pub struct HostAccessContext {
    pub(crate) realm: String,
    pub(crate) principal: Principal,
    pub(crate) lineage: VerifiedLineage,
    pub(crate) member_mobs: BTreeSet<String>,
    pub(crate) managed_mobs: BTreeSet<String>,
    pub(crate) realm_admin: bool,
    pub(crate) policy: HostPolicy,
    pub(crate) valid_owners: BTreeSet<Owner>,
    pub(crate) authority_generation: Option<u64>,
}
impl HostAccessContext {
    pub fn new(
        realm: impl Into<String>,
        principal: Principal,
        lineage: VerifiedLineage,
        policy: HostPolicy,
    ) -> Result<Self> {
        let realm = realm.into();
        identifier(&realm)?;
        principal.validate()?;
        if lineage
            .links
            .first()
            .is_some_and(|link| link.child != principal)
        {
            return Err(Error::AuthorityUnavailable);
        }
        Ok(Self {
            realm,
            principal,
            lineage,
            member_mobs: BTreeSet::new(),
            managed_mobs: BTreeSet::new(),
            realm_admin: false,
            policy,
            valid_owners: BTreeSet::new(),
            authority_generation: None,
        })
    }
    pub fn with_memberships(
        mut self,
        member_mobs: BTreeSet<String>,
        managed_mobs: BTreeSet<String>,
        realm_admin: bool,
    ) -> Result<Self> {
        for id in member_mobs.iter().chain(&managed_mobs) {
            identifier(id)?;
        }
        self.member_mobs = member_mobs;
        self.managed_mobs = managed_mobs;
        self.realm_admin = realm_admin;
        Ok(self)
    }
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    pub fn realm(&self) -> &str {
        &self.realm
    }
    /// The host has resolved these recipients in this realm's authoritative
    /// identity/member registry. Tool-supplied names alone are insufficient.
    pub fn with_valid_owners(mut self, owners: BTreeSet<Owner>) -> Self {
        self.valid_owners = owners;
        self
    }
    /// Stamp the host's authority generation this context was resolved at.
    /// A host that orders the changes of which mob holds an id (a create
    /// admitted under it, a destroy) against document writes compares the
    /// stamp when a write commits, and refuses a write that names a mob id
    /// changed after its caller was resolved: that caller was resolved
    /// against the mob that held the id before. Stores do not read it.
    pub fn with_authority_generation(mut self, generation: u64) -> Self {
        self.authority_generation = Some(generation);
        self
    }
    /// The generation [`Self::with_authority_generation`] stamped, if any.
    pub fn authority_generation(&self) -> Option<u64> {
        self.authority_generation
    }
    /// The mob ids this context's authority names: the mobs the caller is a
    /// member of, the mobs it manages, and the mob of every owner it was
    /// validated for ([`Self::with_valid_owners`]), a worker's mob included.
    /// A host that orders changes of which mob holds an id checks each of
    /// them against [`Self::authority_generation`].
    pub fn authority_mob_ids(&self) -> BTreeSet<&str> {
        let owners = self.valid_owners.iter().filter_map(|owner| match owner {
            Owner::Mob(id) => Some(id.as_str()),
            Owner::Agent(Principal::Worker { mob_id, .. }) => Some(mob_id.as_str()),
            Owner::Agent(Principal::Agent(_)) | Owner::Realm => None,
        });
        self.member_mobs
            .iter()
            .chain(&self.managed_mobs)
            .map(String::as_str)
            .chain(owners)
            .collect()
    }
    pub(crate) fn owner_is_valid(&self, owner: &Owner) -> bool {
        self.valid_owners.contains(owner)
            || match owner {
                Owner::Agent(principal) => principal == &self.principal,
                Owner::Mob(id) => self.managed_mobs.contains(id),
                Owner::Realm => self.realm_admin,
            }
    }
}

/// The holder of a mob id in a realm, as a host names it: the owner session
/// the mob under the id was created for, or the host itself for a mob
/// created with no owner session. Documents name a mob by its id alone
/// ([`Owner::Mob`], [`Audience::Mob`]), so a host records the holder each
/// time a mob takes or keeps an id
/// ([`crate::ExtensionDocumentStore::record_mob_id_holder`]) and compares it
/// when a persisted mob is restored: a mob of another holder would inherit
/// what documents written for the recorded one grant the id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MobIdHolder {
    /// A mob the host created with no owner session. Every such mob is the
    /// same holder.
    Host,
    /// A mob created for this owner session: an agent's own mob, its
    /// implicit delegation mob or a temporary council's mob.
    OwnerSession(String),
}
impl MobIdHolder {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Host => Ok(()),
            Self::OwnerSession(session) => identifier(session),
        }
    }
}
impl std::fmt::Display for MobIdHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host => f.write_str("the host"),
            Self::OwnerSession(session) => write!(f, "owner session {session}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Owner {
    Agent(Principal),
    Mob(String),
    Realm,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Audience {
    Agent {
        principal: Principal,
        #[serde(default)]
        reach: Reach,
    },
    Mob(String),
    Realm,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub audience: Audience,
    pub role: Role,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deny {
    pub audience: Audience,
    pub operation: DeniedOperation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeniedOperation {
    Read,
    Edit,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentAccess {
    #[serde(default)]
    pub owner_reach: Reach,
    #[serde(default)]
    pub grants: Vec<Grant>,
    #[serde(default)]
    pub denies: Vec<Deny>,
}
impl DocumentAccess {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.grants.len() + self.denies.len() > 256 {
            return Err(Error::Invalid("too many access entries".into()));
        }
        for audience in self
            .grants
            .iter()
            .map(|g| &g.audience)
            .chain(self.denies.iter().map(|d| &d.audience))
        {
            match audience {
                Audience::Agent { principal, .. } => principal.validate()?,
                Audience::Mob(id) => identifier(id)?,
                Audience::Realm => {}
            }
        }
        Ok(())
    }
}

pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentContent {
    pub title: String,
    pub content_type: String,
    pub schema_version: u32,
    pub payload: Vec<u8>,
}
impl DocumentContent {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.title.len() > 1024
            || self.payload.len() > MAX_PAYLOAD_BYTES
            || self.schema_version == 0
        {
            return Err(Error::Invalid(
                "content exceeds limits or has invalid schema version".into(),
            ));
        }
        identifier(&self.content_type)
    }
}
#[derive(Debug, Clone)]
pub struct NewDocument {
    pub content: DocumentContent,
    pub owner: Option<Owner>,
    pub access: DocumentAccess,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub revision: Revision,
    pub content: DocumentContent,
    pub owner: Owner,
    pub access: DocumentAccess,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
impl Document {
    /// Whether this document names the mob `mob_id`: as its owner
    /// ([`Owner::Mob`]) or in an audience entry ([`Audience::Mob`]), a grant
    /// or a deny. Ownership and audiences name a mob by its id alone, so a
    /// new mob under that id would be named the same way.
    pub fn names_mob(&self, mob_id: &str) -> bool {
        matches!(&self.owner, Owner::Mob(owner) if owner == mob_id)
            || self
                .access
                .grants
                .iter()
                .map(|grant| &grant.audience)
                .chain(self.access.denies.iter().map(|deny| &deny.audience))
                .any(|audience| matches!(audience, Audience::Mob(id) if id == mob_id))
    }
    /// Every mob id this document names, as its owner or in an audience
    /// grant or deny ([`Self::names_mob`]).
    pub(crate) fn named_mobs(&self) -> BTreeSet<&str> {
        let owner = match &self.owner {
            Owner::Mob(id) => Some(id.as_str()),
            Owner::Agent(_) | Owner::Realm => None,
        };
        let audiences = self
            .access
            .grants
            .iter()
            .map(|grant| &grant.audience)
            .chain(self.access.denies.iter().map(|deny| &deny.audience))
            .filter_map(|audience| match audience {
                Audience::Mob(id) => Some(id.as_str()),
                Audience::Agent { .. } | Audience::Realm => None,
            });
        owner.into_iter().chain(audiences).collect()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub id: DocumentId,
    pub revision: Revision,
    pub title: String,
    pub content_type: String,
    pub schema_version: u32,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
impl From<&Document> for DocumentSummary {
    fn from(doc: &Document) -> Self {
        Self {
            id: doc.id.clone(),
            revision: doc.revision.clone(),
            title: doc.content.title.clone(),
            content_type: doc.content.content_type.clone(),
            schema_version: doc.content.schema_version,
            created_at_ms: doc.created_at_ms,
            updated_at_ms: doc.updated_at_ms,
        }
    }
}
#[derive(Debug, Clone)]
pub struct ListRequest {
    pub limit: usize,
    pub after: Option<DocumentId>,
}
impl Default for ListRequest {
    fn default() -> Self {
        Self {
            limit: 50,
            after: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentPage {
    pub documents: Vec<DocumentSummary>,
    pub next: Option<DocumentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentScope {
    realm: String,
    namespace: String,
}
impl DocumentScope {
    pub fn new(realm: impl Into<String>, namespace: impl Into<String>) -> Result<Self> {
        let realm = realm.into();
        let namespace = namespace.into();
        identifier(&realm)?;
        identifier(&namespace)?;
        Ok(Self { realm, namespace })
    }
    pub fn realm(&self) -> &str {
        &self.realm
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub(crate) fn validate(&self, caller: &HostAccessContext) -> Result<()> {
        if self.realm == caller.realm {
            Ok(())
        } else {
            Err(Error::NotFound)
        }
    }
}

/// Fingerprint the normalized original action, before state reads/evaluation.
/// The host adapter must never fingerprint the newly calculated payload.
#[derive(Debug, Clone)]
pub struct RequestIdentity {
    pub(crate) id: String,
    pub(crate) fingerprint: Vec<u8>,
}
impl RequestIdentity {
    pub fn new(id: impl Into<String>, canonical_original_request: &[u8]) -> Result<Self> {
        let id = id.into();
        identifier(&id)?;
        if canonical_original_request.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::Invalid("request too large".into()));
        }
        Ok(Self {
            id,
            fingerprint: Sha256::digest(canonical_original_request).to_vec(),
        })
    }
}
#[derive(Debug, Clone)]
pub enum Mutation {
    Create(NewDocument),
    Replace {
        id: DocumentId,
        expected_revision: Revision,
        content: DocumentContent,
    },
    SetAccess {
        id: DocumentId,
        expected_revision: Revision,
        access: DocumentAccess,
    },
    Transfer {
        id: DocumentId,
        expected_revision: Revision,
        owner: Owner,
        access: Option<DocumentAccess>,
    },
    Delete {
        id: DocumentId,
        expected_revision: Revision,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationReceipt {
    pub document_id: DocumentId,
    pub revision: Revision,
    pub deleted: bool,
}

/// Whether this call committed or replayed a prior commit. Adapters must not
/// attach newly calculated cell results to a replayed receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    pub receipt: MutationReceipt,
    pub replayed: bool,
}
