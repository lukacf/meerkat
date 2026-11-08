//! The declared lock order of the source owner's synchronization (stage B, I10).
//!
//! This is the one rank table. The combined model's generator reads it through the
//! canonical exporter (`lock_order/ranks.json`), the lock-order model must state the
//! same ranks (a drift test and the model generator refuse a difference), and the
//! stage B runtime lock-order guard reads [`SOURCE_OWNER_LOCK_ORDER`] itself.
//!
//! Rule: a blocking acquisition may only take a lock ranked after every lock its
//! holder already holds. Two exemptions:
//! - a non-blocking try may go against the order: it never waits, it refuses;
//! - the order never licenses a wait inside a protected section (I1): Meta's
//!   lifecycle lock during a bind, Core's driver lock and turn boundary during native
//!   reads and the historical admission callback. Nothing blocks there at all.

/// One lock of the source owner's synchronization and its rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockRank {
    /// The models' name for the lock.
    pub lock: &'static str,
    /// Its rank: a lower rank is taken first.
    pub rank: u8,
    /// What the lock is in the code.
    pub owner: &'static str,
}

/// The declared stage B order, lowest rank first. The source writer precedes the class
/// callback exclusion, as the code takes and documents them: the class input path holds
/// the source writer while its callback targets take the exclusion, and the native and
/// peer operation writers hold it while a member-context route takes the exclusion
/// ("Source writer precedes this existing policy exclusion", on `callback_targets`).
pub const SOURCE_OWNER_LOCK_ORDER: &[LockRank] = &[
    LockRank {
        lock: "SW",
        rank: 1,
        owner: "source writer (tokio mutex)",
    },
    LockRank {
        lock: "CX",
        rank: 2,
        owner: "class callback exclusion (tokio mutex)",
    },
    LockRank {
        lock: "PUB",
        rank: 3,
        owner: "shared publication writer",
    },
    LockRank {
        lock: "CONN",
        rank: 4,
        owner: "store connection and its IMMEDIATE transaction",
    },
    LockRank {
        lock: "RC",
        rank: 5,
        owner: "recovery control state",
    },
    LockRank {
        lock: "OC",
        rank: 6,
        owner: "originals collection",
    },
    LockRank {
        lock: "GO",
        rank: 7,
        owner: "grant owner state",
    },
    LockRank {
        lock: "GF",
        rank: 8,
        owner: "grant file writer",
    },
];

/// The rank of `lock`, if it is in the table.
#[must_use]
pub fn lock_rank(lock: &str) -> Option<u8> {
    SOURCE_OWNER_LOCK_ORDER
        .iter()
        .find(|entry| entry.lock == lock)
        .map(|entry| entry.rank)
}
