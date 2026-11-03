//! Durable agent-memory record model.
//!
//! Implements docs/design/agent-memory-architecture.md §7.1 (record model),
//! §7.2 (scopes — all realm-keyed) and the deterministic halves of §10.2
//! (trust-tier transition lattice as pure functions). Everything here is
//! structure, not judgment: types, caps, ordering, hashing. LLM stages write
//! into this model exclusively through validated staged mutations
//! (`crate::memory::staged`).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable, content-independent record identifier.
pub type MemoryId = String;

/// Identifier for a pending mob/operator-scope proposal (§7.3 `propose`).
pub type ProposalId = String;

/// Byte caps (§7.1). Deterministic write-time guards, enforced by the staged
/// validator and the bundled store.
pub const MAX_RECORD_TITLE_BYTES: usize = 200;
pub const MAX_RECORD_DESCRIPTION_BYTES: usize = 400;
pub const MAX_RECORD_BODY_BYTES: usize = 64 * 1024;

/// Memory scope (§7.2). **Every scope is realm-keyed**: realms are the
/// platform's isolation boundary, and memory is state. `Operator` is part of
/// the P0 schema (no migration later) but populates only from P4.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum MemoryScope {
    Identity { realm: String, identity: String },
    Mob { realm: String, mob: String },
    Operator { realm: String, operator: String },
    Realm { realm: String },
}

impl MemoryScope {
    pub fn realm(&self) -> &str {
        match self {
            Self::Identity { realm, .. }
            | Self::Mob { realm, .. }
            | Self::Operator { realm, .. }
            | Self::Realm { realm } => realm,
        }
    }

    /// Stable discriminant used for storage columns and audit rows.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Identity { .. } => "identity",
            Self::Mob { .. } => "mob",
            Self::Operator { .. } => "operator",
            Self::Realm { .. } => "realm",
        }
    }

    /// The non-realm scope key (`""` for realm scope).
    pub fn key(&self) -> &str {
        match self {
            Self::Identity { identity, .. } => identity,
            Self::Mob { mob, .. } => mob,
            Self::Operator { operator, .. } => operator,
            Self::Realm { .. } => "",
        }
    }

    /// This scope as the typed memory operation targets of Meta's memory
    /// tools name it. A host's operation policy maps it to its own action,
    /// and a targeted grant selects one lineage for exactly this scope.
    ///
    /// The format is a stored contract and never changes: work
    /// associations persist the reference as the grant target of memory
    /// operations (`GrantTarget::Memory { scope }`), and host policies key on
    /// it. The reference is `MemoryScopeRef::Host { owner, scope }`, where
    /// `owner` is [`AGENT_MEMORY_TARGET_OWNER`] (`meta.agent_memory`) and
    /// `scope` is `<kind>/<realm>/<key>`:
    ///
    /// - `<kind>` is [`Self::kind_str`]: `identity`, `mob`, `operator` or
    ///   `realm`;
    /// - `<realm>` is the scope's realm, and `<key>` is [`Self::key`]: the
    ///   identity, mob or operator name, empty for a realm scope (which
    ///   therefore ends with `/`);
    /// - in `<realm>` and `<key>`, every `%` is written `%25` and every `/`
    ///   is written `%2F`, and every other character is kept as it is.
    ///
    /// The escaping leaves the two separators as the only `/` in the
    /// reference, so each reference decodes back to exactly one scope and
    /// two scopes never share a reference. For example, identity
    /// `agent:alice` in realm `default` is `identity/default/agent:alice`,
    /// mob `team/a` in realm `default` is `mob/default/team%2Fa`, and realm
    /// `default` itself is `realm/default/`.
    pub fn operation_scope_ref(&self) -> meerkat_core::MemoryScopeRef {
        meerkat_core::MemoryScopeRef::Host {
            owner: AGENT_MEMORY_TARGET_OWNER.to_string(),
            scope: format!(
                "{}/{}/{}",
                self.kind_str(),
                escape_scope_ref_part(self.realm()),
                escape_scope_ref_part(self.key()),
            ),
        }
    }
}

