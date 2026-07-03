//! Read boundary for UTF-8 JSON payload columns.
//!
//! Meerkat writes JSON payload columns as BLOB (`serde_json::to_vec`), but
//! SQLite column affinity does not rewrite values, so carried stores written
//! by external hosts can hold the same UTF-8 JSON as TEXT. Both physical
//! encodings are byte-identical UTF-8 JSON; a typed read boundary accepts
//! either so one legacy TEXT row cannot fail every read of the table.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};

/// UTF-8 JSON payload read from a SQLite column, tolerant of both the
/// canonical BLOB encoding and legacy TEXT rows.
pub(crate) struct JsonColumnBytes(Vec<u8>);

impl JsonColumnBytes {
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl FromSql for JsonColumnBytes {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Ok(Self(bytes.to_vec())),
            ValueRef::Null | ValueRef::Integer(_) | ValueRef::Real(_) => {
                Err(FromSqlError::InvalidType)
            }
        }
    }
}
