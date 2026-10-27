//! Real durable transcript fork, exposed through [`UnifiedRuntime`].
//!
//! This is a WRAPPER over meerkat 0.8.22's `MobHandle::fork_member`
//! (`meerkat-mob/src/runtime/handle.rs:9343`). It reimplements no transcript,
//! cache, identity or blob logic - all four remain owned by the persistent
//! session service behind `SessionService::fork_persisted_session`.
//!
//! # What upstream guarantees, and what it refuses
//!
//! Read this before changing anything here; the contract is upstream's, not
//! Meta's.
//!
//! - The child transcript is a REAL durable fork. `message_count = None`
//!   selects the source's exact committed transcript end while the persistent
//!   session owner holds its mutation guard; an explicit count selects that
//!   prefix and is REFUSED if it splits a tool-use/result group.
//! - The fork COMMITS BEFORE resume provisioning. If provisioning then fails,
//!   `MobError::ForkMemberProvisionFailed` retains the durable
//!   `fork_session_id` so the branch stays recoverable instead of being
//!   silently deleted. That typed shape (and its
//!   `structured_data()["recovery"] == "resume_committed_fork_session"`) is
//!   the ENTIRE recoverability affordance - see
//!   [`ForkMemberError::structured_data`]. Never flatten it to a string.
//! - Running-source rejection is upstream's: `SessionError::Busy` becomes
//!   `MobError::ForkSourceUnavailable { cause: Running }`, and a source with
//!   no session becomes `cause: NoSession`. Meta passes both through
//!   verbatim in [`ForkMemberError::Mob`] and never reclassifies them.
//! - Self-fork is refused upstream (`MobError::MemberAlreadyExists`), and a
//!   `SpawnMemberSpec::placement` is refused with `MobError::WiringError`
//!   ("durable fork resume currently requires the source session store's
//!   controlling host"). Those guards are deliberately NOT duplicated here:
//!   a second copy would drift from upstream's.
//!
//! # The cache boundary
//!
//! Upstream constructs the child's `DurableSessionForkTarget` with
//! `cache_identity: None` (handle.rs:9368-9380) because profile resolution and
//! resume override masks are actor-owned and happen AFTER the handle admits
//! the spawn. Every mob fork therefore reports
//! `ForkCacheInheritance::Unavailable { reason: TargetIdentityUnresolved }`.
//!
//! Meta passes `cache_inheritance` through VERBATIM and must never try to
//! make inheritance available: resolving a profile/model into a
//! `SessionLlmIdentity` to enable it is exactly the cache logic this wrapper
//! is forbidden to reimplement, and persisting evidence a later profile
//! override could invalidate is how a cache breakpoint becomes a lie.
//!
//! # Meta-owned wrapping
//!
//! Only three things are Meta's here, and each mirrors
//! [`UnifiedRuntime::spawn`](crate::unified_runtime::UnifiedRuntime::spawn):
//!
//! 1. Aliases. Wire-facing member ids are public aliases; the mob roster
//!    speaks comms-safe encoded ids. Source and child both route through
//!    [`crate::member_comms_id`], so a fork addresses the same roster
//!    identities a spawn would.
//! 2. Source lifecycle authority. When an identity runtime is SUPPLIED AT THE
//!    CALL or installed on the runtime (in that precedence - see
//!    [`UnifiedRuntime::fork_member_with_identity_runtime`]), the fork runs
//!    inside the source alias's tracked lifecycle target, so a stale, deleted
//!    or mid-lifecycle source alias is refused before the durable read - the
//!    same guard `meta/fork_helper` already carries, resolved the same way.
//!    The child's raw-namespace reservation is taken SECOND, matching durable
//!    materialization's lock order, so the two cannot deadlock by inversion.
//! 3. The shared error hook, fired exactly as `spawn` fires it.

use meerkat_mob::launch::MemberLaunchMode;
use meerkat_mob::{ForkMemberResult, MobError, SpawnMemberSpec};
use serde::Serialize;