/// The stable name Meta's agent memory carries as the owner of every typed
/// memory operation target its tools produce
/// ([`MemoryScope::operation_scope_ref`]). Part of that stored contract: it
/// never changes.
pub const AGENT_MEMORY_TARGET_OWNER: &str = "meta.agent_memory";

/// Escapes one realm or key for [`MemoryScope::operation_scope_ref`]: `%` as
/// `%25` and `/` as `%2F`, every other character unchanged.
fn escape_scope_ref_part(part: &str) -> String {
    let mut escaped = String::with_capacity(part.len());
    for ch in part.chars() {
        match ch {
            '%' => escaped.push_str("%25"),
            '/' => escaped.push_str("%2F"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Closed record taxonomy (§7.1). `OpenLoop` is the prospective-memory kind:
/// unfinished intentions with an explicit resolution condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Preference,
    Fact,
    Gotcha,
    Procedure,
    Relationship,
    OpenLoop,
    Reference,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Fact => "fact",
            Self::Gotcha => "gotcha",
            Self::Procedure => "procedure",
            Self::Relationship => "relationship",
            Self::OpenLoop => "open_loop",
            Self::Reference => "reference",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "preference" => Some(Self::Preference),
            "fact" => Some(Self::Fact),
            "gotcha" => Some(Self::Gotcha),
            "procedure" => Some(Self::Procedure),
            "relationship" => Some(Self::Relationship),
            "open_loop" => Some(Self::OpenLoop),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// Trust tier (§7.1, §10.2). Variant order is authority order:
/// `Untrusted < AgentObserved < AgentVerified < Application < Operator`,
/// so `derive(Ord)` gives the lattice's comparison for free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Untrusted,
    AgentObserved,
    AgentVerified,
    Application,
    Operator,
}

impl TrustTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::AgentObserved => "agent_observed",
            Self::AgentVerified => "agent_verified",
            Self::Application => "application",
            Self::Operator => "operator",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "untrusted" => Some(Self::Untrusted),
            "agent_observed" => Some(Self::AgentObserved),
            "agent_verified" => Some(Self::AgentVerified),
            "application" => Some(Self::Application),
            "operator" => Some(Self::Operator),
            _ => None,
        }
    }

    /// §10.2: `Operator` and `Application` tiers are assignable only by
    /// non-LLM principals through direct (non-staged) surfaces — never via
    /// any `StagedMutationBatch`.
    pub fn assignable_via_staged_batch(&self) -> bool {
        !matches!(self, Self::Operator | Self::Application)
    }

    /// §10.2: all LLM-authored writes enter at `AgentObserved` or below.
    pub fn llm_write_ceiling() -> Self {
        Self::AgentObserved
    }

    /// §10.2 transitive-provenance ceiling: a record whose evidence or
    /// supersede/derivation chain reaches `Untrusted`/quarantined provenance
    /// is capped at `AgentObserved` forever.
    pub fn capped_for_tainted_provenance(self) -> Self {
        self.min(Self::AgentObserved)
    }
}

/// Record lifecycle status (§7.1). Superseded records stay retrievable with
/// provenance; only `Active` records are injected or recalled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RecordStatus {
    Active,
    Superseded { by: MemoryId },
    Quarantined { reason: String },
    Tombstoned,
}

impl RecordStatus {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded { .. } => "superseded",
            Self::Quarantined { .. } => "quarantined",
            Self::Tombstoned => "tombstoned",
        }
    }
}

/// Who authored a record (§7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "author", rename_all = "snake_case")]
pub enum MemoryAuthor {
    Operator,
    Application,
    Agent { identity: String },
    Steward { run_id: String },
    Distiller { run_id: String },
}

impl MemoryAuthor {
    /// §10.2 splits the lattice on this: LLM authors are tier-ceilinged.
    pub fn is_llm(&self) -> bool {
        matches!(
            self,
            Self::Agent { .. } | Self::Steward { .. } | Self::Distiller { .. }
        )
    }
}

