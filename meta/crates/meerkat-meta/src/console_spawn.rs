//! Console projection for agent-tool spawned mob members.
//!
//! Members spawned through the agent-facing mob tools (`mob_spawn_member`,
//! `spawn_member`, `spawn_many_members`, `fork_off`, and `delegate`'s implicit
//! helpers)
//! historically never reached the console: the spawn succeeded, the
//! `initial_message` was delivered to the member, and no frame or identity
//! metadata was ever written under the member's identity. Embedders had to
//! hand-roll kickoff frames server-side to make their workers visible.
//!
//! This module is the spawn-tool-side bridge. After a successful agent-tool
//! spawn the dispatcher hands each spawned member to [`ConsoleSpawnSink`],
//! which:
//!
//! 1. registers the member's runtime-id → identity mapping with the
//!    [`ConsoleEventStore`] so live runtime events aggregate into the same
//!    chat as the kickoff (the identity string must match what
//!    `frame_from_console_event` derives for the member's events);
//! 2. registers spawn labels (`spawned_by`, `via_tool`, plus any labels the
//!    spawn carried) as console identity metadata for sidebar grouping and
//!    ABAC attribute priming;
//! 3. appends the spawn `initial_message` (when present) as a programmatic
//!    `user_input` console event with a deterministic
//!    `spawn-kickoff:{mob_id}:{member_id}:{hash}` event id, so retries and
//!    respawns dedupe to exactly one kickoff frame.
//!
//! A runtime without a console event store simply never installs a sink and
//! the spawn path behaves exactly as before.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::unified_runtime::ConsoleEventStore;

/// Late-binding slot for the console spawn sink. Created when the agent mob
/// tools are installed (no console exists yet at that point) and filled by
/// the unified runtime once its console event store is constructed.
pub(crate) type SharedConsoleSpawnSinkSlot = Arc<std::sync::RwLock<Option<ConsoleSpawnSink>>>;

pub(crate) fn new_console_spawn_sink_slot() -> SharedConsoleSpawnSinkSlot {
    Arc::new(std::sync::RwLock::new(None))
}

/// Console-side label key recording which agent spawned this member.
pub(crate) const SPAWNED_BY_LABEL: &str = "spawned_by";
/// Console-side label key recording which tool performed the spawn.
pub(crate) const VIA_TOOL_LABEL: &str = "via_tool";
/// Console-side label key recording the session of the spawning agent's
/// incarnation, so captured lineage binds the parent incarnation that
/// performed the spawn and not a later member under the same identity.
pub(crate) const SPAWNED_BY_SESSION_LABEL: &str = "spawned_by_session";

/// The label keys of a member's row that are the Console's own. It writes
/// some from its own facts (the member's role, the mob it sits in, the host
/// of an implicit delegate, its group, whether it can be addressed) and
/// reads the host's marks from the others (its attention marks, and
/// `singleton`, which keeps a member from being retired there). The
/// visibility policy, the access rules and the member controls read them,
/// so no spawn call
/// sets one: the agent tool wrapper refuses it
/// ([`console_row_label_in_spawn_args`]), the spawn registry's merge leaves
/// it out ([`merge_registered_labels`]), and the row builder writes the
/// ones it knows from where it read the member.
pub(crate) const CONSOLE_ROW_LABELS: [&str; 10] = [
    "role",
    "source_mob_id",
    "delegate_host_identity",
    "group",
    "addressable",
    "console_watched",
    "console_alert_level",
    "console_degraded",
    "console_degraded_reason",
    "singleton",
];

/// The label a spawn call carries into its member's row for display, and
/// which the row's builders therefore read from a label. The census row
/// holds the builders' label keys against this list and the one above.
#[cfg(test)]
pub(crate) const CARRIED_ROW_LABELS: [&str; 1] = ["display_name"];

/// Whether `key` is one of the [`CONSOLE_ROW_LABELS`].
pub(crate) fn is_console_row_label(key: &str) -> bool {
    CONSOLE_ROW_LABELS.contains(&key)
}

/// The refusal of a spawn call's [`CONSOLE_ROW_LABELS`] entry, naming it.
pub(crate) fn console_row_label_refusal(key: &str) -> String {
    format!(
        "labels.{key} is the Console's own: it writes or reads this key of a member's row itself, and a spawn cannot set it; remove it from the spawn's labels"
    )
}

/// One entry of the Console's spawn registry: what an agent-tool spawn
/// registered for the member it seated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SpawnRegistryEntry {
    /// The roster member id the spawn seated: the mob's own name of the
    /// member, by which the mob's member lifecycle names it.
    pub(crate) member_id: String,
    /// The generation that member held when its spawn registered the
    /// entry: a roster row seated before it is an earlier member's. `None`
    /// for an entry that no spawn of the tool path registered.
    pub(crate) seat_generation: Option<u64>,
    /// The registered console labels: the spawn's own labels and its
    /// runtime-derived lineage (`spawned_by`, `spawned_by_session`,
    /// `via_tool`).
    pub(crate) labels: BTreeMap<String, String>,
}

/// The Console's spawn registry: the entries the agent-tool spawn path
/// registered, keyed by the mob a spawn seated its member in and the
/// member's console identity. A member's name is its mob's own: the same
/// name in two mobs is two members, and each has its own entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SpawnRegistry {
    entries: BTreeMap<(String, String), SpawnRegistryEntry>,
    /// The mobs whose entries cannot be told for the request this copy of
    /// the registry serves: their member lifecycle journal was not read
    /// to its head. Empty in the store's own registry.
    unread_mobs: std::collections::BTreeSet<String>,
}

impl SpawnRegistry {
    /// The entry of a roster row: the one registered for the row's mob and
    /// the row's console identity. Every reader of the registry asks here,
    /// with the mob of the row it reads for. A row of another mob has no
    /// entry, and the row alone decides. Nor has a row of a mob this copy
    /// notes as not read ([`Self::note_unread_mob`]): what the registry
    /// holds of that mob cannot be told for the request.
    pub(crate) fn entry_of_row(&self, mob_id: &str, identity: &str) -> Option<&SpawnRegistryEntry> {
        // A mob this copy notes as not read cannot be told: no row takes
        // an entry of it.
        if self.unread_mobs.contains(mob_id) {
            return None;
        }
        self.entries
            .get(&(mob_id.to_string(), identity.to_string()))
    }

    /// The registered labels of a roster row ([`Self::entry_of_row`]).
    pub(crate) fn labels_of_row(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Option<&BTreeMap<String, String>> {
        self.entry_of_row(mob_id, identity)
            .map(|entry| &entry.labels)
    }

    /// The console identities the registry holds an entry of, each once,
    /// whichever mob its entries are of.
    pub(crate) fn identities(&self) -> impl Iterator<Item = &str> {
        self.entries
            .keys()
            .map(|(_, identity)| identity.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
    }

    /// Whether the registry holds an entry of `identity` in any mob.
    #[cfg(test)]
    pub(crate) fn holds_identity(&self, identity: &str) -> bool {
        self.identities().any(|held| held == identity)
    }

    /// The mobs the registry holds an entry of.
    pub(crate) fn mobs(&self) -> std::collections::BTreeSet<String> {
        self.entries
            .keys()
            .map(|(mob_id, _)| mob_id.clone())
            .collect()
    }

    /// End every entry of `mob_id` that was registered for one of the roster
    /// members `members`, or every entry of `mob_id` when `all` is set: the
    /// member such an entry was registered for is a member no more. Answers
    /// the console identities of the entries ended.
    pub(crate) fn end_entries(
        &mut self,
        mob_id: &str,
        members: &std::collections::BTreeSet<String>,
        all: bool,
    ) -> std::collections::BTreeSet<String> {
        let ended: Vec<(String, String)> = self
            .entries
            .iter()
            .filter(|((mob, _), entry)| {
                mob.as_str() == mob_id && (all || members.contains(&entry.member_id))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &ended {
            self.entries.remove(key);
        }
        ended.into_iter().map(|(_, identity)| identity).collect()
    }

    /// Note the generation the member of the entry of `identity` in
    /// `mob_id` was seated at when its spawn registered the entry.
    pub(crate) fn note_entry_seat(&mut self, mob_id: &str, identity: &str, seat: u64) {
        if let Some(entry) = self
            .entries
            .get_mut(&(mob_id.to_string(), identity.to_string()))
        {
            entry.seat_generation = Some(seat);
        }
    }

    /// Note that `mob_id`'s entries cannot be told for the request this copy
    /// of the registry serves.
    pub(crate) fn note_unread_mob(&mut self, mob_id: &str) {
        self.unread_mobs.insert(mob_id.to_string());
    }

    /// The console identities whose entry this copy cannot tell for its
    /// request: the entries of the mobs noted as unread, and every entry
    /// that `now`, a later copy, holds no more or holds changed. A request
    /// that took this copy before it read the roster reads `now` after: a
    /// name seated anew by no rotation in between has no entry there.
    pub(crate) fn identities_not_told(&self, now: &Self) -> std::collections::BTreeSet<String> {
        self.entries
            .iter()
            .filter(|(key, entry)| {
                self.unread_mobs.contains(&key.0)
                    || now.unread_mobs.contains(&key.0)
                    || now.entries.get(*key) != Some(*entry)
            })
            .map(|((_, identity), _)| identity.clone())
            .collect()
    }

    /// Every label key an entry of `identity` holds, in whichever mob.
    pub(crate) fn label_keys_of(&self, identity: &str) -> std::collections::BTreeSet<String> {
        self.entries
            .iter()
            .filter(|((_, held), _)| held.as_str() == identity)
            .flat_map(|(_, entry)| entry.labels.keys().cloned())
            .collect()
    }

    /// Merge `labels` into the entry of `identity` in `mob_id`, seated as
    /// `member_id`: per key, the latest registration wins, so a respawn can
    /// update its member's labels.
    pub(crate) fn register(
        &mut self,
        mob_id: &str,
        identity: &str,
        member_id: &str,
        labels: BTreeMap<String, String>,
    ) {
        let entry = self
            .entries
            .entry((mob_id.to_string(), identity.to_string()))
            .or_default();
        entry.member_id = member_id.to_string();
        entry.labels.extend(labels);
    }

    /// Remove one registered key from the entry of `identity` in `mob_id`,
    /// and the entry with its last key.
    pub(crate) fn unregister_label(&mut self, mob_id: &str, identity: &str, key: &str) {
        let entry_key = (mob_id.to_string(), identity.to_string());
        if let Some(entry) = self.entries.get_mut(&entry_key) {
            entry.labels.remove(key);
            if entry.labels.is_empty() {
                self.entries.remove(&entry_key);
            }
        }
    }
}

/// What one fact of a mob's member lifecycle journal says of the spawn
/// registry's entries of that mob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeatFact {
    /// A member was seated at `generation`. `fresh` when no rotation made
    /// the seat: a new member holds the name, and an entry registered for
    /// an earlier member of the name was that member's, and ends. A seat a
    /// rotation made (a respawn's, a rotation's, a requested next life's)
    /// and a member's reset seat the same member on: its entry stays.
    Seat {
        member_id: String,
        generation: u64,
        fresh: bool,
    },
    /// The mob was reset: no member before it is a member after it.
    MobReset,
    /// A fact that seats no member: a retirement or its start, an adoption.
    NoSeat,
    /// A fact this reading does not know. It is never passed over as
    /// nothing: what it means for an entry cannot be told.
    Unclassified,
}

/// Classify one member lifecycle fact for the spawn registry. MobMachine
/// decides each fact in the step that writes its journal row: a seat names
/// the rotation that made it, and a seat that names none is a new member.
pub(crate) fn seat_fact(event: &meerkat_mob::MemberLifecycleEvent) -> SeatFact {
    use meerkat_mob::MemberLifecycleEvent as Fact;
    match event {
        Fact::Spawned {
            agent_identity,
            generation,
            rotation,
            ..
        } => SeatFact::Seat {
            member_id: agent_identity.as_str().to_string(),
            generation: generation.get(),
            fresh: rotation.is_none(),
        },
        Fact::Reset {
            agent_identity,
            new_generation,
            ..
        } => SeatFact::Seat {
            member_id: agent_identity.as_str().to_string(),
            generation: new_generation.get(),
            fresh: false,
        },
        Fact::EntryAdopted { .. } | Fact::RetirementStarted { .. } | Fact::Retired { .. } => {
            SeatFact::NoSeat
        }
        Fact::MobReset { .. } => SeatFact::MobReset,
        _ => SeatFact::Unclassified,
    }
}

/// The seats of one roster member that the settles of the spawn registry
/// have read from its mob's journal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SeenSeats {
    /// The generation of the latest seat no rotation made: the member that
    /// holds the name began there.
    pub(crate) fresh: Option<u64>,
    /// The highest generation the settles know of: a row at or below it is
    /// told, a row above it is of a seat not read yet.
    pub(crate) seen: u64,
}

/// What one settle of the spawn registry against a mob's member lifecycle
/// journal found ([`ConsoleSpawnSink::settle_with_journal`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SpawnJournalRead {
    /// The console identities whose entry this settle ended.
    pub(crate) ended: std::collections::BTreeSet<String>,
    /// The journal was not read to its head: what the registry holds of
    /// this mob cannot be told.
    pub(crate) unread: bool,
}

/// What the spawn registry holds of a member cannot be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpawnEntryNotTold;

/// How many journal events one page of a settle asks for, and how many
/// pages one settle reads at most.
const SPAWN_JOURNAL_PAGE: usize = 256;
const SPAWN_JOURNAL_PAGES: usize = 1024;

/// One spawned member, as seen by the agent-tool spawn path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleSpawnSeed {
    /// Target mob, when the spawn path knows it.
    pub(crate) mob_id: Option<String>,
    /// Runtime member id (the mob roster `AgentIdentity`).
    pub(crate) member_id: String,
    /// Console identity for the member's chat. Matches the identity the
    /// console event projection derives for the member's runtime events:
    /// the `agent_identity` label override when the spawn carried one,
    /// otherwise the member id.
    pub(crate) identity: String,
    /// The kickoff content delivered to the member, when present. Either a
    /// JSON string or a content-blocks array, exactly as the spawn carried it.
    pub(crate) initial_message: Option<Value>,
    /// Labels the spawn carried (e.g. `display_name`, `agent_type`, `team`).
    pub(crate) labels: BTreeMap<String, String>,
    /// Identity of the agent that performed the spawn, when known.
    pub(crate) spawned_by: Option<String>,
    /// Session of the spawning agent's incarnation, when known.
    pub(crate) spawned_by_session: Option<String>,
    /// Session of the member this spawn made, when the spawn's own outcome
    /// names it ([`outcome_session_field`]). The kickoff is written only
    /// with it: nothing else says which member the kickoff is the record of.
    pub(crate) session_id: Option<String>,
    /// Tool that performed the spawn (`mob_spawn_member`, `delegate`, ...).
    pub(crate) via_tool: String,
}

