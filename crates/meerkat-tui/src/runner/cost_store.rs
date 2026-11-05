//! The per-session cost record: token totals by model (and the helpers'
//! share), kept next to the realm in `<realm>/tui_cost/<session id>.toml`.
//!
//! Meerkat persists a session's cumulative usage but not which model each
//! call went to, so the TUI records the totals the UI counted
//! ([`crate::ui::Effect::CostSnapshot`]). The record is only an attribution
//! of the session's usage: on resume it is reconciled with the session's
//! authoritative `total_usage` ([`SessionCost::reconcile`]). Writes are
//! atomic and owner-only ([`crate::paths::write_atomic`]); a record that
//! cannot be parsed is moved aside as `<id>.toml.corrupt-<unix seconds>`
//! (never overwritten) and the session starts from its authoritative usage.
//!
//! Records written before this layout lived in `<realm>/tui_endpoints/`
//! together with an endpoint choice; their totals are still read.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::paths::{AtomicWriteError, FileMode, write_atomic};
use crate::ui::cost::{CostConfidence, ModelTotals, SessionCost};

/// Directory (inside the realm root) of the cost records.
pub const COST_DIR: &str = "tui_cost";

/// Directory of the records written before [`COST_DIR`] (read only).
const LEGACY_DIR: &str = "tui_endpoints";

/// The cost record could not be written.
#[derive(Debug, thiserror::Error)]
pub enum CostStoreError {
    /// The totals could not be encoded.
    #[error("cannot encode the cost record: {0}")]
    Encode(#[source] toml::ser::Error),
    /// The record could not be written.
    #[error(transparent)]
    Write(#[from] AtomicWriteError),
}

/// On-disk form of a cost record.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CostFile {
    #[serde(default)]
    confidence: CostConfidence,
    /// `[cost."<model>"]`: token totals per model.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    cost: ModelTotals,
    /// `[helper_cost.<label>."<model>"]`: the helpers' share of `cost`.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    helper_cost: IndexMap<String, ModelTotals>,
}

/// The legacy sidecar: an endpoint choice plus the totals.
#[derive(Debug, Deserialize)]
struct LegacyFile {
    #[serde(default)]
    cost_estimated: bool,
    #[serde(default)]
    cost: ModelTotals,
    #[serde(default)]
    helper_cost: IndexMap<String, ModelTotals>,
}

/// Where session `session_id`'s record lives.
#[must_use]
pub fn cost_path(realm_root: &Path, session_id: &str) -> PathBuf {
    realm_root.join(COST_DIR).join(format!("{session_id}.toml"))
}

/// Store `cost` as session `session_id`'s record.
///
/// # Errors
///
/// [`CostStoreError`] when the record cannot be encoded or written.
pub fn save_cost(
    realm_root: &Path,
    session_id: &str,
    cost: &SessionCost,
) -> Result<(), CostStoreError> {
    let file = CostFile {
        confidence: cost.confidence,
        cost: cost.per_model.clone(),
        helper_cost: cost.helpers.clone(),
    };
    let text = toml::to_string(&file).map_err(CostStoreError::Encode)?;
    write_atomic(
        &cost_path(realm_root, session_id),
        text.as_bytes(),
        FileMode::Private,
        Some(realm_root),
    )?;
    Ok(())
}

/// Session `session_id`'s record, if any. A record that cannot be parsed
/// is moved aside (see the module docs) and counts as missing.
#[must_use]
pub fn load_cost(realm_root: &Path, session_id: &str) -> Option<SessionCost> {
    let path = cost_path(realm_root, session_id);
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<CostFile>(&text) {
            Ok(file) => Some(SessionCost {
                per_model: file.cost,
                confidence: file.confidence,
                helpers: file.helper_cost,
            }),
            Err(err) => {
                quarantine(&path, &err);
                None
            }
        },
        Err(_) => load_legacy(realm_root, session_id),
    }
}

/// The totals of a pre-[`COST_DIR`] sidecar.
fn load_legacy(realm_root: &Path, session_id: &str) -> Option<SessionCost> {
    let path = realm_root
        .join(LEGACY_DIR)
        .join(format!("{session_id}.toml"));
    let text = std::fs::read_to_string(path).ok()?;
    let file = toml::from_str::<LegacyFile>(&text).ok()?;
    (!file.cost.is_empty()).then(|| SessionCost {
        per_model: file.cost,
        confidence: if file.cost_estimated {
            CostConfidence::Approximate
        } else {
            CostConfidence::Exact
        },
        helpers: file.helper_cost,
    })
}

/// Move an unreadable record aside, so the next write does not destroy it.
fn quarantine(path: &Path, err: &toml::de::Error) {
    let stamp = crate::pricing::unix_now();
    let mut aside = path.as_os_str().to_os_string();
    aside.push(format!(".corrupt-{stamp}"));
    match std::fs::rename(path, &aside) {
        Ok(()) => tracing::warn!(
            error = %err,
            path = %path.display(),
            moved_to = %Path::new(&aside).display(),
            "unreadable cost record moved aside"
        ),
        Err(rename) => tracing::warn!(
            error = %err,
            rename_error = %rename,
            path = %path.display(),
            "unreadable cost record could not be moved aside"
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ui::cost::UsageTotals;

    fn sample() -> SessionCost {
        let mut cost = SessionCost::default();
        cost.add(
            "gpt-5.4",
            &UsageTotals {
                input: 10,
                output: 20,
                cache_read: 30,
                cache_write: 0,
            },
        );
        cost.add(
            "claude-sonnet-4-6",
            &UsageTotals {
                input: 1,
                ..UsageTotals::default()
            },
        );
        cost
    }

    #[test]
    fn records_round_trip_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_cost(tmp.path(), "abc").is_none());
        let mut cost = sample();
        save_cost(tmp.path(), "abc", &cost).unwrap();
        let text = std::fs::read_to_string(cost_path(tmp.path(), "abc")).unwrap();
        assert!(text.contains("[cost.\"gpt-5.4\"]"), "{text}");
        assert_eq!(load_cost(tmp.path(), "abc"), Some(cost.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(cost_path(tmp.path(), "abc"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        cost.confidence = CostConfidence::Unmeasured;
        save_cost(tmp.path(), "abc", &cost).unwrap();
        assert_eq!(
            load_cost(tmp.path(), "abc").map(|c| c.confidence),
            Some(CostConfidence::Unmeasured)
        );
    }

    #[test]
    fn a_corrupt_record_is_moved_aside_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = cost_path(tmp.path(), "abc");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not = [toml").unwrap();
        assert!(load_cost(tmp.path(), "abc").is_none());
        assert!(!path.exists());
        let aside: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            aside.iter().any(|name| name.starts_with("abc.toml.corrupt-")),
            "{aside:?}"
        );
        // The next save starts a fresh record; the corrupt one stays.
        save_cost(tmp.path(), "abc", &sample()).unwrap();
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
    }

    #[test]
    fn legacy_sidecars_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(LEGACY_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("old.toml"),
            "endpoint = \"openai\"\nmodel = \"gpt-5.4\"\ncost_estimated = true\n[cost.\"gpt-5.4\"]\ninput = 5\n",
        )
        .unwrap();
        let cost = load_cost(tmp.path(), "old").unwrap();
        assert_eq!(cost.per_model["gpt-5.4"].input, 5);
        assert_eq!(cost.confidence, CostConfidence::Approximate);
        std::fs::write(
            dir.join("bare.toml"),
            "endpoint = \"openai\"\nmodel = \"gpt-5.4\"\n",
        )
        .unwrap();
        assert!(load_cost(tmp.path(), "bare").is_none());
    }
}
