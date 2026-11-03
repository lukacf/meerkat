//! Upgrade-carry regression (Bug 2, a downstream app's 0.7.20 report): a schedule store
//! written with `schedule_json` as TEXT — the encoding pre-0.7.20 deployments
//! carried — must still read after the upgrade.
//!
//! SQLite column affinity does not rewrite existing values, so a store authored
//! before the BLOB rework holds the same UTF-8 JSON as TEXT. meerkat 0.7.14
//! (Ask A) reads schedule JSON columns through a `Text|Blob`-tolerant boundary;
//! before it, `list()` (and therefore meta's identity-target repair and the
//! steward dream find-or-create) failed EVERY read with
//! `Invalid column type Text at index: 0, name: schedule_json` on carried
//! stores — invisible to fresh-state tests. This crosses that version boundary:
//! author BLOB, rewrite to TEXT to simulate the carried store, reopen, and
//! assert meta's schedule paths still work.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use meerkat::{ScheduleService, SqliteScheduleStore};
use meerkat_meta::schedule_wiring::{SCHEDULE_STORE_FILE, ensure_steward_dream_schedule};

fn typeof_schedule_json(path: &std::path::Path) -> String {
    let conn = rusqlite::Connection::open(path).expect("raw open");
    conn.query_row(
        "SELECT typeof(schedule_json) FROM schedule_schedules LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    )
    .expect("typeof schedule_json")
}

#[tokio::test]
async fn carried_text_schedule_json_rows_read_after_upgrade() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(SCHEDULE_STORE_FILE);
    let now = "2026-07-01T00:00:00Z".parse().expect("fixed now");

    // 1. Author a schedule through the store today (writes schedule_json BLOB).
    {
        let store = SqliteScheduleStore::open(&path).expect("open store");
        let service = ScheduleService::new(Arc::new(store));
        ensure_steward_dream_schedule(&service, Duration::from_hours(6), now)
            .await
            .expect("create dream schedule");
        assert_eq!(service.list().await.expect("list").len(), 1);
    }
    assert_eq!(
        typeof_schedule_json(&path),
        "blob",
        "the store writes schedule_json as BLOB today"
    );

    // 2. Simulate a pre-0.7.20 carried store: rewrite the row to TEXT in place.
    //    `CAST(blob AS TEXT)` reinterprets the identical UTF-8 JSON bytes with
    //    TEXT affinity — exactly the legacy on-disk shape.
    {
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE schedule_schedules SET schedule_json = CAST(schedule_json AS TEXT)",
            [],
        )
        .expect("rewrite schedule_json to TEXT");
    }
    assert_eq!(
        typeof_schedule_json(&path),
        "text",
        "row now carries the legacy TEXT encoding a carried store would hold"
    );

    // 3. Reopen on 0.7.14: list() must succeed over the TEXT row (Ask A's
    //    Text|Blob boundary), and the idempotent ensure must reuse the carried
    //    schedule rather than choke or duplicate it.
    {
        let store = SqliteScheduleStore::open(&path).expect("reopen store");
        let service = ScheduleService::new(Arc::new(store));
        let schedules = service
            .list()
            .await
            .expect("list must not fail on carried TEXT schedule_json rows");
        assert_eq!(
            schedules.len(),
            1,
            "the carried schedule survives the upgrade and reads back"
        );
        let later = "2026-07-02T00:00:00Z".parse().expect("later now");
        ensure_steward_dream_schedule(&service, Duration::from_hours(6), later)
            .await
            .expect("ensure_steward_dream_schedule over a carried TEXT store");
        assert_eq!(
            service.list().await.expect("list").len(),
            1,
            "idempotent ensure reuses the carried schedule (no duplicate)"
        );
    }
}
