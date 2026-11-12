//! Where the identity runtime keeps the family of changes and reads of its
//! mob's long-term agents (`identity_creation::IdentityFamily`).
//!
//! A launch attaches the family when its identity runtime is installed.
//! Every surface reads it here at each call: the stdio gateway's and the
//! Console's `meta/identity/*` methods, and the agents' identity tools. A
//! launch whose identity policy is present and enabled serves the family's
//! changes ([`IdentityRuntime::identity_changes_enabled`]): the agents'
//! identity tools are mounted and the Console offers changes exactly then.
//! A launch without one serves the family's reads and refuses every change
//! `change_denied`. The family is detached when the mob stops, with the
//! identity authority.
//!
//! It also keeps what this process read of the records of the agents no
//! member is seated for ([`UnseatedRecords`]): the access rules decide such
//! an agent on its record's profile and labels, and a route that primes
//! them reads a record once, not at every request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::IdentityRuntime;
use crate::identity_creation::{IdentityFamily, PeerVisibility};
use crate::identity_first::AgentIdentity;

/// How long the profile and labels read from an agent's record are decided
/// on before the record is read again. A change of a record that this
/// process did not see committed (a host that writes through the mob's own
/// handle) is decided on at the latest this long after it.
pub(crate) const UNSEATED_RECORD_MAX_AGE: Duration = Duration::from_secs(20);

/// What the access rules decide an agent no member is seated for on: the
/// profile and the labels of its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnseatedRecord {
    /// The record's profile, which the rules read as the agent's role.
    pub(crate) profile: String,
    /// The record's labels.
    pub(crate) labels: BTreeMap<String, String>,
}

/// The records this process read, by agent name, for the agents whose
/// record is their row in the mob's identity store and that no roster row
/// names, each with when it was read.
///
/// An entry leaves when a change of its agent is committed, or attempted,
/// in this process ([`Self::forget`]), when its agent is no longer asked
/// for (a roster row names it now, or it is gone) and when it is older than
/// [`UNSEATED_RECORD_MAX_AGE`]. A record that could not be read is never
/// kept: it is read again the next time it is asked for.
#[derive(Debug, Default)]
pub(crate) struct UnseatedRecords {
    /// Counts the forgets: a read that started before one keeps nothing,
    /// so a record read before a change never outlives the change.
    forgets: u64,
    held: BTreeMap<String, (UnseatedRecord, Instant)>,
}

/// What [`UnseatedRecords::ask`] answers: the records held, and the names
/// to read.
#[derive(Debug)]
pub(crate) struct UnseatedRecordsAsked {
    /// The records held for the names asked.
    pub(crate) held: BTreeMap<String, UnseatedRecord>,
    /// The names asked that no record is held for.
    pub(crate) missing: Vec<String>,
    forgets: u64,
}

impl UnseatedRecords {
    /// The records held at `now` for `names`, which are all the agents that
    /// have a record and no roster row: every other entry leaves, and so
    /// does one read [`UNSEATED_RECORD_MAX_AGE`] ago or longer.
    pub(crate) fn ask<'a>(
        &mut self,
        names: impl IntoIterator<Item = &'a str>,
        now: Instant,
    ) -> UnseatedRecordsAsked {
        let names: BTreeSet<&str> = names.into_iter().collect();
        self.held.retain(|name, (_, read_at)| {
            names.contains(name.as_str())
                && now.saturating_duration_since(*read_at) < UNSEATED_RECORD_MAX_AGE
        });
        let mut held = BTreeMap::new();
        let mut missing = Vec::new();
        for name in names {
            match self.held.get(name) {
                Some((record, _)) => {
                    held.insert(name.to_owned(), record.clone());
                }
                None => missing.push(name.to_owned()),
            }
        }
        UnseatedRecordsAsked {
            held,
            missing,
            forgets: self.forgets,
        }
    }

    /// Keep the records `read` for what `asked` missed, as read at
    /// `read_at`, unless a record was forgotten since `asked` was answered.
    pub(crate) fn keep(
        &mut self,
        asked: &UnseatedRecordsAsked,
        read: impl IntoIterator<Item = (String, UnseatedRecord)>,
        read_at: Instant,
    ) {
        if self.forgets != asked.forgets {
            return;
        }
        for (name, record) in read {
            self.held.insert(name, (record, read_at));
        }
    }

    /// Forget the record held for the agent `name`: its record changed, or
    /// may have.
    pub(crate) fn forget(&mut self, name: &str) {
        self.forgets = self.forgets.wrapping_add(1);
        self.held.remove(name);
    }
}

