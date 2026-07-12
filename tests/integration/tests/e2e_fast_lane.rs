// The multi-host mob support fixtures are composed ONCE at the binary root
// (clippy::duplicate_mod: two `#[path]` includes of the same file are two
// distinct modules); the multi_host_* lanes reach it via `super::support`.
// The file's own inner attributes carry the test-lint allows.
#[cfg(not(target_arch = "wasm32"))]
#[path = "../../../meerkat-mob/tests/support/mod.rs"]
mod support;

// Phase 6b: the live-plane fixture half (support/live_plane.rs is
// deliberately NOT declared inside support/mod.rs — only live-capable test
// roots compose it).
#[cfg(not(target_arch = "wasm32"))]
#[path = "../../../meerkat-mob/tests/support/live_plane.rs"]
mod live_support;

#[cfg(not(target_arch = "wasm32"))]
#[path = "e2e_fast/cross_host_live_member.rs"]
mod cross_host_live_member;
#[path = "e2e_fast/multi_host_bind.rs"]
mod multi_host_bind;
// Phase 7 (T-A8): the console verbs served through the REAL RPC handler →
// MobMcpState → MobHandle path.
#[cfg(not(target_arch = "wasm32"))]
#[path = "e2e_fast/multi_host_console.rs"]
mod multi_host_console;
#[path = "e2e_fast/multi_host_flow.rs"]
mod multi_host_flow;
#[path = "e2e_fast/multi_host_spawn.rs"]
mod multi_host_spawn;
#[path = "phase1_wiring.rs"]
mod phase1_wiring;
#[path = "phase2_mode_routing.rs"]
mod phase2_mode_routing;