use crate::unified_runtime::{ErrorEvent, UnifiedRuntime};

/// Outcome of a durable member fork.
///
/// `result` is meerkat's [`ForkMemberResult`] verbatim, including
/// `cache_inheritance`. The aliases are Meta's wire-facing spelling of the
/// two roster identities involved.
#[derive(Debug, Clone, Serialize)]
pub struct ForkMemberOutcome {
    /// Public alias of the member whose transcript was forked.
    pub source_member_alias: String,
    /// Public alias seated for the forked child.
    pub member_alias: String,
    /// Upstream fork result, passed through unmodified.
    #[serde(flatten)]
    pub result: ForkMemberResult,
}

/// Why a durable member fork did not complete.
///
/// Deliberately three-way: Meta's own refusals are distinguishable from
/// meerkat's typed fork outcome, and the latter is never collapsed into a
/// string.
#[derive(Debug)]
pub enum ForkMemberError {
    /// Meta refused the request before reaching meerkat.
    InvalidRequest(String),
    /// The source alias could not be pinned to a current identity for the
    /// duration of the fork (stale alias, deleted member, lifecycle
    /// contention).
    SourceAuthority(String),
    /// meerkat's typed fork/provision failure, verbatim.
    Mob(MobError),
}

impl std::fmt::Display for ForkMemberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "{message}"),
            Self::SourceAuthority(message) => {
                write!(f, "fork source alias authority unavailable: {message}")
            }
            Self::Mob(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ForkMemberError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mob(error) => Some(error),
            Self::InvalidRequest(_) | Self::SourceAuthority(_) => None,
        }
    }
}

impl From<MobError> for ForkMemberError {
    fn from(value: MobError) -> Self {
        Self::Mob(value)
    }
}

impl ForkMemberError {
    /// Structured wire payload for this failure.
    ///
    /// For [`Self::Mob`] this forwards `MobError::structured_data` unchanged,
    /// which is what preserves `ForkMemberProvisionFailed`'s
    /// `fork_session_id` plus its `"recovery": "resume_committed_fork_session"`
    /// hint. A surface that renders only `Display` DELETES the recovery path.
    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::Mob(error) => error.structured_data(),
            Self::InvalidRequest(_) | Self::SourceAuthority(_) => None,
        }
    }

    /// Whether the durable child session committed and only resume
    /// provisioning failed. Such a fork is recoverable by resuming the
    /// retained `fork_session_id`; it must NOT be retried as a fresh fork.
    pub fn committed_fork_is_recoverable(&self) -> bool {
        matches!(self, Self::Mob(MobError::ForkMemberProvisionFailed { .. }))
    }
}

/// Reserve the child's raw member alias, then delegate to meerkat's fork.
///
/// Owned arguments only: this is invoked both directly and as the `'static`
/// body of a tracked source-alias lifecycle operation.
///
/// When a source lifecycle target is held, it is ALREADY held when this runs.
/// The child's raw-namespace reservation is therefore taken SECOND, matching
/// durable identity materialization's lock order (and `meta/fork_helper`'s),
/// so the two orders cannot invert into a deadlock.
///
/// The returned inner `Result` deliberately carries meerkat's typed
/// [`MobError`] rather than a string: the outer `String` error belongs to the
/// reservation, and collapsing the fork's own failure would delete
/// `ForkMemberProvisionFailed`'s recoverable session id.
async fn reserve_and_fork(
    identity_runtime: Option<std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    handle: meerkat_mob::MobHandle,
    source_identity: meerkat_mob::ids::AgentIdentity,
    mut spec: SpawnMemberSpec,
    child_alias: String,
    message_count: Option<usize>,
) -> Result<(String, Result<ForkMemberResult, MobError>), String> {
    let raw_reservation = crate::member_comms_id::reserve_raw_member_target(
        identity_runtime.as_ref(),
        child_alias.as_str(),
    )
    .await?;
    let alias = raw_reservation.alias().to_string();
    spec.identity = crate::member_comms_id::mob_member_id(alias.as_str());
    let fork = Box::pin(handle.fork_member(&source_identity, spec, message_count)).await;
    drop(raw_reservation);
    Ok((alias, fork))
}