/// Sink that projects spawned members into the runtime's console event
/// store. Cheap to clone; failure-isolated — console projection must never
/// fail or delay a spawn.
#[derive(Clone)]
pub(crate) struct ConsoleSpawnSink {
    console_events: ConsoleEventStore,
}

impl ConsoleSpawnSink {
    pub(crate) fn new(console_events: ConsoleEventStore) -> Self {
        Self { console_events }
    }

    /// All console identity metadata registered through this sink: the
    /// spawn registry, each entry under its mob and console identity.
    /// Consumed by roster/attribute projections so spawn labels and lineage
    /// survive wholesale cache rebuilds.
    pub(crate) async fn identity_labels_snapshot(&self) -> SpawnRegistry {
        self.console_events.identity_labels_snapshot().await
    }

    /// The mobs the spawn registry holds an entry of.
    pub(crate) async fn spawn_registry_mobs(&self) -> std::collections::BTreeSet<String> {
        self.console_events.spawn_registry_mobs().await
    }

    /// Settle the registry's entries of `handle`'s mob against that mob's
    /// member lifecycle journal: read it from where the last settle stopped
    /// to its head, end the entry of every member the journal shows seated
    /// anew by no rotation since (the entry was an earlier member's), and
    /// every entry of the mob at a mob reset. A respawn, a rotation and a
    /// reset seat the same member, and its entry stays. A mob never read is
    /// read from its head on.
    ///
    /// Fails closed: a journal that cannot be read answers `unread` for
    /// this settle, and a fact this reading does not know marks the mob's
    /// entries as not to be told from then on.
    pub(crate) async fn settle_with_journal(
        &self,
        handle: &meerkat_mob::MobHandle,
    ) -> SpawnJournalRead {
        let mob_id = handle.mob_id().to_string();
        let unread = || SpawnJournalRead {
            ended: std::collections::BTreeSet::new(),
            unread: true,
        };
        // A second pass only when another settle applied the stretch first.
        for _ in 0..2 {
            let from = match self.console_events.spawn_journal_cursor(&mob_id).await {
                Ok(Some(from)) => from,
                Ok(None) => {
                    #[cfg(test)]
                    self.console_events.note_spawn_journal_reads(1).await;
                    return match handle.events().latest_cursor().await {
                        Ok(head) => {
                            self.console_events.begin_spawn_journal(&mob_id, head).await;
                            SpawnJournalRead::default()
                        }
                        Err(error) => {
                            tracing::warn!(
                                %mob_id,
                                %error,
                                "spawn registry: the head of the mob's journal could not be \
                                 read; its entries cannot be told for this request"
                            );
                            unread()
                        }
                    };
                }
                Err(SpawnEntryNotTold) => return unread(),
            };
            let mut cursor = from;
            let mut seats: BTreeMap<String, SeenSeats> = BTreeMap::new();
            let mut reset = false;
            let mut classified = true;
            let mut at_head = false;
            for _ in 0..SPAWN_JOURNAL_PAGES {
                #[cfg(test)]
                self.console_events.note_spawn_journal_reads(1).await;
                let page = match handle
                    .member_lifecycle_events(
                        meerkat_mob::MobJournalCursor::new(cursor),
                        SPAWN_JOURNAL_PAGE,
                    )
                    .await
                {
                    Ok(page) => page,
                    Err(error) => {
                        tracing::warn!(
                            %mob_id,
                            %error,
                            "spawn registry: the mob's member lifecycle journal could not be \
                             read; its entries cannot be told for this request"
                        );
                        return unread();
                    }
                };
                for record in &page.records {
                    match seat_fact(&record.event) {
                        SeatFact::Seat {
                            member_id,
                            generation,
                            fresh,
                        } => {
                            let read = seats.entry(member_id).or_default();
                            read.seen = read.seen.max(generation);
                            if fresh {
                                read.fresh = Some(generation);
                            }
                        }
                        SeatFact::MobReset => reset = true,
                        SeatFact::NoSeat => {}
                        SeatFact::Unclassified => classified = false,
                    }
                }
                if page.next.get() == cursor {
                    at_head = true;
                    break;
                }
                cursor = page.next.get();
            }
            let Some(ended) = self
                .console_events
                .apply_spawn_journal(&mob_id, from, cursor, &seats, reset)
                .await
            else {
                continue;
            };
            if !classified {
                tracing::warn!(
                    %mob_id,
                    "spawn registry: the mob's journal holds a member lifecycle fact this \
                     reading does not know; its entries cannot be told until a restart"
                );
                self.console_events.mark_spawn_journal_untold(&mob_id).await;
            }
            return SpawnJournalRead {
                ended,
                unread: !(classified && at_head),
            };
        }
        unread()
    }

    /// Settle `mob_id`'s entries before a spawn registers one there, so
    /// that what an earlier member of the name left is ended first and the
    /// new entry is not ended by a later read of its own member's seat.
    /// `handle` is the mob's, when one is found. Without that read behind
    /// it the entry about to be registered cannot be told from an earlier
    /// member's, and the mob's entries are marked so.
    pub(crate) async fn settle_before_registering(
        &self,
        mob_id: &str,
        handle: Option<&meerkat_mob::MobHandle>,
    ) {
        let unread = match handle {
            Some(handle) => self.settle_with_journal(handle).await.unread,
            None => true,
        };
        if unread {
            tracing::warn!(
                %mob_id,
                "spawn registry: a spawn registers an entry with no read of the mob's journal \
                 behind it; the mob's entries cannot be told until a restart"
            );
            self.console_events.mark_spawn_journal_untold(mob_id).await;
        }
    }

    /// Note the generation `member_id` holds in `handle`'s mob as the seat
    /// the entry of `identity` there was registered for: the spawn that
    /// registered it has just seated that member.
    pub(crate) async fn note_registered_seat(
        &self,
        handle: &meerkat_mob::MobHandle,
        identity: &str,
        member_id: &str,
    ) {
        let seated = handle
            .get_member(&meerkat_mob::AgentIdentity::from(member_id))
            .await;
        if let Ok(Some(entry)) = seated {
            self.console_events
                .note_spawn_seat(
                    handle.mob_id().as_str(),
                    member_id,
                    entry.generation.get(),
                    Some(identity),
                )
                .await;
        }
    }

    /// The registered labels of one incarnation of a member, for a reader
    /// that has the roster row in hand before it reads the registry and
    /// decides by what it reads alone: the row of `identity` in `handle`'s
    /// mob, seated as `member_id` at `generation`. The mob's journal is read
    /// only when the row is of a seat the settles have not read yet, so a
    /// reader that asks for every event of a member reads it once per seat.
    /// `Err` when what a spawn registered of that incarnation cannot be
    /// told: the journal could not be read, or the row is an earlier
    /// member's than the one that holds the name now.
    pub(crate) async fn labels_of_seated_row(
        &self,
        handle: &meerkat_mob::MobHandle,
        identity: &str,
        member_id: &str,
        generation: Option<u64>,
    ) -> Result<Option<BTreeMap<String, String>>, SpawnEntryNotTold> {
        let mob_id = handle.mob_id().to_string();
        match self.console_events.spawn_journal_cursor(&mob_id).await {
            // Never read: no spawn the settle stood behind registered an
            // entry there, and an entry that is there is read as it is.
            Ok(None) => {
                return Ok(self.console_events.identity_labels(&mob_id, identity).await);
            }
            Ok(Some(_)) => {}
            Err(SpawnEntryNotTold) => {
                return match self.console_events.identity_labels(&mob_id, identity).await {
                    Some(_) => Err(SpawnEntryNotTold),
                    None => Ok(None),
                };
            }
        }
        // A row that names no generation is settled every time, and takes
        // the entry the settled registry holds.
        let Some(generation) = generation else {
            if self.settle_with_journal(handle).await.unread {
                return Err(SpawnEntryNotTold);
            }
            return Ok(self.console_events.identity_labels(&mob_id, identity).await);
        };
        if !self
            .console_events
            .spawn_seat_is_read(&mob_id, member_id, generation)
            .await
        {
            if self.settle_with_journal(handle).await.unread {
                return Err(SpawnEntryNotTold);
            }
            // The row is in hand, so its seat is at or before the head just
            // read: a seat the stretches read does not name was made before
            // the settle began, and no seat of the name was read since.
            self.console_events
                .note_spawn_seat(&mob_id, member_id, generation, None)
                .await;
        }
        self.console_events
            .spawn_labels_of_incarnation(&mob_id, identity, member_id, generation)
            .await
    }

