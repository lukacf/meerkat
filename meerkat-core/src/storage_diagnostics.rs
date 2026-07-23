//! Shape-stable storage diagnosis vocabulary and the [`StorageMigrator`]
//! diagnose seam (Phase 1 of the storage unification arc).
//!
//! `rkat storage doctor` renders a [`StorageDiagnosis`]; the disk
//! implementation lives in `meerkat-store::doctor`. The vocabulary lives
//! here, below every store crate, because remote storage providers and the
//! mobkit companion (its M1 doctor phase) bind to this shape *before* the
//! full Phase 4 provider trait exists — the report format is wire-stable
//! from Phase 1 on.
//!
//! Shape-stability rules:
//!
//! - Structs are `#[non_exhaustive]` with `#[serde(default)]` on optional
//!   fields, so fields can be added compatibly. Construct via the provided
//!   constructors, then set public fields.
//! - Finding `code`s are stable kebab-case strings (`"split-brain-realm"`,
//!   `"schema-from-the-future"`, ...). Codes are added over time, never
//!   renamed; consumers must tolerate unknown codes.
//! - [`StorageMigrator`] carries only the read-only `diagnose` verb in this
//!   phase. Mutation verbs (migrate/prune) arrive with the Phase 6
//!   migration framework as *defaulted* trait methods, so implementations
//!   written against this trait stay source-compatible.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Severity of one storage finding.
///
/// `Error`-severity findings drive the doctor exit code (a report with at
/// least one error exits nonzero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// Inventory-grade observation; no action needed.
    Info,
    /// Something an operator should look at; not necessarily broken.
    Warning,
    /// A broken or refusal-grade condition (split-brain twin, dangling blob
    /// reference, schema from the future, ...).
    Error,
}

/// One diagnostic finding.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageFinding {
    /// Severity class.
    pub severity: FindingSeverity,
    /// Stable kebab-case code (e.g. `"split-brain-realm"`,
    /// `"schema-from-the-future"`, `"legacy-unverified-sessions"`,
    /// `"dangling-blob-reference"`, `"orphaned-lease"`,
    /// `"no-schema-ledger"`, `"backup-artifact"`,
    /// `"maintenance-fence-lock"`). Consumers must tolerate unknown codes.
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Filesystem path the finding is about, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Realm id the finding is scoped to, when realm-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
}

impl StorageFinding {
    /// Construct a finding (path/realm attach via the builders).
    pub fn new(
        severity: FindingSeverity,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.into(),
            message: message.into(),
            path: None,
            realm: None,
        }
    }

    /// Attach the filesystem path this finding is about.
    #[must_use]
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    /// Attach the realm id this finding is scoped to.
    #[must_use]
    pub fn with_realm(mut self, realm: impl Into<String>) -> Self {
        self.realm = Some(realm.into());
        self
    }
}

/// Schema-ledger state of one database file.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInventory {
    /// Path of the database file.
    pub path: PathBuf,
    /// Ledger domain → version. `None` means the domain has no ledger row
    /// (a pre-ledger file, or a domain whose store never touched this file).
    #[serde(default)]
    pub domains: Vec<(String, Option<i64>)>,
}

impl DatabaseInventory {
    /// Construct an inventory row for one database file.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            domains: Vec::new(),
        }
    }
}

/// Inventory of one materialized realm under one state root.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageInventoryEntry {
    /// Realm id (from the manifest; the sanitized directory name when the
    /// manifest is unreadable).
    pub realm: String,
    /// The realm directory.
    pub root: PathBuf,
    /// Backend pinned in the realm manifest (`"sqlite"`, `"jsonl"`,
    /// `"memory"`, ...); `None` when the manifest is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Database files found under the realm directory.
    #[serde(default)]
    pub databases: Vec<DatabaseInventory>,
}

impl StorageInventoryEntry {
    /// Construct an inventory entry for one realm directory.
    pub fn new(realm: impl Into<String>, root: PathBuf) -> Self {
        Self {
            realm: realm.into(),
            root,
            backend: None,
            databases: Vec::new(),
        }
    }
}

/// The full diagnosis report.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageDiagnosis {
    /// Findings across all swept roots and realms.
    #[serde(default)]
    pub findings: Vec<StorageFinding>,
    /// Per-realm inventory across all swept roots.
    #[serde(default)]
    pub inventory: Vec<StorageInventoryEntry>,
}

impl StorageDiagnosis {
    /// Count findings of one severity.
    pub fn count(&self, severity: FindingSeverity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }

    /// True when at least one `Error`-severity finding exists (doctor exits
    /// nonzero).
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == FindingSeverity::Error)
    }
}

/// What to diagnose.
///
/// Hermeticity contract: implementations read **only** the given
/// `state_roots` — never ambient candidate roots. The caller (CLI bootstrap,
/// a gateway) decides the candidate set.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnoseScope {
    /// Candidate state roots to sweep (typically the project-local
    /// `<context>/.rkat/realms` plus the user-global data root; or exactly
    /// the roots an operator passed).
    pub state_roots: Vec<PathBuf>,
    /// Restrict the sweep to one realm id (twins for that realm are still
    /// reported across all roots). `None` sweeps every realm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm: Option<String>,
}

