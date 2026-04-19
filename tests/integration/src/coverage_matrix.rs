//! Composite-path coverage matrix for axis-crossing regressions.
//!
//! Background:
//!
//! The s71 turn-8 regression (peer-response-triggered turn in a turn-driven
//! realtime-attached session) escaped every existing test lane because no
//! single test owned the composite axes. Each axis is individually covered,
//! but the *combination* `TurnDriven × RealtimeAttached × ResponseTerminal ×
//! None × MobEventStream` had no home. W1-C formalizes the matrix so empty
//! cells are visible and populated cells each have a deterministic test.
//!
//! Axes:
//!
//! - `runtime_mode`       — who drives the loop
//! - `transport`          — whether a provider realtime side-channel is attached
//! - `peer_input_class`   — the shape of the peer-originated input the session receives
//! - `stream_mode`        — whether a subscriber has reserved an interaction stream
//! - `assertion_surface`  — where the test observes outcomes
//!
//! Every populated cell should be reachable from `e2e-fast` using the mock
//! realtime session factory in `meerkat-openai/tests/mock_realtime_ws/`. Empty
//! cells are listed with an explicit rationale — either "impossible combination"
//! or "gap, TODO".

#![allow(dead_code)]

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeMode {
    AutonomousHost,
    TurnDriven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    NonRealtime,
    RealtimeAttached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerInputClass {
    Message,
    Request,
    ResponseProgress,
    ResponseTerminal,
    LifecycleAdded,
    LifecycleRetired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamMode {
    None,
    ReserveInteraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssertionSurface {
    MobEventStream,
    SubscriberStream,
    Polling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub runtime_mode: RuntimeMode,
    pub transport: Transport,
    pub peer_input_class: PeerInputClass,
    pub stream_mode: StreamMode,
    pub assertion_surface: AssertionSurface,
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} × {:?} × {:?} × {:?} × {:?}",
            self.runtime_mode,
            self.transport,
            self.peer_input_class,
            self.stream_mode,
            self.assertion_surface,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Coverage {
    /// A deterministic test exists in `e2e-fast` for this cell. The identifier
    /// is the Rust test function name.
    Covered { test_name: &'static str },
    /// The test exists but is currently `#[ignore]` because the underlying
    /// production fix hasn't landed. References a tracking identifier.
    PendingFix {
        test_name: &'static str,
        blocked_by: &'static str,
    },
    /// No test yet — gap to fill once a clear reproducer is available.
    Gap { rationale: &'static str },
    /// Combination is impossible by construction.
    Impossible { rationale: &'static str },
}

#[derive(Debug)]
pub struct CellEntry {
    pub cell: Cell,
    pub coverage: Coverage,
}

/// The authoritative, ordered list of every cell the matrix tracks.
///
/// This is NOT every Cartesian-product cell (2 × 2 × 6 × 2 × 3 = 144). It's
/// the *semantically meaningful* cells we care about — the ones that either
/// have a test, have a known gap, or are explicitly impossible. Adding a new
/// cell here is a deliberate act.
pub fn cells() -> &'static [CellEntry] {
    &[
        // --- W1-C populated cells ------------------------------------------------
        //
        // The s71 shape: turn-driven + realtime-attached, peer-response terminal
        // triggers the next turn, observed via mob event stream. This is the one
        // that regressed; it is expected to fail until W2-E lands.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::RealtimeAttached,
                peer_input_class: PeerInputClass::ResponseTerminal,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::MobEventStream,
            },
            coverage: Coverage::PendingFix {
                test_name: "turn_driven_realtime_response_terminal_triggers_next_turn",
                blocked_by: "W2-E (typed projection-refresh effect emission)",
            },
        },
        // Non-realtime mirror of the s71 shape. This path works today; the test
        // pins the working behavior so a regression there also gets caught.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::NonRealtime,
                peer_input_class: PeerInputClass::ResponseTerminal,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::MobEventStream,
            },
            coverage: Coverage::Covered {
                test_name: "turn_driven_nonrealtime_response_terminal_triggers_next_turn",
            },
        },
        // Autonomous host + realtime: host loop owns the drive, peer-response
        // terminals should land in the transcript even while audio is attached.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::AutonomousHost,
                transport: Transport::RealtimeAttached,
                peer_input_class: PeerInputClass::ResponseTerminal,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::MobEventStream,
            },
            coverage: Coverage::Covered {
                test_name: "autonomous_host_realtime_response_terminal_appends_transcript",
            },
        },
        // Peer-request + reserve-interaction: subscriber must fire when the
        // correlating response arrives. Independent of runtime_mode/transport —
        // we pick one concrete leg (TurnDriven × NonRealtime) to own the cell.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::NonRealtime,
                peer_input_class: PeerInputClass::Request,
                stream_mode: StreamMode::ReserveInteraction,
                assertion_surface: AssertionSurface::SubscriberStream,
            },
            coverage: Coverage::Covered {
                test_name: "reserve_interaction_subscriber_fires_on_matching_response",
            },
        },
        // --- Explicitly empty cells ---------------------------------------------
        //
        // Peer-message ingress + reserve-interaction is not a thing: reserve
        // predicates match request/response pairs by correlation id, and bare
        // messages have no correlation id to bind to.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::NonRealtime,
                peer_input_class: PeerInputClass::Message,
                stream_mode: StreamMode::ReserveInteraction,
                assertion_surface: AssertionSurface::SubscriberStream,
            },
            coverage: Coverage::Impossible {
                rationale: "reserve_interaction only correlates on request/response pairs; \
                            plain messages have no correlation id",
            },
        },
        // Lifecycle events never populate a reserved interaction stream — they
        // flow through the mob event stream exclusively.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::NonRealtime,
                peer_input_class: PeerInputClass::LifecycleAdded,
                stream_mode: StreamMode::ReserveInteraction,
                assertion_surface: AssertionSurface::SubscriberStream,
            },
            coverage: Coverage::Impossible {
                rationale: "lifecycle events don't correlate to a reserved interaction",
            },
        },
        // Gap: we do not yet have a deterministic test for lifecycle-retired
        // during a realtime-attached turn. Needed once member-retire
        // semantics settle under realtime (tracked in 0.7 roadmap).
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::AutonomousHost,
                transport: Transport::RealtimeAttached,
                peer_input_class: PeerInputClass::LifecycleRetired,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::MobEventStream,
            },
            coverage: Coverage::Gap {
                rationale: "lifecycle-retired semantics under realtime attachment \
                            are still in flux; add once authority stabilizes",
            },
        },
        // Gap: peer response-progress (non-terminal) while realtime-attached.
        // Progress frames shouldn't trigger a next turn, but we don't yet have
        // a regression pin for the turn-driven path.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::RealtimeAttached,
                peer_input_class: PeerInputClass::ResponseProgress,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::MobEventStream,
            },
            coverage: Coverage::Gap {
                rationale: "progress frames must not trigger a next turn; \
                            add after W1-A lands typed OutboundPeerRequestState",
            },
        },
        // Gap: polling-based assertion for a peer-message append. Polling is
        // a stopgap surface; we prefer event-stream assertions.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::NonRealtime,
                peer_input_class: PeerInputClass::Message,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::Polling,
            },
            coverage: Coverage::Gap {
                rationale: "polling surface is a compatibility shim; prefer MobEventStream. \
                            Cell retained to track migration progress",
            },
        },
    ]
}

