//! The identity census of `meta_gateway storage-migrate-native`.
//!
//! Each identity the projection side binds (a continuity record of the
//! directory, or an entry of a continuity export) is compared with Core's
//! last binding for it in the directory's durable mob storage, read from a
//! copy with Core's own member-history reader. With in-memory mob storage
//! Core has nothing to compare, and each bound identity gets a first-boot
//! manifest row instead.
//!
//! Durable mob storage, an identity the projection side binds to session S:
//! - R4: Core last bound S to another identity: refused, IdentityDisagrees.
//! - R1: Core's last binding for the identity is on S (live or retired):
//!   bound, with a first-boot Resume row on S. The row reads done at the
//!   first boot; the first boot respawns a live member it finds unmarked
//!   once, so every migrated identity comes back the same way.
//! - R2: it is on another session: refused, IdentityDisagrees.
//! - R3: it has no session: refused, IdentityDisagrees.
//! - R5: Core holds no binding for the identity since its latest mob reset:
//!   refused, CoreMissing.
//!
//! Durable mob storage, an identity with no session on the projection side
//! (an uninitialized export entry, or an identity Core binds that no
//! continuity record names):
//! - U2: Core holds no binding for it: nothing.
//! - U3: Core's last binding is live: refused, IdentityDisagrees (0.9.3
//!   retires before it deletes, so a live member without a record is not a
//!   settled state).
//! - U4: Core's last binding is retired, which is what 0.9.3's delete leaves:
//!   a manifest row identity -> Fresh. The first 0.9.4 boot gives the
//!   identity a fresh session, as 0.9.3 would have; the old session stays in
//!   Core, unbound. (A retired raw member, below, gets no row.)
//!
//! An export file lists only the identities its operator named. An identity
//! Core binds to a session the census knows, that the file does not list,
//! is refused Unlisted: its provider copy may be ahead of Core's, and a
//! deleted one would come back. A directory's continuity records list every
//! identity 0.9.3 knew, so there such an identity takes the rules above for
//! an identity without a session.
//!
//! U3 is about members the identity runtime spawned: each spawn labels its
//! member `agent_identity` with the identity its member id encodes, and the
//! raw member creation surfaces refuse that label. A live member Core binds
//! to a session the census knows, that no claim names and that carries no
//! such label, is a raw member the host made directly on the mob (0.9.3's
//! `meta/ensure_member` and `meta/spawn_member`, a helper, a fork child, an
//! embedder's own mob call), for which 0.9.3 never kept a record: RawMember,
//! admitted, with no manifest row. Core keeps the member, its session and
//! its history, and 0.9.4 leaves it to the host, as 0.9.3 did. A retired
//! member without the label (a helper or a fork child the host made on the
//! mob and retired) is a RawMember too, with no manifest row: an identity's
//! wake resumes only a member the identity runtime spawned, so nothing is
//! left for a first boot to carry out, and a Fresh row for it would wait
//! for a roster that never lists it. This holds for both claim sources: an
//! export file cannot list a member 0.9.3 never recorded.
//!
//! In-memory mob storage: R6, each bound identity gets a manifest row
//! identity -> Resume { session }; U1, an identity without a session gets
//! nothing, and its first wake is fresh.
//!
//! The 0.9.3 record values (runtime id, generation, checkpoint version)
//! never decide a row; details print them for the operator.
//!
//! Core's mob keys a member by the roster's comms-safe encoding of the
//! identity (`mk--agent_calpha` for `agent:alpha`), 0.9.3's spelling too:
//! every read goes through that encoding, and every identity Core reports is
//! decoded back before it is compared or shown.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_core::types::SessionId;
use meerkat_mob::{MemberBinding, MemberBindingHistory, MemberBindingState};
use serde::{Deserialize, Serialize};

use crate::member_comms_id::{roster_member_id_for_identity, runtime_alias_str};

