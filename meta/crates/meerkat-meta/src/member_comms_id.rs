//! Single owner of the mapping between Meta's public member aliases and
//! the mob-roster member ids handed to meerkat-mob.
//!
//! Meerkat 0.7 made the mob member comms name a fail-closed typed owner
//! (`meerkat_core::connection::MemberCommsName`): every component — including
//! the member id — must start with an ASCII letter or `_` and contain only
//! ASCII alphanumerics, `-`, or `_`. Meta's identity-first surface mints
//! runtime-id-shaped member identities (`rt:{identity}:{generation}`, where
//! the durable identity itself conventionally contains `:`, e.g.
//! `review:singleton`), which 0.6.34 accepted (the comms name was an
//! unvalidated `format!`) and 0.7 rejects at spawn.
//!
//! The public alias space is unchanged: consoles, RPC surfaces, SDKs, and
//! persisted continuity records keep speaking `rt:review:singleton:0`. This
//! module encodes those aliases into comms-safe roster ids at the
//! meta→meerkat-mob boundary and decodes roster ids back to public aliases
//! at projection boundaries.
//!
//! Encoding contract:
//! - An alias that is already a valid comms-name component (and does not
//!   start with the reserved `mk--` marker) maps to itself, so plain
//!   definition-mob member names are untouched.
//! - Anything else maps to `mk--` + escaped body, where `_` → `__`,
//!   `:` → `_c`, and any other non-`[A-Za-z0-9-]` char → `_x{hex}_`.
//! - `decode(encode(s)) == s` for every alias, and `decode` is the identity
//!   on ids that were never encoded. The `mk--` prefix is a reserved
//!   namespace: user-chosen member names must not start with it (encode
//!   re-encodes such names so the round-trip still holds).
//!
//! This is a **public stability surface**: consumers that talk to raw
//! `MobHandle` APIs (which speak the encoded roster-id space on meerkat 0.7+)
//! must encode aliases on the way in and decode roster ids on the way out
//! using exactly this codec — there is no separate "correct" encoding. Use
//! [`mob_member_id`]/[`mob_member_id_str`] at the meta→meerkat-mob boundary
//! and [`runtime_alias_str`]/[`runtime_event_alias`] at projection boundaries.
//!
//! # The member-id alias rule
//!
//! Every caller-supplied member id crosses into the roster by one rule:
//!
//! 1. Decode first ([`runtime_alias_str`]): an encoded roster id is never
//!    encoded a second time. Public JSON-RPC ingress still refuses the
//!    encoded spelling before any access check, so decoding never widens
//!    what a policy sees.
//! 2. An id the identity runtime owns names that identity's durable roster
//!    row ([`roster_member_id_for_supplied_id`]): a durable identity, or a
//!    generated alias the identity authority confirms is the current
//!    incarnation. A lifecycle operation confirms that under the
//!    identity's lifecycle lock, so the generation cannot change between
//!    the check and the act.
//! 3. A read maps a supplied id the same way. Where its path has an
//!    identity runtime, it first refuses a stale alias
//!    (`stale_identity_runtime_binding`).
//! 4. An action with no identity authority never collapses a generated
//!    alias onto the current incarnation: it refuses the alias, or, in a
//!    fallback that only matches names, keeps its exact spelling.
//! 5. [`mob_member_id`] stays only where the input is already canonical: a
//!    roster row, a decoded resolver result, a durable identity from an
//!    identity record, or a spawn reservation.

use std::borrow::Cow;

/// Return a safe durable-identity projection from raw member labels.
/// `rt:*` belongs exclusively to generated runtime aliases and must never be
/// minted by a caller-controlled label on an unrelated raw member.
pub(crate) fn durable_identity_label(
    labels: &std::collections::BTreeMap<String, String>,
) -> Option<&str> {
    labels
        .get("agent_identity")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_reserved_generated_alias(value))
}

/// Validate labels accepted by raw member-creation surfaces.
pub(crate) fn validate_raw_identity_labels(
    labels: &std::collections::BTreeMap<String, String>,
) -> Result<(), &'static str> {
    if labels.contains_key("agent_identity") {
        return Err(
            "labels.agent_identity is runtime-authoritative and may not be supplied by raw member creation",
        );
    }
    Ok(())
}

/// Reserved marker prefix for encoded member ids.
const MARKER: &str = "mk--";

/// True when `s` is a valid meerkat 0.7 comms-name component
/// (mirrors `meerkat_core::connection::validate_member_comms_name_component`).
fn is_valid_comms_component(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn escape_body(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '_' => out.push_str("__"),
            ':' => out.push_str("_c"),
            c if c.is_ascii_alphanumeric() || c == '-' => out.push(c),
            c => {
                out.push_str("_x");
                out.push_str(&format!("{:x}", c as u32));
                out.push('_');
            }
        }
    }
    out
}

