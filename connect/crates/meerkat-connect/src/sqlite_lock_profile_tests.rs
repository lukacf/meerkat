//! Read the linked SQLite build, not a caller environment or runtime fallback.

#[test]
fn bundled_sqlite_uses_required_lock_profile() -> rusqlite::Result<()> {
    let connection = rusqlite::Connection::open_in_memory()?;
    let (memory_status_disabled, memory_management_enabled, overflow_stats_disabled): (
        i64,
        i64,
        i64,
    ) = connection.query_row(
        "SELECT sqlite_compileoption_used('DEFAULT_MEMSTATUS=0'), \
             sqlite_compileoption_used('ENABLE_MEMORY_MANAGEMENT'), \
             sqlite_compileoption_used('DISABLE_PAGECACHE_OVERFLOW_STATS')",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(
        memory_status_disabled, 1,
        "linked SQLite must disable allocator statistics"
    );
    assert_eq!(
        memory_management_enabled, 0,
        "linked SQLite must use per-connection page caches"
    );
    assert_eq!(
        overflow_stats_disabled, 1,
        "linked SQLite must disable page-cache overflow statistics"
    );
    Ok(())
}