/// The remedy for every IdentityDisagrees row.
const SETTLE_ACTION: &str = "settle the identity's record with the 0.9.3 binary (or delete the \
                             identity), then rerun the export and storage-migrate-native";

/// One identity as the projection side states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdentityClaim {
    pub(crate) identity: String,
    /// The session the projection side binds the identity to; `None` for an
    /// uninitialized export entry.
    pub(crate) session: Option<SessionId>,
    /// The record's 0.9.3 values, appended to every detail.
    pub(crate) values_093: Option<String>,
}

/// Where the claims come from, which decides how an identity Core binds and
/// the claims do not name is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimSource {
    /// The directory's own continuity records: every identity 0.9.3 knew.
    ContinuityRecords,
    /// An operator's export file: only the identities the operator listed.
    ExportFile,
}

/// How one identity compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityCensusClass {
    /// Core's last binding is on the session the projection side binds.
    Bound,
    /// In-memory mob storage: the first 0.9.4 boot resumes the identity on
    /// its session (a manifest Resume row).
    Resume,
    /// No session on either side: nothing to carry; the first wake is fresh.
    Unbound,
    /// The identity's last binding is retired and the projection side holds
    /// no session for it: the first 0.9.4 boot starts it fresh (a manifest
    /// Fresh row).
    FreshAfterDelete,
    /// Core's binding and the projection side disagree.
    IdentityDisagrees,
    /// Core holds no binding for an identity the projection side binds.
    CoreMissing,
    /// Core binds an identity the export file does not list.
    Unlisted,
    /// A member, live or retired, that no claim names and no identity-first
    /// spawn made: the host made it directly on the mob (a helper or a fork
    /// child, say), and 0.9.3 never recorded it. Core keeps it; no
    /// first-boot row.
    RawMember,
}

impl IdentityCensusClass {
    /// Whether migrating keeps the identity as 0.9.3 would serve it.
    pub fn admitted(self) -> bool {
        matches!(
            self,
            Self::Bound | Self::Resume | Self::Unbound | Self::FreshAfterDelete | Self::RawMember
        )
    }
}

/// One classified identity.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct IdentityCensusRow {
    pub identity: String,
    pub class: IdentityCensusClass,
    pub admitted: bool,
    /// The session the projection side binds, when it binds one.
    pub session_id: Option<String>,
    pub detail: String,
    /// What the operator does before rerunning, for a refused row.
    pub action: Option<String>,
}

/// A first-boot manifest row the apply writes, in the manifest's own JSON
/// row shape (the apply journal keeps the planned rows).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum FirstBootRowPlan {
    /// Resume the identity on `session`.
    Resume { identity: String, session: String },
    /// Start the identity fresh; `retired_session` is the session its last
    /// binding retired.
    Fresh {
        identity: String,
        retired_session: String,
    },
}

impl FirstBootRowPlan {
    /// The manifest row the first-boot manifest writes.
    pub(crate) fn to_manifest_row(&self) -> Result<crate::identity_first::FirstBootRow, String> {
        let identity = |value: &str| {
            crate::identity_first::AgentIdentity::parse(value)
                .map_err(|error| format!("first-boot row identity {value:?}: {error}"))
        };
        let session = |value: &str| {
            SessionId::parse(value)
                .map_err(|error| format!("first-boot row session {value:?}: {error}"))
        };
        Ok(match self {
            Self::Resume {
                identity: name,
                session: id,
            } => crate::identity_first::FirstBootRow::Resume {
                identity: identity(name)?,
                session: session(id)?,
            },
            Self::Fresh {
                identity: name,
                retired_session,
            } => crate::identity_first::FirstBootRow::Fresh {
                identity: identity(name)?,
                retired_session: session(retired_session)?,
            },
        })
    }
}

/// The identity census's result: one row per identity, in identity order,
/// and the manifest rows of the admitted ones.
#[derive(Debug, Clone, Default)]
pub(crate) struct IdentityCensus {
    pub(crate) rows: Vec<IdentityCensusRow>,
    pub(crate) first_boot: Vec<FirstBootRowPlan>,
}