/// Decode an escaped body; `None` when the body is not a well-formed escape
/// production (such an id cannot have come from [`mob_member_id_str`]).
fn unescape_body(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '_' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '_' => out.push('_'),
            'c' => out.push(':'),
            'x' => {
                let mut hex = String::new();
                loop {
                    match chars.next()? {
                        '_' => break,
                        h => hex.push(h),
                    }
                }
                let code = u32::from_str_radix(&hex, 16).ok()?;
                out.push(char::from_u32(code)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Map a public member alias (identity-first runtime id, durable identity, or
/// plain member name) to the comms-safe mob roster member id string.
pub fn mob_member_id_str(alias: &str) -> Cow<'_, str> {
    if is_valid_comms_component(alias) && !alias.starts_with(MARKER) {
        Cow::Borrowed(alias)
    } else {
        Cow::Owned(format!("{MARKER}{}", escape_body(alias)))
    }
}

/// Map a public member alias to a typed mob roster member id.
/// # This encodes WHATEVER YOU GIVE IT
///
/// Passing a runtime alias (`rt:review:singleton:0`) produces a VALID roster id
/// for a member that does not exist, because since the stable-identity lowering
/// the roster is keyed by the bare durable identity. Nothing fails at the call -
/// it surfaces later as "mob member not found" about a healthy member, or worse
/// as a well-formed status for a member never looked up.
///
/// If you have an identity and want its roster row, use
/// [`roster_member_id_for_identity`]. If you have a caller-supplied string of
/// unknown shape, decode first.
pub fn mob_member_id(alias: &str) -> meerkat_mob::ids::AgentIdentity {
    meerkat_mob::ids::AgentIdentity::from(mob_member_id_str(alias).as_ref())
}

/// Map a mob roster member id back to the public member alias. Identity on
/// ids that were never encoded.
pub fn runtime_alias_str(member_id: &str) -> Cow<'_, str> {
    match member_id.strip_prefix(MARKER) {
        Some(body) => match unescape_body(body) {
            Some(alias) => Cow::Owned(alias),
            // Not an encode production; treat as a literal member id.
            None => Cow::Borrowed(member_id),
        },
        None => Cow::Borrowed(member_id),
    }
}

/// Whether an input names Meta's reserved generated-runtime namespace.
///
/// Raw mob surfaces may receive either the public alias (`rt:...`) or the
/// comms-safe roster encoding (`mk--rt_c...`). Decode first so the latter
/// cannot bypass identity authority checks.
pub(crate) fn is_reserved_generated_alias(member_id: &str) -> bool {
    runtime_alias_str(member_id).starts_with("rt:")
}

/// `rt:{identity}:{generation}` → `{identity}`. The identity itself may
/// contain `:` (e.g. `review:singleton`), so the generation is the trailing
/// numeric segment (the `rpc_runtime_alias_generation` shape). `None` when
/// the input is not a generated runtime alias.
pub(crate) fn durable_identity_from_runtime_alias(alias: &str) -> Option<String> {
    let rest = alias.strip_prefix("rt:")?;
    let (identity, generation) = rest.rsplit_once(':')?;
    if identity.is_empty() || generation.parse::<u64>().is_err() {
        return None;
    }
    Some(identity.to_string())
}

/// The durable identity a mob roster member id names, for callers outside
/// this crate that receive a roster id from meerkat and must speak identity.
///
/// meerkat reports a member by its roster id (`MobMemberBinding::member`,
/// e.g. a fork child's `ForkBuildSource::source_member`). In a Meta mob
/// that is the comms-safe encoding of the member's durable identity
/// (`mk--domain_ccalendar`), never the identity itself (`domain:calendar`).
/// This inverts Meta's own mapping exactly as every in-crate producer does
/// (`logical_memory_identity`): decode the encoding, strip a generated
/// runtime-alias shape, and leave plain names unchanged.
pub fn durable_identity_for_roster_member(member_id: &str) -> String {
    logical_memory_identity(member_id)
}

/// The LOGICAL identity for a mob-plane member id or public alias: decode
/// the comms-safe roster encoding, then strip the generated runtime-alias
/// shape to the durable identity. Identity on plain names.
///
/// This is the ONE identity spelling the memory stack keys scopes on
/// (`MemoryScope::Identity`), the write gate queries, and the SDK
/// `agent_memory` surface speaks. Every producer that receives a mob-plane
/// member id (the observer fan-out, trigger sinks, the classic spawn
/// customizer, the dispatch-taint join) normalizes through here - never a
/// local re-parse.
pub(crate) fn logical_memory_identity(member_id_or_alias: &str) -> String {
    let alias = runtime_alias_str(member_id_or_alias);
    durable_identity_from_runtime_alias(&alias).unwrap_or_else(|| alias.into_owned())
}

/// The PUBLIC runtime alias for one incarnation: `rt:{identity}:{generation}`.
///
/// ONE owner for "what does this incarnation look like to a console, an SDK, an
/// RPC surface, or a persisted continuity record". The module contract above
/// pins that space to the `rt:` shape and states it is PERSISTED, so it must
/// not drift with whatever the roster happens to be keyed by.
///
/// Before the stable-identity lowering the roster identity WAS the runtime
/// alias, so `meerkat_mob::ids::AgentRuntimeId`'s own Display - which writes
/// `{identity}:{generation}` - already produced the `rt:` shape for free. Now
/// the roster identity is the bare durable identity, so that same Display
/// silently drops the marker and emits `reviewer:1` where every consumer, and
/// every already-persisted record, expects `rt:reviewer:1`. Nothing fails at
/// the mint: it surfaces later as a status field that no longer round-trips
/// through `durable_identity_from_runtime_alias`, which requires the prefix.
///
/// Idempotent by construction: a legacy binding whose roster identity still
/// carries the marker is not prefixed twice. That is the same
/// non-idempotency hazard `mob_member_id_str` carries, and the reason this is
/// a function rather than a `format!` at each call site.
pub(crate) fn public_runtime_alias(runtime_id: &meerkat_mob::ids::AgentRuntimeId) -> String {
    let identity = runtime_alias_str(runtime_id.identity.as_str());
    let generation = runtime_id.generation.get();
    if identity.starts_with("rt:") {
        format!("{identity}:{generation}")
    } else {
        format!("rt:{identity}:{generation}")
    }
}

/// The roster member id for WHATEVER SPELLING A CALLER SUPPLIED.
///
/// Callers hand us three shapes and all of them are legitimate, because our own
/// surfaces hand them out: the bare durable identity (`review:singleton`), the
/// public runtime alias (`rt:review:singleton:0`, which `status_identity`
/// RETURNS), and the comms-safe roster encoding (`mk--...`, which mob-plane
/// projections carry). The roster is keyed by exactly one of those.
///
/// So decode before encoding: strip the comms marker, then reduce a runtime
/// alias to the durable identity it names, then encode. Skipping the middle step
/// is silent - it produces a well-formed roster id for an identity that does not
/// exist, and the caller gets "member not found" for a healthy member. That is
/// the shape of the defect this function exists to remove, found when a caller
/// round-tripped `agent_runtime_id` from a status call straight back into a
/// member lookup, which is the obvious thing to do.
///
/// PUBLIC, and the difference from [`roster_member_id_for_identity`] is which
/// spelling the caller HAS, not which is preferred. An embedder that already
/// holds the durable identity should call that one and say so. An embedder
/// holding a value it read back from a surface - `status_identity`, a console
/// projection, a persisted binding - has no way to know which of the three
/// shapes it is holding, and should call this.
///
/// It was `pub(crate)` while being the only correct answer for an embedder that
/// only has an alias, which left the obvious mistake as the only reachable
/// option. The operator deployment asked which function to call and there was no right answer to
/// give them.
///
/// # NOT for deciding whether two values agree
///
/// This answers "which roster row" and deliberately COLLAPSES generation:
/// `rt:x:0` and `rt:x:7` return the same row, because generation is incarnation
/// detail and a lookup must not miss a healthy member over it.
///
/// That makes it wrong for any check that asks "do these two persisted values
/// agree" - a tamper boundary, a binding-vs-session postcondition, an equality
/// gate on two stored fields. Routed through here, a pair differing ONLY in
/// generation compares equal, so a substituted value passes. Meerkat hit exactly
/// this at 0.8.29: a matcher made tolerant for lookup accepted binding/comms
/// aliases with the same durable identity and different legacy generations, and
/// an existing tamper test caught it.
///
/// Same function, two questions, opposite requirements. For agreement checks,
/// compare the values as stored and let the difference be a difference. No
/// caller in this crate does otherwise today - the other users of the underlying
/// strict derivation produce identities rather than compare them - but this
/// function is public now, so the boundary is documented rather than merely
/// true.
pub fn roster_member_id_for_supplied_id(supplied: &str) -> meerkat_mob::ids::AgentIdentity {
    let decoded = runtime_alias_str(supplied);
    let durable = durable_identity_from_runtime_alias(decoded.as_ref())
        .unwrap_or_else(|| decoded.into_owned());
    mob_member_id(&durable)
}

/// The stable roster member id for a durable identity.
///
/// ONE spelling for "which roster row is this identity". Every site that used
/// to turn `IdentityStatus::agent_runtime_id` into a roster key must come here
/// instead: since the stable-identity lowering the roster is keyed by the
/// encoded DURABLE identity, and an `AgentRuntimeId` is binding detail that no
/// longer names a roster row at all. A site that keeps the old conversion does
/// not fail loudly - it silently misses and its surface reports "not found" for
/// a healthy member.
pub fn roster_member_id_for_identity(identity: &str) -> meerkat_mob::ids::AgentIdentity {
    mob_member_id(identity)
}

/// Whether a live roster member id names a given durable identity.
///
/// The identity half of [`live_binding_matches_identity`], for call sites that
/// have no session to compare. Decodes the comms-safe encoding and nothing
/// else: it must not strip an `rt:{identity}:{generation}` shape, or a stale
/// generated alias would be accepted as the stable roster identity.
pub(crate) fn live_member_is_identity(live_member_id: &str, identity: &str) -> bool {
    runtime_alias_str(live_member_id) == identity
}

/// Whether the roster materialized `binding`'s member: its spawn carried the
/// `agent_identity` label naming the identity its member id encodes. Every
/// identity-first spawn of a roster identity writes that label (Meta 0.9.3's
/// included) and the raw member creation surfaces refuse it, and Core keeps
/// each incarnation's spawn labels in its member history. A member spawned
/// at runtime (by mob tools, `delegate` or a helper) never carries it, even
/// when its profile makes it durable: the roster pass never retires such a
/// member, which ends only through its own lifecycle.
pub(crate) fn binding_materialized_by_roster(binding: &meerkat_mob::MemberBinding) -> bool {
    durable_identity_label(binding.labels())
        .is_some_and(|identity| live_member_is_identity(binding.identity().as_str(), identity))
}

/// Whether a live roster member id names an identity OR its currently
/// registered incarnation.
///
/// The session-less form of [`live_binding_matches_identity`], for control gates
/// that have no session to compare. Same two admissible spellings: the id
/// decodes to the durable identity, or it is EXACTLY the registered
/// `AgentRuntimeId`. A caller legitimately holding a current runtime alias must
/// still be able to name its member; a STALE alias does not equal the registered
/// one and is still refused.
pub(crate) fn live_member_names_identity_or_incarnation(
    live_member_id: &str,
    identity: &str,
    registered_runtime_id: Option<&str>,
) -> bool {
    live_member_is_identity(live_member_id, identity)
        || registered_runtime_id
            .is_some_and(|registered| runtime_alias_str(live_member_id).as_ref() == registered)
}

/// Whether a live roster member and session are the binding that a given
/// identity is registered for.
///
/// ONE rule, used by every surface that guards an identity-control call, so a
/// stale binding cannot be accepted on one plane and refused on another.
///
/// Identity side: the live roster id is DECODED ONLY - the comms-safe `mk--`
/// encoding is undone and nothing else. It deliberately does not strip an
/// `rt:{identity}:{generation}` shape. After the stable-identity lowering a
/// live roster id decodes exactly to the durable identity, so tolerating a
/// generated runtime spelling here would re-admit the very stale binding this
/// check exists to catch.
///
/// Session side: EXACT equality. `Some`/`Some` must carry the same value and
/// `None`/`None` matches, but a one-sided `None` fails closed - a missing
/// session on either side is an unknown, and an unknown is not a match.
///
/// `AgentRuntimeId` is not consulted. It remains binding bookkeeping and
/// presence detail; it is not the roster spelling and Meerkat owns its
/// generation.
pub(crate) fn live_binding_matches_identity(
    live_member_id: &str,
    live_session_id: Option<&str>,
    identity: &str,
    registered_session_id: Option<&str>,
    registered_runtime_id: Option<&str>,
) -> bool {
    // Two ways a live member id can be the right one, and only two.
    //
    // Either it decodes to the durable identity - the stable roster row - or it
    // is EXACTLY the runtime id this binding is registered for, which is a
    // caller naming the current incarnation. The second is exact equality, not
    // generation-stripping: a STALE generated alias does not equal the
    // registered one, so it is still refused. Accepting any rt:{id}:{gen} whose
    // identity part matched would be the hole worth avoiding.
    let names_identity = live_member_is_identity(live_member_id, identity);
    let is_registered_incarnation = registered_runtime_id
        .is_some_and(|registered| runtime_alias_str(live_member_id).as_ref() == registered);
    (names_identity || is_registered_incarnation) && live_session_id == registered_session_id
}

/// Whether a caller supplied the comms-safe roster marker directly.
/// Public surfaces speak aliases, never encoded roster ids; accepting this
/// spelling at a raw lower-plane creation boundary can collide with the
/// projection of the corresponding decoded alias.
pub(crate) fn uses_reserved_roster_marker(member_id: &str) -> bool {
    member_id.trim().starts_with(MARKER)
}

/// Validate one caller-supplied member/identity target at a public ingress.
///
/// Public APIs speak aliases. The comms-safe `mk--*` roster spelling is an
/// implementation detail and must be rejected before ABAC, dispatch, or
/// alias resolution; otherwise a policy written against the decoded alias
/// can be bypassed by presenting its encoded spelling.
pub(crate) fn validate_public_member_alias(field: &str, value: &str) -> Result<(), String> {
    if uses_reserved_roster_marker(value) {
        return Err(format!(
            "{field} may not use the reserved encoded roster-id namespace"
        ));
    }
    Ok(())
}

/// Validate every member/identity target field understood by public JSON-RPC
/// surfaces. Keep this list at the ingress choke point so new handlers cannot
/// accidentally perform ABAC against an encoded roster id and decode it only
/// afterwards.
pub(crate) fn validate_public_rpc_member_aliases(params: &serde_json::Value) -> Result<(), String> {
    const TARGET_FIELDS: &[&str] = &[
        "identity",
        "member_id",
        "agent_id",
        "agent_identity",
        "meerkat_id",
        "local_member_id",
        "remote_member_id",
        "from_member_id",
        "source_member_id",
        "runtime_member_id",
        "agent_runtime_id",
    ];

    let Some(params) = params.as_object() else {
        return Ok(());
    };
    for field in TARGET_FIELDS {
        if let Some(value) = params.get(*field).and_then(serde_json::Value::as_str) {
            validate_public_member_alias(field, value)?;
        }
    }
    // `meta/run_flow` names the members of each bound role.
    if let Some(roles) = params
        .get("role_targets")
        .and_then(serde_json::Value::as_object)
    {
        for member in roles
            .values()
            .filter_map(serde_json::Value::as_array)
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            validate_public_member_alias("role_targets", member)?;
        }
    }
    Ok(())
}

/// Admit a caller-chosen member id to the raw mob plane.
///
/// Generated aliases and encoded roster ids are runtime-owned namespaces.
/// Once an identity runtime is present, a registered plain durable identity
/// is runtime-owned as well. Raw creation surfaces must use this single check
/// before spawning, ensuring, attaching, or forking a member so they cannot
/// create a second lower-plane owner behind continuity and lease authority.
pub(crate) async fn validate_raw_member_target(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_id: &str,
) -> Result<String, String> {
    let member_id = member_id.trim();
    let alias = runtime_alias_str(member_id).into_owned();
    if uses_reserved_roster_marker(member_id) || is_reserved_generated_alias(&alias) {
        return Err(format!(
            "member id '{member_id}' uses an identity-runtime reserved namespace"
        ));
    }
    refuse_runtime_plane_member(&mob_member_id_str(&alias), None)?;
    if let Some(identity_runtime) = identity_runtime
        && let Some(identity) = identity_runtime.identity_for_member_mutation(&alias).await
    {
        return Err(format!(
            "member id '{alias}' is owned by durable identity '{identity}'"
        ));
    }
    Ok(alias)
}

/// The single creation check for the console's runtime-plane identities.
///
/// The console treats frames under `_system` and `__console__` as the
/// runtime's own events and exempts them from member visibility, so no member
/// may have its events attributed to either. The check evaluates the
/// identities console attribution itself computes for a new member with this
/// roster id and these labels ([`runtime_plane_console_identity`]), never a
/// spelling normalized beside them.
pub(crate) fn refuse_runtime_plane_member(
    roster_member_id: &str,
    labels: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<(), String> {
    let runtime_id = meerkat_mob::ids::AgentRuntimeId::initial(
        meerkat_mob::ids::AgentIdentity::from(roster_member_id),
    );
    match runtime_plane_console_identity(roster_member_id, &runtime_id, labels) {
        Some(identity) => Err(format!(
            "member id '{}' is reserved: the console attributes its events to the runtime-plane identity '{identity}'",
            runtime_alias_str(roster_member_id.trim())
        )),
        None => Ok(()),
    }
}

/// [`refuse_runtime_plane_member`] for the member an identity-first identity
/// materializes as: its roster id encodes the durable identity and its
/// `agent_identity` label names it.
pub(crate) fn refuse_runtime_plane_identity(
    identity: &crate::identity_first::AgentIdentity,
) -> Result<(), String> {
    let labels = std::collections::BTreeMap::from([(
        "agent_identity".to_string(),
        identity.as_str().to_string(),
    )]);
    refuse_runtime_plane_member(&mob_member_id_str(identity.as_str()), Some(&labels))
}

/// The runtime-plane identity, if any, that console attribution gives a
/// member with this roster id, runtime id and labels: the identity its spawn
/// projection registers
/// ([`crate::console_spawn::spawn_seed_console_identity`]) or the one its
/// agent events derive from the runtime id
/// ([`crate::unified_runtime::console_events::attributed_identity_for_runtime`]).
pub(crate) fn runtime_plane_console_identity(
    roster_member_id: &str,
    runtime_id: &meerkat_mob::ids::AgentRuntimeId,
    labels: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<String> {
    let no_labels = std::collections::BTreeMap::new();
    let registered = crate::console_spawn::spawn_seed_console_identity(
        roster_member_id,
        labels.unwrap_or(&no_labels),
    );
    let derived =
        crate::unified_runtime::console_events::attributed_identity_for_runtime(runtime_id);
    std::iter::once(registered)
        .chain(derived)
        .find(|identity| crate::console_contracts::is_runtime_plane_identity(identity))
}

/// The spawn sources [`RuntimePlaneIdentityGuard`] admits without the
/// reserved-name check: the ones that rebuild or replay a member that already
/// exists. Every other source, including any a later Core adds, is checked.
const SOURCES_THAT_NAME_NO_NEW_MEMBER: [meerkat_mob::SpawnSource; 2] = [
    // Core's rebuild of an existing durable member: restart restore,
    // explicit resume, and a fork child's own-identity overlay. A host spawn
    // with a resume launch mode is reported as one too; every Meta route
    // checks the name before Core sees it, so only host code that calls a mob
    // handle directly passes here.
    meerkat_mob::SpawnSource::Resume,
    // Replays a spec sealed in an identity intent when the intent was
    // created, where this guard already ran. Core asks no customizer for it;
    // a refusal could stop a legacy member's reconcile if a later Core asks.
    meerkat_mob::SpawnSource::IdentityReconcile,
];

/// The guard's decision for one spawn reaching it from `source`: admitted when
/// the source names no new member, else refused when console attribution
/// would give the member one of its runtime-plane identities.
pub(crate) fn runtime_plane_spawn_check(
    source: meerkat_mob::SpawnSource,
    spec: &meerkat_mob::SpawnMemberSpec,
) -> Result<(), meerkat_mob::MobError> {
    if SOURCES_THAT_NAME_NO_NEW_MEMBER.contains(&source) {
        return Ok(());
    }
    refuse_runtime_plane_member(spec.identity.as_str(), spec.labels.as_ref())
        .map_err(meerkat_mob::MobError::WiringError)
}

/// Refuses every spawn whose member the console would attribute to a
/// runtime-plane identity (`_system`, `__console__`), judged as the spawn
/// reaches it. Compose it last in a mob's spawn customizer chain, after every
/// customizer that may rename or relabel a spawn; every mob a
/// [`crate::MobBootstrapSpec`] builds, and every child mob its agents create,
/// already has it so. A runtime adopted from a bare handle
/// ([`crate::RealMobRuntime::from_handle`]) does not, unless the handle's
/// builder composed it.
///
/// It checks every spawn source but the two in its allowlist, which rebuild
/// or replay a member that already exists (see
/// [`runtime_plane_spawn_check`]): fresh spawns, helpers, batch items, flow
/// and policy members, fork launches, council seats and respawns are all
/// checked. Core asks no spawn customizer when it seats a durable fork's
/// child; Meta's session service refuses that fork instead
/// ([`refuse_runtime_plane_fork_target`]). The resume it admits is how Core
/// rebuilds an existing durable member on restart or explicit resume, so a
/// member created under such a name before this check existed still comes
/// up; the console attributes none of its events to such a name, and the
/// runtime names it at startup and in `meta/status`.
///
/// Three host-only paths pass it. Core reports a host spawn with a resume
/// launch mode as a resume. Core asks no spawn customizer when it auto-spawns
/// a member the definition's spawn policy maps, for external work submitted
/// to a member the roster does not hold. Nor does it ask one when it seats a
/// council participant built with its source's fork build inheritance (the
/// seat's spec carries a fork source), so the seat keeps the target identity
/// its builder chose. Every Meta creation surface checks the name before Core
/// sees it, no Meta surface submits work to a member the roster does not
/// hold, and the agent council tool names its seats itself
/// (`council-p{order}`), so only host code that calls a mob handle or builds
/// a council request directly reaches these paths; the console drops that
/// member's events all the same, and `meta/status` names it.
pub struct RuntimePlaneIdentityGuard {
    _private: (),
}

impl RuntimePlaneIdentityGuard {
    /// The guard, as a spawn customizer to append last to a chain.
    pub fn customizer() -> std::sync::Arc<dyn meerkat_mob::SpawnMemberCustomizer> {
        std::sync::Arc::new(Self { _private: () })
    }
}

impl meerkat_mob::SpawnMemberCustomizer for RuntimePlaneIdentityGuard {
    fn customize_spawn(
        &self,
        ctx: &meerkat_mob::SpawnCustomizationContext,
        spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        runtime_plane_spawn_check(ctx.spawn_source, spec)
    }

    fn customize_resume(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        _durable: &meerkat_mob::ResumedMemberView,
        _spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        // Core asks this only to rebuild an existing durable member.
        Ok(())
    }
}

/// The spawn customizer of a mob built under `mob_id`: `customizer` followed
/// by [`RuntimePlaneIdentityGuard`], through the crate's one composer. The
/// guard runs last, so it judges each spawn as every earlier customizer
/// leaves it, and each customizer still gets its own resume hook.
pub(crate) fn runtime_plane_guarded_customizer(
    mob_id: &meerkat_mob::MobId,
    customizer: Option<std::sync::Arc<dyn meerkat_mob::SpawnMemberCustomizer>>,
) -> Option<std::sync::Arc<dyn meerkat_mob::SpawnMemberCustomizer>> {
    #[cfg(test)]
    if pre_check_mob_for_test(mob_id.as_str()) {
        return customizer;
    }
    #[cfg(not(test))]
    let _ = mob_id;
    Some(crate::identity_first::ComposedSpawnMemberCustomizer::over(
        customizer,
        RuntimePlaneIdentityGuard::customizer(),
    ))
}

/// Refuses a durable fork whose child would take one of the console's
/// runtime-plane identities. Core seats a durable fork's child (an agent's
/// `fork_off`, [`meerkat_mob::MobHandle::fork_member`]) without asking any
/// spawn customizer, so [`RuntimePlaneIdentityGuard`] never sees it; the
/// session service checks the fork's target member before it forks the
/// transcript.
pub(crate) fn refuse_runtime_plane_fork_target(
    target: &meerkat_core::DurableSessionForkTarget,
) -> Result<(), String> {
    #[cfg(test)]
    if pre_check_mob_for_test(&target.member_binding.mob_id) {
        return Ok(());
    }
    refuse_runtime_plane_member(&target.member_binding.member, None)
}

#[cfg(test)]
static PRE_CHECK_MOBS_FOR_TEST: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Test seam: while set for `mob_id`, a mob built under it gets no
/// runtime-plane guard and its session service forks any child, as a mob
/// built before these checks existed did, so a test can create the members
/// such a build left behind.
#[cfg(test)]
pub(crate) fn set_pre_check_mob_for_test(mob_id: &str, pre_check: bool) {
    let mut mobs = PRE_CHECK_MOBS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    mobs.retain(|id| id != mob_id);
    if pre_check {
        mobs.push(mob_id.to_string());
    }
}

#[cfg(test)]
fn pre_check_mob_for_test(mob_id: &str) -> bool {
    PRE_CHECK_MOBS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|id| id == mob_id)
}

/// A roster member the console attributes to a runtime-plane identity: one
/// created under such a name before every creation path refused them. It
/// keeps running, but the console records none of its events.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ReservedIdentityMember {
    /// The member's public id.
    pub(crate) member_id: String,
    /// The runtime id its events carry.
    pub(crate) runtime_id: String,
    /// The runtime-plane identity console attribution gives it.
    pub(crate) identity: String,
}

/// Every member of `handle`'s roster that console attribution gives a
/// runtime-plane identity.
pub(crate) async fn reserved_identity_members(
    handle: &meerkat_mob::MobHandle,
) -> Vec<ReservedIdentityMember> {
    handle
        .roster()
        .await
        .list()
        .filter_map(|entry| {
            runtime_plane_console_identity(
                entry.agent_identity.as_str(),
                &entry.agent_runtime_id,
                Some(&entry.labels),
            )
            .map(|identity| ReservedIdentityMember {
                member_id: runtime_alias_str(entry.agent_identity.as_str()).into_owned(),
                runtime_id: runtime_event_alias(&entry.agent_runtime_id),
                identity,
            })
        })
        .collect()
}

/// The `reserved_identity_members` field of `meta/status`: each member
/// [`reserved_identity_members`] names, or `None` (the field is omitted) when
/// the mob has none.
pub(crate) async fn reserved_identity_members_status(
    handle: &meerkat_mob::MobHandle,
) -> Option<serde_json::Value> {
    let reserved = reserved_identity_members(handle).await;
    if reserved.is_empty() {
        return None;
    }
    serde_json::to_value(reserved).ok()
}

/// Warn once per member of `handle`'s roster that holds a runtime-plane
/// identity.
pub(crate) async fn warn_reserved_identity_members(handle: &meerkat_mob::MobHandle) {
    for member in reserved_identity_members(handle).await {
        tracing::warn!(
            member_id = %member.member_id,
            runtime_id = %member.runtime_id,
            identity = %member.identity,
            "a member holds an identity the console reserves for its own events; it keeps \
             running, but the console records none of its events (meta/status lists it). \
             Retire it and recreate it under another name"
        );
    }
}

/// Reservation for one or more caller-chosen raw member aliases.
///
/// When an identity runtime is installed, the owned namespace guard keeps the
/// validation result stable until the raw member has been created. Durable
/// identity materialization takes the same guard before checking the raw
/// roster, so the two planes cannot both claim the same plain alias through a
/// validate-then-spawn race.
pub(crate) struct RawMemberTargetReservation {
    aliases: Vec<String>,
    _alias_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
}

impl RawMemberTargetReservation {
    pub(crate) fn alias(&self) -> &str {
        self.aliases.first().map(String::as_str).unwrap_or_default()
    }

    pub(crate) fn aliases(&self) -> &[String] {
        &self.aliases
    }
}

pub(crate) async fn reserve_raw_member_target(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_id: &str,
) -> Result<RawMemberTargetReservation, String> {
    reserve_raw_member_targets(identity_runtime, std::iter::once(member_id)).await
}

pub(crate) async fn reserve_raw_member_targets<'a>(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_ids: impl IntoIterator<Item = &'a str>,
) -> Result<RawMemberTargetReservation, String> {
    let member_ids = member_ids
        .into_iter()
        .map(str::trim)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if member_ids.is_empty() {
        return Err("raw member reservation requires at least one member id".to_string());
    }
    // Reject statelessly invalid and currently identity-owned targets before
    // allocating keyed locks. This preflight is not the authority boundary:
    // durable ownership can change while we wait, so the same validation is
    // repeated below after all canonical alias guards are held.
    for member_id in &member_ids {
        validate_raw_member_target(identity_runtime, member_id).await?;
    }
    let canonical_aliases = member_ids
        .iter()
        .map(|member_id| runtime_alias_str(member_id).into_owned())
        .collect::<Vec<_>>();
    let sorted_aliases = canonical_aliases
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();

    // Every multi-member caller takes the same deterministic alias order.
    // Duplicate spellings share one guard, while the returned aliases retain
    // request order so batch spawn results continue to line up with specs.
    let mut alias_guards = Vec::with_capacity(sorted_aliases.len());
    if let Some(identity_runtime) = identity_runtime {
        for alias in sorted_aliases {
            let lock = identity_runtime.raw_member_alias_lock(&alias).await;
            alias_guards.push(lock.lock_owned().await);
        }
    }

    // Re-run the full authority check only after all target locks are held.
    // Preserve the original trimmed spelling for reserved encoded-id checks;
    // successful values are the canonical aliases used by the lock map.
    let mut aliases = Vec::with_capacity(member_ids.len());
    for member_id in member_ids {
        aliases.push(validate_raw_member_target(identity_runtime, &member_id).await?);
    }
    debug_assert_eq!(aliases, canonical_aliases);
    Ok(RawMemberTargetReservation {
        aliases,
        _alias_guards: alias_guards,
    })
}

