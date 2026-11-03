//! The one-way native-storage migration of #306: a state directory written
//! by Meta's continuity facade moves onto Core's own session and identity
//! stores, behind `meta_gateway storage-migrate-native`.
//!
//! [`native_layout_status`] is the startup predicate: whether a directory
//! still holds storage the migration has to move. [`migrate_state_dir_native`]
//! runs the census over scratch copies of the directory's databases and, with
//! `--apply`, carries out a clean census (`apply`). `relation` holds the
//! durable-head relation provers the census runs, moved here from the facade
//! unchanged. [`reset_identity`] (`meta_gateway identity-reset`) gives one
//! identity of a stopped native directory a fresh life at its next start,
//! through a first-boot manifest row.
//! [`import_continuity_export_into_host_store`] carries a continuity export
//! into a realm kept in a host store instead (see [`crate::host_store`]).

mod apply;
// The census's live-runtime exclusion and the layout's stamp, claim and scan
// helpers are shared with `storage-migrate --session-db`.
pub(crate) mod census;
mod export_census;
mod export_file;
mod host_store_import;
mod identity;
mod identity_reset;
mod import;
pub(crate) mod layout_status;
mod offline_identities;
mod relation;

pub use apply::{DiscardedRuntimeReport, NativeApplyOutcome};
pub use census::{
    CensusMobStore, GatewayStateShape, IdentityCensusStatus, NativeMigrateRefusal,
    NativeMigrateReport, NativeMigrateRequest, SessionCensusClass, SessionCensusRow,
    migrate_state_dir_native,
};
pub use export_census::{ContinuityExportMode, ContinuityExportSummary};
pub use host_store_import::{
    HostStoreImportClass, HostStoreImportExport, HostStoreImportRefusal, HostStoreImportReport,
    HostStoreImportRequest, HostStoreImportRow, import_continuity_export_into_host_store,
};
pub use identity::{FirstBootRowPlan, IdentityCensusClass, IdentityCensusRow};
pub use identity_reset::{
    IDENTITY_RESET_FORMAT, IDENTITY_RESET_VERSION, IdentityResetRefusal, IdentityResetReport,
    IdentityResetRequest, reset_identity,
};
pub use import::{
    ImportStep, ImportedActivation, ImportedCrossing, ImportedRow, SessionImport,
    SessionImportRefusal,
};
/// Core's head-canonical durable claim on a state directory's
/// `runtime.sqlite`, read-only: the check `storage-migrate --session-db`
/// makes before it moves anything (a 0.9.4 runtime already used the
/// directory).
pub(crate) use layout_status::runtime_conversion_began;
pub use layout_status::{
    CONTINUITY_SCHEMA_DOMAIN, NATIVE_MIGRATION_JOURNAL_FILE_NAME, NativeLayoutReason,
    NativeLayoutStatus, RETIRED_PROJECTION_SCHEMA_VERSION, native_layout_status,
};
pub use offline_identities::{
    IDENTITY_CENSUS_FORMAT, IDENTITY_CENSUS_VERSION, IN_MEMORY_MOB_STORE_NOTE, IdentityCensusEntry,
    IdentityCensusRefusal, IdentityCensusReport, PendingFirstBootRow, identity_census_offline,
};