impl IdentityCensus {
    pub(crate) fn refused(&self) -> usize {
        self.rows.iter().filter(|row| !row.admitted).count()
    }
}

/// Run the identity census. `history` is Core's member binding history of
/// the directory's durable mob storage, `None` for in-memory mob storage.
/// `known_sessions` are the sessions the census knows (runtime and
/// projection side): an identity Core last bound to one of them, that no
/// claim names, is read by `source`'s rule.
pub(crate) fn identity_census(
    claims: &[IdentityClaim],
    source: ClaimSource,
    history: Option<&MemberBindingHistory>,
    known_sessions: &BTreeSet<SessionId>,
) -> IdentityCensus {
    let mut rows: BTreeMap<String, IdentityCensusRow> = BTreeMap::new();
    let mut first_boot = Vec::new();
    let Some(history) = history else {
        // With nothing to compare, two claims on one session cannot both
        // resume it: each would get a Resume row on the same session.
        let mut claimants: BTreeMap<&SessionId, Vec<&str>> = BTreeMap::new();
        for claim in claims {
            if let Some(session) = &claim.session {
                claimants
                    .entry(session)
                    .or_default()
                    .push(claim.identity.as_str());
            }
        }
        for claim in claims {
            let shared = claim
                .session
                .as_ref()
                .and_then(|session| claimants.get(session).map(|names| (session, names)))
                .filter(|(_, names)| names.len() > 1);
            let row = match (&claim.session, shared) {
                (_, Some((session, names))) => row(
                    claim,
                    IdentityCensusClass::IdentityDisagrees,
                    format!(
                        "the records bind session {session} to identities {}; one session \
                         resumes one identity",
                        names.join(", ")
                    ),
                    Some(SETTLE_ACTION.to_owned()),
                ),
                (Some(session), None) => {
                    first_boot.push(FirstBootRowPlan::Resume {
                        identity: claim.identity.clone(),
                        session: session.to_string(),
                    });
                    row(
                        claim,
                        IdentityCensusClass::Resume,
                        format!(
                            "in-memory mob storage: the first 0.9.4 boot resumes the identity on \
                             session {session}"
                        ),
                        None,
                    )
                }
                (None, _) => row(
                    claim,
                    IdentityCensusClass::Unbound,
                    "no session: the identity's first wake is fresh".to_owned(),
                    None,
                ),
            };
            rows.insert(claim.identity.clone(), row);
        }
        return finish(rows, first_boot);
    };

    let claimed: BTreeSet<&str> = claims.iter().map(|claim| claim.identity.as_str()).collect();
    for claim in claims {
        let last = history.last_binding(&roster_member_id_for_identity(&claim.identity));
        let row = match &claim.session {
            Some(session) => {
                let row = bound_claim(claim, session, last, history);
                if row.class == IdentityCensusClass::Bound {
                    first_boot.push(FirstBootRowPlan::Resume {
                        identity: claim.identity.clone(),
                        session: session.to_string(),
                    });
                }
                row
            }
            None => unbound_claim(claim, last, &mut first_boot),
        };
        rows.insert(claim.identity.clone(), row);
    }

    // Identities Core binds that no claim names, found through the sessions
    // the census knows.
    for session in known_sessions {
        let Some(binding) = history.binding_for_session(session) else {
            continue;
        };
        let identity = runtime_alias_str(binding.identity().as_str()).into_owned();
        let identity = identity.as_str();
        if claimed.contains(identity) || rows.contains_key(identity) {
            continue;
        }
        let Some(last) = history.last_binding(binding.identity()) else {
            continue;
        };
        let claim = IdentityClaim {
            identity: identity.to_owned(),
            session: None,
            values_093: None,
        };
        if raw_member(last) {
            rows.insert(identity.to_owned(), raw_member_row(&claim, last));
            continue;
        }
        let row = match source {
            ClaimSource::ContinuityRecords => unbound_claim(&claim, Some(last), &mut first_boot),
            ClaimSource::ExportFile => match last.session_id() {
                Some(last_session) => row(
                    &claim,
                    IdentityCensusClass::Unlisted,
                    format!(
                        "the mob storage holds identity {identity} (session {last_session}, {}), \
                         which the export does not list",
                        binding_state(last)
                    ),
                    Some(
                        "add the identity to the identity list, then rerun the export and \
                         storage-migrate-native"
                            .to_owned(),
                    ),
                ),
                None => continue,
            },
        };
        rows.insert(identity.to_owned(), row);
    }
    finish(rows, first_boot)
}