/// Provenance pointer into immutable session evidence (§7.1).
///
/// `revision` is the content-addressed transcript revision that was head at
/// capture time. It is `Option` until the Hygienist (transcript revisions,
/// P4) lands — `None` means "head at capture time; revision pinning not yet
/// available", not "unknown provenance".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub session_id: String,
    /// Continuity generation — fresh-start (`reset`) boundaries are
    /// first-class; session→generation is unrecoverable after reset without
    /// this.
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Message range within the pinned revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(u64, u64)>,
}

/// Calibration profile reference (§11). All strings for now: the calibration
/// harness (P1) defines the artifact family these point into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationRef {
    pub stage: String,
    pub bundle: String,
    pub version: String,
    pub model: String,
}

/// Agent-cited evidence of verification — a CLAIM, not a tier (§8.2). The
/// tier upgrade to `AgentVerified` is a steward-only staged operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationClaim {
    /// What was checked, in the author's words.
    pub checked: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProvenance {
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    pub author: MemoryAuthor,
    /// §7.1 models this as required; it is `Option` until calibration
    /// profiles exist (P1) — imports and pre-calibration writes carry `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<CalibrationRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationClaim>,
}

/// Usage ledger counters (§9.2). Deterministic side only; judged-useful
/// verdicts come from the steward's usage audit (P3). Ambient injection and
/// explicit recall are counted distinctly: the steward's usage audit treats
/// "pushed and ignored" very differently from "pulled on purpose".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageStats {
    #[serde(default)]
    pub injected_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_injected_at_ms: Option<u64>,
    #[serde(default)]
    pub explicit_recall_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recalled_at_ms: Option<u64>,
    #[serde(default)]
    pub judged_useful_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_useful_at_ms: Option<u64>,
}

/// Mechanical usage events (§9.2). `JudgedUseful` is reserved for the
/// steward's audit verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageEvent {
    Injected,
    ExplicitRecall,
    JudgedUseful,
}

/// Which injection surface delivered a record into context (§9.1 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionSurface {
    /// Build-time assembly (`customize_build` → system prompt).
    Build,
    /// Ambient per-turn injection (opt-in `budgeted` mode).
    Turn,
}

impl InjectionSurface {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Turn => "turn",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "build" => Some(Self::Build),
            "turn" => Some(Self::Turn),
            _ => None,
        }
    }
}

/// One injection-ledger row (§9.2): which record entered whose context,
/// through which surface, when. Telemetry, not record mutation — rows are
/// plain appends, never staged. The session key is `None` for build-time
/// assembly, where the session does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionLogEntry {
    pub record_id: MemoryId,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    pub surface: InjectionSurface,
    pub at_ms: u64,
}

/// The full record model (§7.1).
///
/// Two additions beyond the §7.1 field list, both deliberate:
/// - `tags`: wire-compat with the markdown-era `AgentMemoryRecord`
///   projection (recall keeps tags); a legacy retrieval surface, not part of
///   the hub-compatible core.
/// - `derived_from`: consolidation lineage. Without it the §10.2 transitive
///   ceiling cannot see a merge — "laundering by consolidation is a
///   validator reject" requires the merge edge to be recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub title: String,
    /// Written for retrieval ranking; this line is the retrieval contract.
    #[serde(default)]
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub provenance: MemoryProvenance,
    pub trust: TrustTier,
    pub status: RecordStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<MemoryId>,
    #[serde(default)]
    pub derived_from: Vec<MemoryId>,
    /// Steward-maintained recall ordering (§8.3); superseding records
    /// inherit the prior record's rank until the next dream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_set_rank: Option<u32>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub usage: UsageStats,
    /// §10.2 durable taint marker: the record landed quarantined, or
    /// descends from one that did. Unlike `status`, it survives the
    /// tombstone a review or forget applies, so read surfaces can keep a
    /// quarantined record's content reviewer-only for good.
    #[serde(default)]
    pub ever_quarantined: bool,
}

