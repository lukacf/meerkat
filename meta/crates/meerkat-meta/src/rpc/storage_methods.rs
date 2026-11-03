//! `meta/storage/doctor` dispatch, shared by the module-only stdin RPC,
//! the unified stdin RPC, and the console RPC surfaces.
//!
//! Read-only by contract (see [`crate::storage_doctor`]'s safety contract);
//! the console registers it as a read method. Params carry an explicit
//! `state_dir` because no runtime handle exposes its persistent state
//! directory yet — the storage layout authority that would provide the
//! default lands in Phase M2. Until then, runtime-backed surfaces answer a
//! missing `state_dir` with the typed capability-unavailable error
//! ([`storage_doctor_state_dir_unavailable_error`], code `-32004`).

use std::path::{Path, PathBuf};

use meerkat_core::storage_diagnostics::{DiagnoseScope, StorageDiagnosis};
use serde_json::Value;

use crate::storage_doctor;
use crate::storage_health::ResolvedStorageSummary;

use super::JsonRpcError;

/// The doctor RPC method name (read method on every surface).
pub(crate) const STORAGE_DOCTOR_METHOD: &str = "meta/storage/doctor";

/// Parsed `meta/storage/doctor` params.
pub(crate) struct StorageDoctorParams {
    pub state_dir: PathBuf,
    /// Optional identity filter for the continuity checkpoint census
    /// (mapped onto [`DiagnoseScope::realm`]).
    pub identity: Option<String>,
    /// Verbose report shaping: per-session session-format findings
    /// ([`crate::storage_doctor::DoctorOptions::verbose`]).
    pub verbose: bool,
}

impl StorageDoctorParams {
    pub(crate) fn scope(&self) -> DiagnoseScope {
        let scope = DiagnoseScope::new(vec![self.state_dir.clone()]);
        match &self.identity {
            Some(identity) => scope.with_realm(identity.clone()),
            None => scope,
        }
    }

    pub(crate) fn doctor_options(&self) -> storage_doctor::DoctorOptions {
        storage_doctor::DoctorOptions {
            verbose: self.verbose,
        }
    }
}

/// Parse doctor params. `Ok(None)` = no `state_dir` given — the caller maps
/// that to its surface's typed error; `Err` = a param is present but
/// mistyped (`-32602` on every surface). The optional `verbose` boolean
/// turns on per-session session-format findings.
pub(crate) fn parse_storage_doctor_params(
    params: &Value,
) -> Result<Option<StorageDoctorParams>, String> {
    if !params.is_null() && !params.is_object() {
        return Err("params must be an object".to_string());
    }
    let state_dir = match params.get("state_dir") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(dir)) if !dir.trim().is_empty() => PathBuf::from(dir),
        Some(_) => return Err("state_dir must be a non-empty string".to_string()),
    };
    let identity = match params.get("identity") {
        None | Some(Value::Null) => None,
        Some(Value::String(identity)) => Some(identity.clone()),
        Some(_) => return Err("identity must be a string".to_string()),
    };
    let verbose = match params.get("verbose") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(verbose)) => *verbose,
        Some(_) => return Err("verbose must be a boolean".to_string()),
    };
    Ok(Some(StorageDoctorParams {
        state_dir,
        identity,
        verbose,
    }))
}

/// The typed error a runtime-backed surface returns when no `state_dir` is
/// given: `-32004` (the SDKs' reserved capability-unavailable code) because
/// the runtime cannot yet report its own persistent state directory.
pub(crate) fn storage_doctor_state_dir_unavailable_error() -> JsonRpcError {
    JsonRpcError {
        code: -32004,
        message: "storage doctor requires params.state_dir: the runtime does not expose its \
                  persistent state directory (the storage layout authority lands in Phase M2)"
            .to_string(),
        data: None,
    }
}

/// The doctor result payload: the serialized [`StorageDiagnosis`] plus the
/// live H1/H2 storage summary when the invoking surface has one.
pub(crate) fn storage_doctor_result_json(
    params: &StorageDoctorParams,
    diagnosis: &StorageDiagnosis,
    resolved: Option<ResolvedStorageSummary>,
) -> Value {
    serde_json::json!({
        "state_dir": params.state_dir.display().to_string(),
        "diagnosis": serde_json::to_value(diagnosis).unwrap_or(Value::Null),
        "storage": resolved
            .map(|summary| summary.status_json())
            .unwrap_or(Value::Null),
    })
}

