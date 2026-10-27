//! Staging for the released Meta 0.9.3 H-app state directories under
//! `tests/fixtures/released_0_9_3_h_app/` (see tests/fixtures/README.md):
//! what an application that embeds `UnifiedRuntimeBuilder` with
//! `persistent_state` left behind, written by the published 0.9.3 crates.
//!
//! The staging is the H-local one, by directory: a test stages one pristine,
//! owner-writable copy per use, checked against the variant's
//! `checksums.sha256` and MANIFEST.json `files.state` byte for byte.

#![allow(dead_code, clippy::expect_used, clippy::panic)]

#[path = "released_0_9_3_h_local.rs"]
mod capture;

use std::path::{Path, PathBuf};

use serde_json::Value;

pub use capture::sha256_hex;

/// The captures. See the mint script beside them for the plans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// A `RosterProvider` over `app:app-01`..`app:app-06` with the builder's
    /// default in-memory mob storage. Over three boots the identities take
    /// turns, `app:app-01` compacts, `app:app-03` is retired and woken,
    /// `app:app-04` is reset and `app:app-05` deleted and re-added; the last
    /// boot's roster drops `app:app-06`.
    Roster,
    /// The roster plan with persistent mob storage at
    /// `state/event_log.sqlite3`, the gateway's file name.
    RosterDurable,
    /// The roster plan with persistent mob storage at `state/mob.sqlite3`,
    /// the name 0.9.3's own non-gateway call sites use.
    RosterDurableMobSqlite,
    /// No roster provider: over two boots the app spawns `worker-1` and
    /// `worker-2` on its in-memory mob and runs a turn on each.
    Plain,
}

impl Variant {
    pub const ALL: [Variant; 4] = [
        Variant::Roster,
        Variant::RosterDurable,
        Variant::RosterDurableMobSqlite,
        Variant::Plain,
    ];

    pub fn dir_name(self) -> &'static str {
        match self {
            Variant::Roster => "roster",
            Variant::RosterDurable => "roster-durable",
            Variant::RosterDurableMobSqlite => "roster-durable-mob-sqlite",
            Variant::Plain => "plain",
        }
    }

    /// Whether the app served a roster, and so 0.9.3 kept identity records
    /// in `continuity.sqlite3`.
    pub fn has_roster(self) -> bool {
        self != Variant::Plain
    }

    /// The persistent mob store the app supplied, inside the state
    /// directory; `None` when the mob lived in memory.
    pub fn mob_store(self) -> Option<&'static str> {
        match self {
            Variant::RosterDurable => Some("event_log.sqlite3"),
            Variant::RosterDurableMobSqlite => Some("mob.sqlite3"),
            Variant::Roster | Variant::Plain => None,
        }
    }

    pub fn dir(self) -> PathBuf {
        fixture_root().join(self.dir_name())
    }

    pub fn manifest(self) -> Value {
        capture::read_manifest(&self.dir())
    }
}

/// The mint script beside the variants; every MANIFEST pins its sha256.
pub const MINT_SCRIPT: &str = "mint_released_0_9_3_h_app.py";

/// The driver crate's files under `driver/`, as every MANIFEST pins them.
/// The manifest and lockfile keep non-manifest names, so that nothing in
/// the repository reads the driver as a crate or a lockfile.
pub const DRIVER_FILES: [&str; 4] = [
    "Cargo.toml.fixture",
    "Cargo.lock.fixture",
    "src/main.rs",
    "src/llm_usage.rs",
];

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/released_0_9_3_h_app")
}

/// The committed variant is exactly the capture: every file listed in its
/// `checksums.sha256` holds the listed digest, and `state/` holds nothing
/// else.
pub fn assert_variant_is_the_released_capture(variant: Variant) {
    capture::assert_capture_is_the_released_bytes(&variant.dir(), variant.dir_name());
}

/// Stage one pristine, owner-writable copy of the variant's final state into
/// `<temp>/state` and return its path. Panics when the committed bytes or the
/// staged tree differ from what the release wrote.
pub fn stage_variant(variant: Variant, temp: &Path) -> PathBuf {
    capture::stage_capture(&variant.dir(), variant.dir_name(), temp)
}