    /// Register the member's console identity and, for a seed that names
    /// the member's session, append its kickoff event.
    /// Idempotent per `(mob_id, member_id, initial_message)`.
    pub(crate) async fn project_spawned_member(&self, seed: &ConsoleSpawnSeed) {
        let identity = seed.identity.trim();
        let member_id = seed.member_id.trim();
        if identity.is_empty() || member_id.is_empty() {
            return;
        }
        // The runtime-plane identities name the console's own events: the
        // console aggregator exempts them from the roster-visibility gate,
        // so a member registered under one would bypass it. Every creation
        // path refuses them; this projection-side backstop covers a member
        // created before they did.
        if crate::console_contracts::is_runtime_plane_identity(identity) {
            crate::console_contracts::warn_runtime_plane_member_drop(
                member_id,
                None,
                identity,
                "spawn projection",
            );
            return;
        }

        // Live runtime events arrive keyed by runtime ids — `{member}:{gen}`
        // (and `rt:{member}:{gen}` in identity-first packs). Register both
        // prefixes so every event form resolves to the kickoff's identity.
        self.console_events
            .register_runtime_identity(member_id, identity)
            .await;
        self.console_events
            .register_runtime_identity(format!("rt:{member_id}"), identity)
            .await;

        // The registry's entry is its mob's: a spawn whose mob is not known
        // registers none, and the member's row alone decides it.
        if let Some(mob_id) = seed
            .mob_id
            .as_deref()
            .map(str::trim)
            .filter(|mob_id| !mob_id.is_empty())
        {
            self.register_spawn_labels(mob_id, identity, member_id, seed)
                .await;
        }

        let Some(initial_message) = seed.initial_message.as_ref() else {
            return;
        };
        // The kickoff is the record of the member this spawn made, and only
        // the session the spawn's own outcome names says which member that
        // is: the console witnesses the kickoff by exactly that session.
        // Without it no kickoff is written, and the member's first message
        // stands on its page from its own session history.
        let session_id = seed
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|session| !session.is_empty());
        if session_id.is_none() {
            return;
        }
        let content = match initial_message {
            Value::String(text) => json!([{ "type": "text", "text": text }]),
            other => other.clone(),
        };
        let mut data = json!({
            "content": content,
            "message": { "role": "user", "content": initial_message },
            "source_event_type": "spawn_initial_message",
            "via_tool": seed.via_tool,
            "member_id": member_id,
        });
        if let Some(object) = data.as_object_mut() {
            if let Some(mob_id) = seed.mob_id.as_deref().filter(|id| !id.trim().is_empty()) {
                object.insert("mob_id".to_string(), json!(mob_id));
            }
            if let Some(parent) = seed.spawned_by.as_deref() {
                object.insert("parent_identity".to_string(), json!(parent));
            }
            if let Some(session_id) = session_id {
                object.insert("session_id".to_string(), json!(session_id));
            }
        }
        // `append_envelope` dedupes by event id against the replay window
        // (ALL_EVENTS_REPLAY_CAP), so retries are exactly-once for as long
        // as the original kickoff is still replayable; a respawn issued
        // after the kickoff scrolls out re-appends, and the downstream log
        // store still collapses it via the identical dedupe key.
        self.console_events
            .append_envelope(crate::console_contracts::ConsoleIdentityEventEnvelope {
                event_id: kickoff_event_id(seed.mob_id.as_deref(), member_id, initial_message),
                interaction_id: None,
                identity: identity.to_string(),
                event_type: "user_input".to_string(),
                timestamp_ms: current_time_ms(),
                data,
            })
            .await;
    }

    /// Register the seed's labels and runtime-derived lineage as the entry
    /// of `identity` in `mob_id`, the mob the spawn seated `member_id` in.
    async fn register_spawn_labels(
        &self,
        mob_id: &str,
        identity: &str,
        member_id: &str,
        seed: &ConsoleSpawnSeed,
    ) {
        let mut labels = seed.labels.clone();
        // A spawn never names a side chat's source, creator or private
        // owner: only the console's projection of Meerkat's fork record
        // writes them
        // (`crate::side_chat::project_side_chat_lineage`), so the registry
        // never holds them.
        labels.remove(crate::side_chat::SIDE_CHAT_OF_LABEL);
        labels.remove(crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL);
        labels.remove(crate::side_chat::SIDE_CHAT_CREATOR_LABEL);
        labels.remove(crate::access::PRIVATE_TO_LABEL);
        // Lineage is runtime-derived truth: ABAC inheritance hangs off
        // `spawned_by`, so a caller-supplied label must never spoof (or
        // erase) who actually performed the spawn.
        match seed.spawned_by.as_deref() {
            Some(spawned_by) => {
                labels.insert(SPAWNED_BY_LABEL.to_string(), spawned_by.to_string());
            }
            None => {
                labels.remove(SPAWNED_BY_LABEL);
                // Registrations merge per key, so a respawn with an unknown
                // spawner must also evict any previously registered lineage.
                self.console_events
                    .unregister_identity_label(mob_id, identity, SPAWNED_BY_LABEL)
                    .await;
            }
        }
        // The spawning incarnation binds the lineage; without one, a stale
        // binding from an earlier spawn must not stay registered.
        match seed
            .spawned_by
            .as_deref()
            .and(seed.spawned_by_session.as_deref())
            .map(str::trim)
            .filter(|session| !session.is_empty())
        {
            Some(session) => {
                labels.insert(SPAWNED_BY_SESSION_LABEL.to_string(), session.to_string());
            }
            None => {
                labels.remove(SPAWNED_BY_SESSION_LABEL);
                self.console_events
                    .unregister_identity_label(mob_id, identity, SPAWNED_BY_SESSION_LABEL)
                    .await;
            }
        }
        labels.insert(VIA_TOOL_LABEL.to_string(), seed.via_tool.clone());
        self.console_events
            .register_identity_labels(mob_id, identity, member_id, labels)
            .await;
    }
}

/// Tool names whose successful outcomes spawn mob members that should be
/// projected into the console.
///
/// # This vocabulary is meerkat's, and Meta only restates it
///
/// These names are defined upstream on `AgentMobToolSurface`. Meta has no
/// compile-time access to them - they are private consts in
/// `meerkat-mob-mcp` - so this list is a RESTATEMENT, and a restatement of
/// someone else's vocabulary goes stale silently. When meerkat adds a mob tool
/// that spawns members, nothing here fails: the spawn succeeds, the projection
/// is skipped, and the console simply shows no member. An operator reads that
/// as "the tool did not spawn", which sends them upstream instead of to this
/// list.
///
/// That is not hypothetical. `council` landed in meerkat 0.8.31 as a
/// first-class agent mob tool beside `delegate` and `fork_off`; it is absent
/// here deliberately, because its participants live in a temporary mob that is
/// destroyed before the tool returns, and projecting them would write durable
/// console rows for members that no longer exist.
///
/// [`MOB_SPAWN_TOOL_VOCABULARY`] is the single source for this crate, and
/// `console_spawn_vocabulary_is_pinned` forces a human decision whenever it
/// changes. Neither can detect a NEW upstream tool on their own - closing that
/// needs meerkat to export the surface's tool names, which is an upstream ask
/// rather than something Meta can assert from here.
pub(crate) fn is_console_spawn_tool(name: &str) -> bool {
    MOB_SPAWN_TOOL_VOCABULARY.contains(&name)
}

/// Every meerkat mob tool Meta treats as spawning console-visible members.
///
/// Single source of truth for this crate. Consumers that need a NARROWER set
/// must document why and assert their subset against this list, so a name that
/// exists in neither is a typo rather than a silent no-op.
pub(crate) const MOB_SPAWN_TOOL_VOCABULARY: &[&str] = &[
    "mob_spawn_member",
    "spawn_member",
    "spawn_many_members",
    "delegate",
    "fork_off",
];

/// Identity of the spawning agent derived from its comms name. Mob member
/// comms names are `{mob_id}/{role}/{identity}` (meerkat-mob 0.6); the
/// identity itself may contain `/`, so everything after the second
/// separator belongs to it. A name without separators is already an
/// identity; any other shape is unknown and yields no lineage rather than
/// a guess.
pub(crate) fn spawned_by_from_comms_name(comms_name: &str) -> Option<String> {
    let comms_name = comms_name.trim();
    if comms_name.is_empty() {
        return None;
    }
    if !comms_name.contains('/') {
        return Some(comms_name.to_string());
    }
    let mut parts = comms_name.splitn(3, '/');
    let (_mob, _role) = (parts.next()?, parts.next()?);
    let identity = parts.next()?.trim();
    (!identity.is_empty()).then(|| identity.to_string())
}

/// The mob of the spawning agent derived from its comms name
/// (`{mob_id}/{role}/{identity}`): the first segment. A name without
/// separators names no mob.
fn spawner_mob_from_comms_name(comms_name: &str) -> Option<String> {
    let (mob_id, _) = comms_name.trim().split_once('/')?;
    let mob_id = mob_id.trim();
    (!mob_id.is_empty()).then(|| mob_id.to_string())
}

/// The mob a spawn tool seated its members in, as Core did it: the mob
/// `mob_spawn_member` was called for, the mob the outcome of `delegate` or
/// `fork_off` names, and for the member spawn tools the caller's own mob.
/// The spawn registry keys a member's entry by this mob.
fn spawn_seated_mob(
    tool_name: &str,
    args: &Value,
    outcome: Option<&Value>,
    spawner_comms_name: Option<&str>,
) -> Option<String> {
    match tool_name {
        "mob_spawn_member" => text_field(args, "mob_id"),
        "delegate" | "fork_off" => outcome.and_then(|outcome| text_field(outcome, "mob_id")),
        "spawn_member" | "spawn_many_members" => {
            spawner_comms_name.and_then(spawner_mob_from_comms_name)
        }
        _ => None,
    }
}

/// Strip the runtime-derived lineage keys from labels that did not come
/// from the spawn registry (roster labels, spec labels, wire payloads).
/// ABAC inheritance hangs off `spawned_by`: an unverified claim could mint
/// inherited visibility — or hide a member behind a denied parent — so
/// only the spawn registry may assert these keys.
///
/// A side chat's lineage goes too: the console writes a side chat's source
/// and creator only where Meerkat's roster agrees
/// ([`crate::side_chat::project_side_chat_lineage`]), never from a label.
/// A private owner ([`crate::access::PRIVATE_TO_LABEL`]) stays: only Meta
/// writes it on a roster (a side chat fork, a spawn or fork by a private
/// member) or the host declares it, since every caller label path refuses
/// it, so a member's own roster label is the owner, whatever its lineage.
pub(crate) fn sanitize_unverified_lineage_labels(labels: &mut BTreeMap<String, String>) {
    labels.remove(SPAWNED_BY_LABEL);
    labels.remove(SPAWNED_BY_SESSION_LABEL);
    labels.remove(VIA_TOOL_LABEL);
    labels.remove(crate::side_chat::SIDE_CHAT_OF_LABEL);
    labels.remove(crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL);
    labels.remove(crate::side_chat::SIDE_CHAT_CREATOR_LABEL);
}