/// Project a mob runtime id (`{roster_member_id}:{generation}`) into the
/// public alias space (`{alias}:{generation}`).
///
/// Agent events leave meerkat-mob keyed by [`meerkat_mob::ids::AgentRuntimeId`]s built from
/// roster binding atoms; the member-id component is the comms-safe encoding,
/// so it must be decoded before any console/SDK projection — console replay
/// resolution, the `meta/events/subscribe` buffer, `/mob/events` SSE, and
/// per-agent ABAC view checks all speak the alias space.
pub fn runtime_event_alias(runtime_id: &meerkat_mob::ids::AgentRuntimeId) -> String {
    format!(
        "{}:{}",
        runtime_alias_str(runtime_id.identity.as_str()),
        runtime_id.generation.get()
    )
}

/// Deterministic canonical-UUID form of an APP-supplied delivery
/// correlation id (task #58, a downstream app admission break 2026-08-05):
/// meerkat 0.8.16 threads arbitrary app correlation strings ("telegram:
/// primary/i:769307582", connector source ids) into bridge admission,
/// where the delivery-identity contract requires a canonical UUID. A
/// string that already is one passes through byte-identical; anything
/// else canonicalizes as UUIDv5 under a fixed meta namespace, so equal
/// source strings dedup identically across boots and hosts and a
/// well-formed identity PAIR is never refused for its value shape.
///
/// Scope: the app-facing dispatch matrix only. The schedule lane keeps
/// its strict refusal - its correlation is the occurrence UUID by
/// construction, and a non-UUID there is a construction bug that must
/// fail typed, never be laundered.
pub(crate) fn canonical_correlation_id(correlation_id: &str) -> std::borrow::Cow<'_, str> {
    if uuid::Uuid::try_parse(correlation_id)
        .is_ok_and(|parsed| !parsed.is_nil() && parsed.to_string() == correlation_id)
    {
        return std::borrow::Cow::Borrowed(correlation_id);
    }
    let namespace = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        // A durable key, not a name: changing it would make a retry that
        // spans an upgrade miss its dedupe id, so it is unchanged by the
        // 0.9 rename.
        b"rkat-mobkit:delivery-correlation",
    );
    std::borrow::Cow::Owned(uuid::Uuid::new_v5(&namespace, correlation_id.as_bytes()).to_string())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A caller may hand back any spelling our own surfaces emit, and all three
    /// must reach the same roster row.
    ///
    /// This is the defect that shipped past a full green suite and was caught by
    /// a Python SDK test doing the obvious thing: take `agent_runtime_id` out of
    /// a status response and pass it to `get_member`. Encoding an alias without
    /// decoding it first produces a well-formed roster id for an identity that
    /// does not exist, so the caller is told "member not found" about a healthy
    /// member - a lie with no diagnostic in it.
    #[test]
    fn every_spelling_a_caller_can_supply_reaches_one_roster_row() {
        let durable = "review:singleton";
        let expected = roster_member_id_for_identity(durable);

        // 1. the bare durable identity
        assert_eq!(
            roster_member_id_for_supplied_id(durable),
            expected,
            "a bare durable identity must resolve to its own roster row"
        );
        // 2. the PUBLIC RUNTIME ALIAS, which status_identity returns
        assert_eq!(
            roster_member_id_for_supplied_id("rt:review:singleton:0"),
            expected,
            "the alias status_identity hands out must resolve to the same row"
        );
        assert_eq!(
            roster_member_id_for_supplied_id("rt:review:singleton:7"),
            expected,
            "generation is incarnation detail and must not change which row is named"
        );
        // 3. the comms-safe roster encoding, which mob-plane projections carry
        let encoded = mob_member_id_str(durable).into_owned();
        assert_eq!(
            roster_member_id_for_supplied_id(&encoded),
            expected,
            "the encoded roster id must round-trip to its own row"
        );
    }

    /// The STACKED form: a comms encoding wrapped around a runtime ALIAS rather
    /// than around a durable identity. Two encodings deep, alias innermost.
    ///
    /// Its own test, not another assertion appended to the spelling test. A
    /// single test function stops at its first failing assert, so a mutation
    /// that breaks an EARLIER case masks whether these two discriminate at all -
    /// which is exactly what happened when this was written inline: reversing
    /// the decode order failed the plain encoded-identity case first and these
    /// never ran.
    #[test]
    fn a_comms_encoded_runtime_alias_reaches_the_durable_roster_row() {
        let expected = roster_member_id_for_identity("review:singleton");

        let stacked = mob_member_id_str("rt:review:singleton:2").into_owned();
        assert_eq!(
            roster_member_id_for_supplied_id(&stacked),
            expected,
            "a comms encoding of a runtime alias must reach the durable row: {stacked}"
        );

        // the operator deployment's exact production value shape, spelled out by hand rather than constructed here
        // (only the address is a placeholder). Their typed MobMemberBinding.member and comms_name
        // both carry this, and theirs is the only store known to hold the stacked shape - so a
        // helper that agrees with itself about how to build the input cannot stand in for it. If
        // their encoding differs from what this crate would produce, that difference IS the bug and
        // constructing the input would hide it.
        let operator_persisted = "mk--rt_cperson_cjane_x2e_doe_x40_example_x2e_com_c2";
        assert_eq!(
            roster_member_id_for_supplied_id(operator_persisted),
            roster_member_id_for_identity("person:jane.doe@example.com"),
            "the operator deployment's persisted stacked alias must reach the durable person row"
        );
    }

    #[test]
    fn canonical_correlation_id_passes_canonical_uuids_and_canonicalizes_the_rest() {
        // A canonical UUID rides through byte-identical (the schedule
        // occurrence shape must never be rewritten).
        let occurrence = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
        assert_eq!(canonical_correlation_id(occurrence).as_ref(), occurrence);
        // App source strings canonicalize deterministically: equal in,
        // equal out; distinct in, distinct out; output is a canonical
        // non-nil UUID.
        let a1 = canonical_correlation_id("telegram:primary/i:769307582").into_owned();
        let a2 = canonical_correlation_id("telegram:primary/i:769307582").into_owned();
        let b = canonical_correlation_id("telegram:primary/i:769307583").into_owned();
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        let parsed = uuid::Uuid::try_parse(&a1).expect("canonical output");
        assert!(!parsed.is_nil());
        assert_eq!(parsed.to_string(), a1);
        // Non-canonical UUID SPELLINGS (uppercase, braced) and the nil
        // UUID also canonicalize rather than riding through.
        assert_ne!(
            canonical_correlation_id("7C9E6679-7425-40DE-944B-E07FC1F90AE7").as_ref(),
            "7C9E6679-7425-40DE-944B-E07FC1F90AE7"
        );
        assert_ne!(
            canonical_correlation_id("00000000-0000-0000-0000-000000000000").as_ref(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    fn raw_lock_test_runtime()
    -> Result<std::sync::Arc<crate::identity_first::IdentityRuntime>, Box<dyn std::error::Error>>
    {
        Ok(std::sync::Arc::new(
            crate::identity_first::IdentityRuntime::new(
                crate::identity_first::IdentityRuntimeConfig {
                    continuity_store: std::sync::Arc::new(
                        crate::identity_first::LocalContinuityStore::in_memory()?,
                    ),
                    lease_provider: std::sync::Arc::new(
                        crate::identity_first::LocalLeaseProvider::new(),
                    ),
                    runtime_instance_id: "raw-lock-test".to_string(),
                    has_runtime_store: true,
                    durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                    bridge: None,
                    default_timeout: None,
                },
            ),
        ))
    }

    #[test]
    fn plain_member_names_pass_through_unchanged() {
        for name in ["worker", "worker-one", "_internal", "Agent7"] {
            assert_eq!(mob_member_id_str(name), name);
            assert_eq!(runtime_alias_str(name), name);
        }
    }

    #[test]
    fn colon_aliases_round_trip_and_are_comms_safe() {
        for alias in [
            "rt:review:singleton:0",
            "rt:channel:C0SMOKEOPS:0",
            "agent:beta",
            "review:singleton",
            "rt:agent-x:0",
            "with_underscore:and:colons",
        ] {
            let encoded = mob_member_id_str(alias);
            assert!(
                is_valid_comms_component(&encoded),
                "{encoded:?} must satisfy meerkat 0.7 MemberCommsName"
            );
            assert_eq!(runtime_alias_str(&encoded), alias, "round trip of {alias}");
        }
    }

    #[test]
    fn non_ascii_and_punctuation_round_trip() {
        for alias in ["user@host", "a.b:c", "9starts-with-digit", "ünïcode:1"] {
            let encoded = mob_member_id_str(alias);
            assert!(is_valid_comms_component(&encoded), "{encoded:?}");
            assert_eq!(runtime_alias_str(&encoded), alias);
        }
    }

    #[test]
    fn reserved_marker_names_re_encode_so_round_trip_holds() {
        let alias = "mk--rt_creview";
        let encoded = mob_member_id_str(alias);
        assert_ne!(encoded, alias, "marker-prefixed names must be re-encoded");
        assert_eq!(runtime_alias_str(&encoded), alias);
    }

    #[test]
    fn generated_alias_reservation_applies_to_public_and_encoded_forms() {
        let alias = "rt:review:singleton:0";
        let encoded = mob_member_id_str(alias);
        assert!(is_reserved_generated_alias(alias));
        assert!(is_reserved_generated_alias(&encoded));
        assert!(!is_reserved_generated_alias("review:singleton"));
    }

    #[tokio::test]
    async fn raw_member_targets_reject_reserved_and_registered_durable_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity_runtime = std::sync::Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: std::sync::Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()?,
                ),
                lease_provider: std::sync::Arc::new(
                    crate::identity_first::LocalLeaseProvider::new(),
                ),
                runtime_instance_id: "raw-target-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        let identity = crate::identity_first::AgentIdentity::parse("lead")?;
        identity_runtime
            .register(crate::identity_first::DurableAgentSpec {
                identity,
                profile: meerkat_mob::ProfileName::from("worker"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels: std::collections::BTreeMap::new(),
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            })
            .await;

        for target in [
            "lead".to_string(),
            "rt:lead:0".to_string(),
            mob_member_id_str("rt:lead:0").into_owned(),
        ] {
            let error = match validate_raw_member_target(Some(&identity_runtime), &target).await {
                Err(error) => error,
                Ok(alias) => {
                    return Err(format!(
                        "identity-owned target {target:?} was admitted as {alias:?}"
                    )
                    .into());
                }
            };
            assert!(error.contains("identity-runtime") || error.contains("durable identity"));
        }
        assert_eq!(
            validate_raw_member_target(Some(&identity_runtime), "worker").await,
            Ok("worker".to_string())
        );

        let reservation = reserve_raw_member_target(Some(&identity_runtime), "worker").await?;
        assert_eq!(reservation.alias(), "worker");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(25),
                reserve_raw_member_target(Some(&identity_runtime), " worker "),
            )
            .await
            .is_err(),
            "canonical spellings of one alias must block on the same reservation"
        );
        let other_reservation = match tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reserve_raw_member_target(Some(&identity_runtime), "other-worker"),
        )
        .await
        {
            Ok(result) => result?,
            Err(error) => {
                return Err(
                    format!("different raw aliases must reserve concurrently: {error}").into(),
                );
            }
        };
        assert_eq!(other_reservation.alias(), "other-worker");
        drop(other_reservation);
        drop(reservation);

        let same_alias = match tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reserve_raw_member_target(Some(&identity_runtime), " worker "),
        )
        .await
        {
            Ok(result) => result?,
            Err(error) => {
                return Err(format!("alias lock released with reservation: {error}").into());
            }
        };
        assert_eq!(same_alias.alias(), "worker");
        drop(same_alias);

        let batch = match tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reserve_raw_member_targets(Some(&identity_runtime), ["zeta", " alpha ", "zeta"]),
        )
        .await
        {
            Ok(result) => result?,
            Err(error) => {
                return Err(format!(
                    "sorted deduplicated acquisition must not self-deadlock: {error}"
                )
                .into());
            }
        };
        assert_eq!(batch.aliases(), ["zeta", "alpha", "zeta"]);
        Ok(())
    }

    #[tokio::test]
    async fn rejected_unique_aliases_do_not_allocate_keyed_locks()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity_runtime = raw_lock_test_runtime()?;
        for index in 0..4_096 {
            let alias = format!("rt:rejected:{index}:0");
            assert!(
                reserve_raw_member_target(Some(&identity_runtime), &alias)
                    .await
                    .is_err(),
                "generated alias {alias} must be rejected"
            );
        }

        let (entry_count, _next_sweep_len, sweep_count) =
            identity_runtime.raw_member_alias_lock_metrics().await;
        assert_eq!(
            entry_count, 0,
            "preflight rejection must not allocate locks"
        );
        assert_eq!(sweep_count, 0, "no allocation means no sweep work");
        Ok(())
    }

    #[tokio::test]
    async fn active_alias_lock_survives_sweep_and_blocks_same_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity_runtime = raw_lock_test_runtime()?;
        let held_lock = identity_runtime.raw_member_alias_lock("held-worker").await;
        let held_guard = held_lock.clone().lock_owned().await;

        // Cross the minimum sweep watermark twice with short-lived aliases.
        for index in 0..600 {
            let lock = identity_runtime
                .raw_member_alias_lock(&format!("transient-{index}"))
                .await;
            drop(lock);
        }
        let (_entry_count, _next_sweep_len, sweep_count) =
            identity_runtime.raw_member_alias_lock_metrics().await;
        assert!(sweep_count >= 2, "test must exercise opportunistic sweep");

        let observed = identity_runtime
            .raw_member_alias_lock(" held-worker ")
            .await;
        assert!(
            std::sync::Arc::ptr_eq(&held_lock, &observed),
            "sweep must retain the live mutex for canonical spellings"
        );
        drop(observed);

        let waiter_runtime = identity_runtime.clone();
        let mut waiter = tokio::spawn(async move {
            let lock = waiter_runtime.raw_member_alias_lock("held-worker").await;
            let _guard = lock.lock_owned().await;
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut waiter)
                .await
                .is_err(),
            "same-alias waiter must remain blocked on the live pre-sweep lock"
        );
        drop(held_guard);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter).await??;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_alias_lock_creation_has_one_critical_section()
    -> Result<(), Box<dyn std::error::Error>> {
        const TASKS: usize = 64;
        let identity_runtime = raw_lock_test_runtime()?;
        // Leave an expired weak entry so every racing caller exercises the
        // upgrade-miss and write-side recheck path.
        drop(identity_runtime.raw_member_alias_lock("same-worker").await);

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(TASKS + 1));
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::with_capacity(TASKS);
        for _ in 0..TASKS {
            let runtime = identity_runtime.clone();
            let barrier = barrier.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let lock = runtime.raw_member_alias_lock("same-worker").await;
                let _guard = lock.lock_owned().await;
                let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max_active.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }
        barrier.wait().await;
        for task in tasks {
            task.await?;
        }
        assert_eq!(
            max_active.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "write-side recheck must prevent split locks for one alias"
        );
        Ok(())
    }

    #[tokio::test]
    async fn large_live_alias_set_uses_geometric_sweeps_and_reclaims_after_drop()
    -> Result<(), Box<dyn std::error::Error>> {
        const ALIASES: usize = 4_096;
        let identity_runtime = raw_lock_test_runtime()?;
        let mut live_locks = Vec::with_capacity(ALIASES);
        for index in 0..ALIASES {
            live_locks.push(
                identity_runtime
                    .raw_member_alias_lock(&format!("roster-{index}"))
                    .await,
            );
        }
        let (entry_count, next_sweep_len, sweep_count) =
            identity_runtime.raw_member_alias_lock_metrics().await;
        assert_eq!(entry_count, ALIASES);
        assert!(next_sweep_len >= ALIASES);
        assert!(
            sweep_count <= 8,
            "live roster acquisition must sweep geometrically, got {sweep_count} sweeps"
        );

        drop(live_locks);
        let post_drop_lock = identity_runtime
            .raw_member_alias_lock("post-roster-sweep")
            .await;
        let (entry_count, next_sweep_len, post_drop_sweeps) =
            identity_runtime.raw_member_alias_lock_metrics().await;
        assert_eq!(entry_count, 1, "next miss must reclaim the expired cohort");
        assert_eq!(next_sweep_len, 256);
        assert_eq!(post_drop_sweeps, sweep_count + 1);
        drop(post_drop_lock);
        Ok(())
    }

    #[test]
    fn raw_identity_labels_reserve_agent_identity_for_the_runtime() {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert(
            "agent_identity".to_string(),
            "forged-durable-id".to_string(),
        );
        assert!(validate_raw_identity_labels(&labels).is_err());

        labels.insert(
            "agent_identity".to_string(),
            mob_member_id_str("rt:forged:0").into_owned(),
        );
        assert!(validate_raw_identity_labels(&labels).is_err());
        assert_eq!(durable_identity_label(&labels), None);

        labels.remove("agent_identity");
        labels.insert("team".to_string(), "red".to_string());
        assert_eq!(validate_raw_identity_labels(&labels), Ok(()));
    }

    #[test]
    fn public_rpc_ingress_rejects_encoded_roster_targets() -> Result<(), Box<dyn std::error::Error>>
    {
        for field in [
            "identity",
            "member_id",
            "agent_id",
            "agent_identity",
            "meerkat_id",
            "local_member_id",
            "remote_member_id",
            "from_member_id",
            "source_member_id",
        ] {
            let params = serde_json::json!({ (field): "  mk--rt_csecret_c0  " });
            let error = match validate_public_rpc_member_aliases(&params) {
                Err(error) => error,
                Ok(()) => {
                    return Err(format!(
                        "encoded roster id in {field} was admitted as a public alias"
                    )
                    .into());
                }
            };
            assert!(error.contains(field), "{field}: {error}");
        }

        assert_eq!(
            validate_public_rpc_member_aliases(&serde_json::json!({
                "identity": "rt:secret:0",
                "member_id": "plain-member",
            })),
            Ok(())
        );
        Ok(())
    }

    #[test]
    fn public_rpc_ingress_rejects_encoded_roster_role_targets() {
        let refused = validate_public_rpc_member_aliases(&serde_json::json!({
            "flow_id": "review",
            "role_targets": { "reviewer": ["rev-a1", "  mk--rt_csecret_c0  "] },
        }));
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("role_targets")),
            "an encoded roster id in role_targets must be refused at ingress: {refused:?}"
        );
        assert_eq!(
            validate_public_rpc_member_aliases(&serde_json::json!({
                "flow_id": "review",
                "role_targets": { "reviewer": ["rev-a1", "rt:rev-a2:0"] },
            })),
            Ok(())
        );
    }

    #[test]
    fn runtime_event_alias_decodes_encoded_roster_member_ids() {
        use meerkat_mob::ids::{AgentIdentity, AgentRuntimeId, Generation};

        // Identity-first alias: the roster id is the comms-safe encoding and
        // must decode back to the public alias before any event projection.
        let encoded = mob_member_id_str("rt:review:singleton:0").into_owned();
        let runtime_id =
            AgentRuntimeId::new(AgentIdentity::from(encoded.as_str()), Generation::new(1));
        assert_eq!(runtime_event_alias(&runtime_id), "rt:review:singleton:0:1");

        // Plain member names pass through unchanged.
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("worker-one"));
        assert_eq!(runtime_event_alias(&runtime_id), "worker-one:0");
    }

    #[test]
    fn encoding_is_injective_across_pass_through_and_encoded_forms() {
        let inputs = [
            "rt:review:singleton:0",
            "rt-review-singleton-0",
            "rt_creview_csingleton_c0",
            "mk--rt_creview_csingleton_c0",
        ];
        let mut seen = std::collections::BTreeSet::new();
        for input in inputs {
            assert!(
                seen.insert(mob_member_id_str(input).into_owned()),
                "collision on {input}"
            );
        }
    }

    // --- Defensive suite: map the whole id codec + reconciliation aliases ---

    /// The exact identities + runtime-id forms from the downstream app's deployment
    /// that surfaced the meerkat 0.7 comms-name regression, plus the escape
    /// productions. Every entry must encode to a comms-safe id and round-trip.
    fn defensive_corpus() -> Vec<String> {
        let mut v: Vec<String> = [
            // Plain definition-mob names (pass through unchanged).
            "worker",
            "worker-one",
            "_internal",
            "Agent7",
            "a",
            // Downstream app durable identities (colon-bearing).
            "identity:lead-1",
            "identity:lead-2",
            "identity:member-1",
            "identity:member-2",
            "domain:calendar",
            "domain:research",
            "domain:support",
            "domain:ops",
            "domain:automation",
            "domain:finance",
            "domain:discovery",
            "team-group:main",
            "triage:main",
            "gate:main",
            // Runtime-id shaped aliases (`rt:{identity}:{generation}`).
            "rt:identity:lead-1:0",
            "rt:domain:automation:3",
            "rt:channel:C0SMOKEOPS:0",
            "rt:review:singleton:12",
            // Mixed punctuation / boundary shapes.
            "a:b_c",
            "with_underscore:and:colons",
            "user@host",
            "a.b:c",
            "9starts-with-digit",
            "::",
            ":",
            "-",
            "mk--collision",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        // Non-ASCII, multi-byte, and control characters.
        v.push("ünïcode:1".to_string());
        v.push("emoji-😀:2".to_string());
        v.push("Ω≈ç:3".to_string());
        v.push("tab\tnl\n".to_string());
        v.push("\u{0}\u{1f}x".to_string());
        v.push("a b".to_string());
        v.push(String::new());
        v
    }

    #[test]
    fn downstream_corpus_round_trips_and_is_comms_safe() {
        for alias in defensive_corpus() {
            let encoded = mob_member_id_str(&alias).into_owned();
            assert!(
                is_valid_comms_component(&encoded),
                "encoded {encoded:?} (from {alias:?}) is not a valid comms component"
            );
            assert!(
                !encoded.contains([':', '/']),
                "encoded {encoded:?} leaks a routing separator"
            );
            assert_eq!(
                runtime_alias_str(&encoded),
                alias,
                "round trip of {alias:?}"
            );
        }
    }

    /// The load-bearing guard: the codec's output must be accepted by meerkat
    /// 0.7's own fail-closed comms-name validator — the exact check that
    /// rejected `rt:identity:lead-1:0` before this codec existed. If meerkat
    /// tightens `MemberCommsName` again, this test fails instead of a downstream app.
    #[test]
    fn encoded_ids_satisfy_meerkat_member_comms_name_validator() {
        use meerkat_core::connection::MemberCommsName;
        for alias in defensive_corpus() {
            let encoded = mob_member_id_str(&alias).into_owned();
            assert!(
                MemberCommsName::new("example-mob", "worker", encoded.clone()).is_ok(),
                "MemberCommsName::new rejected encoded id {encoded:?} (from alias {alias:?})"
            );
        }
    }

    /// Pin the on-the-wire encoding so an accidental codec change is caught.
    #[test]
    fn exact_wire_format_for_known_aliases() {
        let cases = [
            ("worker-one", "worker-one"), // already comms-safe -> pass through
            ("identity:lead-1", "mk--identity_clead-1"),
            ("domain:automation", "mk--domain_cautomation"),
            ("team-group:main", "mk--team-group_cmain"),
            ("rt:identity:lead-1:0", "mk--rt_cidentity_clead-1_c0"),
            ("triage:main", "mk--triage_cmain"),
            ("a:b_c", "mk--a_cb__c"),
            ("a b", "mk--a_x20_b"),
            ("", "mk--"),
        ];
        for (alias, expected) in cases {
            assert_eq!(mob_member_id_str(alias), expected, "encode {alias:?}");
            assert_eq!(runtime_alias_str(expected), alias, "decode {expected:?}");
        }
    }

    /// Ids that begin with the marker but are not well-formed escape productions
    /// must decode to themselves (literal fallback) and never panic.
    #[test]
    fn malformed_encoded_bodies_decode_to_literal_without_panic() {
        for bad in [
            "mk--_",           // dangling underscore (no escape selector)
            "mk--_z",          // invalid escape selector
            "mk--_x",          // truncated hex escape (no terminator)
            "mk--_x_",         // empty hex body
            "mk--_xZZ_",       // non-hex digits
            "mk--_x110000_",   // code point above the Unicode max
            "mk--_xffffffff_", // parses as u32 but is not a valid char
            "mk--abc_",        // trailing dangling underscore after a valid run
        ] {
            assert_eq!(runtime_alias_str(bad), bad, "literal fallback for {bad:?}");
        }
    }

    /// Empty, single-character, and marker-adjacent inputs all round-trip.
    #[test]
    fn empty_and_boundary_strings_round_trip() {
        // Empty alias is not a valid comms component, so it becomes the bare
        // marker and decodes back to empty.
        assert_eq!(mob_member_id_str(""), "mk--");
        assert_eq!(runtime_alias_str("mk--"), "");
        for alias in [":", "_", "-", "::", "_x", "mk", "mk-", "mk--"] {
            let encoded = mob_member_id_str(alias).into_owned();
            assert!(
                is_valid_comms_component(&encoded),
                "{encoded:?} not comms-safe"
            );
            assert_eq!(runtime_alias_str(&encoded), alias, "round trip {alias:?}");
        }
    }

    /// Canonicalization contract used by the schedule internal-delivery lane:
    /// decode-then-encode maps BOTH the alias space and the roster-id space to
    /// the same roster key, and is the identity on plain member names. Encode
    /// alone is NOT idempotent on roster ids (the reserved-marker rule
    /// re-encodes them) — a downstream app's 0.7.28 schedule self-delivery failure.
    #[test]
    fn decode_then_encode_canonicalizes_both_id_spaces() {
        for (input, canonical) in [
            ("rt:domain:ops:0", "mk--rt_cdomain_cops_c0"),
            ("mk--rt_cdomain_cops_c0", "mk--rt_cdomain_cops_c0"),
            ("domain:ops", "mk--domain_cops"),
            ("mk--domain_cops", "mk--domain_cops"),
            ("digest-owner", "digest-owner"),
        ] {
            let key = mob_member_id_str(runtime_alias_str(input).as_ref()).into_owned();
            assert_eq!(key, canonical, "canonical roster key for {input:?}");
        }
        // The failure mode this contract exists to prevent:
        assert_ne!(
            mob_member_id_str("mk--rt_cdomain_cops_c0"),
            "mk--rt_cdomain_cops_c0",
            "encode alone re-encodes roster ids — never use it on binding member ids"
        );
    }

    /// Decode only applies inside the reserved marker namespace; a raw id that
    /// merely *looks* like an escape body must pass through untouched.
    #[test]
    fn decode_is_identity_on_unencoded_ids() {
        for id in [
            "worker",
            "worker-one",
            "_internal",
            "Agent7",
            "rt-review-singleton-0",
            "a_cb", // not marker-prefixed -> NOT decoded to "a:b"
        ] {
            assert_eq!(runtime_alias_str(id), id);
        }
    }

    #[test]
    fn runtime_event_alias_across_generations_and_forms() {
        use meerkat_mob::ids::{AgentIdentity, AgentRuntimeId, Generation};

        let encoded = mob_member_id_str("rt:identity:lead-1:0").into_owned();
        let rid = AgentRuntimeId::new(AgentIdentity::from(encoded.as_str()), Generation::new(7));
        assert_eq!(runtime_event_alias(&rid), "rt:identity:lead-1:0:7");

        let encoded = mob_member_id_str("domain:automation").into_owned();
        let rid = AgentRuntimeId::new(AgentIdentity::from(encoded.as_str()), Generation::new(1));
        assert_eq!(runtime_event_alias(&rid), "domain:automation:1");

        // Plain member name keeps its initial generation.
        let rid = AgentRuntimeId::initial(AgentIdentity::from("triage-main"));
        assert_eq!(runtime_event_alias(&rid), "triage-main:0");
    }

    /// Exhaustive total round-trip + comms-safety + injectivity over every
    /// string of length 0..=3 from a charset that exercises every escape branch.
    #[test]
    fn round_trip_is_total_and_injective_over_generated_corpus() {
        use meerkat_core::connection::MemberCommsName;
        use std::collections::BTreeMap;

        let charset = ['a', 'Z', '0', '-', '_', ':', '.', '@', ' ', 'ü', '😀'];
        let mut aliases: Vec<String> = vec![String::new()];
        let mut frontier = vec![String::new()];
        for _ in 0..3 {
            let mut next = Vec::new();
            for prefix in &frontier {
                for c in charset {
                    let mut s = prefix.clone();
                    s.push(c);
                    aliases.push(s.clone());
                    next.push(s);
                }
            }
            frontier = next;
        }

        let mut encoded_to_alias: BTreeMap<String, String> = BTreeMap::new();
        for alias in &aliases {
            let encoded = mob_member_id_str(alias).into_owned();
            assert!(
                MemberCommsName::new("m", "r", encoded.clone()).is_ok(),
                "encoded {encoded:?} (from {alias:?}) is not comms-safe"
            );
            assert_eq!(
                &runtime_alias_str(&encoded).into_owned(),
                alias,
                "round trip of {alias:?}"
            );
            if let Some(prev) = encoded_to_alias.insert(encoded.clone(), alias.clone()) {
                assert_eq!(
                    &prev, alias,
                    "collision: {prev:?} and {alias:?} both encode to {encoded:?}"
                );
            }
        }
    }

    /// meerkat names a fork's source by its roster id (`ForkBuildSource`
    /// `source_member.member`); hosts key grants by durable identity. The
    /// public inverse must name exactly the identity Meta encoded.
    #[test]
    fn a_roster_member_id_from_meerkat_names_its_durable_identity() {
        let encoded = roster_member_id_for_identity("domain:calendar");
        assert_eq!(encoded.as_str(), "mk--domain_ccalendar");
        assert_eq!(
            durable_identity_for_roster_member(encoded.as_str()),
            "domain:calendar"
        );
        // A legacy generated-alias roster id names the same identity.
        assert_eq!(
            durable_identity_for_roster_member(&mob_member_id_str("rt:domain:calendar:3")),
            "domain:calendar"
        );
        // A comms-safe identity is its own roster id.
        assert_eq!(durable_identity_for_roster_member("luka"), "luka");
    }

    // The ONE memory-scope identity spelling (task #53): mob-plane member
    // ids, generated runtime aliases, and plain names all normalize to the
    // logical durable identity - and it is a fixed point.
    #[test]
    fn logical_memory_identity_normalizes_every_member_id_shape() {
        // Identity-first internal member: encoded runtime alias, any
        // generation, normalizes to the durable identity.
        let encoded_gen0 = mob_member_id_str("rt:identity:lead-1:0").into_owned();
        let encoded_gen7 = mob_member_id_str("rt:identity:lead-1:7").into_owned();
        assert_eq!(logical_memory_identity(&encoded_gen0), "identity:lead-1");
        assert_eq!(logical_memory_identity(&encoded_gen7), "identity:lead-1");
        // Decoded (public alias) spelling of the same runtime id.
        assert_eq!(
            logical_memory_identity("rt:identity:lead-1:0"),
            "identity:lead-1"
        );
        // Identity-first external binding: encoded durable identity.
        assert_eq!(
            logical_memory_identity(&mob_member_id_str("review:singleton")),
            "review:singleton"
        );
        // Classic plain member names are untouched.
        assert_eq!(logical_memory_identity("helper"), "helper");
        // Non-generation rt:-prefixed names stay whole (conservative).
        assert_eq!(logical_memory_identity("rt:oddly:named"), "rt:oddly:named");
        // Fixed point: normalizing a logical identity changes nothing.
        for id in ["identity:lead-1", "review:singleton", "helper"] {
            assert_eq!(logical_memory_identity(id), id);
        }
    }

    /// The creation check refuses exactly the members the console would
    /// attribute to a runtime-plane identity: through the identity the spawn
    /// projection registers (trimmed, label override first) or the one agent
    /// events derive from the runtime id (decoded, one `rt:` stripped). Every
    /// other spelling stays an ordinary member name.
    #[test]
    fn the_creation_check_refuses_every_spelling_console_attribution_resolves_to_the_runtime_plane()
    {
        let encoded = |alias: &str| mob_member_id_str(alias).into_owned();
        let cases: Vec<(String, Option<&str>, bool)> = vec![
            ("_system".to_string(), None, true),
            ("__console__".to_string(), None, true),
            // The spawn projection trims the member id it registers.
            (" _system".to_string(), None, true),
            ("_system\u{2003}".to_string(), None, true),
            // Agent events decode the comms-safe roster spelling.
            ("mk--__system".to_string(), None, true),
            (encoded("__console__"), None, true),
            // ... and strip one generated-alias prefix.
            ("rt:_system".to_string(), None, true),
            (encoded("rt:_system"), None, true),
            (encoded("rt:rt:_system"), None, false),
            // The identity override label wins, trimmed.
            ("worker".to_string(), Some("_system"), true),
            ("worker".to_string(), Some(" __console__ "), true),
            ("worker".to_string(), Some("\u{3000}_system"), true),
            // A runtime-plane roster id is refused whatever the label says.
            ("_system".to_string(), Some("worker-label"), true),
            // An empty or generated-alias override falls back to the member id.
            ("worker".to_string(), Some(""), false),
            ("worker".to_string(), Some("rt:_system"), false),
            ("worker".to_string(), Some("mk--__system"), false),
            // Near misses are ordinary names.
            ("_SYSTEM".to_string(), None, false),
            ("__CONSOLE__".to_string(), None, false),
            ("__system".to_string(), None, false),
            ("_system_".to_string(), None, false),
            ("worker".to_string(), Some("_System"), false),
            ("worker".to_string(), None, false),
        ];
        for (roster_member_id, label, refused) in cases {
            let labels = label.map(|label| {
                std::collections::BTreeMap::from([(
                    "agent_identity".to_string(),
                    label.to_string(),
                )])
            });
            let outcome = refuse_runtime_plane_member(&roster_member_id, labels.as_ref());
            assert_eq!(
                outcome.is_err(),
                refused,
                "roster id {roster_member_id:?}, agent_identity label {label:?}: {outcome:?}"
            );
            if let Err(message) = outcome {
                assert!(message.contains("is reserved"), "{message}");
            }
        }
    }

    /// The guard checks every spawn source but the two that rebuild or
    /// replay an existing member, each with a reserved id, a plain id that a
    /// label points at a reserved identity, and a plain id.
    #[test]
    fn the_guard_admits_only_the_sources_that_rebuild_an_existing_member() {
        use meerkat_mob::{AgentIdentity, ProfileName, SpawnMemberSpec, SpawnSource};
        let rows = [
            (SpawnSource::Consumer, false),
            (SpawnSource::AgentSpawnMember, false),
            (SpawnSource::HelperSpawn, false),
            (SpawnSource::BatchItem, false),
            (SpawnSource::FlowProvisioning, false),
            (SpawnSource::PolicySpawn, false),
            (SpawnSource::Respawn, false),
            (SpawnSource::Resume, true),
            (SpawnSource::Fork, false),
            (SpawnSource::PersistedForkResume, false),
            (SpawnSource::IdentityReconcile, true),
            (SpawnSource::AttachedForkedParticipant, false),
        ];
        let spec = |member: &str| {
            SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(member))
        };
        for (source, admitted) in rows {
            let reserved = runtime_plane_spawn_check(source, &spec("_system"));
            assert_eq!(
                reserved.is_ok(),
                admitted,
                "{source:?}, `_system`: {reserved:?}"
            );
            if let Err(error) = reserved {
                assert!(
                    error.to_string().contains("reserved"),
                    "{source:?}: {error}"
                );
            }
            let labelled = spec("worker-a").with_labels(std::collections::BTreeMap::from([(
                "agent_identity".to_string(),
                "__console__".to_string(),
            )]));
            let labelled = runtime_plane_spawn_check(source, &labelled);
            assert_eq!(
                labelled.is_ok(),
                admitted,
                "{source:?}, labelled `__console__`: {labelled:?}"
            );
            let plain = runtime_plane_spawn_check(source, &spec("worker-a"));
            assert!(plain.is_ok(), "{source:?} admits a plain id: {plain:?}");
        }
    }

    /// The roster's provenance: a member the roster materialized carries the
    /// `agent_identity` label naming the identity its member id encodes. A
    /// durable worker spawned at runtime carries the same durable marker but
    /// no such label, and a label naming another identity is not the
    /// roster's either.
    #[test]
    fn only_a_member_labelled_with_its_own_identity_was_materialized_by_the_roster() {
        let binding = |member: &str, labels: &[(&str, &str)]| {
            meerkat_mob::MemberBinding::for_test(
                meerkat_mob::AgentIdentity::from(member),
                meerkat_mob::Generation::new(1),
                None,
                meerkat_mob::MemberBindingState::Live,
                meerkat_mob::ProfileName::from("worker"),
                meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                    continuity_key: member.to_string(),
                },
                meerkat_mob::MemberIdentityManagement::CommandDriven,
            )
            .with_labels(
                labels
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            )
        };
        let alice = roster_member_id_for_identity("personal:alice");
        assert!(binding_materialized_by_roster(&binding(
            alice.as_str(),
            &[("agent_identity", "personal:alice")]
        )));
        assert!(!binding_materialized_by_roster(&binding("worker-1", &[])));
        assert!(!binding_materialized_by_roster(&binding(
            "worker-1",
            &[("role", "worker")]
        )));
        assert!(!binding_materialized_by_roster(&binding(
            alice.as_str(),
            &[("agent_identity", "personal:bob")]
        )));
    }
}