impl DiagnoseScope {
    /// Scope over the given candidate roots, sweeping all realms.
    pub fn new(state_roots: Vec<PathBuf>) -> Self {
        Self {
            state_roots,
            realm: None,
        }
    }

    /// Restrict the sweep to one realm id.
    #[must_use]
    pub fn with_realm(mut self, realm: impl Into<String>) -> Self {
        self.realm = Some(realm.into());
        self
    }
}

/// Failure producing a diagnosis at all.
///
/// Per-entry faults (a corrupt manifest, an unreadable database) are
/// *findings inside the report*, never this error — implementations must be
/// fault-tolerant per entry. This error is for total failures only (a remote
/// provider that cannot reach its backend, an unsupported scope).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum StorageDiagnosticsError {
    /// The provider does not support diagnosis for this scope.
    #[error("storage diagnosis unsupported: {0}")]
    Unsupported(String),
    /// The provider failed to produce a report at all.
    #[error("storage diagnosis failed: {0}")]
    Backend(String),
}

/// Storage maintenance seam a storage provider exposes.
///
/// Phase 1 deliberately ships only the read-only `diagnose` verb, defined as
/// a small standalone trait so downstream consumers (mobkit M1, remote
/// providers) bind to the hook shape before the Phase 4 provider trait
/// (`RealmStorageProvider::migrator()`) exists. Do **not** add mutation
/// verbs here outside the Phase 6 migration framework; when they land they
/// will be defaulted methods so existing implementations stay
/// source-compatible.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait StorageMigrator: Send + Sync {
    /// Produce a read-only diagnosis of the given scope.
    ///
    /// Implementations must be safe against live storage (no leases, no
    /// exclusive locks, no writes) and fault-tolerant per entry: one corrupt
    /// realm yields findings for that realm, never a total failure.
    async fn diagnose(
        &self,
        scope: &DiagnoseScope,
    ) -> Result<StorageDiagnosis, StorageDiagnosticsError>;
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn finding_json_shape_is_stable() {
        let finding = StorageFinding::new(
            FindingSeverity::Error,
            "split-brain-realm",
            "realm 'team' exists under two roots",
        )
        .with_path(PathBuf::from("/a/team"))
        .with_realm("team");
        let json = serde_json::to_value(&finding).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "severity": "error",
                "code": "split-brain-realm",
                "message": "realm 'team' exists under two roots",
                "path": "/a/team",
                "realm": "team",
            })
        );
    }

    #[test]
    fn optional_fields_are_omitted_and_default_on_read() {
        let finding = StorageFinding::new(FindingSeverity::Info, "no-schema-ledger", "pre-arc db");
        let json = serde_json::to_string(&finding).expect("serialize");
        assert!(!json.contains("path"));
        assert!(!json.contains("realm"));
        // Forward-compat: unknown fields tolerated, missing optionals default.
        let parsed: StorageFinding = serde_json::from_str(
            r#"{"severity":"warning","code":"orphaned-lease","message":"m","future_field":1}"#,
        )
        .expect("deserialize with unknown field");
        assert_eq!(parsed.severity, FindingSeverity::Warning);
        assert!(parsed.path.is_none());
    }

    #[test]
    fn diagnosis_error_detection() {
        let mut diagnosis = StorageDiagnosis::default();
        assert!(!diagnosis.has_errors());
        diagnosis.findings.push(StorageFinding::new(
            FindingSeverity::Warning,
            "orphaned-lease",
            "1 stale lease",
        ));
        assert!(!diagnosis.has_errors());
        diagnosis.findings.push(StorageFinding::new(
            FindingSeverity::Error,
            "dangling-blob-reference",
            "missing blob",
        ));
        assert!(diagnosis.has_errors());
        assert_eq!(diagnosis.count(FindingSeverity::Error), 1);
        assert_eq!(diagnosis.count(FindingSeverity::Warning), 1);
        assert_eq!(diagnosis.count(FindingSeverity::Info), 0);
    }

    #[test]
    fn diagnosis_round_trips_through_json() {
        let mut diagnosis = StorageDiagnosis::default();
        let mut entry = StorageInventoryEntry::new("team", PathBuf::from("/roots/a/team"));
        entry.backend = Some("sqlite".to_string());
        let mut db = DatabaseInventory::new(PathBuf::from("/roots/a/team/sessions.sqlite3"));
        db.domains.push(("session-store".to_string(), Some(1)));
        db.domains.push(("schedule-store".to_string(), None));
        entry.databases.push(db);
        diagnosis.inventory.push(entry);
        let json = serde_json::to_string(&diagnosis).expect("serialize");
        let parsed: StorageDiagnosis = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.inventory.len(), 1);
        assert_eq!(parsed.inventory[0].backend.as_deref(), Some("sqlite"));
        assert_eq!(
            parsed.inventory[0].databases[0].domains,
            vec![
                ("session-store".to_string(), Some(1)),
                ("schedule-store".to_string(), None),
            ]
        );
    }
}