/// The console label keys that name a member's lineage parent, each holding
/// the parent's console identity (namespaced alike): the agent that spawned
/// the member, and the source a side chat was forked from.
pub(crate) const LINEAGE_PARENT_LABELS: [&str; 2] =
    [SPAWNED_BY_LABEL, crate::side_chat::SIDE_CHAT_OF_LABEL];

/// A member's lineage parent, as its console labels name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineageParent<'a> {
    /// The parent's console identity.
    pub(crate) identity: &'a str,
    /// The session of the parent incarnation the lineage is bound to, when
    /// the labels name one.
    pub(crate) bound_session: Option<&'a str>,
}

/// The lineage parent a member's console labels name: the agent that spawned
/// it (`spawned_by`, bound by `spawned_by_session`), else the source a side
/// chat was forked from (`side_chat_of`, bound by `side_chat_of_session`).
/// Every lineage walk reads the parent here. Both are runtime-derived: the
/// projections strip either claim from caller-controlled labels.
pub(crate) fn lineage_parent(labels: &BTreeMap<String, String>) -> Option<LineageParent<'_>> {
    let trimmed = |key: &str| {
        labels
            .get(key)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    [
        (SPAWNED_BY_LABEL, SPAWNED_BY_SESSION_LABEL),
        (
            crate::side_chat::SIDE_CHAT_OF_LABEL,
            crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL,
        ),
    ]
    .into_iter()
    .find_map(|(parent_key, session_key)| {
        Some(LineageParent {
            identity: trimmed(parent_key)?,
            bound_session: trimmed(session_key),
        })
    })
}

/// Whether a member's console labels name the source it was forked from as a
/// side chat (a non-empty `side_chat_of`). Such a member holds its source's
/// audience exactly: the console's access decisions decide it as its source,
/// and a grant that matches the side chat itself grants nothing
/// ([`crate::access::AccessView::decide_agent`]). Only the console's
/// projection of Meerkat's fork record writes the label.
pub(crate) fn names_side_chat_source(labels: &BTreeMap<String, String>) -> bool {
    labels
        .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
        .is_some_and(|source| !source.trim().is_empty())
}

/// One argument record from a spawn-tool call: top-level args or one entry
/// of a `specs`/`members` array.
struct SpawnArgRecord {
    member_id: Option<String>,
    mob_id: Option<String>,
    initial_message: Option<Value>,
    labels: BTreeMap<String, String>,
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn member_id_field(value: &Value) -> Option<String> {
    text_field(value, "member_id")
        .or_else(|| text_field(value, "agent_identity"))
        .or_else(|| text_field(value, "identity"))
}

fn labels_field(value: &Value) -> BTreeMap<String, String> {
    value
        .get("labels")
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|text| (key.clone(), text.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn spawn_arg_record(value: &Value, default_mob_id: Option<&str>) -> Option<SpawnArgRecord> {
    if !value.is_object() {
        return None;
    }
    let member_id = member_id_field(value);
    let initial_message = value
        .get("initial_message")
        .or_else(|| value.get("task"))
        .filter(|message| !message.is_null())
        .cloned();
    if member_id.is_none() && initial_message.is_none() {
        return None;
    }
    let mut labels = labels_field(value);
    if let Some(display_name) = text_field(value, "display_name") {
        labels
            .entry("display_name".to_string())
            .or_insert(display_name);
    }
    Some(SpawnArgRecord {
        member_id,
        mob_id: text_field(value, "mob_id").or_else(|| default_mob_id.map(ToString::to_string)),
        initial_message,
        labels,
    })
}

/// The first label only Meta writes
/// ([`crate::member_comms_id::RESERVED_AUTHORITY_LABELS`]) among the labels a
/// spawn-tool call supplies: its own `labels`, or those of an entry of its
/// `specs` or `members` array. An agent's spawn refuses it, naming it.
pub(crate) fn reserved_label_in_spawn_args(args: &Value) -> Option<&'static str> {
    spawn_label_maps(args)
        .find_map(|labels| crate::member_comms_id::reserved_authority_label(labels.keys()))
}

/// The first of the [`CONSOLE_ROW_LABELS`] among the labels a spawn-tool
/// call supplies: its own `labels`, or those of an entry of its `specs` or
/// `members` array. An agent's spawn refuses it, naming it.
pub(crate) fn console_row_label_in_spawn_args(args: &Value) -> Option<&'static str> {
    spawn_label_maps(args).find_map(|labels| {
        CONSOLE_ROW_LABELS
            .into_iter()
            .find(|key| labels.contains_key(*key))
    })
}

/// Every label map a spawn-tool call supplies: its own `labels`, and those
/// of each entry of its `specs` or `members` array.
fn spawn_label_maps(args: &Value) -> impl Iterator<Item = &serde_json::Map<String, Value>> {
    std::iter::once(args)
        .chain(
            ["specs", "members"]
                .into_iter()
                .filter_map(|key| args.get(key).and_then(Value::as_array))
                .flatten(),
        )
        .filter_map(|value| value.get("labels").and_then(Value::as_object))
}

fn spawn_arg_records(args: &Value) -> Vec<SpawnArgRecord> {
    let default_mob_id = text_field(args, "mob_id");
    let mut records = Vec::new();
    for key in ["specs", "members"] {
        let Some(values) = args.get(key).and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            if let Some(record) = spawn_arg_record(value, default_mob_id.as_deref()) {
                records.push(record);
            }
        }
    }
    if records.is_empty()
        && let Some(record) = spawn_arg_record(args, default_mob_id.as_deref())
    {
        records.push(record);
    }
    records
}

/// The members a typed batch outcome reports spawned, in its order: Core's
/// `{"results": [...]}`, one entry per spec, where a spawned entry names its
/// member and a failed entry spawned nothing. `None` when the outcome is not
/// that shape.
fn typed_batch_spawned_members(outcome: &Value) -> Option<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct TypedBatchOutcome {
        results: Vec<meerkat_contracts::MobSpawnManyResultEntry>,
    }
    let typed = <TypedBatchOutcome as serde::Deserialize>::deserialize(outcome).ok()?;
    Some(
        typed
            .results
            .into_iter()
            .filter_map(|entry| match entry.result {
                meerkat_contracts::MobSpawnManyResultPayload::Spawned(spawned) => {
                    Some(spawned.agent_identity)
                }
                meerkat_contracts::MobSpawnManyResultPayload::Failed(_) => None,
            })
            .collect(),
    )
}

/// One member reported spawned by the tool outcome.
struct SpawnOutcomeTarget {
    member_id: String,
    mob_id: Option<String>,
    /// The session the outcome names for this member, read from the field
    /// in which the tool names it.
    session_id: Option<String>,
}

/// The outcome field in which a spawn tool names the session of the member
/// it made: `fork_off` names the child's session (`fork_session_id`),
/// `delegate` the session its helper ran on (`session_id`). The outcomes of
/// the other spawn tools name no session. A field of that name in another
/// tool's outcome is not read: nothing says it is the spawned member's.
fn outcome_session_field(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "fork_off" => Some("fork_session_id"),
        "delegate" => Some("session_id"),
        _ => None,
    }
}

fn collect_outcome_targets(
    value: &Value,
    default_mob_id: Option<&str>,
    session_field: Option<&str>,
    targets: &mut Vec<SpawnOutcomeTarget>,
) {
    if let Some(member_id) = member_id_field(value)
        && !targets.iter().any(|target| target.member_id == member_id)
    {
        targets.push(SpawnOutcomeTarget {
            member_id,
            mob_id: text_field(value, "mob_id").or_else(|| default_mob_id.map(ToString::to_string)),
            session_id: session_field.and_then(|field| text_field(value, field)),
        });
    }
    for key in ["members", "specs", "spawned", "results"] {
        let Some(values) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        let nested_default = text_field(value, "mob_id");
        for nested in values {
            collect_outcome_targets(
                nested,
                nested_default.as_deref().or(default_mob_id),
                session_field,
                targets,
            );
        }
    }
}

/// The console identity a spawned member's projection registers: its usable
/// `agent_identity` label (the console identity override), otherwise its
/// member id, trimmed as [`ConsoleSpawnSink::project_spawned_member`]
/// registers it. The runtime-plane member checks evaluate this same value.
pub(crate) fn spawn_seed_console_identity(
    member_id: &str,
    labels: &BTreeMap<String, String>,
) -> String {
    crate::member_comms_id::durable_identity_label(labels)
        .unwrap_or(member_id)
        .trim()
        .to_string()
}

/// Extract the spawned members from a successful spawn-tool dispatch.
///
/// `args` is the tool-call argument object (single-record shape or
/// `specs`/`members` arrays); `outcome_text` is the tool result payload.
/// Members reported by the outcome are authoritative (they actually
/// spawned); argument records enrich them with `initial_message`/`task`,
/// labels, and identity overrides. A typed batch outcome (`spawn_many_members`)
/// projects only its spawned entries: a failed entry's argument record never
/// stands in for it. Otherwise, when the outcome carries no parsable member,
/// argument records with explicit member ids are used as fallback. A seed
/// names its member's session only when the outcome itself does
/// ([`outcome_session_field`]).
pub(crate) fn console_spawn_seeds(
    tool_name: &str,
    args: &Value,
    outcome_text: &str,
    spawner_comms_name: Option<&str>,
) -> Vec<ConsoleSpawnSeed> {
    if !is_console_spawn_tool(tool_name) {
        return Vec::new();
    }
    let spawned_by = spawner_comms_name.and_then(spawned_by_from_comms_name);
    let arg_records = spawn_arg_records(args);

    let outcome = serde_json::from_str::<Value>(outcome_text).ok();
    let typed_spawned = outcome.as_ref().and_then(typed_batch_spawned_members);
    let mut outcome_targets = Vec::new();
    if let Some(spawned) = &typed_spawned {
        outcome_targets.extend(spawned.iter().map(|member_id| SpawnOutcomeTarget {
            member_id: member_id.clone(),
            mob_id: None,
            session_id: None,
        }));
    } else if let Some(outcome) = &outcome {
        collect_outcome_targets(
            outcome,
            None,
            outcome_session_field(tool_name),
            &mut outcome_targets,
        );
    }

    // The mob Core seated the members in. A seed names it, or no mob: a
    // mob that only an argument record or a nested outcome names is not
    // taken, and one that disagrees with it leaves the seed without a mob.
    let seated_mob = spawn_seated_mob(tool_name, args, outcome.as_ref(), spawner_comms_name);

    let seed_for = |member_id: String,
                    mob_id: Option<String>,
                    session_id: Option<String>,
                    record: Option<&SpawnArgRecord>|
     -> ConsoleSpawnSeed {
        let labels = record
            .map(|record| record.labels.clone())
            .unwrap_or_default();
        let identity = spawn_seed_console_identity(&member_id, &labels);
        let named_mob = mob_id
            .or_else(|| record.and_then(|record| record.mob_id.clone()))
            .or_else(|| text_field(args, "mob_id"));
        ConsoleSpawnSeed {
            mob_id: seated_mob.clone().filter(|seated| {
                named_mob
                    .as_ref()
                    .is_none_or(|named_mob| named_mob == seated)
            }),
            member_id,
            identity,
            initial_message: record.and_then(|record| record.initial_message.clone()),
            labels,
            spawned_by: spawned_by.clone(),
            spawned_by_session: None,
            session_id,
            via_tool: tool_name.to_string(),
        }
    };

    if typed_spawned.is_some() || !outcome_targets.is_empty() {
        return outcome_targets
            .into_iter()
            .map(|target| {
                let record = arg_records
                    .iter()
                    .find(|record| record.member_id.as_deref() == Some(target.member_id.as_str()))
                    // A single argument record with a tool-generated member id
                    // (delegate) pairs with the single spawned target.
                    .or_else(|| match arg_records.as_slice() {
                        [only] if only.member_id.is_none() => Some(only),
                        _ => None,
                    });
                seed_for(target.member_id, target.mob_id, target.session_id, record)
            })
            .collect();
    }

    // An outcome that names no member names no session either: the argument
    // record registers the member's identity, and leaves no kickoff.
    arg_records
        .iter()
        .filter_map(|record| {
            let member_id = record.member_id.clone()?;
            Some(seed_for(
                member_id,
                record.mob_id.clone(),
                None,
                Some(record),
            ))
        })
        .collect()
}

