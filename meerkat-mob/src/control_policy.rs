//! Principal control-scope policy (multi-host plan §8, plane b).
//!
//! [`ResolvedControlPolicy`] is the sealed, resolved form of the MobMachine
//! grant facts (`operator_grant_scopes` / `operator_grant_expiries`) for one
//! presented [`MobControlPrincipal`], evaluated at one shell-read `now_ms`.
//! It is the principal-lane analog of
//! `meerkat_core::tool_execution_policy::ToolExecutionPolicy`: sealed shape,
//! one canonical mint path, fail-closed resolution, enforcement at a
//! downstream seam that never changes what is *visible*.
//!
//! Lane discipline (§15.2): this is the PRINCIPAL lane. The agent authority
//! lane (`MobToolAuthorityContext`, member upcalls, member-context tool
//! surfaces) is a disjoint caller population with its own sealed type;
//! grants never gate member upcalls and upcalls never satisfy `require()`.
//!
//! Clock discipline (§6.5): the machine never reads a clock — `expires_at_ms`
//! is raw data. The chokepoint call site reads UNIX wall-clock milliseconds
//! ONCE per enforcement decision, immediately before [`ResolvedControlPolicy::resolve`],
//! and passes it in as `now_ms`. Nothing in this module performs I/O or reads
//! time, which keeps it wasm-clean.

use crate::machines::mob_machine as mob_dsl;
use meerkat_contracts::wire::supervisor_bridge::BridgeRejectionCause;
use meerkat_contracts::wire::{WireControlScope, WireGrantRecord, WireScopeDeniedDetail};
use meerkat_core::auth::PrincipalId;
use meerkat_core::types::SessionId;
use std::collections::BTreeSet;

pub use crate::machines::mob_machine::ControlScope;

/// A plane-(b) principal presented at a control chokepoint.
///
/// Derivation is a boundary concern (per-surface rules in the phase-5
/// design): local consoles derive `Owner` by surface class (A16), the
/// agent-facing mob tool surface derives `Owner` via
/// [`MobControlPrincipal::from_owner_bridge_session`] (session-id equality
/// with the machine owner fact — identity comparison, never capability
/// inference), future remote console verbs derive `External` from the
/// envelope-signature-bound comms `PeerId`, and every derivation failure is
/// `Unresolved`. Member upcalls NEVER derive a principal (agent lane).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobControlPrincipal {
    /// The owning operator (A16): v1 local RPC/REST/stdio/public-MCP
    /// consoles, the CLI, and the owner bridge session. Implicit full scope
    /// (§8) — no grant record is consulted.
    Owner,
    /// A non-owner principal with a validated id — comms peers today (the
    /// PeerId string), bearer-token principals in v2 (§21.7).
    External(PrincipalId),
    /// Fail-closed sink: a principal that could not be derived/validated.
    /// Resolves to zero scopes, always.
    Unresolved,
}

impl MobControlPrincipal {
    /// Derive the owner principal for an owner-session agent surface by
    /// comparing the surface's injected owner bridge session id against the
    /// machine owner fact (`owner_bridge_session_id`).
    ///
    /// Returns `Some(Owner)` on identity match. Returns `None` on mismatch
    /// or when the owner fact is unset — meaning the caller is NOT a
    /// principal-lane caller at all (it stays agent-lane-adjudicated by its
    /// sealed `MobToolAuthorityContext`), not that it is `Unresolved`.
    #[must_use]
    pub fn from_owner_bridge_session(
        session: &SessionId,
        state: &mob_dsl::MobMachineState,
    ) -> Option<Self> {
        let owner = state.owner_bridge_session_id.as_ref()?;
        (owner == &mob_dsl::SessionId::from_domain(session)).then_some(Self::Owner)
    }
}

/// Typed plane-(b) denial (§8). Field shapes mirror the two wire carriers
/// exactly (`BridgeRejectionCause::ScopeDenied` and
/// [`WireScopeDeniedDetail`]) so no surface reconstructs the pair from a
/// display string. `presented` is always the caller's own EFFECTIVE
/// (post-expiry) scope set — a denial never advertises scopes the principal
/// cannot use and never leaks other principals' grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeDenial {
    /// The scope the denied verb requires.
    pub required: ControlScope,
    /// The caller's effective scope set at the decision.
    pub presented: BTreeSet<ControlScope>,
}