/// Meta-side preflight over the child spec.
///
/// Split out so the refusals are testable without a live runtime.
fn validate_fork_spec(spec: &SpawnMemberSpec) -> Result<(), ForkMemberError> {
    if let Some(labels) = spec.labels.as_ref() {
        crate::member_comms_id::validate_raw_identity_labels(labels)
            .map_err(|message| ForkMemberError::InvalidRequest(message.to_string()))?;
    }
    // `fork_member` overwrites `launch_mode` with `Resume { fork_session_id }`
    // itself. Accepting a caller-supplied mode would silently discard it -
    // in particular a `Fork { .. }` mode, which is the OTHER, prompt-context
    // fork (`meta/fork_helper`) and copies no transcript at all.
    if !matches!(spec.launch_mode, MemberLaunchMode::Fresh) {
        return Err(ForkMemberError::InvalidRequest(
            "durable fork owns the child's launch mode; leave \
             SpawnMemberSpec::launch_mode at Fresh (MemberLaunchMode::Fork is the \
             prompt-context helper fork, not a transcript fork)"
                .to_string(),
        ));
    }
    if spec.identity.as_str().trim().is_empty() {
        return Err(ForkMemberError::InvalidRequest(
            "fork child identity must not be empty".to_string(),
        ));
    }
    Ok(())
}

impl UnifiedRuntime {
    /// Fork `source_member_alias`'s durable transcript into a new member.
    ///
    /// `message_count` is passed to meerkat verbatim: `None` takes the
    /// source's exact committed transcript end, an explicit count takes that
    /// prefix and is refused if it splits a tool-use/result group.
    ///
    /// This is the REAL fork. `meta/fork_helper` is the unrelated
    /// prompt-context helper: it injects rendered context text into a fresh
    /// one-shot member and retires it. Nothing here retires the child - a
    /// forked member is an ordinary seated member.
    ///
    /// See the module docs for the exact upstream guarantees, the running-
    /// source rejection, and the cache boundary.
    ///
    /// # Retry is a decision, so take it from the typed error
    ///
    /// A failure fires the shared error hook as
    /// [`ErrorEvent::SpawnFailure`] - mechanically true, because no member was
    /// seated. That event is TELEMETRY and must not drive retry: for
    /// `MobError::ForkMemberProvisionFailed` the durable child session DID
    /// commit, and a blind retry-on-`SpawnFailure` policy would fork again and
    /// orphan it. A host deciding whether to retry MUST consult the returned
    /// [`ForkMemberError::committed_fork_is_recoverable`] (and resume the
    /// retained `fork_session_id` when it is true), never the event.
    pub async fn fork_member(
        &self,
        source_member_alias: &str,
        spec: SpawnMemberSpec,
        message_count: Option<usize>,
    ) -> Result<ForkMemberOutcome, ForkMemberError> {
        self.fork_member_with_identity_runtime(None, source_member_alias, spec, message_count)
            .await
    }