/// Stable short hash for kickoff dedupe keys.
fn hash_short(input: &str) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest[..8]
        .iter()
        .fold(String::with_capacity(16), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Merge console-registered spawn labels into a member's projected labels.
/// Roster/member labels win on conflict — except the runtime-derived lineage
/// keys (`spawned_by`, `via_tool`), where the spawn registry is the source
/// of truth: ABAC inheritance hangs off `spawned_by`, so a spec-supplied
/// label must not displace who actually performed the spawn. A registered
/// label of the Console's own row keys ([`CONSOLE_ROW_LABELS`]) is never
/// taken: the row says those itself.
pub(crate) fn merge_registered_labels(
    labels: &mut BTreeMap<String, String>,
    registered: &BTreeMap<String, String>,
) {
    for (key, value) in registered {
        if key == SPAWNED_BY_LABEL || key == SPAWNED_BY_SESSION_LABEL || key == VIA_TOOL_LABEL {
            labels.insert(key.clone(), value.clone());
        } else if key != crate::side_chat::SIDE_CHAT_OF_LABEL
            && key != crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL
            && key != crate::side_chat::SIDE_CHAT_CREATOR_LABEL
            && key != crate::access::PRIVATE_TO_LABEL
            && !is_console_row_label(key)
        {
            // A spawn's own labels never name a side chat's source, creator
            // or private owner.
            labels.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

/// The lineage half of [`merge_registered_labels`]: copy the spawn
/// registry's `spawned_by`, `spawned_by_session` and `via_tool` over
/// `labels` and leave every other label as it is. For labels whose own
/// lineage keys [`sanitize_unverified_lineage_labels`] already dropped.
pub(crate) fn merge_registered_lineage_labels(
    labels: &mut BTreeMap<String, String>,
    registered: &BTreeMap<String, String>,
) {
    for key in [SPAWNED_BY_LABEL, SPAWNED_BY_SESSION_LABEL, VIA_TOOL_LABEL] {
        if let Some(value) = registered.get(key) {
            labels.insert(key.to_string(), value.clone());
        }
    }
}

fn kickoff_event_id(mob_id: Option<&str>, member_id: &str, initial_message: &Value) -> String {
    format!(
        "spawn-kickoff:{}:{}:{}",
        mob_id.unwrap_or("-"),
        member_id,
        hash_short(&initial_message.to_string())
    )
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::{EventEnvelope, UnifiedEvent};

    fn seed(member_id: &str, initial_message: Option<Value>) -> ConsoleSpawnSeed {
        ConsoleSpawnSeed {
            mob_id: Some("ops".to_string()),
            member_id: member_id.to_string(),
            identity: member_id.to_string(),
            initial_message,
            labels: BTreeMap::from([("group".to_string(), "workers".to_string())]),
            spawned_by: Some("ops-lead".to_string()),
            spawned_by_session: None,
            // The session of the member the seed is for: the sink appends a
            // kickoff only for a seed that names it.
            session_id: Some("worker-session".to_string()),
            via_tool: "mob_spawn_member".to_string(),
        }
    }

    #[tokio::test]
    async fn projects_kickoff_event_under_member_identity() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-3", Some(json!("Find the person"))))
            .await;

        let replay = store.replay_all(None).await.expect("replay");
        let kickoff = replay
            .iter()
            .find(|event| event.event_type == "user_input")
            .expect("kickoff user_input event projected");
        assert_eq!(kickoff.identity, "worker-3");
        assert!(
            kickoff.event_id.starts_with("spawn-kickoff:ops:worker-3:"),
            "deterministic kickoff id, got {}",
            kickoff.event_id
        );
        assert_eq!(kickoff.data["content"][0]["type"], "text");
        assert_eq!(kickoff.data["content"][0]["text"], "Find the person");
        assert_eq!(kickoff.data["message"]["role"], "user");
        assert_eq!(kickoff.data["source_event_type"], "spawn_initial_message");
        assert_eq!(kickoff.data["via_tool"], "mob_spawn_member");
        assert_eq!(kickoff.data["session_id"], "worker-session");
        assert_eq!(kickoff.data["parent_identity"], "ops-lead");
    }

    /// Backstop for spawn paths that skip rpc validation: a seed carrying
    /// the reserved runtime-plane identity must project nothing — no
    /// runtime-id mapping, no kickoff frame — or the member's frames would
    /// ride the aggregator's `_system` visibility/namespacing exemptions.
    #[tokio::test]
    async fn reserved_system_identity_seed_projects_nothing() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-3", Some(json!("Find the person")));
        spoofed.identity = crate::console_contracts::SYSTEM_EVENT_IDENTITY.to_string();

        sink.project_spawned_member(&spoofed).await;

        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            !replay.iter().any(|event| event.event_type == "user_input"),
            "no kickoff may project under the reserved identity: {replay:#?}"
        );
        assert!(
            !sink
                .identity_labels_snapshot()
                .await
                .identities()
                .any(|identity| identity == crate::console_contracts::SYSTEM_EVENT_IDENTITY),
            "no identity metadata may register under the reserved identity"
        );
    }

    /// The backstop covers both runtime-plane identities: a seed carrying
    /// `__console__` projects nothing either.
    #[tokio::test]
    async fn reserved_console_identity_seed_projects_nothing() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-4", Some(json!("Find the person")));
        spoofed.identity = "__console__".to_string();

        sink.project_spawned_member(&spoofed).await;

        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            !replay.iter().any(|event| event.event_type == "user_input"),
            "no kickoff may project under the reserved identity: {replay:#?}"
        );
        assert!(
            !sink
                .identity_labels_snapshot()
                .await
                .identities()
                .any(|identity| identity == "__console__"),
            "no identity metadata may register under the reserved identity"
        );
    }

    #[tokio::test]
    async fn double_projection_dedupes_to_one_kickoff() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let seed = seed("worker-3", Some(json!("Find the person")));

        sink.project_spawned_member(&seed).await;
        sink.project_spawned_member(&seed).await;

        let replay = store.replay_all(None).await.expect("replay");
        let kickoffs = replay
            .iter()
            .filter(|event| event.event_type == "user_input")
            .count();
        assert_eq!(kickoffs, 1, "spawn retries must not duplicate the kickoff");
    }

    #[tokio::test]
    async fn no_initial_message_registers_identity_without_chat_frames() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-9", None)).await;

        let replay = store.replay_all(None).await.expect("replay");
        assert!(
            replay.iter().all(|event| event.identity != "worker-9"),
            "no initial_message means no chat frames until first activity"
        );
        let labels = store
            .identity_labels("ops", "worker-9")
            .await
            .expect("identity labels registered");
        assert_eq!(labels.get("group").map(String::as_str), Some("workers"));
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead")
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("mob_spawn_member")
        );
    }

    #[tokio::test]
    async fn registers_runtime_identity_so_live_events_join_the_same_chat() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-3", Some(json!("go"))))
            .await;

        // A child runtime id (`{member}:{gen}:{child}`) only resolves through
        // the explicit registration — the `{identity}:{N}` fallback would
        // mis-derive `worker-3:0` as the identity.
        store
            .project_unified_event(&EventEnvelope {
                event_id: "evt-live-1".to_string(),
                source: "test".to_string(),
                timestamp_ms: 7,
                event: UnifiedEvent::Agent {
                    agent_id: "worker-3:0:1".to_string(),
                    event_type: "text_delta".to_string(),
                    payload: Some(json!({ "delta": "on it" })),
                },
            })
            .await;

        let replay = store.replay_all(None).await.expect("replay");
        let live = replay
            .iter()
            .find(|event| event.event_id == "evt-live-1")
            .expect("live event projected");
        assert_eq!(
            live.identity, "worker-3",
            "live runtime events must land in the same chat as the kickoff"
        );
    }

    /// The spawn registry binds lineage to the spawning incarnation: the
    /// session that performed the spawn is registered next to `spawned_by`,
    /// a caller-supplied value never is, and a later spawn of the same
    /// identity without a known spawning session evicts the earlier binding.
    #[tokio::test]
    async fn the_spawning_session_is_registered_only_by_the_spawn_registry() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut bound = ConsoleSpawnSeed {
            spawned_by_session: Some("lead-session-1".to_string()),
            session_id: None,
            ..seed("worker-3", None)
        };
        bound.labels.insert(
            SPAWNED_BY_SESSION_LABEL.to_string(),
            "claimed-session".to_string(),
        );
        sink.project_spawned_member(&bound).await;
        let labels = store
            .identity_labels("ops", "worker-3")
            .await
            .expect("identity labels registered");
        assert_eq!(
            labels.get(SPAWNED_BY_SESSION_LABEL).map(String::as_str),
            Some("lead-session-1")
        );

        // Respawned by a spawner whose session is unknown: no stale binding.
        sink.project_spawned_member(&seed("worker-3", None)).await;
        let labels = store
            .identity_labels("ops", "worker-3")
            .await
            .expect("identity labels registered");
        assert_eq!(labels.get(SPAWNED_BY_SESSION_LABEL), None);

        // Unverified lineage labels lose the binding along with `spawned_by`.
        let mut roster_labels = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string()),
            (
                SPAWNED_BY_SESSION_LABEL.to_string(),
                "claimed-session".to_string(),
            ),
        ]);
        sanitize_unverified_lineage_labels(&mut roster_labels);
        assert!(roster_labels.is_empty());
    }

    #[tokio::test]
    async fn caller_labels_cannot_spoof_runtime_derived_lineage() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-3", None);
        spoofed
            .labels
            .insert(SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string());
        spoofed
            .labels
            .insert(VIA_TOOL_LABEL.to_string(), "console".to_string());

        sink.project_spawned_member(&spoofed).await;

        let labels = store
            .identity_labels("ops", "worker-3")
            .await
            .expect("identity labels registered");
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead"),
            "ABAC lineage must record the actual spawner, not a caller-supplied label"
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("mob_spawn_member")
        );

        let unknown_parent = ConsoleSpawnSeed {
            spawned_by: None,
            spawned_by_session: None,
            session_id: None,
            labels: BTreeMap::from([(SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string())]),
            ..seed("worker-9", None)
        };
        sink.project_spawned_member(&unknown_parent).await;
        let labels = store
            .identity_labels("ops", "worker-9")
            .await
            .expect("identity labels registered");
        assert!(
            !labels.contains_key(SPAWNED_BY_LABEL),
            "unknown spawner must not fall back to a caller-supplied lineage claim"
        );
    }

    /// Pins the vocabulary so a change is a DECISION, not a diff nobody reads.
    ///
    /// This cannot detect a new tool added UPSTREAM - meerkat's tool names are
    /// private consts and Meta cannot see them. What it does is force whoever
    /// edits this list to state, in a failing test, that they considered
    /// whether the new tool spawns console-visible members. `council` is the
    /// worked example: it IS a spawn tool upstream and is deliberately absent
    /// here, and that absence should stay a decision rather than becoming an
    /// oversight the next time someone reads the list.
    #[test]
    fn console_spawn_vocabulary_is_pinned() {
        assert_eq!(
            MOB_SPAWN_TOOL_VOCABULARY,
            [
                "mob_spawn_member",
                "spawn_member",
                "spawn_many_members",
                "delegate",
                "fork_off",
            ],
            "the console spawn vocabulary changed. If you ADDED a tool, confirm \
             it spawns members that should be projected AND that its argument \
             shape is handled. If you REMOVED one, confirm nothing still \
             projects through it. `council` is intentionally absent: its \
             participants are destroyed before the tool returns.",
        );
    }

    #[test]
    fn spawn_tools_are_recognized() {
        for tool in [
            "mob_spawn_member",
            "spawn_member",
            "spawn_many_members",
            "delegate",
            "fork_off",
        ] {
            assert!(is_console_spawn_tool(tool), "{tool} spawns members");
        }
        assert!(!is_console_spawn_tool("mob_retire_member"));
        assert!(!is_console_spawn_tool("send_message"));
    }

    #[test]
    fn spawned_by_uses_comms_name_identity_segment() {
        assert_eq!(
            spawned_by_from_comms_name("ops/orchestrator/ops-lead").as_deref(),
            Some("ops-lead")
        );
        assert_eq!(
            spawned_by_from_comms_name("ops-lead").as_deref(),
            Some("ops-lead")
        );
        // Identities may contain `/` (e.g. `people/finder`): the comms-name
        // shape is `{mob}/{role}/{identity}`, so everything after the second
        // separator belongs to the identity.
        assert_eq!(
            spawned_by_from_comms_name("ops/orchestrator/people/finder").as_deref(),
            Some("people/finder")
        );
        // One separator is not the canonical shape — don't guess lineage.
        assert_eq!(spawned_by_from_comms_name("mob/role"), None);
        assert_eq!(spawned_by_from_comms_name(""), None);
        assert_eq!(spawned_by_from_comms_name("mob/role/"), None);
    }

    #[tokio::test]
    async fn respawn_with_unknown_spawner_clears_stale_lineage() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());

        sink.project_spawned_member(&seed("worker-3", None)).await;
        assert_eq!(
            store
                .identity_labels("ops", "worker-3")
                .await
                .and_then(|labels| labels.get(SPAWNED_BY_LABEL).cloned())
                .as_deref(),
            Some("ops-lead")
        );

        let unknown_spawner = ConsoleSpawnSeed {
            spawned_by: None,
            spawned_by_session: None,
            session_id: None,
            via_tool: "spawn_member".to_string(),
            ..seed("worker-3", None)
        };
        sink.project_spawned_member(&unknown_spawner).await;

        let labels = store
            .identity_labels("ops", "worker-3")
            .await
            .expect("identity labels registered");
        assert!(
            !labels.contains_key(SPAWNED_BY_LABEL),
            "a respawn with an unknown spawner must not keep stale lineage alive"
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("spawn_member")
        );
    }

    #[test]
    fn seeds_from_single_spawn_args_and_outcome() {
        let args = json!({
            "mob_id": "ops",
            "profile": "person-worker",
            "member_id": "worker-3",
            "initial_message": "Find the person",
            "labels": { "group": "workers", "display_name": "Worker 3" }
        });
        let outcome = json!({
            "agent_identity": "worker-3",
            "member_ref": "opaque-ref"
        });

        let seeds = console_spawn_seeds(
            "mob_spawn_member",
            &args,
            &outcome.to_string(),
            Some("ops/orchestrator/ops-lead"),
        );

        assert_eq!(seeds.len(), 1);
        let seed = &seeds[0];
        assert_eq!(seed.mob_id.as_deref(), Some("ops"));
        assert_eq!(seed.member_id, "worker-3");
        assert_eq!(seed.identity, "worker-3");
        assert_eq!(seed.initial_message, Some(json!("Find the person")));
        assert_eq!(
            seed.labels.get("group").map(String::as_str),
            Some("workers")
        );
        assert_eq!(
            seed.labels.get("display_name").map(String::as_str),
            Some("Worker 3")
        );
        assert_eq!(seed.spawned_by.as_deref(), Some("ops-lead"));
        assert_eq!(seed.via_tool, "mob_spawn_member");
    }

    #[tokio::test]
    async fn fork_off_projects_retained_child_from_canonical_result() {
        let args = json!({
            "member_id": "durable-investigator",
            "task": "Inspect the ledger"
        });
        let outcome = json!({
            "output": "The ledger is consistent.",
            "mob_id": "ops",
            "source_member_id": "ops-lead",
            "agent_identity": "durable-investigator",
            "member_ref": "opaque-ref",
            "fork_session_id": "fork-session",
            "turn_session_id": "fork-session",
            "bounded_result": {
                "label": "delegation",
                "text": "The ledger is consistent.",
                "status": "completed",
                "truncated": false
            }
        });

        let seeds = console_spawn_seeds(
            "fork_off",
            &args,
            &outcome.to_string(),
            Some("ops/orchestrator/ops-lead"),
        );
        assert_eq!(seeds.len(), 1);
        let seed = &seeds[0];
        assert_eq!(seed.mob_id.as_deref(), Some("ops"));
        assert_eq!(seed.member_id, "durable-investigator");
        assert_eq!(seed.initial_message, Some(json!("Inspect the ledger")));
        assert_eq!(seed.spawned_by.as_deref(), Some("ops-lead"));
        assert_eq!(seed.via_tool, "fork_off");

        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        sink.project_spawned_member(seed).await;
        let labels = store
            .identity_labels("ops", "durable-investigator")
            .await
            .expect("retained child is registered in the normal console identity snapshot");
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead")
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("fork_off")
        );
        let replay = store.replay_all(None).await.expect("replay");
        assert!(replay.iter().any(|event| {
            event.identity == "durable-investigator"
                && event.data["message"]["content"] == "Inspect the ledger"
        }));
    }

    /// The kickoff events one spawn-tool outcome leaves in the store.
    async fn kickoffs_of(tool: &str, args: Value, outcome_text: &str) -> Vec<Value> {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        for seed in
            console_spawn_seeds(tool, &args, outcome_text, Some("ops/orchestrator/ops-lead"))
        {
            sink.project_spawned_member(&seed).await;
        }
        store
            .replay_all(None)
            .await
            .expect("replay")
            .into_iter()
            .filter(|event| event.event_type == "user_input")
            .map(|event| event.data)
            .collect()
    }

    /// A kickoff is written only when the spawn's own outcome names the
    /// session of the member it made, and it names that session: `fork_off`
    /// (run to its end, or started detached) and `delegate`. The outcomes of
    /// `mob_spawn_member`, `spawn_member` and `spawn_many_members` name no
    /// session, and an outcome with no member names none either: no kickoff.
    #[tokio::test]
    async fn a_kickoff_names_the_session_its_spawn_outcome_names_or_is_not_written() {
        let fork_args = json!({ "member_id": "child", "task": "Inspect the ledger" });
        let forked = kickoffs_of(
            "fork_off",
            fork_args.clone(),
            &json!({
                "mob_id": "ops",
                "source_member_id": "ops-lead",
                "agent_identity": "child",
                "member_ref": "opaque-ref",
                "fork_session_id": "child-session",
                "turn_session_id": "child-turn-session",
            })
            .to_string(),
        )
        .await;
        assert_eq!(forked.len(), 1, "{forked:?}");
        assert_eq!(forked[0]["session_id"], json!("child-session"));

        let detached = kickoffs_of(
            "fork_off",
            fork_args,
            &json!({
                "status": "running",
                "mob_id": "ops",
                "source_member_id": "ops-lead",
                "agent_identity": "child",
                "member_ref": "opaque-ref",
                "fork_session_id": "child-session",
                "job_id": "job-1",
            })
            .to_string(),
        )
        .await;
        assert_eq!(detached.len(), 1, "{detached:?}");
        assert_eq!(detached[0]["session_id"], json!("child-session"));

        let delegated = kickoffs_of(
            "delegate",
            json!({ "task": "Summarize the ledger" }),
            &json!({
                "mob_id": "ops",
                "agent_identity": "helper-1",
                "member_ref": "opaque-ref",
                "session_id": "helper-session",
                "output": "done",
            })
            .to_string(),
        )
        .await;
        assert_eq!(delegated.len(), 1, "{delegated:?}");
        assert_eq!(delegated[0]["session_id"], json!("helper-session"));

        let spawn_args = json!({
            "mob_id": "ops",
            "profile": "person-worker",
            "member_id": "worker-3",
            "initial_message": "Find the person",
        });
        let named_no_session = json!({ "agent_identity": "worker-3", "member_ref": "opaque-ref" });
        // A field another tool uses for its session is not this tool's.
        let foreign_session_field = json!({
            "agent_identity": "worker-3",
            "member_ref": "opaque-ref",
            "session_id": "some-session",
            "fork_session_id": "some-session",
        });
        for (tool, args, outcome_text) in [
            (
                "mob_spawn_member",
                spawn_args.clone(),
                named_no_session.to_string(),
            ),
            (
                "spawn_member",
                spawn_args.clone(),
                named_no_session.to_string(),
            ),
            (
                "mob_spawn_member",
                spawn_args.clone(),
                foreign_session_field.to_string(),
            ),
            (
                "spawn_many_members",
                json!({ "specs": [spawn_args.clone()] }),
                spawn_many_outcome(vec![spawned_row("worker-3")]),
            ),
            // An outcome that names no member: the argument record alone
            // names the member, and nothing names a session.
            (
                "mob_spawn_member",
                spawn_args.clone(),
                "spawned".to_string(),
            ),
            ("delegate", spawn_args.clone(), "done".to_string()),
        ] {
            let kickoffs = kickoffs_of(tool, args, &outcome_text).await;
            assert!(
                kickoffs.is_empty(),
                "{tool} with outcome {outcome_text} names no session of the member it made: \
                 {kickoffs:?}"
            );
        }
    }

    #[test]
    fn seeds_honor_agent_identity_label_override() {
        let args = json!({
            "mob_id": "ops",
            "member_id": "worker-3",
            "initial_message": "go",
            "labels": { "agent_identity": "people/finder" }
        });
        let outcome = json!({ "agent_identity": "worker-3" });

        let seeds = console_spawn_seeds("mob_spawn_member", &args, &outcome.to_string(), None);

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].member_id, "worker-3");
        assert_eq!(
            seeds[0].identity, "people/finder",
            "kickoff must land under the same identity the event projection derives"
        );
    }

    /// The registry's entry is its mob's: the same console identity in two
    /// mobs is two entries, a row reads the one of its own mob, and a row of
    /// a mob that registered none reads none.
    #[test]
    fn a_registry_entry_is_read_by_its_mob_and_console_identity() {
        let spawned_by =
            |spawner: &str| BTreeMap::from([(SPAWNED_BY_LABEL.to_string(), spawner.to_string())]);
        let mut registry = SpawnRegistry::default();
        registry.register("launch", "worker", "worker", spawned_by("lead"));
        registry.register("side", "worker", "worker", spawned_by("other"));
        let spawner_of = |registry: &SpawnRegistry, mob: &str| {
            registry
                .labels_of_row(mob, "worker")
                .and_then(|labels| labels.get(SPAWNED_BY_LABEL))
                .cloned()
        };
        assert_eq!(spawner_of(&registry, "launch").as_deref(), Some("lead"));
        assert_eq!(spawner_of(&registry, "side").as_deref(), Some("other"));
        assert!(
            registry.entry_of_row("elsewhere", "worker").is_none(),
            "a row of a mob that registered no entry reads none"
        );
        assert_eq!(
            registry.identities().collect::<Vec<_>>(),
            vec!["worker"],
            "the registry names an identity once, whichever mobs hold an entry of it"
        );
        assert_eq!(
            registry
                .entry_of_row("launch", "worker")
                .map(|entry| entry.member_id.as_str()),
            Some("worker"),
            "the entry keeps the roster member id its spawn seated"
        );

        registry.unregister_label("side", "worker", SPAWNED_BY_LABEL);
        assert!(
            registry.entry_of_row("side", "worker").is_none(),
            "an entry goes with its last key"
        );
        assert_eq!(
            spawner_of(&registry, "launch").as_deref(),
            Some("lead"),
            "the other mob's entry is as it was"
        );
    }

    /// A spawn registers its entry under the mob that seated its member: an
    /// entry of the same name in another mob stays as it was, and a seed
    /// that names no mob registers no entry.
    #[tokio::test]
    async fn a_spawn_registers_its_entry_under_its_own_mob_only() {
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        sink.project_spawned_member(&seed("worker-3", None)).await;

        let mut elsewhere = seed("worker-3", None);
        elsewhere.mob_id = Some("side".to_string());
        elsewhere.spawned_by = Some("other-lead".to_string());
        sink.project_spawned_member(&elsewhere).await;

        let mut unspawned = seed("worker-3", None);
        unspawned.mob_id = Some("third".to_string());
        unspawned.spawned_by = None;
        sink.project_spawned_member(&unspawned).await;

        let spawner_in = |labels: Option<BTreeMap<String, String>>| {
            labels.and_then(|labels| labels.get(SPAWNED_BY_LABEL).cloned())
        };
        assert_eq!(
            spawner_in(store.identity_labels("ops", "worker-3").await).as_deref(),
            Some("ops-lead"),
            "the first mob's entry keeps its own spawner"
        );
        assert_eq!(
            spawner_in(store.identity_labels("side", "worker-3").await).as_deref(),
            Some("other-lead")
        );
        assert_eq!(
            spawner_in(store.identity_labels("third", "worker-3").await),
            None,
            "a spawn with no known spawner evicts no other mob's lineage"
        );

        let mut no_mob = seed("worker-7", None);
        no_mob.mob_id = None;
        sink.project_spawned_member(&no_mob).await;
        assert!(
            !sink
                .identity_labels_snapshot()
                .await
                .holds_identity("worker-7"),
            "a seed that names no mob registers no entry"
        );
    }

    /// A seed's mob is the one Core seated its member in: the mob
    /// `mob_spawn_member` was called for, the mob the outcome of `delegate`
    /// or `fork_off` names, the caller's own mob for the member spawn tools.
    /// A mob that only an argument record names is not taken, and one that
    /// disagrees leaves the seed without a mob.
    #[test]
    fn a_seed_names_the_mob_its_member_was_seated_in() {
        let caller = Some("ops/orchestrator/ops-lead");
        let mob_of = |tool: &str, args: Value, outcome: Value| {
            let seeds = console_spawn_seeds(tool, &args, &outcome.to_string(), caller);
            assert_eq!(seeds.len(), 1, "{tool}: one seed");
            seeds[0].mob_id.clone()
        };
        let spawned = json!({ "agent_identity": "worker-3" });
        assert_eq!(
            mob_of(
                "mob_spawn_member",
                json!({ "mob_id": "own", "member_id": "worker-3" }),
                spawned.clone(),
            )
            .as_deref(),
            Some("own")
        );
        assert_eq!(
            mob_of(
                "mob_spawn_member",
                json!({
                    "mob_id": "own",
                    "member_id": "worker-3",
                    "specs": [{ "member_id": "worker-3", "mob_id": "ops" }]
                }),
                spawned.clone(),
            ),
            None,
            "an argument record that names another mob leaves the seed without a mob"
        );
        for tool in ["delegate", "fork_off"] {
            assert_eq!(
                mob_of(
                    tool,
                    json!({ "task": "look it up", "mob_id": "ops" }),
                    json!({ "agent_identity": "worker-3", "mob_id": "implicit-1" }),
                )
                .as_deref(),
                Some("implicit-1"),
                "{tool}: the outcome's mob is taken, whatever the arguments name"
            );
            assert_eq!(
                mob_of(
                    tool,
                    json!({ "task": "look it up" }),
                    json!({ "agent_identity": "worker-3", "mob_id": "implicit-1" }),
                )
                .as_deref(),
                Some("implicit-1")
            );
            assert_eq!(
                mob_of(tool, json!({ "task": "look it up" }), spawned.clone()),
                None,
                "{tool}: an outcome that names no mob leaves the seed without one"
            );
        }
        assert_eq!(
            mob_of(
                "spawn_member",
                json!({ "member_id": "worker-3" }),
                spawned.clone(),
            )
            .as_deref(),
            Some("ops"),
            "a member spawn tool seats in the caller's own mob"
        );
        assert_eq!(
            mob_of(
                "spawn_member",
                json!({ "member_id": "worker-3", "mob_id": "elsewhere" }),
                spawned,
            ),
            None
        );
    }

    /// The outcome `spawn_many_members` returns: Core's typed results, one
    /// entry per spec, in spec order.
    fn spawn_many_outcome(results: Vec<meerkat_contracts::MobSpawnManyResultEntry>) -> String {
        json!({ "results": results }).to_string()
    }

    fn spawned_row(member_id: &str) -> meerkat_contracts::MobSpawnManyResultEntry {
        meerkat_contracts::MobSpawnManyResultEntry::spawned(
            member_id,
            meerkat_contracts::WireMemberRef::encode("ops", member_id),
        )
    }

    fn failed_row() -> meerkat_contracts::MobSpawnManyResultEntry {
        meerkat_contracts::MobSpawnManyResultEntry::failed(
            meerkat_contracts::MobSpawnManyFailureCause::ProfileNotFound,
            "profile person-worker-x is not defined",
        )
    }

    /// A failed row spawned nothing: only the members Core reports spawned
    /// are projected, never a failed row's spec in its place.
    #[test]
    fn spawn_many_projects_only_the_rows_that_spawned() {
        let args = json!({
            "mob_id": "ops",
            "specs": [
                { "profile": "person-worker", "member_id": "worker-a", "initial_message": "task a" },
                { "profile": "person-worker-x", "member_id": "worker-b", "initial_message": "task b" }
            ]
        });
        let outcome = spawn_many_outcome(vec![spawned_row("worker-a"), failed_row()]);

        // The batch tool seats its members in the caller's own mob.
        let seeds = console_spawn_seeds(
            "spawn_many_members",
            &args,
            &outcome,
            Some("ops/orchestrator/ops-lead"),
        );

        let projected = seeds
            .iter()
            .map(|seed| {
                (
                    seed.member_id.as_str(),
                    seed.mob_id.as_deref(),
                    seed.initial_message.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            projected,
            vec![("worker-a", Some("ops"), Some(json!("task a")))]
        );
    }

    /// A batch whose every row failed projects no member at all.
    #[test]
    fn spawn_many_with_every_row_failed_projects_nothing() {
        let args = json!({
            "mob_id": "ops",
            "specs": [
                { "profile": "person-worker-x", "member_id": "worker-a", "initial_message": "task a" }
            ]
        });
        let outcome = spawn_many_outcome(vec![failed_row()]);

        let seeds = console_spawn_seeds("spawn_many_members", &args, &outcome, None);

        let projected = seeds
            .iter()
            .map(|seed| seed.member_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(projected, Vec::<&str>::new());
    }

    #[test]
    fn seeds_from_specs_array_enrich_each_member() {
        let args = json!({
            "mob_id": "ops",
            "specs": [
                {
                    "profile": "person-worker",
                    "member_id": "worker-a",
                    "initial_message": "task a",
                    "labels": { "group": "workers" }
                },
                {
                    "profile": "person-worker",
                    "member_id": "worker-b",
                    "initial_message": "task b"
                }
            ]
        });
        let outcome = spawn_many_outcome(vec![spawned_row("worker-a"), spawned_row("worker-b")]);

        let seeds = console_spawn_seeds("spawn_many_members", &args, &outcome, None);

        assert_eq!(seeds.len(), 2);
        let worker_a = seeds
            .iter()
            .find(|seed| seed.member_id == "worker-a")
            .unwrap();
        assert_eq!(worker_a.initial_message, Some(json!("task a")));
        assert_eq!(
            worker_a.labels.get("group").map(String::as_str),
            Some("workers")
        );
        let worker_b = seeds
            .iter()
            .find(|seed| seed.member_id == "worker-b")
            .unwrap();
        assert_eq!(worker_b.initial_message, Some(json!("task b")));
    }

    #[test]
    fn delegate_seed_pairs_generated_member_with_task() {
        let args = json!({ "task": "Review the diff" });
        let outcome = json!({
            "agent_identity": "helper-3f2a",
            "member_ref": "opaque",
            "mob_id": "implicit-1",
            "wired": true
        });

        let seeds = console_spawn_seeds(
            "delegate",
            &args,
            &outcome.to_string(),
            Some("ops/orchestrator/ops-lead"),
        );

        assert_eq!(seeds.len(), 1);
        let seed = &seeds[0];
        assert_eq!(seed.member_id, "helper-3f2a");
        assert_eq!(seed.mob_id.as_deref(), Some("implicit-1"));
        assert_eq!(seed.initial_message, Some(json!("Review the diff")));
        assert_eq!(seed.spawned_by.as_deref(), Some("ops-lead"));
        assert_eq!(seed.via_tool, "delegate");
    }

    #[test]
    fn unparsable_outcome_falls_back_to_args_members() {
        let args = json!({
            "mob_id": "ops",
            "member_id": "worker-3",
            "initial_message": "go"
        });

        let seeds = console_spawn_seeds("mob_spawn_member", &args, "spawned ok", None);

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].member_id, "worker-3");
    }

    #[test]
    fn merge_registered_labels_fills_gaps_without_overriding() {
        let mut labels = BTreeMap::from([
            ("group".to_string(), "roster-group".to_string()),
            ("role".to_string(), "worker".to_string()),
        ]);
        let registered = BTreeMap::from([
            ("group".to_string(), "registry-group".to_string()),
            (SPAWNED_BY_LABEL.to_string(), "ops-lead".to_string()),
        ]);

        merge_registered_labels(&mut labels, &registered);

        assert_eq!(
            labels.get("group").map(String::as_str),
            Some("roster-group")
        );
        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead")
        );
        assert_eq!(labels.get("role").map(String::as_str), Some("worker"));
    }

    /// The Console writes some keys of a member's row from its own facts
    /// (the member's role, the mob it sits in, the host of an implicit
    /// delegate, its group, whether it can be addressed) and reads the
    /// host's attention marks from others. A registered label of such a key
    /// is not taken into a row that lacks it; a display name still fills a
    /// gap, and so does a label the Console reads nothing from.
    #[test]
    fn merge_registered_labels_takes_no_key_the_row_writes_itself() {
        let registered = BTreeMap::from([
            ("role".to_string(), "delegate".to_string()),
            ("source_mob_id".to_string(), "another-mob".to_string()),
            ("delegate_host_identity".to_string(), "a-host".to_string()),
            ("addressable".to_string(), "false".to_string()),
            ("group".to_string(), "workers".to_string()),
            ("console_watched".to_string(), "true".to_string()),
            ("console_alert_level".to_string(), "critical".to_string()),
            ("console_degraded".to_string(), "true".to_string()),
            (
                "console_degraded_reason".to_string(),
                "a reason".to_string(),
            ),
            ("singleton".to_string(), "true".to_string()),
            ("display_name".to_string(), "Worker 3".to_string()),
            ("team".to_string(), "ops".to_string()),
        ]);
        let mut labels = BTreeMap::new();

        merge_registered_labels(&mut labels, &registered);

        assert_eq!(
            labels,
            BTreeMap::from([
                ("display_name".to_string(), "Worker 3".to_string()),
                ("team".to_string(), "ops".to_string()),
            ]),
            "the display name and the other label are taken, no key of the Console's"
        );
    }

    /// The census of a member row's keys. Every label key the builders of a
    /// member's row read or write stands in one of two lists: the Console's
    /// own ([`CONSOLE_ROW_LABELS`]), which no spawn call sets (refused in
    /// every label position, left out by the registry's merge), or the
    /// carried ones ([`CARRIED_ROW_LABELS`]). A key a builder starts to read
    /// fails this row until it stands in one of them.
    #[test]
    fn the_keys_a_member_rows_builders_read_are_refused_or_carried() {
        for key in CONSOLE_ROW_LABELS {
            for args in [
                json!({ "labels": { key: "x" } }),
                json!({ "specs": [{ "labels": { key: "x" } }] }),
                json!({ "members": [{ "labels": { key: "x" } }] }),
            ] {
                assert_eq!(
                    console_row_label_in_spawn_args(&args),
                    Some(key),
                    "{key} is found in {args}"
                );
            }
            assert!(
                console_row_label_refusal(key).contains(key),
                "the refusal names {key}"
            );
            let mut labels = BTreeMap::new();
            merge_registered_labels(
                &mut labels,
                &BTreeMap::from([(key.to_string(), "x".to_string())]),
            );
            assert!(labels.is_empty(), "{key} is not taken from the registry");
        }
        for key in CARRIED_ROW_LABELS {
            assert_eq!(
                console_row_label_in_spawn_args(&json!({ "labels": { key: "x" } })),
                None,
                "{key} is carried"
            );
        }

        // The label keys the row's builders name, read from their source.
        let console = include_str!("http_console.rs");
        let experience = include_str!("runtime/console_ingress.rs");
        let mut named = std::collections::BTreeSet::new();
        let mut unlisted = Vec::new();
        for (source, function) in [
            (console, "fn write_where_the_member_sits("),
            (console, "fn projected_console_member_labels("),
            (console, "fn console_member_from_runtime_alias("),
            (console, "fn console_identity_record_from_console_member("),
            (console, "fn console_identity_record_from_runtime_alias("),
            (console, "fn record_labels_with_the_members_role("),
            (console, "async fn build_live_snapshot("),
            (experience, "fn build_console_experience_contract("),
        ] {
            let Some(start) = source.find(function) else {
                unlisted.push(format!("{function} is not in its source file"));
                continue;
            };
            let body = &source[start..];
            let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
            // The reads and writes of a label map, however their lines break.
            let body = body.split_whitespace().collect::<String>();
            for read in [
                "labels.get(\"",
                "labels.entry(\"",
                "labels.insert(\"",
                "labels.remove(\"",
                "labels.contains_key(\"",
            ] {
                let mut rest = body.as_str();
                while let Some(at) = rest.find(read) {
                    let tail = &rest[at + read.len()..];
                    let key = &tail[..tail.find('"').unwrap_or(0)];
                    named.insert(key.to_string());
                    if !CONSOLE_ROW_LABELS.contains(&key) && !CARRIED_ROW_LABELS.contains(&key) {
                        unlisted.push(format!("{function} names {key}"));
                    }
                    rest = tail;
                }
            }
        }
        assert!(unlisted.is_empty(), "{unlisted:#?}");
        // The scan finds what it is meant to find: the keys the row builder
        // writes and the policy records read.
        for key in [
            "source_mob_id",
            "delegate_host_identity",
            "group",
            "role",
            "singleton",
        ] {
            assert!(named.contains(key), "the builders name {key}: {named:?}");
        }
    }

    #[test]
    fn merge_registered_labels_keeps_lineage_keys_runtime_derived() {
        let mut labels = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), "victim-agent".to_string()),
            (VIA_TOOL_LABEL.to_string(), "console".to_string()),
        ]);
        let registered = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), "ops-lead".to_string()),
            (VIA_TOOL_LABEL.to_string(), "mob_spawn_member".to_string()),
        ]);

        merge_registered_labels(&mut labels, &registered);

        assert_eq!(
            labels.get(SPAWNED_BY_LABEL).map(String::as_str),
            Some("ops-lead"),
            "registry lineage must displace spec-supplied spawned_by claims"
        );
        assert_eq!(
            labels.get(VIA_TOOL_LABEL).map(String::as_str),
            Some("mob_spawn_member")
        );
    }

    /// Only the console's projection of Meerkat's fork record writes a side
    /// chat's lineage: a spawn never registers one, and neither the roster
    /// nor the spawn registry gives one to a projection.
    #[tokio::test]
    async fn no_label_claims_a_side_chat_lineage() {
        let claim = || {
            BTreeMap::from([
                (
                    crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                    "victim-agent".to_string(),
                ),
                (
                    crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL.to_string(),
                    "victim-session".to_string(),
                ),
            ])
        };
        let store = ConsoleEventStore::new();
        let sink = ConsoleSpawnSink::new(store.clone());
        let mut spoofed = seed("worker-5", None);
        spoofed.labels.extend(claim());
        sink.project_spawned_member(&spoofed).await;
        let registered = store
            .identity_labels("ops", "worker-5")
            .await
            .expect("identity labels registered");
        assert!(
            claim().keys().all(|key| !registered.contains_key(key)),
            "a spawn registered a side chat lineage: {registered:?}"
        );

        let mut roster_labels = claim();
        sanitize_unverified_lineage_labels(&mut roster_labels);
        assert!(roster_labels.is_empty(), "{roster_labels:?}");

        let mut projected = BTreeMap::new();
        merge_registered_labels(&mut projected, &claim());
        assert!(projected.is_empty(), "{projected:?}");
    }

    /// Every lineage walk reads one parent: the agent that spawned the
    /// member, else the source a side chat was forked from, each with the
    /// session that binds it.
    #[test]
    fn the_lineage_parent_is_the_spawner_else_the_side_chat_source() {
        let spawned = BTreeMap::from([
            (SPAWNED_BY_LABEL.to_string(), " ops-lead ".to_string()),
            (
                SPAWNED_BY_SESSION_LABEL.to_string(),
                "lead-session".to_string(),
            ),
        ]);
        assert_eq!(
            lineage_parent(&spawned),
            Some(LineageParent {
                identity: "ops-lead",
                bound_session: Some("lead-session"),
            })
        );
        assert!(!names_side_chat_source(&spawned));
        let side_chat = BTreeMap::from([
            (
                crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                "alpha".to_string(),
            ),
            (
                crate::side_chat::SIDE_CHAT_OF_SESSION_LABEL.to_string(),
                "alpha-session".to_string(),
            ),
        ]);
        assert_eq!(
            lineage_parent(&side_chat),
            Some(LineageParent {
                identity: "alpha",
                bound_session: Some("alpha-session"),
            })
        );
        assert!(names_side_chat_source(&side_chat));
        let unbound = BTreeMap::from([(
            crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
            "alpha".to_string(),
        )]);
        assert_eq!(
            lineage_parent(&unbound),
            Some(LineageParent {
                identity: "alpha",
                bound_session: None,
            })
        );
        let blank = BTreeMap::from([(SPAWNED_BY_LABEL.to_string(), "  ".to_string())]);
        assert_eq!(lineage_parent(&blank), None);
        let blank_side_chat = BTreeMap::from([(
            crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
            " ".to_string(),
        )]);
        assert!(!names_side_chat_source(&blank_side_chat));
        assert_eq!(lineage_parent(&BTreeMap::new()), None);
    }
}