impl ScopeDenial {
    /// Stable snake_case name of the required scope (the wire spelling),
    /// for human-facing renderings.
    #[must_use]
    pub fn required_name(&self) -> &'static str {
        self.required.name()
    }
}

/// Sealed, resolved principal control policy (§8, D3(b)).
///
/// Constructed only through [`ResolvedControlPolicy::resolve`]; the shape is
/// private so no caller can mint a policy that skipped resolution against
/// the machine grant facts. The type deliberately derives NO serde traits: a
/// policy is resolved per enforcement decision on the controlling host and
/// is never stored, never sent, never rehydrated (the
/// `WireResolvedToolAccessPolicy` "projection cannot authorize" discipline
/// taken to its limit — here not even a projection exists).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct ResolvedControlPolicy {
    kind: ResolvedControlPolicyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedControlPolicyKind {
    /// The owning operator (A16). Implicit full scope; never denies.
    OwnerFull,
    /// A granted (possibly empty) effective scope set, expiry already
    /// applied.
    Granted(BTreeSet<ControlScope>),
}

impl ResolvedControlPolicy {
    /// Resolve `principal` against the machine grant facts at `now_ms`.
    ///
    /// Infallible and fail-closed: an unresolvable principal, an absent
    /// grant, an expired grant, or a desynced expiry row (scopes row present
    /// with no expiry row — the 1:1 map keying broke) all resolve to the
    /// EMPTY effective scope set. Unlike `ToolExecutionPolicy::resolve`
    /// there is no wiring-fault arm — "no grant" is a legitimate deny, not
    /// an error.
    ///
    /// `Owner` resolves to implicit-full without consulting any grant row;
    /// a coincidental grant record for the owner is inert (grants are for
    /// OTHER principals).
    ///
    /// Expiry boundary: a grant is live iff `now_ms < expires_at_ms` — it
    /// expires AT its deadline (inclusive at `now`). An expired grant
    /// presents the empty set, indistinguishable from never-granted; the
    /// raw record stays visible through the grants projection.
    ///
    /// `now_ms` is UNIX wall-clock milliseconds read ONCE by the caller
    /// immediately before this call. The machine never reads a clock; this
    /// is the enforcement seam the DSL grant-block comment points at.
    pub fn resolve(
        principal: &MobControlPrincipal,
        state: &mob_dsl::MobMachineState,
        now_ms: u64,
    ) -> Self {
        let kind = match principal {
            MobControlPrincipal::Owner => ResolvedControlPolicyKind::OwnerFull,
            MobControlPrincipal::External(id) => {
                let key = machine_principal(id);
                match state.operator_grant_scopes.get(&key) {
                    None => ResolvedControlPolicyKind::Granted(BTreeSet::new()),
                    Some(scopes) => match state.operator_grant_expiries.get(&key) {
                        Some(None) => ResolvedControlPolicyKind::Granted(scopes.clone()),
                        Some(Some(expires_at_ms)) if now_ms < *expires_at_ms => {
                            ResolvedControlPolicyKind::Granted(scopes.clone())
                        }
                        // Expired at/after the deadline: effective ∅.
                        Some(Some(_)) => ResolvedControlPolicyKind::Granted(BTreeSet::new()),
                        // Desynced expiry row: fail closed to ∅ (the 1:1
                        // keying invariant is ADJ-12 errata; this is the
                        // runtime posture when the impossible happens).
                        None => ResolvedControlPolicyKind::Granted(BTreeSet::new()),
                    },
                }
            }
            MobControlPrincipal::Unresolved => ResolvedControlPolicyKind::Granted(BTreeSet::new()),
        };
        Self { kind }
    }

    /// Require `scope`; `Err` carries the typed denial (§8).
    ///
    /// # Errors
    ///
    /// [`ScopeDenial`] when the effective scope set does not contain
    /// `scope`. `OwnerFull` never denies.
    pub fn require(&self, scope: ControlScope) -> Result<(), ScopeDenial> {
        match &self.kind {
            ResolvedControlPolicyKind::OwnerFull => Ok(()),
            ResolvedControlPolicyKind::Granted(set) => {
                if set.contains(&scope) {
                    Ok(())
                } else {
                    Err(ScopeDenial {
                        required: scope,
                        presented: set.clone(),
                    })
                }
            }
        }
    }