/// R4, then R1-R3 and R5, for an identity the projection side binds to
/// `session`.
fn bound_claim(
    claim: &IdentityClaim,
    session: &SessionId,
    last: Option<&MemberBinding>,
    history: &MemberBindingHistory,
) -> IdentityCensusRow {
    let identity = &claim.identity;
    if let Some(holder) = history.binding_for_session(session)
        && runtime_alias_str(holder.identity().as_str()) != identity.as_str()
    {
        return row(
            claim,
            IdentityCensusClass::IdentityDisagrees,
            format!(
                "the export binds session {session} to identity {identity}, but the mob storage \
                 last bound the session to identity {}",
                runtime_alias_str(holder.identity().as_str())
            ),
            Some(SETTLE_ACTION.to_owned()),
        );
    }
    let Some(last) = last else {
        return row(
            claim,
            IdentityCensusClass::CoreMissing,
            format!(
                "the export binds identity {identity} to session {session}, but the mob storage \
                 holds no member for it since its latest reset"
            ),
            Some(
                "settle or delete the identity with the 0.9.3 binary, then rerun the export and \
                 storage-migrate-native"
                    .to_owned(),
            ),
        );
    };
    match last.session_id() {
        Some(last_session) if last_session == session => row(
            claim,
            IdentityCensusClass::Bound,
            format!(
                "the mob storage's last binding is on session {session} ({}, Core generation {})",
                binding_state(last),
                last.generation().get()
            ),
            None,
        ),
        Some(other) => row(
            claim,
            IdentityCensusClass::IdentityDisagrees,
            format!(
                "the export binds identity {identity} to session {session}, but the mob \
                 storage's last binding for it is session {other} ({}, Core generation {})",
                binding_state(last),
                last.generation().get()
            ),
            Some(SETTLE_ACTION.to_owned()),
        ),
        None => row(
            claim,
            IdentityCensusClass::IdentityDisagrees,
            format!(
                "the export binds identity {identity} to session {session}, but the mob storage \
                 holds it without a session"
            ),
            Some(SETTLE_ACTION.to_owned()),
        ),
    }
}

/// U2-U4 for an identity the projection side holds no session for.
fn unbound_claim(
    claim: &IdentityClaim,
    last: Option<&MemberBinding>,
    first_boot: &mut Vec<FirstBootRowPlan>,
) -> IdentityCensusRow {
    let identity = &claim.identity;
    let Some(last) = last else {
        return row(
            claim,
            IdentityCensusClass::Unbound,
            "the mob storage holds no member for the identity: its first wake is fresh".to_owned(),
            None,
        );
    };
    match (last.state(), last.session_id()) {
        (MemberBindingState::Retired, Some(retired)) => {
            first_boot.push(FirstBootRowPlan::Fresh {
                identity: identity.clone(),
                retired_session: retired.to_string(),
            });
            row(
                claim,
                IdentityCensusClass::FreshAfterDelete,
                format!(
                    "the provider holds no record for identity {identity} and the mob storage's \
                     last binding (session {retired}) is retired, as 0.9.3's delete leaves it: \
                     the first 0.9.4 boot starts the identity fresh, and session {retired} stays \
                     in Core unbound"
                ),
                None,
            )
        }
        (MemberBindingState::Retired, None) => row(
            claim,
            IdentityCensusClass::Unbound,
            "the mob storage's last binding for the identity is retired and holds no session: \
             its first wake is fresh"
                .to_owned(),
            None,
        ),
        (_, session) => row(
            claim,
            IdentityCensusClass::IdentityDisagrees,
            format!(
                "the provider holds no record for identity {identity}, but the mob storage has a \
                 live member for it{}",
                session.map_or(String::new(), |session| format!(" on session {session}"))
            ),
            Some(SETTLE_ACTION.to_owned()),
        ),
    }
}