#[cfg(test)]
mod spawn_registry_settle_tests {
    use super::*;

    fn labels(key: &str, value: &str) -> BTreeMap<String, String> {
        [(key.to_string(), value.to_string())].into_iter().collect()
    }

    fn members(names: &[&str]) -> std::collections::BTreeSet<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn an_entry_ends_with_the_member_it_was_registered_for_and_no_other() {
        let mut registry = SpawnRegistry::default();
        registry.register("launch", "worker", "worker", labels("tier", "vault"));
        registry.register("launch", "scribe", "scribe-1", labels("tier", "open"));
        registry.register("side", "worker", "worker", labels("tier", "side"));

        let ended = registry.end_entries("launch", &members(&["worker", "absent"]), false);
        assert_eq!(ended, members(&["worker"]));
        assert!(registry.entry_of_row("launch", "worker").is_none());
        assert!(
            registry.entry_of_row("launch", "scribe").is_some(),
            "another member's entry stays"
        );
        assert!(
            registry.entry_of_row("side", "worker").is_some(),
            "the same name's entry in another mob stays"
        );

        let ended = registry.end_entries("launch", &members(&[]), true);
        assert_eq!(
            ended,
            members(&["scribe"]),
            "a mob reset ends the mob's entries"
        );
        assert_eq!(registry.mobs(), members(&["side"]));
    }