    /// Non-erroring probe (the `ToolExecutionPolicy::permits` mirror).
    #[must_use]
    pub fn permits(&self, scope: ControlScope) -> bool {
        match &self.kind {
            ResolvedControlPolicyKind::OwnerFull => true,
            ResolvedControlPolicyKind::Granted(set) => set.contains(&scope),
        }
    }

    /// Owner-full marker (the `ToolExecutionPolicy::is_unrestricted`
    /// mirror).
    #[must_use]
    pub fn is_owner_full(&self) -> bool {
        matches!(self.kind, ResolvedControlPolicyKind::OwnerFull)
    }
}

/// Domain projection of one principal's raw grant record, read from machine
/// state by the grants projection (`MobHandle::grants`). Raw truth: an
/// expired grant appears with its verbatim `expires_at_ms` and NO expired
/// flag — evaluating expiry requires a clock read, and the enforcement seam
/// is the ONE place that reads the clock against grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorGrant {
    /// The validated principal id string (the machine map key).
    pub principal: String,
    /// The recorded scope set.
    pub scopes: BTreeSet<ControlScope>,
    /// The raw recorded expiry, verbatim (`None` = never expires).
    pub expires_at_ms: Option<u64>,
}

impl OperatorGrant {
    /// Wire projection. Scopes ride in `BTreeSet` order, so the wire `Vec`
    /// is deterministic.
    #[must_use]
    pub fn to_wire(&self) -> WireGrantRecord {
        WireGrantRecord {
            principal: self.principal.clone(),
            scopes: self
                .scopes
                .iter()
                .copied()
                .map(ControlScope::to_wire)
                .collect(),
            expires_at_ms: self.expires_at_ms,
        }
    }
}

impl ControlScope {
    /// Wire projection of the closed ten-variant vocabulary. Exhaustive so a
    /// future (v2) variant addition fails compile here.
    ///
    /// Inherent method (DEC-P5P-12): the machine enum must not grow serde or
    /// wire trait impls of its own — the wire spelling has one owner
    /// ([`WireControlScope`]) and this module owns every conversion.
    #[must_use]
    pub fn to_wire(self) -> WireControlScope {
        match self {
            Self::List => WireControlScope::List,
            Self::ReadHistory => WireControlScope::ReadHistory,
            Self::SubscribeEvents => WireControlScope::SubscribeEvents,
            Self::SendCommand => WireControlScope::SendCommand,
            Self::Cancel => WireControlScope::Cancel,
            Self::Retire => WireControlScope::Retire,
            Self::WireTopology => WireControlScope::WireTopology,
            Self::Live => WireControlScope::Live,
            Self::AdminHost => WireControlScope::AdminHost,
            Self::AdminGrants => WireControlScope::AdminGrants,
        }
    }

    /// Stable snake_case name — exactly the [`WireControlScope`] serde
    /// spelling ("grants and bridge scope denials speak exactly this set").
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::ReadHistory => "read_history",
            Self::SubscribeEvents => "subscribe_events",
            Self::SendCommand => "send_command",
            Self::Cancel => "cancel",
            Self::Retire => "retire",
            Self::WireTopology => "wire_topology",
            Self::Live => "live",
            Self::AdminHost => "admin_host",
            Self::AdminGrants => "admin_grants",
        }
    }
}

impl From<WireControlScope> for ControlScope {
    fn from(scope: WireControlScope) -> Self {
        match scope {
            WireControlScope::List => Self::List,
            WireControlScope::ReadHistory => Self::ReadHistory,
            WireControlScope::SubscribeEvents => Self::SubscribeEvents,
            WireControlScope::SendCommand => Self::SendCommand,
            WireControlScope::Cancel => Self::Cancel,
            WireControlScope::Retire => Self::Retire,
            WireControlScope::WireTopology => Self::WireTopology,
            WireControlScope::Live => Self::Live,
            WireControlScope::AdminHost => Self::AdminHost,
            WireControlScope::AdminGrants => Self::AdminGrants,
        }
    }
}