    /// [`Self::fork_member`] against an EXPLICITLY supplied identity runtime,
    /// falling back to the installed one.
    ///
    /// This exists for one reason and it is a safety reason. The RPC surface
    /// supports an identity runtime that is passed per request and NEVER
    /// installed on the [`UnifiedRuntime`]: `rpc.rs`'s
    /// `IdentityFirstContext` is a caller-owned value, and meta's own
    /// `rpc-explicit-identity-context-reservation-test` asserts
    /// `runtime.identity_runtime().is_none()` while dispatching with a live
    /// `IdentityRuntime` in the context. Every sibling handler
    /// (`meta/fork_helper`, `meta/spawn_helper`,
    /// `meta/force_cancel_member`, `meta/attach_existing_session`) is
    /// handed `identity_ctx.map(|ctx| &ctx.runtime)` and resolves
    /// `explicit.or_else(installed)`.
    ///
    /// A fork surface that consulted only the installed runtime would, on
    /// exactly that seam, skip BOTH the source-alias lifecycle pin and the
    /// durable-ownership half of the child reservation, leaving only the
    /// static reserved-namespace string checks. An RPC handler for
    /// `fork_member` MUST therefore call this method with the request's
    /// identity context, not [`Self::fork_member`].
    pub async fn fork_member_with_identity_runtime(
        &self,
        identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
        source_member_alias: &str,
        spec: SpawnMemberSpec,
        message_count: Option<usize>,
    ) -> Result<ForkMemberOutcome, ForkMemberError> {
        let identity_runtime = identity_runtime.or_else(|| self.identity_runtime_ref());
        validate_fork_spec(&spec)?;
        let source_alias =
            crate::member_comms_id::runtime_alias_str(source_member_alias).into_owned();
        if source_alias.trim().is_empty() {
            return Err(ForkMemberError::InvalidRequest(
                "fork source member alias must not be empty".to_string(),
            ));
        }
        let requested_child_alias = spec.identity.as_str().to_string();
        let profile = spec.role_name.to_string();
        // The fork is foreground work: it holds the public owner (`handle`)
        // until it ends, so the identity runtime it reserves through is the
        // plain one.
        let handle = self.mob_handle();
        let identity_runtime_owned = identity_runtime.cloned();

        // Pre-flight the CHILD alias before either branch, exactly as
        // `meta/fork_helper` does. Inside the tracked branch the
        // reservation's `Err(String)` is rewrapped as an identity `Internal`
        // error, so without this the SAME bad child alias would surface as
        // `InvalidRequest` on a host with no identity runtime and as
        // `SourceAuthority` on one with it - a typed shape that depends on
        // host configuration rather than on what the caller got wrong.
        // This is a preflight, not the authority boundary: the reservation
        // revalidates under its guard, so a genuine race still ends as
        // `SourceAuthority`.
        crate::member_comms_id::validate_raw_member_target(
            identity_runtime,
            requested_child_alias.as_str(),
        )
        .await
        .map_err(ForkMemberError::InvalidRequest)?;

        // Pin the SOURCE alias to a current identity first.
        //
        // `meta/fork_helper` carries a fourth arm here -
        // `Ok(None) if is_reserved_generated_alias(&source)` -> refuse. With
        // an identity runtime that arm is UNREACHABLE:
        // `IdentityRuntime::member_alias_lifecycle_target`
        // (identity_first/runtime.rs, the `None if
        // crate::member_comms_id::is_reserved_generated_alias(&alias)` arm)
        // already returns `Err(IdentityRuntimeError::Internal("generated
        // member alias requires identity authority: ..."))` for precisely
        // that case, so a generated source alias can never reach an
        // `Ok(None)` here. It becomes `SourceAuthority` through the `map_err`
        // below.
        //
        // `Ok(None)` therefore means exactly one thing: the alias is a plain
        // raw member. `None` from the `match` means no identity runtime was
        // supplied or installed; there the source mapping below refuses a
        // generated alias, as that fourth arm does. A plain alias runs the
        // fork unlocked in both cases, exactly as `meta/fork_helper` does.
        let source_target = match identity_runtime {
            Some(identity_runtime) => identity_runtime
                .member_alias_lifecycle_target(&source_alias)
                .await
                .map_err(|error| ForkMemberError::SourceAuthority(error.to_string()))?,
            None => None,
        };

        // Member-id alias rule for the source: one the identity authority owns
        // (checked current, and again under the lock below) names its durable
        // roster row; with no identity runtime a generated alias has no
        // authority to pin its generation and is refused.
        let source_identity = if source_target.is_some() {
            crate::member_comms_id::roster_member_id_for_supplied_id(source_alias.as_str())
        } else if crate::member_comms_id::is_reserved_generated_alias(&source_alias) {
            return Err(ForkMemberError::SourceAuthority(format!(
                "generated member alias requires identity authority: {source_alias}"
            )));
        } else {
            crate::member_comms_id::mob_member_id(source_alias.as_str())
        };
        let (member_alias, fork) = if let Some(source_target) = source_target {
            // The tracked wrapper acquires the source alias's lifecycle lock
            // before the operation body runs, so a stale or mid-lifecycle
            // source cannot be read out from under the durable fork.
            crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
                vec![source_target],
                move || {
                    reserve_and_fork(
                        identity_runtime_owned,
                        handle,
                        source_identity,
                        spec,
                        requested_child_alias,
                        message_count,
                    )
                },
            )
            .await
            .map_err(|error| ForkMemberError::SourceAuthority(error.to_string()))?
        } else {
            reserve_and_fork(
                identity_runtime_owned,
                handle,
                source_identity,
                spec,
                requested_child_alias,
                message_count,
            )
            .await
            .map_err(ForkMemberError::InvalidRequest)?
        };

