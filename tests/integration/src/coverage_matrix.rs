//! Composite-path coverage matrix for axis-crossing regressions.
//!
//! Background:
//!
//! Formalizes the matrix of composite axes so empty cells are visible and
//! every `Covered` cell points at a real, passing test.
//!
//! Axes:
//!
//! - `runtime_mode`       — who drives the loop
//! - `peer_input_class`   — the shape of the peer-originated input the session receives
//! - `stream_mode`        — whether a subscriber has reserved an interaction stream
//! - `assertion_surface`  — where the test observes outcomes
//!
//! `Covered` cells point at a fully-qualified test path
//! (`crate::module::tests::fn_name`). The test may live in the
//! integration-tests crate or in an in-crate `#[cfg(test)] mod tests` block;
//! both count as covered so long as the test actually exists. `PendingFix`
//! cells name the specific seam/fix they are blocked on — no placeholder
//! stubs are shipped ahead of the fix. `Gap` and `Impossible` cells carry
//! rationale strings.

#![allow(dead_code)]

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeMode {
    AutonomousHost,
    TurnDriven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Standard,
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
    /// Mob-lifecycle events (FlowStarted, StepDispatched, TopologyViolation,
    /// …). Observes mob-level facts, NOT per-session turn triggers.
    MobEventStream,
    /// Subscriber channel registered by a `ReserveInteraction` send; the
    /// subscriber fires when a correlating response arrives.
    SubscriberStream,
    /// Runtime-loop staged-input primitive — the observable artifact that a
    /// peer-response-terminal has been turned into next-turn prompt context.
    /// This is the correct surface for "peer response drives next turn"
    /// assertions; `MobEventStream` does not carry per-session turn events.
    RuntimeLoopStaged,
    /// Polling-based compatibility shim. Prefer event-stream surfaces.
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

/// Where the covering test lives. Integration-scope coverage (a test in the
/// `meerkat-integration-tests` crate) exercises cross-crate composition and
/// is the preferred home for axis-crossing cells. In-crate unit coverage is
/// accepted when the cell's semantics are fully observable at a single
/// crate's public API, but the matrix flags these explicitly so a reviewer
/// can tell at a glance whether a `Covered` entry is "the composite path
/// runs in integration" vs "a single-crate unit test exercises the same
/// behavior."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageScope {
    /// Test lives in the `meerkat-integration-tests` crate.
    Integration,
    /// Test lives in an in-crate `#[cfg(test)] mod tests` block. Acceptable
    /// when the cell's semantics are fully observable at a single crate's
    /// public API; flagged so the matrix stays honest about scope.
    InCrateUnit,
}

#[derive(Debug, Clone, Copy)]
pub enum Coverage {
    /// A deterministic test function exists in the workspace that exercises
    /// this cell. `test_name` is a fully-qualified Rust path
    /// (`crate::module::tests::fn_name`). `scope` distinguishes
    /// integration-scope coverage from in-crate unit coverage so the matrix
    /// is explicit about the lane / crate the covering test runs in.
    Covered {
        test_name: &'static str,
        scope: CoverageScope,
    },
    /// The cell is blocked on a named fix (seam, feature, bug). The test
    /// lands when the fix lands — no placeholder `#[ignore]`'d stub is
    /// shipped ahead of the fix. `blocked_by` names the specific fix.
    PendingFix { blocked_by: &'static str },
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
        // Peer-response terminal is turned into an immediate-boundary staged
        // input by the runtime loop. Covered by an in-crate test in meerkat-runtime.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::Standard,
                peer_input_class: PeerInputClass::ResponseTerminal,
                stream_mode: StreamMode::None,
                assertion_surface: AssertionSurface::RuntimeLoopStaged,
            },
            coverage: Coverage::Covered {
                test_name: "meerkat_runtime::runtime_loop::tests::\
                            peer_response_terminal_creates_immediate_context_staged_input",
                scope: CoverageScope::InCrateUnit,
            },
        },
        // Peer-request + reserve-interaction: subscriber must be registered
        // on the requester, the response must route back from the responder,
        // and the reservation correlates via the request envelope id carried
        // in the response's `in_reply_to`. Covered at integration scope by
        // a two-runtime end-to-end test.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::Standard,
                peer_input_class: PeerInputClass::Request,
                stream_mode: StreamMode::ReserveInteraction,
                assertion_surface: AssertionSurface::SubscriberStream,
            },
            coverage: Coverage::Covered {
                test_name: "reserve_interaction_subscriber_fires::\
                            reserve_interaction_subscriber_fires_on_matching_response",
                scope: CoverageScope::Integration,
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
                transport: Transport::Standard,
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
                transport: Transport::Standard,
                peer_input_class: PeerInputClass::LifecycleAdded,
                stream_mode: StreamMode::ReserveInteraction,
                assertion_surface: AssertionSurface::SubscriberStream,
            },
            coverage: Coverage::Impossible {
                rationale: "lifecycle events don't correlate to a reserved interaction",
            },
        },
        // Gap: polling-based assertion for a peer-message append. Polling is
        // a stopgap surface; we prefer event-stream assertions.
        CellEntry {
            cell: Cell {
                runtime_mode: RuntimeMode::TurnDriven,
                transport: Transport::Standard,
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
            Coverage::Covered { test_name, scope } => {
                let tag = match scope {
                    CoverageScope::Integration => "integration",
                    CoverageScope::InCrateUnit => "in-crate/unit",
                };
                format!("COVERED[{tag}] ({test_name})")
            }
            Coverage::PendingFix { blocked_by } => {
                format!("PENDING_FIX (blocked_by={blocked_by})")
            }
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
    fn covered_entries_reference_fully_qualified_test_paths() {
        // Honesty pin: every `Covered` entry must name a fully-qualified
        // Rust path (`crate::module::tests::fn_name`). We can't grep the
        // workspace from inside the type, but we can pin the shape so a
        // typo like `my_test_name` without a `::` path fails the build.
        for entry in cells() {
            if let Coverage::Covered { test_name, .. } = entry.coverage {
                assert!(
                    !test_name.is_empty(),
                    "Covered cell has empty test_name: {}",
                    entry.cell,
                );
                assert!(
                    test_name.contains("::"),
                    "Covered cell test_name must be fully-qualified \
                     (crate::module::tests::fn_name); got {:?} for cell {}",
                    test_name,
                    entry.cell,
                );
            }
        }
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