impl From<&ScopeDenial> for BridgeRejectionCause {
    fn from(denial: &ScopeDenial) -> Self {
        Self::ScopeDenied {
            required: denial.required.to_wire(),
            presented: denial
                .presented
                .iter()
                .copied()
                .map(ControlScope::to_wire)
                .collect(),
        }
    }
}

impl From<&ScopeDenial> for WireScopeDeniedDetail {
    fn from(denial: &ScopeDenial) -> Self {
        Self {
            required: denial.required.to_wire(),
            presented: denial
                .presented
                .iter()
                .copied()
                .map(ControlScope::to_wire)
                .collect(),
        }
    }
}

/// Convert a surface-validated core principal id into the machine map key.
///
/// The ONLY conversion site (crate-private): nothing outside meerkat-mob
/// ever constructs a machine `PrincipalId`, and nothing converts
/// machine→core (keys read back from state surface as plain strings in
/// projections).
pub(crate) fn machine_principal(id: &PrincipalId) -> mob_dsl::PrincipalId {
    mob_dsl::PrincipalId::from(id.as_str())
}

// ---------------------------------------------------------------------------
// Command authority lanes (chokepoint (a) routing vocabulary — DEC-P5E-2)
// ---------------------------------------------------------------------------

/// Which authority lane a mob actor command travels on.
///
/// Sealed-ctor discipline (the `ToolExecutionPolicy` pattern): the inner
/// lane is private; the ONLY public constructor is [`Self::principal`] (the
/// console lane). `agent_lane()` and `internal()` are `pub(crate)` — both
/// legitimate mint sites (the member-tool dispatcher and the upcall
/// executor; actor self-sends) are in-crate, so no surface crate can
/// launder a console call onto the agent lane (§15.2 confinement).
#[derive(Debug, Clone)]
pub struct CommandAuthority {
    lane: CommandAuthorityLane,
    member_operator_execution_fence: Option<MemberOperatorExecutionFence>,
}

/// Exact remote member residency attached only to controller-side execution
/// of one durable member-operator request. The actor revalidates this tuple
/// on every routed command immediately before handler/effect admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberOperatorExecutionFence {
    pub(crate) agent_identity: String,
    pub(crate) host_id: String,
    pub(crate) host_binding_generation: u64,
    pub(crate) requester_member_session_id: String,
    pub(crate) generation: u64,
    pub(crate) fence_token: u64,
}

#[derive(Debug, Clone)]
enum CommandAuthorityLane {
    /// Console lane: a plane-(b) principal presented at a chokepoint.
    Principal(MobControlPrincipal),
    /// §15.2 agent authority lane: the machine-composed
    /// `MobToolAuthorityContext` gates already ran; principal grants
    /// neither gate nor are satisfied on this lane.
    AgentLane,
    /// Actor/machine-internal self-sends. No principal semantics; an
    /// internal send of an operator-class command is a wiring fault and is
    /// denied loudly at the gate (ADJ-P5-15).
    Internal,
}

/// Payload-free lane discriminant for pins/audit (T-LS4 introspection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAuthorityKind {
    Principal,
    AgentLane,
    Internal,
}

impl CommandAuthority {
    /// Console lane: the sole public mint (injected at each surface's
    /// composition point, never ambient — gotcha #19).
    #[must_use]
    pub fn principal(principal: MobControlPrincipal) -> Self {
        Self {
            lane: CommandAuthorityLane::Principal(principal),
            member_operator_execution_fence: None,
        }
    }

    /// The principal this authority carries, if it rides the console lane.
    /// One acting principal per command: verbs that ALSO take an explicit
    /// `caller` argument (the grant trio, ADJ-P5-9) must present the same
    /// principal on both — asserted at their dispatch arms so an
    /// on-behalf-of surface can never desync the audit principal from the
    /// transport principal (zealot M2 contract).
    #[must_use]
    pub(crate) fn carried_principal(&self) -> Option<&MobControlPrincipal> {
        match &self.lane {
            CommandAuthorityLane::Principal(principal) => Some(principal),
            CommandAuthorityLane::AgentLane | CommandAuthorityLane::Internal => None,
        }
    }

    #[must_use]
    pub(crate) fn agent_lane() -> Self {
        Self {
            lane: CommandAuthorityLane::AgentLane,
            member_operator_execution_fence: None,
        }
    }