/// What the row `row` of an agent's record says the access rules decide the
/// agent on; `None` for a record that cannot be read (no row, a row this
/// build cannot read, a row without a spec).
pub(crate) fn unseated_record_of(
    row: Option<&meerkat_mob::CreatedIdentityObservation>,
) -> Option<UnseatedRecord> {
    match row {
        Some(meerkat_mob::CreatedIdentityObservation::Created(record)) => {
            record.spec.as_ref().map(|spec| UnseatedRecord {
                profile: spec.profile.as_str().to_owned(),
                labels: spec.labels.clone(),
            })
        }
        _ => None,
    }
}

/// The records of `members` (each agent's name and its member name in
/// `mob`), read from the mob's identity store in one read per page of
/// names; `None` for a record that cannot be read, and for every record
/// when the read fails.
pub(crate) async fn read_unseated_records(
    mob: &meerkat_mob::MobHandle,
    members: &[(String, meerkat_mob::AgentIdentity)],
) -> BTreeMap<String, Option<UnseatedRecord>> {
    let asked = members
        .iter()
        .map(|(_, member)| member.clone())
        .collect::<Vec<_>>();
    let rows = match mob.identity_records(&asked).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(
                %error,
                "the records of the agents no member is seated for could not be read; each is \
                 decided as private to nobody known"
            );
            BTreeMap::new()
        }
    };
    members
        .iter()
        .map(|(name, member)| (name.clone(), unseated_record_of(rows.get(member))))
        .collect()
}

impl IdentityRuntime {
    /// Attach `family`, of a launch whose identity policy is present and
    /// enabled: every surface of the mob calls it from now on, and it
    /// serves changes.
    pub fn attach_identity_family(&self, family: Arc<dyn IdentityFamily>) {
        self.identity_family.attach(family);
    }

    /// Attach `family`, of a launch without an enabled identity policy:
    /// every surface of the mob reads through it, and it refuses every
    /// change.
    pub fn attach_identity_reads(&self, family: Arc<dyn IdentityFamily>) {
        self.identity_family.attach_reads(family);
    }

    /// Attach the launch's own family, which serves changes when
    /// `changes_enabled`.
    pub(crate) fn attach_identity_gate(
        &self,
        gate: Arc<crate::identity_creation::IdentityGate>,
        changes_enabled: bool,
    ) {
        self.identity_family.attach_gate(gate, changes_enabled);
    }

    /// The mob this runtime serves, as the handle its composition gave
    /// its services: one that does not keep the mob owned. The family's
    /// background work (a change carried out after a person approved it)
    /// acts through it.
    pub(crate) fn family_mob_handle(&self) -> Option<meerkat_mob::MobHandle> {
        self.runtime_services.mob_handle()
    }

    /// Whether the record of the agent `name` is its row in the mob's
    /// identity store: an agent created at run time, or adopted. `false`
    /// for an agent its host configures, for a name no agent holds, and for
    /// a record this runtime cannot read: a run-time change of such an
    /// agent is refused, so a surface offers none.
    pub(crate) async fn record_is_store_owned(&self, name: &str) -> bool {
        let Ok(identity) = AgentIdentity::parse(name) else {
            return false;
        };
        matches!(
            self.spec_record(&identity).await,
            Ok(super::created::SpecRecord::Store)
        )
    }