/// Run the doctor for a runtime-backed surface: live durability census when
/// the runtime resolved its storage at composition time.
///
/// The census describes THIS runtime's composed stores, so it is attached
/// only when the requested `state_dir` is provably the runtime's own state
/// directory (canonicalized identity via
/// [`ResolvedStorageSummary::state_dir`]). Any other directory — or a
/// summary that recorded no state directory — gets a disk-only diagnosis:
/// `storage: null` (a shape the RPC docs already promise) plus an explicit
/// `storage_note`. One runtime's census must never decorate another
/// directory's disk report.
pub(crate) async fn run_storage_doctor(
    params: &StorageDoctorParams,
    resolved: Option<ResolvedStorageSummary>,
) -> Value {
    let (census, note) = match resolved {
        Some(summary) if summary_covers_state_dir(&summary, &params.state_dir) => {
            (Some(summary), None)
        }
        Some(_) => (
            None,
            Some(
                "live durability census omitted: params.state_dir is not this runtime's own \
                 state directory (or the runtime recorded none); the diagnosis is disk-only",
            ),
        ),
        None => (None, None),
    };
    let diagnosis = storage_doctor::diagnose_state_dir_with_options(
        &params.scope(),
        census.clone(),
        params.doctor_options(),
    )
    .await;
    let mut result = storage_doctor_result_json(params, &diagnosis, census);
    if let (Some(note), Some(map)) = (note, result.as_object_mut()) {
        map.insert("storage_note".to_string(), Value::String(note.to_string()));
    }
    result
}

/// True when the summary's recorded state directory and the requested one
/// name the same directory (canonicalized when resolvable). A summary
/// without a recorded state directory never matches — fail closed.
fn summary_covers_state_dir(summary: &ResolvedStorageSummary, requested: &Path) -> bool {
    let Some(own) = summary.state_dir.as_deref() else {
        return false;
    };
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical(own) == canonical(requested)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::storage_health::BlobDurability;

    fn summary_with_state_dir(dir: Option<&Path>) -> ResolvedStorageSummary {
        let summary = ResolvedStorageSummary::new(BlobDurability::PersistentDisk, Some(true));
        match dir {
            Some(dir) => summary.with_state_dir(dir),
            None => summary,
        }
    }

    #[test]
    fn parse_accepts_the_optional_verbose_flag() {
        let params = parse_storage_doctor_params(&serde_json::json!({
            "state_dir": "/tmp/state", "verbose": true
        }))
        .expect("parse")
        .expect("params");
        assert!(params.verbose);
        assert!(params.doctor_options().verbose);

        let params = parse_storage_doctor_params(&serde_json::json!({
            "state_dir": "/tmp/state"
        }))
        .expect("parse")
        .expect("params");
        assert!(!params.verbose);

        assert!(
            parse_storage_doctor_params(&serde_json::json!({
                "state_dir": "/tmp/state", "verbose": "yes"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn census_attaches_only_for_the_runtimes_own_state_dir() {
        let own = tempfile::tempdir().expect("own state dir");
        let other = tempfile::tempdir().expect("other state dir");
        let params = StorageDoctorParams {
            state_dir: own.path().to_path_buf(),
            identity: None,
            verbose: false,
        };

        // Matching directory: census attached, no omission note.
        let result =
            run_storage_doctor(&params, Some(summary_with_state_dir(Some(own.path())))).await;
        assert!(!result["storage"].is_null(), "{result}");
        assert!(result.get("storage_note").is_none(), "{result}");

        // Foreign directory: disk-only report with the explicit note — the
        // runtime's census never decorates another directory's diagnosis.
        let params_other = StorageDoctorParams {
            state_dir: other.path().to_path_buf(),
            identity: None,
            verbose: false,
        };
        let result = run_storage_doctor(
            &params_other,
            Some(summary_with_state_dir(Some(own.path()))),
        )
        .await;
        assert!(result["storage"].is_null(), "{result}");
        assert!(
            result["storage_note"]
                .as_str()
                .is_some_and(|note| note.contains("disk-only")),
            "{result}"
        );

        // A summary that recorded no state directory never matches.
        let result = run_storage_doctor(&params, Some(summary_with_state_dir(None))).await;
        assert!(result["storage"].is_null(), "{result}");
        assert!(result.get("storage_note").is_some(), "{result}");
    }
}