    #[test]
    fn a_copy_names_what_it_cannot_tell_against_a_later_copy() {
        let mut earlier = SpawnRegistry::default();
        earlier.register("launch", "worker", "worker", labels("tier", "vault"));
        earlier.register("launch", "scribe", "scribe", labels("tier", "open"));
        earlier.register("side", "helper", "helper", labels("tier", "side"));
        assert_eq!(
            earlier.label_keys_of("worker"),
            members(&["tier"]),
            "the keys an entry of the name holds"
        );

        // Nothing moved: every entry is told.
        assert!(earlier.identities_not_told(&earlier.clone()).is_empty());

        // A name seated anew by no rotation has no entry in the later copy,
        // and an entry registered anew differs.
        let mut now = earlier.clone();
        now.end_entries("launch", &members(&["worker"]), false);
        now.register("launch", "scribe", "scribe", labels("tier", "other"));
        assert_eq!(
            earlier.identities_not_told(&now),
            members(&["worker", "scribe"])
        );

        // A mob whose journal the later settle could not read.
        let mut unread = earlier.clone();
        unread.note_unread_mob("side");
        assert_eq!(earlier.identities_not_told(&unread), members(&["helper"]));

        // The seat an entry was registered for is the entry's own.
        let mut seated = earlier.clone();
        seated.note_entry_seat("launch", "worker", 4);
        assert_eq!(
            seated
                .entry_of_row("launch", "worker")
                .and_then(|entry| entry.seat_generation),
            Some(4)
        );
    }
}
