//! Read boundary for UTF-8 JSON payload columns.
//!
//! Meerkat writes JSON payload columns as BLOB (`serde_json::to_vec`), but
//! SQLite column affinity does not rewrite values, so carried stores written
//! by external hosts can hold the same UTF-8 JSON as TEXT. Both physical
//! encodings are byte-identical UTF-8 JSON; a typed read boundary accepts
//! either so one legacy TEXT row cannot fail every read of the table.
//!
//! (Moved here from `meerkat-store`, which re-exports it, so every SQLite
//! store — including ones below `meerkat-store` in the dependency order —
//! stops re-solving the TEXT-vs-BLOB question.)

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};

/// UTF-8 JSON payload read from a SQLite column, tolerant of both the
/// canonical BLOB encoding and legacy TEXT rows.
pub struct JsonColumnBytes(Vec<u8>);

impl JsonColumnBytes {
    pub fn into_bytes(self) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn accepts_text_and_blob_rejects_others() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE t (v);
             INSERT INTO t VALUES (CAST('{\"a\":1}' AS BLOB));
             INSERT INTO t VALUES ('{\"a\":2}');
             INSERT INTO t VALUES (42);",
        )
        .expect("seed");
        let mut stmt = conn
            .prepare("SELECT v FROM t ORDER BY rowid")
            .expect("prepare");
        let mut rows = stmt.query([]).expect("query");

        let blob_row = rows.next().expect("row").expect("present");
        let blob: JsonColumnBytes = blob_row.get(0).expect("blob accepted");
        assert_eq!(blob.into_bytes(), b"{\"a\":1}");

        let text_row = rows.next().expect("row").expect("present");
        let text: JsonColumnBytes = text_row.get(0).expect("text accepted");
        assert_eq!(text.into_bytes(), b"{\"a\":2}");

        let int_row = rows.next().expect("row").expect("present");
        match int_row.get::<_, JsonColumnBytes>(0) {
            Err(rusqlite::Error::InvalidColumnType(..)) => {}
            Err(other) => panic!("wrong error for integer column: {other}"),
            Ok(_) => panic!("integer column must be rejected"),
        }
    }
}