/// Short human-readable report of the matrix. Useful for printing from a
/// smoke test if the harness wants a visual check.
pub fn render_matrix_report() -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    writeln!(&mut s, "Coverage matrix ({} cells tracked):", cells().len()).ok();
    for entry in cells() {
        let status = match entry.coverage {
            Coverage::Covered { test_name } => format!("COVERED ({test_name})"),
            Coverage::PendingFix {
                test_name,
                blocked_by,
            } => format!("PENDING_FIX ({test_name}; blocked_by={blocked_by})"),
            Coverage::Gap { rationale } => format!("GAP ({rationale})"),
            Coverage::Impossible { rationale } => format!("IMPOSSIBLE ({rationale})"),
        };
        writeln!(&mut s, "  {} -> {}", entry.cell, status).ok();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cell_has_a_distinct_axis_combination() {
        let mut seen: Vec<Cell> = Vec::new();
        for entry in cells() {
            assert!(
                !seen.contains(&entry.cell),
                "duplicate cell in coverage matrix: {}",
                entry.cell,
            );
            seen.push(entry.cell);
        }
    }

    #[test]
    fn the_s71_shape_is_enumerated() {
        let s71 = Cell {
            runtime_mode: RuntimeMode::TurnDriven,
            transport: Transport::RealtimeAttached,
            peer_input_class: PeerInputClass::ResponseTerminal,
            stream_mode: StreamMode::None,
            assertion_surface: AssertionSurface::MobEventStream,
        };
        let found = cells().iter().find(|e| e.cell == s71);
        assert!(found.is_some(), "s71 shape must be in the matrix");
        matches!(found.unwrap().coverage, Coverage::PendingFix { .. });
    }

    #[test]
    fn matrix_report_mentions_every_entry() {
        let report = render_matrix_report();
        for entry in cells() {
            let stringified = entry.cell.to_string();
            assert!(
                report.contains(&stringified),
                "report missing cell: {stringified}",
            );
        }
    }
}