/// Whether `binding` is a member no identity-first spawn made, live or
/// retired: its spawn labels are labels a raw member creation surface
/// accepts, so they carry no `agent_identity` label, the label every
/// identity-first spawn writes (0.9.4 tells raw members from identities by
/// the same label). A member with that label is never raw, whichever
/// identity it names, and neither is a generated runtime alias.
fn raw_member(binding: &MemberBinding) -> bool {
    matches!(
        binding.state(),
        MemberBindingState::Live | MemberBindingState::Retired
    ) && !crate::member_comms_id::is_reserved_generated_alias(binding.identity().as_str())
        && crate::member_comms_id::validate_raw_identity_labels(binding.labels()).is_ok()
}

/// The RawMember row of a raw member no claim names, live or retired.
fn raw_member_row(claim: &IdentityClaim, member: &MemberBinding) -> IdentityCensusRow {
    let identity = &claim.identity;
    let on_session = member
        .session_id()
        .map_or(String::new(), |session| format!(" on session {session}"));
    let detail = if member.is_live() {
        format!(
            "no continuity record names {identity} and its live member{on_session} carries no \
             identity-first label: the host made it directly on the mob, and 0.9.3 never \
             recorded it. Core keeps the member, its session and its history, and 0.9.4 leaves \
             it to the host, as 0.9.3 did"
        )
    } else {
        format!(
            "no continuity record names {identity} and its retired member{on_session} carries \
             no identity-first label: the host made it directly on the mob (a helper or a fork \
             child, say), and 0.9.3 never recorded it. Core keeps its session and its history, \
             bound to nothing, and no first-boot row is written for it: an identity's wake \
             resumes only a member the identity runtime spawned"
        )
    };
    row(claim, IdentityCensusClass::RawMember, detail, None)
}

fn binding_state(binding: &MemberBinding) -> &'static str {
    match binding.state() {
        MemberBindingState::Live => "live",
        MemberBindingState::Retired => "retired",
        _ => "moved",
    }
}

fn row(
    claim: &IdentityClaim,
    class: IdentityCensusClass,
    detail: String,
    action: Option<String>,
) -> IdentityCensusRow {
    let detail = match &claim.values_093 {
        Some(values) => format!("{detail} ({values})"),
        None => detail,
    };
    IdentityCensusRow {
        identity: claim.identity.clone(),
        admitted: class.admitted(),
        class,
        session_id: claim.session.as_ref().map(ToString::to_string),
        detail,
        action,
    }
}