/// Payload for creating or superseding a record (§7.3 `NewRecord`).
/// Authorship comes from the surrounding batch/call context, never from the
/// payload itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMemoryRecord {
    pub kind: MemoryKind,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationClaim>,
}

/// Manifest row (§7.3): id+kind+title+description+age+rank. Deliberately
/// body-free — the manifest is an index, never a dump.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordMeta {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub age_days: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
}

/// Manifest tiers (§8.3). `WorkingSet(k)` = top-K ranked ∪ recent/unranked
/// slice (newest-first), union capped at `2*k`; `Full` = every active
/// record's metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestTier {
    WorkingSet(usize),
    Full,
}

/// Exact content hash used by the write-time dedup guard and the
/// tombstone-recreation check. Length-prefixed so `("ab","c")` and
/// `("a","bc")` differ. Uses sha2, which the crate already depends on.
pub fn content_hash(title: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((title.len() as u64).to_le_bytes());
    hasher.update(title.as_bytes());
    hasher.update(body.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Deterministic field caps (§7.1). Description may be empty (the Recorder
/// that writes retrieval-facing descriptions lands in P1).
pub fn validate_record_fields(title: &str, description: &str, body: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("title must not be empty".to_string());
    }
    if title.len() > MAX_RECORD_TITLE_BYTES {
        return Err(format!(
            "title must be at most {MAX_RECORD_TITLE_BYTES} bytes"
        ));
    }
    if description.len() > MAX_RECORD_DESCRIPTION_BYTES {
        return Err(format!(
            "description must be at most {MAX_RECORD_DESCRIPTION_BYTES} bytes"
        ));
    }
    if body.trim().is_empty() {
        return Err("body must not be empty".to_string());
    }
    if body.len() > MAX_RECORD_BODY_BYTES {
        return Err(format!(
            "body must be at most {MAX_RECORD_BODY_BYTES} bytes"
        ));
    }
    Ok(())
}

/// Age in whole days, saturating (clock skew must not underflow).
pub fn age_days(updated_at_ms: u64, now_ms: u64) -> u64 {
    now_ms.saturating_sub(updated_at_ms) / (24 * 60 * 60 * 1000)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn trust_tier_order_matches_lattice_authority() {
        assert!(TrustTier::Untrusted < TrustTier::AgentObserved);
        assert!(TrustTier::AgentObserved < TrustTier::AgentVerified);
        assert!(TrustTier::AgentVerified < TrustTier::Application);
        assert!(TrustTier::Application < TrustTier::Operator);
    }

    #[test]
    fn operator_and_application_tiers_never_staged_assignable() {
        assert!(!TrustTier::Operator.assignable_via_staged_batch());
        assert!(!TrustTier::Application.assignable_via_staged_batch());
        assert!(TrustTier::AgentVerified.assignable_via_staged_batch());
        assert!(TrustTier::AgentObserved.assignable_via_staged_batch());
        assert!(TrustTier::Untrusted.assignable_via_staged_batch());
    }

    #[test]
    fn tainted_provenance_caps_at_agent_observed() {
        assert_eq!(
            TrustTier::AgentVerified.capped_for_tainted_provenance(),
            TrustTier::AgentObserved
        );
        assert_eq!(
            TrustTier::Operator.capped_for_tainted_provenance(),
            TrustTier::AgentObserved
        );
        assert_eq!(
            TrustTier::Untrusted.capped_for_tainted_provenance(),
            TrustTier::Untrusted
        );
    }

    #[test]
    fn content_hash_is_stable_and_boundary_safe() {
        assert_eq!(content_hash("a", "b"), content_hash("a", "b"));
        assert_ne!(content_hash("ab", "c"), content_hash("a", "bc"));
        assert_eq!(content_hash("t", "b").len(), 64);
    }

    /// Typed memory targets name Meta's memory owner and a scope reference
    /// that keeps every scope distinct: the kind, realm and key never run
    /// into each other, whatever separators the names contain.
    #[test]
    fn operation_scope_refs_are_owned_by_meta_memory_and_never_collide() {
        let host = |scope: &str| meerkat_core::MemoryScopeRef::Host {
            owner: AGENT_MEMORY_TARGET_OWNER.to_string(),
            scope: scope.to_string(),
        };
        let identity = |realm: &str, identity: &str| MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity.to_string(),
        };
        let cases = [
            (
                identity("family", "identity:luka"),
                "identity/family/identity:luka",
            ),
            (
                MemoryScope::Mob {
                    realm: "family".to_string(),
                    mob: "identity:luka".to_string(),
                },
                "mob/family/identity:luka",
            ),
            (
                MemoryScope::Operator {
                    realm: "family".to_string(),
                    operator: "ops".to_string(),
                },
                "operator/family/ops",
            ),
            (
                MemoryScope::Realm {
                    realm: "family".to_string(),
                },
                "realm/family/",
            ),
            (identity("a/b", "c"), "identity/a%2Fb/c"),
            (identity("a", "b/c"), "identity/a/b%2Fc"),
            (identity("a%2Fb", "c"), "identity/a%252Fb/c"),
        ];
        for (scope, expected) in &cases {
            assert_eq!(scope.operation_scope_ref(), host(expected), "{scope:?}");
            assert!(scope.operation_scope_ref().is_named());
        }
        let refs: std::collections::BTreeSet<String> = cases
            .iter()
            .map(|(_, expected)| (*expected).to_string())
            .collect();
        assert_eq!(refs.len(), cases.len(), "every scope has its own reference");
        assert_eq!(AGENT_MEMORY_TARGET_OWNER, "meta.agent_memory");
    }

    /// Decodes a reference exactly as [`MemoryScope::operation_scope_ref`]
    /// documents its stored format, refusing anything else.
    fn decode_operation_scope_ref(reference: &meerkat_core::MemoryScopeRef) -> Option<MemoryScope> {
        let meerkat_core::MemoryScopeRef::Host { owner, scope } = reference else {
            return None;
        };
        if owner != AGENT_MEMORY_TARGET_OWNER {
            return None;
        }
        let mut parts = scope.split('/');
        let (Some(kind), Some(realm), Some(key), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let realm = unescape_scope_ref_part(realm)?;
        let key = unescape_scope_ref_part(key)?;
        match kind {
            "identity" => Some(MemoryScope::Identity {
                realm,
                identity: key,
            }),
            "mob" => Some(MemoryScope::Mob { realm, mob: key }),
            "operator" => Some(MemoryScope::Operator {
                realm,
                operator: key,
            }),
            "realm" if key.is_empty() => Some(MemoryScope::Realm { realm }),
            _ => None,
        }
    }

    fn unescape_scope_ref_part(part: &str) -> Option<String> {
        let mut unescaped = String::with_capacity(part.len());
        let mut rest = part;
        while let Some(index) = rest.find('%') {
            unescaped.push_str(rest.get(..index)?);
            unescaped.push(match rest.get(index..index + 3)? {
                "%25" => '%',
                "%2F" => '/',
                _ => return None,
            });
            rest = rest.get(index + 3..)?;
        }
        unescaped.push_str(rest);
        Some(unescaped)
    }

    /// The stored format round-trips: every scope's reference decodes back
    /// to exactly that scope, so no two scopes share a reference, whatever
    /// the realm and key contain (separators, the escape character, text
    /// that already looks escaped, kind names, empty and non-ASCII names).
    #[test]
    fn operation_scope_refs_round_trip_and_never_collide() {
        let names = [
            "",
            "default",
            "identity",
            "realm",
            "a/b",
            "a%b",
            "a%2Fb",
            "a%25b",
            "%2",
            "%",
            "/",
            "%%//",
            "x/y/z",
            "agent:alice",
            "\u{e9}/\u{fc}%",
        ];
        let mut scopes = Vec::new();
        for realm in names {
            scopes.push(MemoryScope::Realm {
                realm: realm.to_string(),
            });
            for key in names {
                scopes.push(MemoryScope::Identity {
                    realm: realm.to_string(),
                    identity: key.to_string(),
                });
                scopes.push(MemoryScope::Mob {
                    realm: realm.to_string(),
                    mob: key.to_string(),
                });
                scopes.push(MemoryScope::Operator {
                    realm: realm.to_string(),
                    operator: key.to_string(),
                });
            }
        }
        let distinct_scopes: std::collections::BTreeSet<&MemoryScope> = scopes.iter().collect();
        assert_eq!(distinct_scopes.len(), scopes.len());

        let mut references = std::collections::BTreeSet::new();
        for scope in &scopes {
            let reference = scope.operation_scope_ref();
            assert_eq!(
                decode_operation_scope_ref(&reference).as_ref(),
                Some(scope),
                "{reference:?}"
            );
            assert!(reference.is_named(), "{reference:?}");
            references.insert(reference);
        }
        assert_eq!(
            references.len(),
            scopes.len(),
            "two scopes share a reference"
        );

        // A reference another owner names, or one outside the format, is no
        // Meta memory scope.
        for (owner, scope) in [
            ("another.owner", "identity/default/agent:alice"),
            (AGENT_MEMORY_TARGET_OWNER, "identity/default"),
            (AGENT_MEMORY_TARGET_OWNER, "identity/default/a/b"),
            (AGENT_MEMORY_TARGET_OWNER, "identity/default/a%2fb"),
            (AGENT_MEMORY_TARGET_OWNER, "realm/default/key"),
            (AGENT_MEMORY_TARGET_OWNER, "session/default/agent:alice"),
        ] {
            let reference = meerkat_core::MemoryScopeRef::Host {
                owner: owner.to_string(),
                scope: scope.to_string(),
            };
            assert_eq!(
                decode_operation_scope_ref(&reference),
                None,
                "{reference:?}"
            );
        }
    }

    #[test]
    fn record_serde_round_trips() {
        let record = MemoryRecord {
            id: "mem-1".to_string(),
            scope: MemoryScope::Identity {
                realm: "family".to_string(),
                identity: "identity:luka".to_string(),
            },
            kind: MemoryKind::OpenLoop,
            title: "Try the staging DB".to_string(),
            description: "When smoke tests need a database".to_string(),
            body: "Next time try the staging DB first. Resolved when tried.".to_string(),
            tags: vec!["staging".to_string()],
            provenance: MemoryProvenance {
                evidence: vec![EvidenceRef {
                    session_id: "sess-1".to_string(),
                    generation: 2,
                    revision: None,
                    range: Some((3, 9)),
                }],
                author: MemoryAuthor::Agent {
                    identity: "identity:luka".to_string(),
                },
                profile: None,
                verification: None,
            },
            trust: TrustTier::AgentObserved,
            status: RecordStatus::Superseded {
                by: "mem-2".to_string(),
            },
            supersedes: None,
            derived_from: Vec::new(),
            working_set_rank: Some(4),
            created_at_ms: 10,
            updated_at_ms: 20,
            usage: UsageStats::default(),
            ever_quarantined: false,
        };
        let json = serde_json::to_string(&record).expect("serialize");
        let back: MemoryRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, record);
    }

    #[test]
    fn field_caps_reject_oversized_and_empty() {
        assert!(validate_record_fields("t", "", "b").is_ok());
        assert!(validate_record_fields("", "", "b").is_err());
        assert!(validate_record_fields("t", "", " ").is_err());
        assert!(validate_record_fields(&"t".repeat(201), "", "b").is_err());
        assert!(validate_record_fields("t", &"d".repeat(401), "b").is_err());
        assert!(validate_record_fields("t", "", &"b".repeat(64 * 1024 + 1)).is_err());
    }
}
