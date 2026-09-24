//! Token-accounting projection for mob run results.
//!
//! The mob run aggregate owns no token usage, so this projection reads each
//! member's bridge session instead. Collection (async, fallible I/O) is done
//! by [`crate::MobMcpState::mob_run_accounting`]; the meaning - what is summed,
//! what is deliberately absent, and how the numbers are labelled - lives here
//! as a pure function so it is testable without a live mob.

use meerkat_contracts::wire::MOB_RUN_ACCOUNTING_UNPRICED_REASON;
use meerkat_contracts::{
    WireMobRunAccounting, WireMobRunMemberAccounting, WireMobRunUsageAttribution,
};

/// One member's contribution to the accounting projection.
#[derive(Debug, Clone)]
pub struct MobMemberUsageInput {
    pub agent_identity: String,
    pub role: String,
    pub session: MobMemberSessionUsage,
}

/// What the collector could observe about a member's bridge session.
///
/// Absence is typed rather than collapsed to zero: a member whose session
/// cannot be read here has unknown usage, which is not the same fact as a
/// member that used no tokens.
#[derive(Debug, Clone)]
pub enum MobMemberSessionUsage {
    /// The member's status could not be resolved, so not even its session id
    /// is known. One such member must not erase the accounting of every other
    /// member, which is why this is a per-member absence rather than a
    /// collection error.
    StatusUnresolved { reason: String },
    /// MobMachine holds no current bridge session for this member.
    NoSession,
    /// The session id is known but the session is not readable from this host
    /// (remote placement, archived document).
    Unreadable { session_id: String, reason: String },
    /// Session read succeeded; usage is that session's lifetime total.
    Read {
        session_id: String,
        model: String,
        provider: meerkat_core::Provider,
        message_count: u64,
        usage: meerkat_core::Usage,
    },
}