    /// The attached family as a caller that sees the agents `seen` admits
    /// calls it: a peer that caller may not see is answered as a name no
    /// agent holds.
    pub fn identity_family_seen_by(
        &self,
        seen: Arc<dyn PeerVisibility>,
    ) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_family.seen_by(seen)
    }

    /// The attached family: `None` before the launch attached one, and once
    /// the mob stopped.
    pub fn identity_family(&self) -> Option<Arc<dyn IdentityFamily>> {
        self.identity_family.get()
    }

    /// Whether this launch serves changes of its agents: true exactly when
    /// its identity policy is present and enabled. The agents' identity
    /// tools are mounted, the Console offers changes and a decision is
    /// driven exactly when it is true; the family's reads are served either
    /// way.
    pub fn identity_changes_enabled(&self) -> bool {
        self.identity_family.changes_enabled()
    }

    /// Detach the family: the mob stopped.
    pub(crate) fn detach_identity_family(&self) {
        self.identity_family.clear();
    }

    /// A change of the family wrote the record of the agent `name` (a
    /// create, an update, an archive, an unarchive, an adoption, a
    /// removal): serve the agent from its record from now on. Its
    /// registration is read again from its record, made when this runtime
    /// had none, and forgotten when the record is gone. It reads one
    /// record, sends the mob no command and wakes nothing; a read that
    /// fails leaves the registration to the agent's next door, which reads
    /// the record again.
    pub(crate) async fn note_agent_record_changed(&self, name: &str) {
        let Ok(identity) = AgentIdentity::parse(name) else {
            return;
        };
        if let Err(error) = self.register_from_row(&identity).await {
            tracing::debug!(
                identity = %identity,
                %error,
                "the registration of a changed agent is left to its next door"
            );
        }
    }

    /// Forget what this process read of the record of the agent `name`: a
    /// change of it was committed, or attempted, here. The next route that
    /// primes the access rules reads the record again.
    pub(crate) fn forget_unseated_record(&self, name: &str) {
        self.unseated_records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forget(name);
    }

    /// What the records of `members` say the access rules decide them on,
    /// by agent name: `members` are all the agents of `mob` whose record is
    /// their row in its identity store and that no roster row names, each
    /// with its member name. A record this process read less than
    /// [`UNSEATED_RECORD_MAX_AGE`] ago, with no change of its agent
    /// committed or attempted here since, is answered as read; the others
    /// are read now, in one read per page of names. `None` for a record
    /// that cannot be read.
    pub(crate) async fn unseated_records(
        &self,
        mob: &meerkat_mob::MobHandle,
        members: &[(String, meerkat_mob::AgentIdentity)],
    ) -> BTreeMap<String, Option<UnseatedRecord>> {
        let now = Instant::now();
        let asked = self
            .unseated_records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ask(members.iter().map(|(name, _)| name.as_str()), now);
        let mut records = asked
            .held
            .iter()
            .map(|(name, record)| (name.clone(), Some(record.clone())))
            .collect::<BTreeMap<_, _>>();
        if asked.missing.is_empty() {
            return records;
        }
        let missing = members
            .iter()
            .filter(|(name, _)| asked.missing.contains(name))
            .cloned()
            .collect::<Vec<_>>();
        let read = read_unseated_records(mob, &missing).await;
        self.unseated_records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keep(
                &asked,
                read.iter()
                    .filter_map(|(name, record)| Some((name.clone(), record.clone()?))),
                now,
            );
        records.extend(read);
        records
    }

    /// [`Self::unseated_records`] for a route that has no identity runtime:
    /// every record is read now.
    pub(crate) async fn read_unseated_records(
        mob: &meerkat_mob::MobHandle,
        members: &[(String, meerkat_mob::AgentIdentity)],
    ) -> BTreeMap<String, Option<UnseatedRecord>> {
        read_unseated_records(mob, members).await
    }

    /// What this runtime's registration of each agent its host configures
    /// says the access rules decide the agent on, by agent name: the
    /// registered profile and labels, as registered now. No read of Core or
    /// of a record, and nothing is kept: an agent the runtime no longer
    /// serves is not answered, and a registration that changed is answered
    /// as it is.
    pub(crate) async fn configured_records(&self) -> BTreeMap<String, UnseatedRecord> {
        self.entries
            .read()
            .await
            .iter()
            .filter(|(_, entry)| !entry.created)
            .map(|(identity, entry)| {
                (
                    identity.as_str().to_owned(),
                    UnseatedRecord {
                        profile: entry.spec.profile.as_str().to_owned(),
                        labels: entry.spec.labels.clone(),
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn record(profile: &str, labels: &[(&str, &str)]) -> UnseatedRecord {
        UnseatedRecord {
            profile: profile.to_owned(),
            labels: labels
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    /// Whether a person the rules deny the agents labelled `tier = sealed`
    /// may view the agent `scout`, decided on `record` as a route that
    /// primes the access rules records it.
    fn denied_person_views(record: &UnseatedRecord) -> bool {
        let config: crate::AccessControlConfig = toml::from_str(
            r#"
enabled = true
admins = ["root@example.org"]

[[rules]]
id = "people"
actions = ["agent.view"]

[[rules]]
id = "nothing-sealed"
effect = "deny"
subjects = ["bob@example.org"]
actions = ["agent.view"]
match_labels = { tier = "sealed" }
"#,
        )
        .expect("access TOML");
        let controller = crate::AccessController::new(config).expect("controller");
        controller.record_agent_attributes(crate::access::AgentResourceAttributes {
            identity: "scout".to_owned(),
            agent_id: None,
            role: Some(record.profile.clone()),
            labels: record.labels.clone(),
        });
        controller
            .view_for_subject(Some("bob@example.org"))
            .allows_agent("agent.view", "scout")
    }

    /// A record that changed with no change committed or attempted in this
    /// process (a host wrote it through the mob's own handle) is decided on
    /// as it was read while it is younger than the bound, with no read, and
    /// is read again once it is as old as the bound: the rules then decide
    /// on the record as it is.
    #[test]
    fn a_record_changed_past_this_process_is_decided_on_again_after_the_bound() {
        let mut records = UnseatedRecords::default();
        let start = Instant::now();
        let asked = records.ask(["scout"], start);
        assert_eq!(asked.missing, vec!["scout".to_owned()]);
        records.keep(&asked, [("scout".to_owned(), record("worker", &[]))], start);

        // The record gets a label a deny names, past this process.
        let now = record("worker", &[("tier", "sealed")]);

        let inside = start + UNSEATED_RECORD_MAX_AGE - Duration::from_millis(1);
        let asked = records.ask(["scout"], inside);
        assert!(asked.missing.is_empty(), "no read inside the bound");
        assert_eq!(asked.held.get("scout"), Some(&record("worker", &[])));

        let at_the_bound = start + UNSEATED_RECORD_MAX_AGE;
        let asked = records.ask(["scout"], at_the_bound);
        assert_eq!(
            asked.missing,
            vec!["scout".to_owned()],
            "the record is read again at the bound"
        );
        assert!(asked.held.is_empty());
        records.keep(&asked, [("scout".to_owned(), now.clone())], at_the_bound);
        let asked = records.ask(["scout"], at_the_bound);
        let decided_on = asked.held.get("scout").expect("the record read again");
        assert!(denied_person_views(&record("worker", &[])));
        assert!(
            !denied_person_views(decided_on),
            "the rules decide on the record as it is now"
        );
    }

    /// A record read before a change of its agent was committed or
    /// attempted here is not kept, whichever agent the change named; a
    /// forgotten record is read again; and a record of an agent that is no
    /// longer asked for (a roster row names it, or it is gone) leaves.
    #[test]
    fn a_record_read_before_a_change_is_not_kept_and_a_forgotten_one_is_read_again() {
        let mut records = UnseatedRecords::default();
        let start = Instant::now();
        let asked = records.ask(["scout", "clerk"], start);
        // A change is committed while the rows are read.
        records.forget("scout");
        records.keep(
            &asked,
            [
                ("scout".to_owned(), record("worker", &[])),
                ("clerk".to_owned(), record("worker", &[])),
            ],
            start,
        );
        let asked = records.ask(["scout", "clerk"], start);
        assert_eq!(
            asked.missing,
            vec!["clerk".to_owned(), "scout".to_owned()],
            "nothing read before the change is kept"
        );
        records.keep(
            &asked,
            [
                ("scout".to_owned(), record("worker", &[])),
                ("clerk".to_owned(), record("worker", &[])),
            ],
            start,
        );
        assert!(records.ask(["scout", "clerk"], start).missing.is_empty());

        records.forget("scout");
        let asked = records.ask(["scout", "clerk"], start);
        assert_eq!(asked.missing, vec!["scout".to_owned()]);
        records.keep(&asked, [("scout".to_owned(), record("worker", &[]))], start);

        // Only one agent is still asked for: the other's record leaves.
        assert!(records.ask(["clerk"], start).missing.is_empty());
        assert_eq!(
            records.ask(["scout", "clerk"], start).missing,
            vec!["scout".to_owned()]
        );
    }

    /// A record that cannot be read answers nothing to decide on: no row,
    /// and a row this build cannot read.
    #[test]
    fn a_record_that_cannot_be_read_answers_nothing_to_decide_on() {
        assert_eq!(unseated_record_of(None), None);
        let unreadable = meerkat_mob::CreatedIdentityObservation::Unreadable {
            evidence_digest: "sha256:00".to_owned(),
            detail: "a row of a later release".to_owned(),
        };
        assert_eq!(unseated_record_of(Some(&unreadable)), None);
    }
}