    #[must_use]
    pub(crate) fn remote_member_operator(fence: MemberOperatorExecutionFence) -> Self {
        Self {
            lane: CommandAuthorityLane::AgentLane,
            member_operator_execution_fence: Some(fence),
        }
    }

    #[must_use]
    pub(crate) fn internal() -> Self {
        Self {
            lane: CommandAuthorityLane::Internal,
            member_operator_execution_fence: None,
        }
    }

    pub(crate) fn member_operator_execution_fence(&self) -> Option<&MemberOperatorExecutionFence> {
        self.member_operator_execution_fence.as_ref()
    }

    #[must_use]
    pub fn kind(&self) -> CommandAuthorityKind {
        match &self.lane {
            CommandAuthorityLane::Principal(_) => CommandAuthorityKind::Principal,
            CommandAuthorityLane::AgentLane => CommandAuthorityKind::AgentLane,
            CommandAuthorityLane::Internal => CommandAuthorityKind::Internal,
        }
    }

    /// The console principal, iff this is the principal lane.
    #[must_use]
    pub(crate) fn control_principal(&self) -> Option<&MobControlPrincipal> {
        match &self.lane {
            CommandAuthorityLane::Principal(principal) => Some(principal),
            CommandAuthorityLane::AgentLane | CommandAuthorityLane::Internal => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const ALL_SCOPES: [ControlScope; 10] = [
        ControlScope::List,
        ControlScope::ReadHistory,
        ControlScope::SubscribeEvents,
        ControlScope::SendCommand,
        ControlScope::Cancel,
        ControlScope::Retire,
        ControlScope::WireTopology,
        ControlScope::Live,
        ControlScope::AdminHost,
        ControlScope::AdminGrants,
    ];

    const ALL_WIRE_SCOPES: [WireControlScope; 10] = [
        WireControlScope::List,
        WireControlScope::ReadHistory,
        WireControlScope::SubscribeEvents,
        WireControlScope::SendCommand,
        WireControlScope::Cancel,
        WireControlScope::Retire,
        WireControlScope::WireTopology,
        WireControlScope::Live,
        WireControlScope::AdminHost,
        WireControlScope::AdminGrants,
    ];

    fn external(id: &str) -> MobControlPrincipal {
        MobControlPrincipal::External(PrincipalId::new(id).expect("valid principal id"))
    }

    fn granted_state(
        principal: &str,
        scopes: &[ControlScope],
        expires_at_ms: Option<u64>,
    ) -> mob_dsl::MobMachineState {
        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::GrantOperatorScopes {
                principal: mob_dsl::PrincipalId::from(principal),
                scopes: scopes.iter().copied().collect(),
                expires_at_ms,
            },
        )
        .expect("grant accepted");
        authority.state().clone()
    }

    // ── P-1: owner ⇒ implicit full, grant rows inert ─────────────────────

    #[test]
    fn owner_is_full_and_ignores_grant_rows() {
        // Even an EXPIRED grant row keyed by some principal changes nothing
        // for the owner: owner-hood never consults grant records.
        let state = granted_state("console:someone", &[ControlScope::List], Some(1));
        let policy = ResolvedControlPolicy::resolve(&MobControlPrincipal::Owner, &state, u64::MAX);
        assert!(policy.is_owner_full());
        for scope in ALL_SCOPES {
            assert!(policy.permits(scope));
            policy.require(scope).expect("owner passes every scope");
        }
    }

    // ── P-2: unresolvable / ungranted ⇒ ∅ ────────────────────────────────

    #[test]
    fn unresolved_and_ungranted_principals_resolve_to_empty() {
        let state = granted_state("console:granted", &[ControlScope::List], None);
        for principal in [
            external("console:stranger"),
            MobControlPrincipal::Unresolved,
        ] {
            let policy = ResolvedControlPolicy::resolve(&principal, &state, 0);
            assert!(!policy.is_owner_full());
            for scope in ALL_SCOPES {
                assert!(!policy.permits(scope));
                let denial = policy
                    .require(scope)
                    .expect_err("empty effective set denies every scope");
                assert_eq!(denial.required, scope);
                assert!(
                    denial.presented.is_empty(),
                    "denial must present the empty set"
                );
            }
        }
    }

    // ── P-3: per-scope matrix at the policy level ────────────────────────

    #[test]
    fn each_scope_grants_exactly_itself() {
        for granted in ALL_SCOPES {
            let state = granted_state("console:one", &[granted], None);
            let policy = ResolvedControlPolicy::resolve(&external("console:one"), &state, 0);
            assert!(policy.permits(granted));
            policy.require(granted).expect("granted scope passes");
            for other in ALL_SCOPES {
                if other == granted {
                    continue;
                }
                let denial = policy
                    .require(other)
                    .expect_err("non-granted scope must deny");
                assert_eq!(denial.required, other);
                assert_eq!(
                    denial.presented,
                    BTreeSet::from([granted]),
                    "presented must be the caller's own effective set"
                );
            }
        }
    }

    // ── P-4: semantic pairs (§8) ─────────────────────────────────────────

    #[test]
    fn semantic_scope_pairs_stay_distinct() {
        let pairs: &[(&[ControlScope], ControlScope)] = &[
            // Drives-but-can't-read.
            (&[ControlScope::SendCommand], ControlScope::ReadHistory),
            // Watches-but-can't-stop.
            (&[ControlScope::SubscribeEvents], ControlScope::Cancel),
            // Topology admin is not command dispatch.
            (&[ControlScope::WireTopology], ControlScope::SendCommand),
            // AdminHost is bind/revoke hosts ONLY (A9).
            (&[ControlScope::AdminHost], ControlScope::AdminGrants),
            // Grant administration does not confer host administration.
            (&[ControlScope::AdminGrants], ControlScope::AdminHost),
        ];
        for (granted, denied) in pairs {
            let state = granted_state("console:pair", granted, None);
            let policy = ResolvedControlPolicy::resolve(&external("console:pair"), &state, 0);
            for scope in *granted {
                assert!(policy.permits(*scope));
            }
            let denial = policy.require(*denied).expect_err("pair scope must deny");
            assert_eq!(denial.required, *denied);
        }
    }

    // ── P-5: expiry boundary (expired ⟺ expires_at_ms <= now_ms) ─────────

    #[test]
    fn expiry_is_inclusive_at_the_deadline_and_presents_empty() {
        let state = granted_state("console:exp", &[ControlScope::List], Some(1_000));
        let principal = external("console:exp");

        let live = ResolvedControlPolicy::resolve(&principal, &state, 999);
        assert!(live.permits(ControlScope::List));

        for now_ms in [1_000, 1_001, u64::MAX] {
            let expired = ResolvedControlPolicy::resolve(&principal, &state, now_ms);
            assert!(!expired.permits(ControlScope::List));
            let denial = expired
                .require(ControlScope::List)
                .expect_err("expired grant denies");
            assert!(
                denial.presented.is_empty(),
                "expired presents the empty set (indistinguishable from never-granted)"
            );
        }

        let unexpiring = granted_state("console:exp", &[ControlScope::List], None);
        let policy = ResolvedControlPolicy::resolve(&principal, &unexpiring, u64::MAX);
        assert!(
            policy.permits(ControlScope::List),
            "None expiry never expires"
        );
    }

    // ── P-6: desynced expiry row fail-closes to ∅ ────────────────────────

    #[test]
    fn desynced_expiry_row_fails_closed_to_empty() {
        let mut state = granted_state("console:desync", &[ControlScope::List], None);
        // The machine inserts/removes both maps in lockstep; a desynced row
        // is only constructible by direct field surgery. The policy must
        // fail closed rather than trust the orphaned scopes row.
        state
            .operator_grant_expiries
            .remove(&mob_dsl::PrincipalId::from("console:desync"));
        let policy = ResolvedControlPolicy::resolve(&external("console:desync"), &state, 0);
        for scope in ALL_SCOPES {
            assert!(!policy.permits(scope));
        }
        let denial = policy
            .require(ControlScope::List)
            .expect_err("desynced row denies");
        assert!(denial.presented.is_empty());
    }

    // ── P-7: Live readiness, ten-variant pin, wire round-trip ────────────

    #[test]
    fn live_scope_is_generic_and_distinct() {
        let live_state = granted_state("console:live", &[ControlScope::Live], None);
        let live = ResolvedControlPolicy::resolve(&external("console:live"), &live_state, 0);
        live.require(ControlScope::Live)
            .expect("Live-granted principal passes require(Live)");

        let non_live_state = granted_state(
            "console:live",
            &[ControlScope::SendCommand, ControlScope::SubscribeEvents],
            None,
        );
        let non_live =
            ResolvedControlPolicy::resolve(&external("console:live"), &non_live_state, 0);
        let denial = non_live
            .require(ControlScope::Live)
            .expect_err("SendCommand+SubscribeEvents does not confer Live");
        assert_eq!(denial.required, ControlScope::Live);
        assert_eq!(denial.required_name(), "live");
    }

    #[test]
    fn scope_vocabulary_is_ten_variants_with_total_wire_round_trip() {
        // Machine → wire → machine identity over the closed vocabulary.
        for scope in ALL_SCOPES {
            assert_eq!(ControlScope::from(scope.to_wire()), scope);
        }
        // Wire → machine → wire identity.
        for wire in ALL_WIRE_SCOPES {
            assert_eq!(ControlScope::from(wire).to_wire(), wire);
        }
        // Exactly ten distinct names; no v2 scopes (`Approve`,
        // `RewriteTranscript`) exist in v1.
        let names: BTreeSet<&'static str> = ALL_SCOPES.iter().map(|scope| scope.name()).collect();
        assert_eq!(names.len(), 10);
        assert!(!names.contains("approve"));
        assert!(!names.contains("rewrite_transcript"));
    }

    // ── P-8: owner-bridge-session derivation ─────────────────────────────

    #[test]
    fn from_owner_bridge_session_matches_only_the_owner_fact() {
        let owner_session = SessionId::new();
        let other_session = SessionId::new();

        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::BindOwnerBridgeSession {
                bridge_session_id: mob_dsl::SessionId::from_domain(&owner_session),
                destroy_on_owner_archive: false,
                implicit_delegation_mob: false,
            },
        )
        .expect("owner bind accepted");
        let bound_state = authority.state().clone();

        assert_eq!(
            MobControlPrincipal::from_owner_bridge_session(&owner_session, &bound_state),
            Some(MobControlPrincipal::Owner)
        );
        assert_eq!(
            MobControlPrincipal::from_owner_bridge_session(&other_session, &bound_state),
            None,
            "a non-owner session derives NO principal (agent lane), not Unresolved"
        );

        let unbound_state = mob_dsl::MobMachineAuthority::new().state().clone();
        assert_eq!(
            MobControlPrincipal::from_owner_bridge_session(&owner_session, &unbound_state),
            None,
            "an unset owner fact derives no principal"
        );
    }