/// Project collected member session usage into the wire accounting envelope.
///
/// Invariants this function is the single owner of:
///
/// - `usage_total` is the sum of exactly the readable member usages, so it is a
///   floor whenever `members_usage_unavailable > 0`.
/// - `member_session_ids` lists every session id that exists, readable or not,
///   because an unreadable-here session is still exportable where it lives.
/// - Every member the collector saw is projected, including the ones it could
///   learn nothing about: `members_usage_unavailable` counts them so the total
///   is never read as complete.
/// - No monetary cost is produced (there is no price data in the catalog);
///   `unpriced_reason` carries that fact instead of a zero.
#[must_use]
pub fn mob_run_accounting_projection(members: Vec<MobMemberUsageInput>) -> WireMobRunAccounting {
    let mut projected = Vec::with_capacity(members.len());
    let mut session_ids = Vec::new();
    let mut total = meerkat_core::Usage::default();
    let mut unavailable = 0usize;

    for member in members {
        let mut entry = WireMobRunMemberAccounting {
            agent_identity: member.agent_identity,
            role: member.role,
            session_id: None,
            model: None,
            provider: None,
            message_count: None,
            usage: None,
            usage_unavailable: None,
        };
        match member.session {
            MobMemberSessionUsage::StatusUnresolved { reason } => {
                entry.usage_unavailable = Some(reason);
                unavailable += 1;
            }
            MobMemberSessionUsage::NoSession => {
                entry.usage_unavailable = Some("member has no current bridge session".to_string());
                unavailable += 1;
            }
            MobMemberSessionUsage::Unreadable { session_id, reason } => {
                session_ids.push(session_id.clone());
                entry.session_id = Some(session_id);
                entry.usage_unavailable = Some(reason);
                unavailable += 1;
            }
            MobMemberSessionUsage::Read {
                session_id,
                model,
                provider,
                message_count,
                usage,
            } => {
                session_ids.push(session_id.clone());
                entry.session_id = Some(session_id);
                entry.model = Some(model);
                entry.provider = Some(provider);
                entry.message_count = Some(message_count);
                // `Usage::add` is the canonical accumulator for already
                // normalized cumulative usage. It drops cache counters because
                // their relation to `input_tokens` is provider-specific, so the
                // total reports none while each member entry keeps its own.
                total.add(&usage);
                entry.usage = Some(usage.into());
            }
        }
        projected.push(entry);
    }

    WireMobRunAccounting {
        attribution: WireMobRunUsageAttribution::SessionCumulative,
        members: projected,
        usage_total: total.into(),
        members_usage_unavailable: unavailable,
        member_session_ids: session_ids,
        unpriced_reason: MOB_RUN_ACCOUNTING_UNPRICED_REASON.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn usage(input_tokens: u64, output_tokens: u64) -> meerkat_core::Usage {
        meerkat_core::Usage {
            input_tokens,
            output_tokens,
            ..Default::default()
        }
    }

    fn read_member(
        agent_identity: &str,
        session_id: &str,
        usage: meerkat_core::Usage,
    ) -> MobMemberUsageInput {
        MobMemberUsageInput {
            agent_identity: agent_identity.to_string(),
            role: "reviewer".to_string(),
            session: MobMemberSessionUsage::Read {
                session_id: session_id.to_string(),
                model: "claude-opus-5".to_string(),
                provider: meerkat_core::Provider::Anthropic,
                message_count: 4,
                usage,
            },
        }
    }

    #[test]
    fn totals_are_the_exact_sum_of_readable_member_usage() {
        let mut cached = usage(250, 25);
        cached.cache_creation_tokens = Some(64);
        cached.cache_read_tokens = Some(128);
        let projection = mob_run_accounting_projection(vec![
            read_member("m-1", "s-1", usage(100, 10)),
            read_member("m-2", "s-2", cached),
        ]);
        assert_eq!(projection.usage_total.input_tokens, 350);
        assert_eq!(projection.usage_total.output_tokens, 35);
        assert_eq!(projection.usage_total.total_tokens, 385);
        // Deliberate asymmetry, owned by `Usage::add`: cache counters are kept
        // per member but dropped from the aggregate, because their relation to
        // `input_tokens` is provider-specific and the aggregate may span
        // providers. An auditor summing the member cache counters will not
        // find them in `usage_total`; the wire doc states that.
        assert_eq!(projection.usage_total.cache_creation_tokens, None);
        assert_eq!(projection.usage_total.cache_read_tokens, None);
        let cached_member = projection
            .members
            .iter()
            .find(|member| member.agent_identity == "m-2")
            .expect("member with cache counters is projected");
        let member_usage = cached_member
            .usage
            .as_ref()
            .expect("readable member reports usage");
        assert_eq!(member_usage.cache_creation_tokens, Some(64));
        assert_eq!(member_usage.cache_read_tokens, Some(128));
        assert_eq!(projection.members_usage_unavailable, 0);
        assert_eq!(projection.member_session_ids, vec!["s-1", "s-2"]);
        assert_eq!(
            projection.attribution,
            WireMobRunUsageAttribution::SessionCumulative
        );
    }

    #[test]
    fn unreadable_members_are_named_not_counted_as_zero() {
        let projection = mob_run_accounting_projection(vec![
            read_member("m-1", "s-1", usage(100, 10)),
            MobMemberUsageInput {
                agent_identity: "m-2".to_string(),
                role: "worker".to_string(),
                session: MobMemberSessionUsage::Unreadable {
                    session_id: "s-2".to_string(),
                    reason: "member session is not readable here: remote".to_string(),
                },
            },
            MobMemberUsageInput {
                agent_identity: "m-3".to_string(),
                role: "worker".to_string(),
                session: MobMemberSessionUsage::NoSession,
            },
        ]);
        assert_eq!(projection.usage_total.input_tokens, 100);
        assert_eq!(projection.members_usage_unavailable, 2);
        // The unreadable member still exposes its session id (exportable where
        // it lives) but carries no usage number.
        let unreadable = projection
            .members
            .iter()
            .find(|member| member.agent_identity == "m-2")
            .expect("unreadable member is projected");
        assert_eq!(unreadable.session_id.as_deref(), Some("s-2"));
        assert!(unreadable.usage.is_none());
        assert!(unreadable.usage_unavailable.is_some());
        let sessionless = projection
            .members
            .iter()
            .find(|member| member.agent_identity == "m-3")
            .expect("sessionless member is projected");
        assert!(sessionless.session_id.is_none());
        assert!(sessionless.usage.is_none());
        // Every session that exists is exportable, including the one this host
        // could not read; only the sessionless member contributes no pointer.
        assert_eq!(projection.member_session_ids, vec!["s-1", "s-2"]);
    }

    /// One member the collector could learn nothing about must not erase the
    /// accounting of the members it could read.
    #[test]
    fn an_unresolvable_member_status_degrades_only_that_member() {
        let projection = mob_run_accounting_projection(vec![
            read_member("m-1", "s-1", usage(70, 7)),
            MobMemberUsageInput {
                agent_identity: "m-2".to_string(),
                role: "worker".to_string(),
                session: MobMemberSessionUsage::StatusUnresolved {
                    reason: "member status is not resolvable here: retired".to_string(),
                },
            },
            read_member("m-3", "s-3", usage(30, 3)),
        ]);
        assert_eq!(projection.usage_total.input_tokens, 100);
        assert_eq!(projection.usage_total.output_tokens, 10);
        assert_eq!(projection.members_usage_unavailable, 1);
        assert_eq!(projection.members.len(), 3);
        let unresolved = projection
            .members
            .iter()
            .find(|member| member.agent_identity == "m-2")
            .expect("unresolved member is still projected");
        assert!(unresolved.session_id.is_none());
        assert!(unresolved.usage.is_none());
        assert!(unresolved.usage_unavailable.is_some());
        // No session id is known for it, so it contributes no export pointer.
        assert_eq!(projection.member_session_ids, vec!["s-1", "s-3"]);
    }

    /// The envelope must never imply a monetary cost it cannot compute.
    #[test]
    fn projection_never_emits_a_cost_field() {
        let projection =
            mob_run_accounting_projection(vec![read_member("m-1", "s-1", usage(5, 5))]);
        let encoded =
            serde_json::to_value(&projection).expect("accounting projection serializes to json");
        let object = encoded
            .as_object()
            .expect("accounting encodes as an object");
        for key in object.keys() {
            assert!(
                !key.contains("cost") || key == "unpriced_reason",
                "accounting must not expose a cost number, found key '{key}'"
            );
        }
        assert_eq!(
            object
                .get("unpriced_reason")
                .and_then(serde_json::Value::as_str),
            Some(MOB_RUN_ACCOUNTING_UNPRICED_REASON)
        );
        assert_eq!(
            object
                .get("attribution")
                .and_then(serde_json::Value::as_str),
            Some("session_cumulative"),
            "attribution must stay a stable machine-readable tag"
        );
    }

    #[test]
    fn empty_mob_projects_zero_totals_without_claiming_completeness() {
        let projection = mob_run_accounting_projection(Vec::new());
        assert_eq!(projection.usage_total.total_tokens, 0);
        assert!(projection.members.is_empty());
        assert!(projection.member_session_ids.is_empty());
        assert_eq!(projection.members_usage_unavailable, 0);
    }
}
