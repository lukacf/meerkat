//! Census of the gating doors: every production call site that reads a
//! gating entry or a question set out of its owner.
//!
//! A gating entry and a question set are their asking member's. A door that
//! shows or decides one for a Console caller asks the one rule
//! ([`crate::access::AccessView::sees_origin`], through
//! `gating_origin_visible` and `gating_origin_shown_when_applied` of the
//! Console), with the entry's origin name AND the time its request was
//! made: the name alone says who holds it now, not whose the entry is. So
//! every read of the owners is listed here with what decides it, and a new
//! read fails this row until it is listed.

use std::collections::BTreeMap;

use super::{crate_production_sources, production};

/// The reads of the gating owner and of the decision store's question sets.
const READERS: &[&str] = &[
    "list_gating_pending",
    "gating_audit_entries",
    "gating_audit_after",
    "gating_state_snapshot",
    "decide_gating_action_attested",
    "subscribe_gating_changes",
    "open_question_sets",
    "held_question_set",
    "question_set_asker",
    "question_set_askers",
];

/// The files that are the owners themselves: their own reads are no door.
const OWNERS: &[&str] = &["runtime.rs", "runtime/gating.rs"];

/// The Console's deciding functions: a Console door calls one of them.
const DECIDERS: &[&str] = &[
    "gating_origin_visible(",
    "gating_origin_shown_when_applied(",
    "gating_audit_entry_visible(",
    "approval_notification_shown(",
];

/// What decides the entries a file's reads hand out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecidedBy {
    /// A Console door: each entry it shows or decides passes the one rule,
    /// with its origin's name and the time its request was made.
    TheOneRule,
    /// The trusted host plane (stdio, the Rust API, the gateway's own
    /// composition): it shows and decides everything, for no caller's
    /// audience.
    TheHost,
    /// No entry leaves the file for a caller: it follows a member's end, or
    /// keeps the owner's state.
    NoCaller,
}

/// One production file's reads: the file (by its path from `src/`), each
/// reader it calls with how often, and what decides what it hands out.
struct Door {
    file: &'static str,
    reads: &'static [(&'static str, usize)],
    decided_by: DecidedBy,
}

const DOORS: &[Door] = &[
    Door {
        file: "ask_user.rs",
        reads: &[("open_question_sets", 1)],
        decided_by: DecidedBy::NoCaller,
    },
    Door {
        file: "durable_decisions/console.rs",
        reads: &[("question_set_asker", 1)],
        decided_by: DecidedBy::TheOneRule,
    },
    Door {
        file: "gating_persistence.rs",
        reads: &[("gating_state_snapshot", 1)],
        decided_by: DecidedBy::NoCaller,
    },
    Door {
        file: "http_console.rs",
        reads: &[
            ("decide_gating_action_attested", 1),
            ("gating_audit_after", 3),
            ("gating_audit_entries", 6),
            ("list_gating_pending", 2),
            ("subscribe_gating_changes", 1),
        ],
        decided_by: DecidedBy::TheOneRule,
    },
    Door {
        file: "http_console/question_sets.rs",
        reads: &[("held_question_set", 1), ("open_question_sets", 1)],
        decided_by: DecidedBy::TheOneRule,
    },
    Door {
        file: "rpc.rs",
        reads: &[
            ("gating_audit_after", 2),
            ("gating_audit_entries", 2),
            ("list_gating_pending", 2),
        ],
        decided_by: DecidedBy::TheHost,
    },
    Door {
        file: "rpc/question_set_methods.rs",
        reads: &[("held_question_set", 1), ("open_question_sets", 1)],
        decided_by: DecidedBy::TheHost,
    },
    Door {
        file: "unified_runtime/module_ops.rs",
        reads: &[
            ("gating_audit_after", 1),
            ("gating_audit_entries", 1),
            ("gating_state_snapshot", 2),
            ("list_gating_pending", 1),
            ("question_set_askers", 1),
            ("subscribe_gating_changes", 1),
        ],
        decided_by: DecidedBy::TheHost,
    },
];

/// How often `code` (comments and literals blanked) calls `reader`: the
/// name followed by `(`, not as part of a longer name and not its own
/// definition.
fn calls(code: &str, reader: &str) -> usize {
    let call = format!("{reader}(");
    code.match_indices(&call)
        .filter(|(at, _)| {
            let before = &code[..*at];
            let part_of_a_longer_name = before
                .chars()
                .next_back()
                .is_some_and(|last| last.is_alphanumeric() || last == '_');
            !part_of_a_longer_name && !before.trim_end().ends_with("fn")
        })
        .count()
}

/// Every production file reads the gating owner and the question sets only
/// where this census lists it, as often as it lists, and a Console door
/// calls the one rule. A new read (a new door) fails here: list it with
/// what decides it, and for a Console caller decide it by the entry's
/// origin and the time its request was made.
#[test]
fn every_read_of_a_gating_entry_or_a_question_set_is_listed_with_what_decides_it() {
    let mut found: BTreeMap<String, BTreeMap<&str, usize>> = BTreeMap::new();
    let mut codes: BTreeMap<String, String> = BTreeMap::new();
    for (file, source) in crate_production_sources() {
        if OWNERS.contains(&file.as_str()) {
            continue;
        }
        let (code, _) = production(&source);
        for reader in READERS.iter().copied() {
            let count = calls(&code, reader);
            if count > 0 {
                found.entry(file.clone()).or_default().insert(reader, count);
            }
        }
        codes.insert(file, code);
    }
    let listed: BTreeMap<String, BTreeMap<&str, usize>> = DOORS
        .iter()
        .map(|door| (door.file.to_string(), door.reads.iter().copied().collect()))
        .collect();
    assert_eq!(
        found, listed,
        "the reads of the gating owner and the question sets, by production file (left: found; \
         right: listed in DOORS). A new read is a new door: list it with what decides it"
    );
    for door in DOORS {
        if door.decided_by != DecidedBy::TheOneRule {
            continue;
        }
        let code = codes.get(door.file).map(String::as_str).unwrap_or_default();
        assert!(
            DECIDERS.iter().any(|decider| code.contains(decider)),
            "{} hands gating entries to Console callers and calls none of {DECIDERS:?}",
            door.file
        );
    }
}