    // ── Denial carriers: one shape across bridge and console ─────────────

    #[test]
    fn scope_denial_converts_to_both_wire_carriers_deterministically() {
        let denial = ScopeDenial {
            required: ControlScope::AdminGrants,
            presented: BTreeSet::from([ControlScope::SendCommand, ControlScope::List]),
        };

        let cause = BridgeRejectionCause::from(&denial);
        assert_eq!(
            cause,
            BridgeRejectionCause::ScopeDenied {
                required: WireControlScope::AdminGrants,
                // BTreeSet order: List < SendCommand (declaration order).
                presented: vec![WireControlScope::List, WireControlScope::SendCommand],
            }
        );

        let detail = WireScopeDeniedDetail::from(&denial);
        assert_eq!(detail.required, WireControlScope::AdminGrants);
        assert_eq!(
            detail.presented,
            vec![WireControlScope::List, WireControlScope::SendCommand]
        );
    }

    #[test]
    fn operator_grant_projects_raw_wire_records() {
        let grant = OperatorGrant {
            principal: "console:luka".to_string(),
            scopes: BTreeSet::from([ControlScope::ReadHistory, ControlScope::List]),
            expires_at_ms: Some(5),
        };
        let wire = grant.to_wire();
        assert_eq!(wire.principal, "console:luka");
        assert_eq!(
            wire.scopes,
            vec![WireControlScope::List, WireControlScope::ReadHistory]
        );
        assert_eq!(
            wire.expires_at_ms,
            Some(5),
            "expiry rides verbatim — the projection never evaluates it"
        );
    }
}