fn finish(
    rows: BTreeMap<String, IdentityCensusRow>,
    mut first_boot: Vec<FirstBootRowPlan>,
) -> IdentityCensus {
    let rows: Vec<IdentityCensusRow> = rows.into_values().collect();
    // A refused identity writes nothing, so it carries no manifest row.
    let refused: BTreeSet<&str> = rows
        .iter()
        .filter(|row| !row.admitted)
        .map(|row| row.identity.as_str())
        .collect();
    first_boot.retain(|row| match row {
        FirstBootRowPlan::Resume { identity, .. } | FirstBootRowPlan::Fresh { identity, .. } => {
            !refused.contains(identity.as_str())
        }
    });
    first_boot.sort();
    IdentityCensus { rows, first_boot }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_mob::{Generation, MemberIdentityManagement, ProfileName, SpawnContinuityIntent};

    fn session(n: u8) -> SessionId {
        SessionId::parse(&format!("019f0000-0000-7000-8000-0000000000{n:02x}")).expect("id")
    }

    fn binding(
        identity: &str,
        session: Option<SessionId>,
        state: MemberBindingState,
    ) -> MemberBinding {
        MemberBinding::for_test(
            roster_member_id_for_identity(identity),
            Generation::new(1),
            session,
            state,
            ProfileName::from("worker"),
            SpawnContinuityIntent::default(),
            MemberIdentityManagement::CommandDriven,
        )
    }

    fn history(bindings: Vec<MemberBinding>) -> MemberBindingHistory {
        MemberBindingHistory::for_test(bindings.clone(), bindings, [], 1)
    }

    fn claim(identity: &str, session: Option<SessionId>) -> IdentityClaim {
        IdentityClaim {
            identity: identity.to_owned(),
            session,
            values_093: Some(
                "0.9.3 record: agent_runtime_id rt, generation 0, checkpoint_version 1".to_owned(),
            ),
        }
    }

    fn classes(census: &IdentityCensus) -> Vec<(&str, IdentityCensusClass)> {
        census
            .rows
            .iter()
            .map(|row| (row.identity.as_str(), row.class))
            .collect()
    }

    #[test]
    fn durable_rules_r1_to_r5() {
        let history = history(vec![
            binding("agent:r1", Some(session(1)), MemberBindingState::Live),
            binding("agent:r2", Some(session(9)), MemberBindingState::Retired),
            binding("agent:r3", None, MemberBindingState::Live),
            binding("agent:other", Some(session(4)), MemberBindingState::Live),
        ]);
        let claims = [
            claim("agent:r1", Some(session(1))),
            claim("agent:r2", Some(session(2))),
            claim("agent:r3", Some(session(3))),
            claim("agent:r4", Some(session(4))),
            claim("agent:r5", Some(session(5))),
        ];
        let census = identity_census(
            &claims,
            ClaimSource::ExportFile,
            Some(&history),
            &BTreeSet::new(),
        );
        assert_eq!(
            classes(&census),
            [
                ("agent:r1", IdentityCensusClass::Bound),
                ("agent:r2", IdentityCensusClass::IdentityDisagrees),
                ("agent:r3", IdentityCensusClass::IdentityDisagrees),
                ("agent:r4", IdentityCensusClass::IdentityDisagrees),
                ("agent:r5", IdentityCensusClass::CoreMissing),
            ]
        );
        assert_eq!(
            census.first_boot,
            [FirstBootRowPlan::Resume {
                identity: "agent:r1".to_owned(),
                session: session(1).to_string(),
            }]
        );
        assert_eq!(census.refused(), 4);
        assert!(
            census.rows[1]
                .detail
                .contains("session 019f0000-0000-7000-8000-000000000009")
        );
        assert!(census.rows[1].detail.ends_with("checkpoint_version 1)"));
        assert!(census.rows[3].detail.contains("to identity agent:other"));
    }

    #[test]
    fn durable_rules_u2_to_u4() {
        let history = history(vec![
            binding("agent:u3", Some(session(3)), MemberBindingState::Live),
            binding("agent:u4", Some(session(4)), MemberBindingState::Retired),
        ]);
        let claims = [
            claim("agent:u2", None),
            claim("agent:u3", None),
            claim("agent:u4", None),
        ];
        let census = identity_census(
            &claims,
            ClaimSource::ExportFile,
            Some(&history),
            &BTreeSet::new(),
        );
        assert_eq!(
            classes(&census),
            [
                ("agent:u2", IdentityCensusClass::Unbound),
                ("agent:u3", IdentityCensusClass::IdentityDisagrees),
                ("agent:u4", IdentityCensusClass::FreshAfterDelete),
            ]
        );
        assert_eq!(
            census.first_boot,
            [FirstBootRowPlan::Fresh {
                identity: "agent:u4".to_owned(),
                retired_session: session(4).to_string(),
            }]
        );
    }

    #[test]
    fn an_identity_the_export_does_not_list_is_unlisted() {
        let history = history(vec![
            binding("agent:listed", Some(session(1)), MemberBindingState::Live),
            // The identity runtime spawned it: a retired raw member is
            // admitted instead (see the raw member test).
            identity_first(
                binding(
                    "agent:forgotten",
                    Some(session(2)),
                    MemberBindingState::Retired,
                ),
                "agent:forgotten",
            ),
        ]);
        let known: BTreeSet<SessionId> = [session(1), session(2)].into_iter().collect();
        let census = identity_census(
            &[claim("agent:listed", Some(session(1)))],
            ClaimSource::ExportFile,
            Some(&history),
            &known,
        );
        assert_eq!(
            classes(&census),
            [
                ("agent:forgotten", IdentityCensusClass::Unlisted),
                ("agent:listed", IdentityCensusClass::Bound),
            ]
        );

        // The directory's own records list every identity: the same
        // identity there is a 0.9.3 delete (U4).
        let census = identity_census(
            &[claim("agent:listed", Some(session(1)))],
            ClaimSource::ContinuityRecords,
            Some(&history),
            &known,
        );
        assert_eq!(census.rows[0].class, IdentityCensusClass::FreshAfterDelete);
        assert_eq!(
            census.first_boot,
            [
                FirstBootRowPlan::Resume {
                    identity: "agent:listed".to_owned(),
                    session: session(1).to_string(),
                },
                FirstBootRowPlan::Fresh {
                    identity: "agent:forgotten".to_owned(),
                    retired_session: session(2).to_string(),
                },
            ]
        );
    }

    /// `binding`, spawned the way the identity runtime spawns: its labels
    /// carry the identity-first label naming `identity`.
    fn identity_first(binding: MemberBinding, identity: &str) -> MemberBinding {
        binding.with_labels(BTreeMap::from([(
            "agent_identity".to_owned(),
            identity.to_owned(),
        )]))
    }

    /// A member no identity-first spawn made is a raw member, live or
    /// retired, with no manifest row: a retired one (a helper or a fork child
    /// the host made on the mob) would otherwise get a `Fresh` row that waits
    /// for a roster that never lists it.
    #[test]
    fn a_member_no_identity_first_spawn_made_is_a_raw_member_live_or_retired() {
        let history = history(vec![
            identity_first(
                binding("agent:lost", Some(session(1)), MemberBindingState::Live),
                "agent:lost",
            ),
            binding("hl-direct", Some(session(2)), MemberBindingState::Live),
            identity_first(
                binding("agent:borrowed", Some(session(3)), MemberBindingState::Live),
                "agent:other",
            ),
            binding("rt:agent:gen:3", Some(session(4)), MemberBindingState::Live),
            binding("hl-gone", Some(session(5)), MemberBindingState::Retired),
        ]);
        let known: BTreeSet<SessionId> = (1..=5).map(session).collect();
        let census = identity_census(&[], ClaimSource::ContinuityRecords, Some(&history), &known);
        assert_eq!(
            classes(&census),
            [
                // A member carrying the identity-first label is never raw,
                // whichever identity the label names, and neither is a
                // generated alias: all three stay refused (U3).
                ("agent:borrowed", IdentityCensusClass::IdentityDisagrees),
                ("agent:lost", IdentityCensusClass::IdentityDisagrees),
                ("hl-direct", IdentityCensusClass::RawMember),
                ("hl-gone", IdentityCensusClass::RawMember),
                ("rt:agent:gen:3", IdentityCensusClass::IdentityDisagrees),
            ]
        );
        let raw = &census.rows[2];
        assert!(raw.admitted && raw.action.is_none(), "{raw:?}");
        assert!(
            raw.detail
                .contains(&format!("live member on session {}", session(2))),
            "{raw:?}"
        );
        let retired = &census.rows[3];
        assert!(retired.admitted && retired.action.is_none(), "{retired:?}");
        assert!(
            retired
                .detail
                .contains(&format!("retired member on session {}", session(5))),
            "{retired:?}"
        );
        assert!(
            census.first_boot.is_empty(),
            "a raw member gets no manifest row, live or retired: {:?}",
            census.first_boot
        );
        assert_eq!(census.refused(), 3);

        // An export cannot list a member 0.9.3 never recorded: the same raw
        // member is admitted, while an unlisted identity-first member stays
        // refused.
        let census = identity_census(&[], ClaimSource::ExportFile, Some(&history), &known);
        let export: BTreeMap<&str, IdentityCensusClass> = classes(&census).into_iter().collect();
        assert_eq!(export["hl-direct"], IdentityCensusClass::RawMember);
        assert_eq!(export["hl-gone"], IdentityCensusClass::RawMember);
        assert_eq!(export["agent:lost"], IdentityCensusClass::Unlisted);
        assert!(census.first_boot.is_empty(), "{:?}", census.first_boot);

        // A claim keeps its own rules: an export entry that names the raw
        // member as an identity without a session meets its live member (U3).
        let census = identity_census(
            &[claim("hl-direct", None)],
            ClaimSource::ExportFile,
            Some(&history),
            &known,
        );
        let claimed: BTreeMap<&str, IdentityCensusClass> = classes(&census).into_iter().collect();
        assert_eq!(claimed["hl-direct"], IdentityCensusClass::IdentityDisagrees);
    }

    #[test]
    fn in_memory_rules_r6_and_u1() {
        let census = identity_census(
            &[claim("agent:a", Some(session(1))), claim("agent:b", None)],
            ClaimSource::ExportFile,
            None,
            &BTreeSet::new(),
        );
        assert_eq!(
            classes(&census),
            [
                ("agent:a", IdentityCensusClass::Resume),
                ("agent:b", IdentityCensusClass::Unbound),
            ]
        );
        assert_eq!(
            census.first_boot,
            [FirstBootRowPlan::Resume {
                identity: "agent:a".to_owned(),
                session: session(1).to_string(),
            }]
        );
    }

    /// With in-memory mob storage nothing tells which of two records that
    /// bind one session 0.9.3 served: both refuse, and neither gets a row.
    #[test]
    fn in_memory_rules_refuse_a_session_two_records_bind() {
        let census = identity_census(
            &[
                claim("agent:a", Some(session(1))),
                claim("agent:b", Some(session(1))),
                claim("agent:c", Some(session(2))),
            ],
            ClaimSource::ContinuityRecords,
            None,
            &BTreeSet::new(),
        );
        assert_eq!(
            classes(&census),
            [
                ("agent:a", IdentityCensusClass::IdentityDisagrees),
                ("agent:b", IdentityCensusClass::IdentityDisagrees),
                ("agent:c", IdentityCensusClass::Resume),
            ]
        );
        assert!(
            census.rows[0].detail.contains("agent:a, agent:b"),
            "{:?}",
            census.rows[0]
        );
        assert_eq!(
            census.first_boot,
            [FirstBootRowPlan::Resume {
                identity: "agent:c".to_owned(),
                session: session(2).to_string(),
            }]
        );
    }

    #[test]
    fn manifest_rows_serialize_in_the_manifest_row_shape() {
        let rows = [
            FirstBootRowPlan::Resume {
                identity: "agent:a".to_owned(),
                session: session(1).to_string(),
            },
            FirstBootRowPlan::Fresh {
                identity: "agent:b".to_owned(),
                retired_session: session(2).to_string(),
            },
        ];
        assert_eq!(
            serde_json::to_string(&rows).expect("json"),
            format!(
                "[{{\"kind\":\"resume\",\"identity\":\"agent:a\",\"session\":\"{}\"}},\
                 {{\"kind\":\"fresh\",\"identity\":\"agent:b\",\"retired_session\":\"{}\"}}]",
                session(1),
                session(2)
            )
        );
    }
}