        match fork {
            Ok(result) => {
                // The child is seated in the primary mob, where idle
                // retirement is opt-in. Opt it in on the runtime default so an
                // RPC or console fork retires like a `fork_off` tool child
                // instead of holding a bounded session until the ceiling.
                if let Some(overrides) = self.mob_runtime().implicit_delegate_retirement_overrides()
                {
                    overrides
                        .set(
                            self.mob_handle().mob_id().to_string(),
                            result.agent_identity.to_string(),
                            crate::mob_handle_runtime::DelegateIdleRetireOverride::RuntimeDefault,
                        )
                        .await;
                }
                Ok(ForkMemberOutcome {
                    source_member_alias: source_alias,
                    member_alias,
                    result,
                })
            }
            Err(error) => {
                // Mirrors `UnifiedRuntime::spawn`: a fork that does not seat a
                // member is a seat failure on the shared error hook. The typed
                // error - including a committed-but-unprovisioned fork's
                // recoverable session id - still returns to the caller.
                self.fire_error(ErrorEvent::SpawnFailure {
                    member_id: member_alias,
                    profile,
                    error: format!("{error}"),
                });
                Err(ForkMemberError::Mob(error))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_mob::error::ForkSourceUnavailableCause;
    use meerkat_mob::ids::AgentIdentity;

    fn spec(alias: &str) -> SpawnMemberSpec {
        SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("general"),
            AgentIdentity::from(alias),
        )
    }

    #[test]
    fn a_caller_supplied_launch_mode_is_refused_not_silently_overwritten() {
        let mut resume = spec("child");
        resume.launch_mode = MemberLaunchMode::Resume {
            // 0.8.25: no migration authority on this path; a declaration is
            // attached only where resume_session detects a genuine role divergence.
            resume_from_role: None,
            bridge_session_id: meerkat_core::types::SessionId::new(),
        };
        let error = validate_fork_spec(&resume).expect_err("resume mode must be refused");
        assert!(matches!(error, ForkMemberError::InvalidRequest(_)));

        let mut prompt_fork = spec("child");
        prompt_fork.launch_mode = MemberLaunchMode::Fork {
            source_member_id: AgentIdentity::from("source"),
            fork_context: meerkat_mob::launch::ForkContext::default(),
        };
        let error = validate_fork_spec(&prompt_fork).expect_err("helper fork mode must be refused");
        assert!(matches!(error, ForkMemberError::InvalidRequest(_)));

        // Positive control: the default spec the wrapper documents IS accepted,
        // so the guard above is not refusing everything.
        validate_fork_spec(&spec("child")).expect("a Fresh spec is accepted");
    }

    #[test]
    fn runtime_authoritative_labels_are_refused() {
        let mut labelled = spec("child");
        labelled.labels = Some(
            [("agent_identity".to_string(), "spoofed".to_string())]
                .into_iter()
                .collect(),
        );
        assert!(matches!(
            validate_fork_spec(&labelled),
            Err(ForkMemberError::InvalidRequest(_))
        ));

        let mut benign = spec("child");
        benign.labels = Some(
            [("team".to_string(), "review".to_string())]
                .into_iter()
                .collect(),
        );
        validate_fork_spec(&benign).expect("ordinary labels stay accepted");
    }

    /// Running-source rejection is upstream's fact. The wrapper must carry the
    /// typed cause through, not re-spell it.
    #[test]
    fn a_running_source_stays_typed_through_the_wrapper() {
        let error = ForkMemberError::from(MobError::ForkSourceUnavailable {
            source_member_id: "reviewer".to_string(),
            cause: ForkSourceUnavailableCause::Running,
        });
        assert!(matches!(
            error,
            ForkMemberError::Mob(MobError::ForkSourceUnavailable {
                cause: ForkSourceUnavailableCause::Running,
                ..
            })
        ));
        assert!(error.to_string().contains("running"));
        assert!(!error.committed_fork_is_recoverable());

        let absent = ForkMemberError::from(MobError::ForkSourceUnavailable {
            source_member_id: "reviewer".to_string(),
            cause: ForkSourceUnavailableCause::NoSession,
        });
        assert!(matches!(
            absent,
            ForkMemberError::Mob(MobError::ForkSourceUnavailable {
                cause: ForkSourceUnavailableCause::NoSession,
                ..
            })
        ));
    }

    /// A committed-but-unprovisioned fork is recoverable ONLY through the
    /// retained session id. Losing it loses the branch.
    #[test]
    fn a_committed_fork_keeps_its_recovery_affordance() {
        let fork_session_id = meerkat_core::types::SessionId::new();
        let error = ForkMemberError::from(MobError::ForkMemberProvisionFailed {
            member_id: AgentIdentity::from("child"),
            fork_session_id: fork_session_id.clone(),
            reason: "resume provisioning failed".to_string(),
        });
        assert!(error.committed_fork_is_recoverable());
        let data = error
            .structured_data()
            .expect("a committed fork must publish structured recovery data");
        assert_eq!(
            data["kind"],
            serde_json::json!("fork_member_provision_failed")
        );
        assert_eq!(
            data["recovery"],
            serde_json::json!("resume_committed_fork_session")
        );
        assert_eq!(
            data["fork_session_id"],
            serde_json::json!(fork_session_id.to_string()),
        );
        // The Display form alone would lose the machine-readable recovery
        // path; assert both carry the session id.
        assert!(error.to_string().contains(&fork_session_id.to_string()));
    }

    #[test]
    fn meta_side_refusals_publish_no_structured_recovery() {
        assert!(
            ForkMemberError::InvalidRequest("bad".to_string())
                .structured_data()
                .is_none()
        );
        assert!(
            ForkMemberError::SourceAuthority("stale".to_string())
                .structured_data()
                .is_none()
        );
    }

    /// The fork wrapper must address exactly the roster identities a spawn
    /// would, or it forks from a member nobody else can name.
    #[test]
    fn aliases_are_encoded_exactly_as_spawn_encodes_them() {
        for alias in ["reviewer", "rt:review:singleton:0"] {
            let identity = crate::member_comms_id::mob_member_id(alias);
            assert_eq!(
                crate::member_comms_id::runtime_alias_str(identity.as_str()).as_ref(),
                alias,
                "fork must round-trip the alias spawn seats",
            );
        }
        // Positive control: an identity-first alias really is re-spelled, so
        // the round-trip above is not vacuous.
        assert_ne!(
            crate::member_comms_id::mob_member_id("rt:review:singleton:0").as_str(),
            "rt:review:singleton:0",
        );
    }
}
