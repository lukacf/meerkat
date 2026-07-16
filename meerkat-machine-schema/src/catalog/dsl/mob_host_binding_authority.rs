use super::OptionValueExt;

/// MobHostBindingAuthority — the member host's process-scoped, MOB-KEYED
/// admission and dedup authority for host-addressed mob commands (plan §6.3,
/// adjudication ledger A11/A12/A14).
///
/// One daemon serves many mobs and controlling hosts, so every fact is keyed
/// by `MobId` (supervisor bindings) or `MemberKey { mob_id, agent_identity }`
/// (materialized members and dedup memory). The turn-outcome journal is keyed
/// by `TurnKey { mob_id, agent_identity, generation, fence_token, input_id }`
/// (§18.9 as amended by DEC-5: identities are mob-scoped names, so the
/// journal is mob-keyed like every other fact). The shell
/// observes typed facts (sender match, token validity, preflight probes) and
/// this authority adjudicates — it never reads a clock, a socket, or a config.
///
/// Persistence (the `runtime_mob_host_bindings` SQLite row, §14 R8) is
/// phase-2/3 shell work; phase 1 is machine facts only.
#[macro_export]
macro_rules! mob_host_binding_authority_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        meerkat_machine_dsl::machine! {
    machine MobHostBindingAuthority {
        version: 1,
        rust: $rust_crate / $rust_module,

        state {
            lifecycle_phase: MobHostBindingAuthorityPhase,
            // Supervisor binding tuple, one row per bound mob. The four maps
            // are key-aligned (invariant `binding_tuple_consistent`); presence
            // in `binding_phases` is the canonical "bound" witness.
            supervisor_peer_ids: Map<MobId, PeerId>,
            supervisor_signing_keys: Map<MobId, PeerSigningKey>,
            supervisor_epochs: Map<MobId, u64>,
            binding_generations: Map<MobId, u64>,
            binding_generation_highwater: Map<MobId, u64>,
            binding_phases: Map<MobId, Enum<HostBindingPhase>>,
            // Durable terminal receipt for an already-completed revoke.
            // Separate from the active binding tuple: boot recovery uses it
            // only to authenticate an exact reply-loss retry and never to
            // revive member rows. A fresh BindHost clears the receipt.
            revoked_supervisor_peer_ids: Map<MobId, PeerId>,
            revoked_supervisor_signing_keys: Map<MobId, PeerSigningKey>,
            revoked_supervisor_epochs: Map<MobId, u64>,
            revoked_binding_generations: Map<MobId, u64>,
            // Materialized-member rows (successes only, §14 R7): the recorded
            // (generation, fence) tuple is the materialize dedup memory; the
            // recorded spec digest pins one idempotency key to one build.
            materialized_generations: Map<MemberKey, Generation>,
            materialized_fences: Map<MemberKey, FenceToken>,
            materialized_sessions: Map<MemberKey, SessionId>,
            materialized_spec_digests: Map<MemberKey, String>,
            // Release dedup memory: replayed releases at the recorded tuple
            // return the recorded disposal instead of re-running disposal.
            release_generations: Map<MemberKey, Generation>,
            release_fences: Map<MemberKey, FenceToken>,
            release_disposals: Map<MemberKey, Enum<MemberSessionDisposal>>,
            // Turn-outcome journal (§18 O2, mob-keyed per DEC-5). Pending
            // rows reserve capacity and pin the original pre-accept durable
            // window before the runtime effect is admitted. A terminal record
            // atomically consumes its exact pending row and installs the
            // terminal facts. Acknowledgement replaces the payload-bearing
            // terminal with a compact exact-key tombstone so a delayed
            // delivery can never recreate the effect after ACK/prune. All
            // Every live row and lifecycle tombstone is owned only by the
            // exact current residency and is pruned together on
            // release/materialization/revoke.
            //
            // Resource law: ACK/cancel tombstones are deliberately NOT an
            // evicting cache. They grow O(exact tracked keys) for the life of
            // a residency, beside the runtime idempotency ledger which already
            // retains a substantially larger exact input row per accepted key.
            // Eviction without a controller-issued cumulative delivery
            // watermark would reopen duplicate execution, so the lifecycle
            // transition is the only safe reclamation boundary in V4.
            turn_outcome_pending_window_starts: Map<TurnKey, u64>,
            turn_outcome_terminal_seqs: Map<TurnKey, u64>,
            turn_outcome_kinds: Map<TurnKey, Enum<FlowTurnOutcomeKind>>,
            turn_outcome_acknowledged: Map<TurnKey, bool>,
            // Durable negative/controlling receipts for exact tracked-input
            // cancellation. `Cancelling` blocks every later Deliver while
            // the shell converges an already-possible runtime effect;
            // `NoEffect` and `Cancelled` are replay-stable terminals. Like
            // ACK tombstones these are lifecycle-bounded, not live-journal
            // payload rows, and therefore do not consume the 256-row quota.
            tracked_input_cancellations: Map<TurnKey, Enum<TrackedInputCancelKind>>,
        }

        init(Ready) {
            supervisor_peer_ids = EmptyMap,
            supervisor_signing_keys = EmptyMap,
            supervisor_epochs = EmptyMap,
            binding_generations = EmptyMap,
            binding_generation_highwater = EmptyMap,
            binding_phases = EmptyMap,
            revoked_supervisor_peer_ids = EmptyMap,
            revoked_supervisor_signing_keys = EmptyMap,
            revoked_supervisor_epochs = EmptyMap,
            revoked_binding_generations = EmptyMap,
            materialized_generations = EmptyMap,
            materialized_fences = EmptyMap,
            materialized_sessions = EmptyMap,
            materialized_spec_digests = EmptyMap,
            release_generations = EmptyMap,
            release_fences = EmptyMap,
            release_disposals = EmptyMap,
            turn_outcome_pending_window_starts = EmptyMap,
            turn_outcome_terminal_seqs = EmptyMap,
            turn_outcome_kinds = EmptyMap,
            turn_outcome_acknowledged = EmptyMap,
            tracked_input_cancellations = EmptyMap,
        }

        terminal []

        phase MobHostBindingAuthorityPhase {
            Ready,
        }

        input MobHostBindingAuthorityInput {
            // Host-bind admission. `sender_matches_supervisor`,
            // `address_matches`, and `token_valid` are shell observations of
            // the comms envelope and bootstrap token (A11 posture: the shell
            // observes, the authority adjudicates).
            ResolveHostBind {
                mob_id: MobId,
                supervisor_peer_id: PeerId,
                supervisor_signing_key: PeerSigningKey,
                epoch: u64,
                binding_generation: u64,
                sender_matches_supervisor: bool,
                address_matches: bool,
                token_valid: bool,
            },
            // Rebind after controlling-host restart or supervisor rotation:
            // strictly monotonic epoch advance over the recorded binding,
            // with an idempotent replay ack at the recorded epoch from the
            // recorded supervisor (FLAG-2 — rotation retry convergence).
            // `sender_matches_supervisor` means the authenticated envelope
            // signer matches the RECORDED current supervisor, never merely
            // the proposed supervisor carried by the payload.
            ResolveHostRebind {
                mob_id: MobId,
                supervisor_peer_id: PeerId,
                supervisor_signing_key: PeerSigningKey,
                epoch: u64,
                binding_generation: u64,
                sender_matches_supervisor: bool,
            },
            // Authenticated host-addressed revoke. Exact current
            // `(sender_peer_id, epoch)` is required while bound; after the
            // durable terminal the same tuple replay-acks from the receipt.
            RevokeHostBinding { mob_id: MobId, sender_peer_id: PeerId, sender_signing_key: PeerSigningKey, epoch: u64, binding_generation: u64 },
            // Generic host-addressed command admission (§6.3 vocabulary).
            // `turn_directive_present`/`turn_directive_supported` carry the
            // §18.9 delivery-turn-directive observation so the
            // `TurnDirectiveUnsupported` class has an honest producer.
            ResolveHostCommandAdmission {
                mob_id: MobId,
                sender_peer_id: PeerId,
                epoch: u64,
                binding_generation: u64,
                turn_directive_present: bool,
                turn_directive_supported: bool,
            },
            // Materialize dedup adjudication: fresh admit / replay at the
            // recorded tuple+digest / stale fence below the recorded tuple /
            // digest mismatch at the recorded tuple (one idempotency key can
            // never name two builds, §14 R8).
            ResolveMaterializeAdmission {
                member_key: MemberKey,
                generation: Generation,
                fence_token: FenceToken,
                spec_digest: String,
            },
            // Tier-2 build preflight (§14 R7, A11): every observation is a
            // shell-read typed fact; each false observation produces exactly
            // its typed cause. Runs after materialize admission and before
            // any session/state side effect.
            ResolveMaterializePreflight {
                member_key: MemberKey,
                generation: Generation,
                fence_token: FenceToken,
                model_resolvable: bool,
                binding_resolvable: bool,
                env_keys_present: bool,
                stdio_commands_present: bool,
                engine_protocol_supported: bool,
                durable_sessions_required: bool,
                realm_backend_persistent: bool,
                memory_required: bool,
                memory_capability: bool,
            },
            // Success recorder: dedup memory records only successes (§14 R7);
            // a retry after a failed build re-runs preflight from scratch.
            RecordMaterializedMember {
                member_key: MemberKey,
                generation: Generation,
                fence_token: FenceToken,
                session_id: SessionId,
                spec_digest: String,
            },
            // Release admission mirror of the materialize trio (§19.L3):
            // exact recorded tuple admits, replay at the released tuple
            // returns the recorded disposal, lower tuple is StaleFence.
            ResolveReleaseAdmission {
                member_key: MemberKey,
                generation: Generation,
                fence_token: FenceToken,
            },
            // Release recorder. Carries the admitted tuple so no update
            // expression reads a state field the same block also writes.
            // Prunes the released generation's turn-outcome rows
            // (generation-scoped retention, §18.9).
            RecordMemberRelease {
                member_key: MemberKey,
                generation: Generation,
                fence_token: FenceToken,
                disposal: Enum<MemberSessionDisposal>,
            },
            // Durable capacity reservation and original pre-accept event
            // window. This transition must persist before the runtime accepts
            // the directed-turn effect. Exact replay returns the first window;
            // the 256-row bound counts Pending + terminal rows.
            ReserveTurnOutcomePending {
                turn_key: TurnKey,
                window_start: u64,
            },
            // Proven non-acceptance cancels a Pending reservation. Ambiguous
            // acceptance failures deliberately do not issue this input.
            CancelTurnOutcomePending {
                turn_key: TurnKey,
            },
            // Turn-outcome journal recorder (§18 O2): idempotent on the
            // existing key so bridge redelivery converges. A fresh terminal
            // requires and consumes the exact Pending row in the same update.
            RecordTurnOutcome {
                turn_key: TurnKey,
                terminal_seq: u64,
                outcome: Enum<FlowTurnOutcomeKind>,
            },
            // Exact durable acknowledgement of one delivered terminal row.
            // A recorded key moves to a compact tombstone; an unknown key is
            // deliberately a no-op so an ACK racing ahead of a delayed
            // journal commit cannot suppress that future record.
            AcknowledgeTurnOutcome {
                turn_key: TurnKey,
            },
            // Level-triggered tracked-input cancellation. The shell observes
            // whether the exact runtime idempotency binding currently exists
            // while holding the per-key admission gate. The generated
            // authority owns the durable no-effect/cancelling decision.
            CancelTrackedInput {
                turn_key: TurnKey,
                runtime_input_present: bool,
            },
            // Shell feedback after exact runtime cancellation/quiescence.
            CompleteTrackedInputCancel {
                turn_key: TurnKey,
            },
        }

        effect MobHostBindingAuthorityEffect {
            HostBindAccepted { mob_id: MobId, supervisor_peer_id: PeerId, epoch: u64, binding_generation: u64 },
            HostBindRejected { mob_id: MobId, cause: Enum<HostAdmissionRejectKind> },
            HostRebindAccepted { mob_id: MobId, supervisor_peer_id: PeerId, epoch: u64, binding_generation: u64 },
            HostBindingRevoked { mob_id: MobId, supervisor_peer_id: PeerId, epoch: u64, binding_generation: u64 },
            HostBindingRevokeReplayed { mob_id: MobId, supervisor_peer_id: PeerId, epoch: u64, binding_generation: u64 },
            HostCommandAdmitted { mob_id: MobId, sender_peer_id: PeerId, epoch: u64, binding_generation: u64 },
            HostCommandRejected { mob_id: MobId, cause: Enum<HostAdmissionRejectKind> },
            MaterializeAdmitted { member_key: MemberKey, generation: Generation, fence_token: FenceToken },
            MaterializeReplay { member_key: MemberKey, generation: Generation, fence_token: FenceToken, session_id: SessionId, spec_digest: String },
            MaterializeRejected { member_key: MemberKey, generation: Generation, fence_token: FenceToken, cause: Enum<MaterializeRejectKind> },
            MaterializedMemberRecorded { member_key: MemberKey, generation: Generation, fence_token: FenceToken, session_id: SessionId },
            ReleaseAdmitted { member_key: MemberKey, generation: Generation, fence_token: FenceToken },
            ReleaseReplay { member_key: MemberKey, disposal: Enum<MemberSessionDisposal> },
            ReleaseRejected { member_key: MemberKey, generation: Generation, fence_token: FenceToken, cause: Enum<HostAdmissionRejectKind> },
            MemberReleaseRecorded { member_key: MemberKey, disposal: Enum<MemberSessionDisposal> },
            TurnOutcomePendingReserved { turn_key: TurnKey, window_start: u64 },
            TurnOutcomePendingReplayed { turn_key: TurnKey, window_start: u64 },
            TurnOutcomePendingTerminalReplay { turn_key: TurnKey },
            TurnOutcomePendingJournalFull { turn_key: TurnKey },
            TurnOutcomePendingStale { turn_key: TurnKey },
            TurnOutcomePendingCanceled { turn_key: TurnKey },
            TurnOutcomePendingCancelReplay { turn_key: TurnKey },
            TurnOutcomeRecorded { turn_key: TurnKey, terminal_seq: u64, outcome: Enum<FlowTurnOutcomeKind> },
            TurnOutcomeReplayed { turn_key: TurnKey, terminal_seq: u64, outcome: Enum<FlowTurnOutcomeKind> },
            TurnOutcomeStaleDropped { turn_key: TurnKey },
            TurnOutcomeUnreservedDropped { turn_key: TurnKey },
            TurnOutcomeAcknowledged { turn_key: TurnKey },
            TurnOutcomeAckReplay { turn_key: TurnKey },
            TrackedInputCancelNoEffect { turn_key: TurnKey },
            TrackedInputCancelRequested { turn_key: TurnKey },
            TrackedInputCancelReplay { turn_key: TurnKey, outcome: Enum<TrackedInputCancelKind> },
            TrackedInputCancelTerminal { turn_key: TurnKey, terminal_seq: u64, outcome: Enum<FlowTurnOutcomeKind> },
            TrackedInputCancelAcknowledgedReplay { turn_key: TurnKey },
            TrackedInputCancelUnreserved { turn_key: TurnKey },
            TrackedInputCancelStale { turn_key: TurnKey },
            TrackedInputCancelCompleted { turn_key: TurnKey },
        }

        // Admission verdicts surface as bridge replies on the member host —
        // local seam, surface-result alignment. Pure records (the recorder
        // inputs' acknowledgements) have no owner realization.
        disposition HostBindAccepted => local seam SurfaceResultAlignment,
        disposition HostBindRejected => local seam SurfaceResultAlignment,
        disposition HostRebindAccepted => local seam SurfaceResultAlignment,
        disposition HostBindingRevoked => local seam SurfaceResultAlignment,
        disposition HostBindingRevokeReplayed => local seam SurfaceResultAlignment,
        disposition HostCommandAdmitted => local seam SurfaceResultAlignment,
        disposition HostCommandRejected => local seam SurfaceResultAlignment,
        disposition MaterializeAdmitted => local seam SurfaceResultAlignment,
        disposition MaterializeReplay => local seam SurfaceResultAlignment,
        disposition MaterializeRejected => local seam SurfaceResultAlignment,
        disposition MaterializedMemberRecorded => local seam NoOwnerRealization,
        disposition ReleaseAdmitted => local seam SurfaceResultAlignment,
        disposition ReleaseReplay => local seam SurfaceResultAlignment,
        disposition ReleaseRejected => local seam SurfaceResultAlignment,
        disposition MemberReleaseRecorded => local seam NoOwnerRealization,
        disposition TurnOutcomePendingReserved => local seam NoOwnerRealization,
        disposition TurnOutcomePendingReplayed => local seam NoOwnerRealization,
        disposition TurnOutcomePendingTerminalReplay => local seam NoOwnerRealization,
        disposition TurnOutcomePendingJournalFull => local seam NoOwnerRealization,
        disposition TurnOutcomePendingStale => local seam NoOwnerRealization,
        disposition TurnOutcomePendingCanceled => local seam NoOwnerRealization,
        disposition TurnOutcomePendingCancelReplay => local seam NoOwnerRealization,
        disposition TurnOutcomeRecorded => local seam NoOwnerRealization,
        disposition TurnOutcomeReplayed => local seam NoOwnerRealization,
        disposition TurnOutcomeStaleDropped => local seam NoOwnerRealization,
        disposition TurnOutcomeUnreservedDropped => local seam NoOwnerRealization,
        disposition TurnOutcomeAcknowledged => local seam NoOwnerRealization,
        disposition TurnOutcomeAckReplay => local seam NoOwnerRealization,
        disposition TrackedInputCancelNoEffect => local seam NoOwnerRealization,
        disposition TrackedInputCancelRequested => local seam NoOwnerRealization,
        disposition TrackedInputCancelReplay => local seam NoOwnerRealization,
        disposition TrackedInputCancelTerminal => local seam NoOwnerRealization,
        disposition TrackedInputCancelAcknowledgedReplay => local seam NoOwnerRealization,
        disposition TrackedInputCancelUnreserved => local seam NoOwnerRealization,
        disposition TrackedInputCancelStale => local seam NoOwnerRealization,
        disposition TrackedInputCancelCompleted => local seam NoOwnerRealization,

        // The binding tuple maps carry the same key set (the
        // `supervisor_authority_tuple_consistent` style, mob-keyed).
        invariant binding_tuple_consistent {
            for_all(m in self.binding_phases.keys(),
                self.supervisor_peer_ids.contains_key(m)
                && self.supervisor_signing_keys.contains_key(m)
                && self.supervisor_epochs.contains_key(m)
                && self.binding_generations.contains_key(m)
                && self.binding_generation_highwater.contains_key(m))
            && for_all(m in self.supervisor_peer_ids.keys(), self.binding_phases.contains_key(m))
            && for_all(m in self.supervisor_signing_keys.keys(), self.binding_phases.contains_key(m))
            && for_all(m in self.supervisor_epochs.keys(), self.binding_phases.contains_key(m))
            && for_all(m in self.binding_generations.keys(), self.binding_phases.contains_key(m))
            && for_all(m in self.binding_phases.keys(),
                self.binding_generation_highwater.get_copied(m).get("value")
                    >= self.binding_generations.get_copied(m).get("value"))
        }

        invariant revoke_receipt_tuple_consistent {
            for_all(m in self.revoked_supervisor_peer_ids.keys(),
                self.revoked_supervisor_signing_keys.contains_key(m)
                && self.revoked_supervisor_epochs.contains_key(m)
                && self.revoked_binding_generations.contains_key(m))
            && for_all(m in self.revoked_supervisor_signing_keys.keys(), self.revoked_supervisor_peer_ids.contains_key(m))
            && for_all(m in self.revoked_supervisor_epochs.keys(), self.revoked_supervisor_peer_ids.contains_key(m))
            && for_all(m in self.revoked_binding_generations.keys(), self.revoked_supervisor_peer_ids.contains_key(m))
        }

        invariant active_binding_excludes_revoke_receipt {
            for_all(m in self.binding_phases.keys(),
                self.revoked_supervisor_peer_ids.contains_key(m) == false)
        }

        invariant materialized_rows_key_aligned {
            for_all(k in self.materialized_generations.keys(),
                self.materialized_fences.contains_key(k)
                && self.materialized_sessions.contains_key(k)
                && self.materialized_spec_digests.contains_key(k))
            && for_all(k in self.materialized_fences.keys(), self.materialized_generations.contains_key(k))
            && for_all(k in self.materialized_sessions.keys(), self.materialized_generations.contains_key(k))
            && for_all(k in self.materialized_spec_digests.keys(), self.materialized_generations.contains_key(k))
        }

        // A durable member session has exactly one host-authority owner across
        // every mob and identity served by this daemon. Resume/adoption may
        // never alias another row and rotate that session's supervisor trust.
        invariant materialized_sessions_injective {
            for_all(a in self.materialized_sessions.keys(),
                for_all(b in self.materialized_sessions.keys(),
                    a == b
                    || self.materialized_sessions.get_cloned(a)
                        != self.materialized_sessions.get_cloned(b)))
        }

        invariant release_rows_key_aligned {
            for_all(k in self.release_generations.keys(),
                self.release_fences.contains_key(k)
                && self.release_disposals.contains_key(k))
            && for_all(k in self.release_fences.keys(), self.release_generations.contains_key(k))
            && for_all(k in self.release_disposals.keys(), self.release_generations.contains_key(k))
        }

        invariant turn_rows_key_aligned {
            for_all(k in self.turn_outcome_terminal_seqs.keys(), self.turn_outcome_kinds.contains_key(k))
            && for_all(k in self.turn_outcome_kinds.keys(), self.turn_outcome_terminal_seqs.contains_key(k))
        }

        invariant pending_turn_rows_exclude_terminals {
            for_all(k in self.turn_outcome_pending_window_starts.keys(),
                self.turn_outcome_terminal_seqs.contains_key(k) == false
                && self.turn_outcome_acknowledged.contains_key(k) == false
                && self.tracked_input_cancellations.contains_key(k) == false)
            && for_all(k in self.turn_outcome_terminal_seqs.keys(),
                self.turn_outcome_acknowledged.contains_key(k) == false
                && self.tracked_input_cancellations.contains_key(k) == false)
            && for_all(k in self.turn_outcome_acknowledged.keys(),
                self.tracked_input_cancellations.contains_key(k) == false)
        }

        invariant materialized_rows_require_binding {
            for_all(k in self.materialized_generations.keys(),
                self.binding_phases.contains_key(k.mob_id))
        }

        invariant release_rows_require_binding {
            for_all(k in self.release_generations.keys(),
                self.binding_phases.contains_key(k.mob_id))
        }

        invariant turn_rows_require_binding {
            for_all(k in self.turn_outcome_terminal_seqs.keys(),
                self.binding_phases.contains_key(k.mob_id))
            && for_all(k in self.turn_outcome_acknowledged.keys(),
                self.binding_phases.contains_key(k.mob_id))
            && for_all(k in self.tracked_input_cancellations.keys(),
                self.binding_phases.contains_key(k.mob_id))
        }

        invariant pending_turn_rows_require_current_materialization {
            for_all(k in self.turn_outcome_pending_window_starts.keys(),
                mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, k))
        }

        invariant turn_rows_require_current_materialization {
            for_all(k in self.turn_outcome_terminal_seqs.keys(),
                mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, k))
            && for_all(k in self.turn_outcome_acknowledged.keys(),
                mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, k))
            && for_all(k in self.tracked_input_cancellations.keys(),
                mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, k))
        }


        invariant turn_outcome_occupancy_bounded {
            for_all(k in self.turn_outcome_pending_window_starts.keys(),
                mob_host_binding_authority_turn_occupancy(self.turn_outcome_pending_window_starts, self.turn_outcome_terminal_seqs, k) <= 256)
            && for_all(k in self.turn_outcome_terminal_seqs.keys(),
                mob_host_binding_authority_turn_occupancy(self.turn_outcome_pending_window_starts, self.turn_outcome_terminal_seqs, k) <= 256)
        }

        // =====================================================================
        // Host bind ladder
        // =====================================================================

        transition ResolveHostBindAccept {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "address_matches" { address_matches == true }
            guard "token_valid" { token_valid == true }
            guard "binding_generation_positive" { binding_generation > 0 }
            guard "binding_generation_advances" {
                self.binding_generation_highwater.contains_key(mob_id) == false
                || binding_generation > self.binding_generation_highwater.get_copied(mob_id).get("value")
            }
            update {
                self.revoked_supervisor_peer_ids.remove(mob_id);
                self.revoked_supervisor_signing_keys.remove(mob_id);
                self.revoked_supervisor_epochs.remove(mob_id);
                self.revoked_binding_generations.remove(mob_id);
                self.supervisor_peer_ids.insert(mob_id, supervisor_peer_id);
                self.supervisor_signing_keys.insert(mob_id, supervisor_signing_key);
                self.supervisor_epochs.insert(mob_id, epoch);
                self.binding_generations.insert(mob_id, binding_generation);
                self.binding_generation_highwater.insert(mob_id, binding_generation);
                self.binding_phases.insert(mob_id, HostBindingPhase::Bound);
            }
            to Ready
            emit HostBindAccepted { mob_id: mob_id, supervisor_peer_id: supervisor_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        // Redelivered bind from the recorded supervisor at the recorded epoch
        // converges without mutation. The bootstrap token authenticates only
        // FRESH unbound admission: that accept consumes T0 and publishes T1,
        // so an ACK-lost exact replay must remain valid with its original T0.
        // Recorded supervisor identity + sender + address + epoch + binding
        // generation are the replay authority.
        transition ResolveHostBindReplay {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "same_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(supervisor_peer_id) }
            guard "same_epoch" { self.supervisor_epochs.get_cloned(mob_id) == Some(epoch) }
            guard "same_binding_generation" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "address_matches" { address_matches == true }
            update {}
            to Ready
            emit HostBindAccepted { mob_id: mob_id, supervisor_peer_id: supervisor_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        // Any bound-state bind that is not an exact replay is AlreadyBound:
        // a second supervisor for the same mob stays typed-rejected until
        // RevokeHostBinding (A14); epoch changes must go through rebind.
        transition ResolveHostBindAlreadyBound {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "not_exact_replay" {
                self.supervisor_peer_ids.get_cloned(mob_id) != Some(supervisor_peer_id)
                || self.supervisor_epochs.get_cloned(mob_id) != Some(epoch)
                || self.binding_generations.get_copied(mob_id) != Some(binding_generation)
                || sender_matches_supervisor == false
                || address_matches == false
            }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::AlreadyBound }
        }

        transition ResolveHostBindSenderMismatch {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "sender_mismatch" { sender_matches_supervisor == false }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::SenderMismatch }
        }

        transition ResolveHostBindAddressMismatch {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "address_mismatch" { address_matches == false }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::AddressMismatch }
        }

        transition ResolveHostBindInvalidToken {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "address_matches" { address_matches == true }
            guard "token_invalid" { token_valid == false }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::InvalidBootstrapToken }
        }

        transition ResolveHostBindStaleBindingGeneration {
            on input ResolveHostBind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor, address_matches, token_valid }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "address_matches" { address_matches == true }
            guard "token_valid" { token_valid == true }
            guard "binding_generation_stale" {
                binding_generation == 0
                || (self.binding_generation_highwater.contains_key(mob_id) == true
                    && binding_generation <= self.binding_generation_highwater.get_copied(mob_id).get("value"))
            }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleFence }
        }

        // =====================================================================
        // Host rebind ladder — strictly monotonic epoch advance authorized by
        // the recorded current supervisor, plus the idempotent replay ack at
        // the recorded (epoch, supervisor) tuple.
        // =====================================================================

        transition ResolveHostRebindAccept {
            on input ResolveHostRebind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "epoch_advances" { epoch > self.supervisor_epochs.get_cloned(mob_id).get("value") }
            update {
                self.supervisor_peer_ids.insert(mob_id, supervisor_peer_id);
                self.supervisor_signing_keys.insert(mob_id, supervisor_signing_key);
                self.supervisor_epochs.insert(mob_id, epoch);
            }
            to Ready
            emit HostRebindAccepted { mob_id: mob_id, supervisor_peer_id: supervisor_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        // Redelivered rebind from the recorded supervisor at the recorded
        // epoch converges without mutation (FLAG-2: the pending-rotation
        // retry probe must be able to distinguish "host already accepted
        // this epoch" from actually-stale) — the member bind retry ack
        // mirror.
        transition ResolveHostRebindReplay {
            on input ResolveHostRebind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "same_epoch" { self.supervisor_epochs.get_cloned(mob_id) == Some(epoch) }
            guard "same_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(supervisor_peer_id) }
            update {}
            to Ready
            emit HostRebindAccepted { mob_id: mob_id, supervisor_peer_id: supervisor_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        // A lower epoch — or the recorded epoch claimed by a DIFFERENT peer —
        // is a stale supervisor. The recorded-epoch/recorded-peer case is the
        // replay ack above; a new peer must advance the epoch.
        transition ResolveHostRebindStaleSupervisor {
            on input ResolveHostRebind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "epoch_stale_or_peer_changed" {
                epoch < self.supervisor_epochs.get_cloned(mob_id).get("value")
                || (self.supervisor_epochs.get_cloned(mob_id) == Some(epoch)
                    && self.supervisor_peer_ids.get_cloned(mob_id) != Some(supervisor_peer_id))
            }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleSupervisor }
        }

        transition ResolveHostRebindSenderMismatch {
            on input ResolveHostRebind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_mismatch" { sender_matches_supervisor == false }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::SenderMismatch }
        }

        transition ResolveHostRebindNotBound {
            on input ResolveHostRebind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::NotBound }
        }

        transition ResolveHostRebindStaleBindingGeneration {
            on input ResolveHostRebind { mob_id, supervisor_peer_id, supervisor_signing_key, epoch, binding_generation, sender_matches_supervisor }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_matches" { sender_matches_supervisor == true }
            guard "binding_generation_stale" { self.binding_generations.get_copied(mob_id) != Some(binding_generation) }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleFence }
        }

        // =====================================================================
        // Revoke — clears the binding and every mob-scoped row (A14 isolation:
        // other mobs' rows are untouched), including the mob's turn-outcome
        // journal rows (DEC-5: TurnKey is mob-keyed).
        // =====================================================================

        transition RevokeHostBindingBound {
            on input RevokeHostBinding { mob_id, sender_peer_id, sender_signing_key, epoch, binding_generation }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_is_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id) }
            guard "signing_key_is_supervisor" { self.supervisor_signing_keys.get_cloned(mob_id) == Some(sender_signing_key) }
            guard "epoch_current" { self.supervisor_epochs.get_cloned(mob_id) == Some(epoch) }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            update {
                self.materialized_generations = mob_host_binding_authority_member_rows_without_mob(self.materialized_generations, mob_id);
                self.materialized_fences = mob_host_binding_authority_member_rows_without_mob(self.materialized_fences, mob_id);
                self.materialized_sessions = mob_host_binding_authority_member_rows_without_mob(self.materialized_sessions, mob_id);
                self.materialized_spec_digests = mob_host_binding_authority_member_rows_without_mob(self.materialized_spec_digests, mob_id);
                self.release_generations = mob_host_binding_authority_member_rows_without_mob(self.release_generations, mob_id);
                self.release_fences = mob_host_binding_authority_member_rows_without_mob(self.release_fences, mob_id);
                self.release_disposals = mob_host_binding_authority_member_rows_without_mob(self.release_disposals, mob_id);
                self.turn_outcome_pending_window_starts = mob_host_binding_authority_turn_rows_without_mob(self.turn_outcome_pending_window_starts, mob_id);
                self.turn_outcome_terminal_seqs = mob_host_binding_authority_turn_rows_without_mob(self.turn_outcome_terminal_seqs, mob_id);
                self.turn_outcome_kinds = mob_host_binding_authority_turn_rows_without_mob(self.turn_outcome_kinds, mob_id);
                self.turn_outcome_acknowledged = mob_host_binding_authority_turn_rows_without_mob(self.turn_outcome_acknowledged, mob_id);
                self.tracked_input_cancellations = mob_host_binding_authority_turn_rows_without_mob(self.tracked_input_cancellations, mob_id);
                self.revoked_supervisor_peer_ids.insert(mob_id, sender_peer_id);
                self.revoked_supervisor_signing_keys.insert(mob_id, self.supervisor_signing_keys.get_cloned(mob_id).get("value"));
                self.revoked_supervisor_epochs.insert(mob_id, epoch);
                self.revoked_binding_generations.insert(mob_id, binding_generation);
                self.supervisor_peer_ids.remove(mob_id);
                self.supervisor_signing_keys.remove(mob_id);
                self.supervisor_epochs.remove(mob_id);
                self.binding_generations.remove(mob_id);
                self.binding_phases.remove(mob_id);
            }
            to Ready
            emit HostBindingRevoked { mob_id: mob_id, supervisor_peer_id: sender_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        transition RevokeHostBindingReplay {
            on input RevokeHostBinding { mob_id, sender_peer_id, sender_signing_key, epoch, binding_generation }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "receipt_present" { self.revoked_supervisor_peer_ids.contains_key(mob_id) == true }
            guard "same_supervisor" { self.revoked_supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id) }
            guard "same_signing_key" { self.revoked_supervisor_signing_keys.get_cloned(mob_id) == Some(sender_signing_key) }
            guard "same_epoch" { self.revoked_supervisor_epochs.get_cloned(mob_id) == Some(epoch) }
            guard "same_binding_generation" { self.revoked_binding_generations.get_copied(mob_id) == Some(binding_generation) }
            update {}
            to Ready
            emit HostBindingRevokeReplayed { mob_id: mob_id, supervisor_peer_id: sender_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        transition RevokeHostBindingSenderMismatch {
            on input RevokeHostBinding { mob_id, sender_peer_id, sender_signing_key, epoch, binding_generation }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "active_or_receipted" {
                self.binding_phases.contains_key(mob_id) == true
                || self.revoked_supervisor_peer_ids.contains_key(mob_id) == true
            }
            guard "sender_mismatch" {
                (self.binding_phases.contains_key(mob_id) == true
                    && (self.supervisor_peer_ids.get_cloned(mob_id) != Some(sender_peer_id)
                        || self.supervisor_signing_keys.get_cloned(mob_id) != Some(sender_signing_key)))
                || (self.binding_phases.contains_key(mob_id) == false
                    && (self.revoked_supervisor_peer_ids.get_cloned(mob_id) != Some(sender_peer_id)
                        || self.revoked_supervisor_signing_keys.get_cloned(mob_id) != Some(sender_signing_key)))
            }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::SenderMismatch }
        }

        transition RevokeHostBindingStaleEpoch {
            on input RevokeHostBinding { mob_id, sender_peer_id, sender_signing_key, epoch, binding_generation }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "active_or_receipted" {
                self.binding_phases.contains_key(mob_id) == true
                || self.revoked_supervisor_peer_ids.contains_key(mob_id) == true
            }
            guard "sender_matches" {
                (self.binding_phases.contains_key(mob_id) == true
                    && self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id)
                    && self.supervisor_signing_keys.get_cloned(mob_id) == Some(sender_signing_key))
                || (self.binding_phases.contains_key(mob_id) == false
                    && self.revoked_supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id)
                    && self.revoked_supervisor_signing_keys.get_cloned(mob_id) == Some(sender_signing_key))
            }
            guard "binding_generation_current" {
                (self.binding_phases.contains_key(mob_id) == true
                    && self.binding_generations.get_copied(mob_id) == Some(binding_generation))
                || (self.binding_phases.contains_key(mob_id) == false
                    && self.revoked_binding_generations.get_copied(mob_id) == Some(binding_generation))
            }
            guard "epoch_mismatch" {
                (self.binding_phases.contains_key(mob_id) == true
                    && self.supervisor_epochs.get_cloned(mob_id) != Some(epoch))
                || (self.binding_phases.contains_key(mob_id) == false
                    && self.revoked_supervisor_epochs.get_cloned(mob_id) != Some(epoch))
            }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleSupervisor }
        }

        transition RevokeHostBindingNotBound {
            on input RevokeHostBinding { mob_id, sender_peer_id, sender_signing_key, epoch, binding_generation }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            guard "no_receipt" { self.revoked_supervisor_peer_ids.contains_key(mob_id) == false }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::NotBound }
        }

        transition RevokeHostBindingStaleBindingGeneration {
            on input RevokeHostBinding { mob_id, sender_peer_id, sender_signing_key, epoch, binding_generation }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "active_or_receipted" {
                self.binding_phases.contains_key(mob_id) == true
                || self.revoked_supervisor_peer_ids.contains_key(mob_id) == true
            }
            guard "sender_matches" {
                (self.binding_phases.contains_key(mob_id) == true
                    && self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id)
                    && self.supervisor_signing_keys.get_cloned(mob_id) == Some(sender_signing_key))
                || (self.binding_phases.contains_key(mob_id) == false
                    && self.revoked_supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id)
                    && self.revoked_supervisor_signing_keys.get_cloned(mob_id) == Some(sender_signing_key))
            }
            guard "binding_generation_stale" {
                (self.binding_phases.contains_key(mob_id) == true
                    && self.binding_generations.get_copied(mob_id) != Some(binding_generation))
                || (self.binding_phases.contains_key(mob_id) == false
                    && self.revoked_binding_generations.get_copied(mob_id) != Some(binding_generation))
            }
            update {}
            to Ready
            emit HostBindRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleFence }
        }

        // =====================================================================
        // Generic host-addressed command admission (§6.3)
        // =====================================================================

        transition ResolveHostCommandAdmissionAdmit {
            on input ResolveHostCommandAdmission { mob_id, sender_peer_id, epoch, binding_generation, turn_directive_present, turn_directive_supported }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_is_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id) }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "epoch_current" { epoch >= self.supervisor_epochs.get_cloned(mob_id).get("value") }
            guard "turn_directive_ok" { turn_directive_present == false || turn_directive_supported == true }
            update {}
            to Ready
            emit HostCommandAdmitted { mob_id: mob_id, sender_peer_id: sender_peer_id, epoch: epoch, binding_generation: binding_generation }
        }

        transition ResolveHostCommandAdmissionNotBound {
            on input ResolveHostCommandAdmission { mob_id, sender_peer_id, epoch, binding_generation, turn_directive_present, turn_directive_supported }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(mob_id) == false }
            update {}
            to Ready
            emit HostCommandRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::NotBound }
        }

        transition ResolveHostCommandAdmissionSenderMismatch {
            on input ResolveHostCommandAdmission { mob_id, sender_peer_id, epoch, binding_generation, turn_directive_present, turn_directive_supported }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_mismatch" { self.supervisor_peer_ids.get_cloned(mob_id) != Some(sender_peer_id) }
            update {}
            to Ready
            emit HostCommandRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::SenderMismatch }
        }

        transition ResolveHostCommandAdmissionStaleSupervisor {
            on input ResolveHostCommandAdmission { mob_id, sender_peer_id, epoch, binding_generation, turn_directive_present, turn_directive_supported }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_is_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id) }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "epoch_stale" { epoch < self.supervisor_epochs.get_cloned(mob_id).get("value") }
            update {}
            to Ready
            emit HostCommandRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleSupervisor }
        }

        transition ResolveHostCommandAdmissionTurnDirectiveUnsupported {
            on input ResolveHostCommandAdmission { mob_id, sender_peer_id, epoch, binding_generation, turn_directive_present, turn_directive_supported }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_is_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id) }
            guard "binding_generation_current" { self.binding_generations.get_copied(mob_id) == Some(binding_generation) }
            guard "epoch_current" { epoch >= self.supervisor_epochs.get_cloned(mob_id).get("value") }
            guard "turn_directive_unsupported" { turn_directive_present == true && turn_directive_supported == false }
            update {}
            to Ready
            emit HostCommandRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::TurnDirectiveUnsupported }
        }

        transition ResolveHostCommandAdmissionStaleBindingGeneration {
            on input ResolveHostCommandAdmission { mob_id, sender_peer_id, epoch, binding_generation, turn_directive_present, turn_directive_supported }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(mob_id) == true }
            guard "sender_is_supervisor" { self.supervisor_peer_ids.get_cloned(mob_id) == Some(sender_peer_id) }
            guard "binding_generation_stale" { self.binding_generations.get_copied(mob_id) != Some(binding_generation) }
            update {}
            to Ready
            emit HostCommandRejected { mob_id: mob_id, cause: HostAdmissionRejectKind::StaleFence }
        }

        // =====================================================================
        // Materialize dedup adjudication (the trio + fresh/superseding arms)
        // =====================================================================

        transition ResolveMaterializeAdmissionNotBound {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(member_key.mob_id) == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::NotBound }
        }

        transition ResolveMaterializeAdmissionFresh {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "no_release_row" { self.release_generations.contains_key(member_key) == false }
            update {}
            to Ready
            emit MaterializeAdmitted { member_key: member_key, generation: generation, fence_token: fence_token }
        }

        transition ResolveMaterializeAdmissionFreshAfterRelease {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "release_row" { self.release_generations.contains_key(member_key) == true }
            guard "tuple_above_release" {
                generation > self.release_generations.get_cloned(member_key).get("value")
                || (generation == self.release_generations.get_cloned(member_key).get("value")
                    && fence_token > self.release_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit MaterializeAdmitted { member_key: member_key, generation: generation, fence_token: fence_token }
        }

        transition ResolveMaterializeAdmissionStaleAfterRelease {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "release_row" { self.release_generations.contains_key(member_key) == true }
            guard "tuple_not_above_release" {
                generation < self.release_generations.get_cloned(member_key).get("value")
                || (generation == self.release_generations.get_cloned(member_key).get("value")
                    && fence_token <= self.release_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::StaleFence }
        }

        // Same tuple + same digest: replay returns the recorded result.
        transition ResolveMaterializeAdmissionReplay {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "same_generation" { self.materialized_generations.get_cloned(member_key) == Some(generation) }
            guard "same_fence" { self.materialized_fences.get_cloned(member_key) == Some(fence_token) }
            guard "same_digest" { self.materialized_spec_digests.get_cloned(member_key) == Some(spec_digest) }
            update {}
            to Ready
            emit MaterializeReplay {
                member_key: member_key,
                generation: generation,
                fence_token: fence_token,
                session_id: self.materialized_sessions.get_cloned(member_key).get("value"),
                spec_digest: spec_digest
            }
        }

        // Same tuple + different digest: one idempotency key can never name
        // two builds (§14 R8 dedup hardening).
        transition ResolveMaterializeAdmissionDigestMismatch {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "same_generation" { self.materialized_generations.get_cloned(member_key) == Some(generation) }
            guard "same_fence" { self.materialized_fences.get_cloned(member_key) == Some(fence_token) }
            guard "digest_mismatch" { self.materialized_spec_digests.get_cloned(member_key) != Some(spec_digest) }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::SpecDigestMismatch }
        }

        transition ResolveMaterializeAdmissionStaleFence {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "tuple_below_recorded" {
                generation < self.materialized_generations.get_cloned(member_key).get("value")
                || (generation == self.materialized_generations.get_cloned(member_key).get("value")
                    && fence_token < self.materialized_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::StaleFence }
        }

        // Higher tuple than recorded: a machine-issued re-materialization of
        // the identity at a new generation/fence (revival, §14 A20) admits;
        // the success recorder then overwrites the recorded row.
        transition ResolveMaterializeAdmissionSuperseding {
            on input ResolveMaterializeAdmission { member_key, generation, fence_token, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "tuple_above_recorded" {
                generation > self.materialized_generations.get_cloned(member_key).get("value")
                || (generation == self.materialized_generations.get_cloned(member_key).get("value")
                    && fence_token > self.materialized_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit MaterializeAdmitted { member_key: member_key, generation: generation, fence_token: fence_token }
        }

        // =====================================================================
        // Materialize preflight matrix (§14 R7 tier 2, A11) — first false
        // observation in the fixed inventory order produces its typed cause.
        // =====================================================================

        transition ResolveMaterializePreflightNotBound {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(member_key.mob_id) == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::NotBound }
        }

        transition ResolveMaterializePreflightModelUnresolvable {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_unresolvable" { model_resolvable == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::ModelUnresolvable }
        }

        transition ResolveMaterializePreflightAuthBindingUnresolvable {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_unresolvable" { binding_resolvable == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::AuthBindingUnresolvable }
        }

        transition ResolveMaterializePreflightEnvKeysMissing {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_resolvable" { binding_resolvable == true }
            guard "env_keys_missing" { env_keys_present == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::EnvKeysMissing }
        }

        transition ResolveMaterializePreflightMcpCommandMissing {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_resolvable" { binding_resolvable == true }
            guard "env_keys_present" { env_keys_present == true }
            guard "stdio_commands_missing" { stdio_commands_present == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::McpCommandMissing }
        }

        transition ResolveMaterializePreflightEngineProtocolUnsupported {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_resolvable" { binding_resolvable == true }
            guard "env_keys_present" { env_keys_present == true }
            guard "stdio_commands_present" { stdio_commands_present == true }
            guard "engine_protocol_unsupported" { engine_protocol_supported == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::EngineProtocolUnsupported }
        }

        transition ResolveMaterializePreflightRealmBackendUnavailable {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_resolvable" { binding_resolvable == true }
            guard "env_keys_present" { env_keys_present == true }
            guard "stdio_commands_present" { stdio_commands_present == true }
            guard "engine_protocol_supported" { engine_protocol_supported == true }
            guard "realm_backend_unavailable" { durable_sessions_required == true && realm_backend_persistent == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::RealmBackendUnavailable }
        }

        transition ResolveMaterializePreflightMemoryStoreUnavailable {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_resolvable" { binding_resolvable == true }
            guard "env_keys_present" { env_keys_present == true }
            guard "stdio_commands_present" { stdio_commands_present == true }
            guard "engine_protocol_supported" { engine_protocol_supported == true }
            guard "realm_backend_ok" { durable_sessions_required == false || realm_backend_persistent == true }
            guard "memory_store_unavailable" { memory_required == true && memory_capability == false }
            update {}
            to Ready
            emit MaterializeRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: MaterializeRejectKind::MemoryStoreUnavailable }
        }

        transition ResolveMaterializePreflightAdmit {
            on input ResolveMaterializePreflight { member_key, generation, fence_token, model_resolvable, binding_resolvable, env_keys_present, stdio_commands_present, engine_protocol_supported, durable_sessions_required, realm_backend_persistent, memory_required, memory_capability }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "model_resolvable" { model_resolvable == true }
            guard "binding_resolvable" { binding_resolvable == true }
            guard "env_keys_present" { env_keys_present == true }
            guard "stdio_commands_present" { stdio_commands_present == true }
            guard "engine_protocol_supported" { engine_protocol_supported == true }
            guard "realm_backend_ok" { durable_sessions_required == false || realm_backend_persistent == true }
            guard "memory_ok" { memory_required == false || memory_capability == true }
            update {}
            to Ready
            emit MaterializeAdmitted { member_key: member_key, generation: generation, fence_token: fence_token }
        }

        // =====================================================================
        // Materialize success recorder (successes only; no regression)
        // =====================================================================

        transition RecordMaterializedMemberFreshOrSuperseding {
            on input RecordMaterializedMember { member_key, generation, fence_token, session_id, spec_digest }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_regression" {
                self.materialized_generations.contains_key(member_key) == false
                || generation > self.materialized_generations.get_cloned(member_key).get("value")
                || (generation == self.materialized_generations.get_cloned(member_key).get("value")
                    && fence_token >= self.materialized_fences.get_cloned(member_key).get("value"))
            }
            update {
                self.turn_outcome_pending_window_starts = mob_host_binding_authority_turn_rows_for_materialization(self.turn_outcome_pending_window_starts, member_key, generation, fence_token);
                self.turn_outcome_terminal_seqs = mob_host_binding_authority_turn_rows_for_materialization(self.turn_outcome_terminal_seqs, member_key, generation, fence_token);
                self.turn_outcome_kinds = mob_host_binding_authority_turn_rows_for_materialization(self.turn_outcome_kinds, member_key, generation, fence_token);
                self.turn_outcome_acknowledged = mob_host_binding_authority_turn_rows_for_materialization(self.turn_outcome_acknowledged, member_key, generation, fence_token);
                self.tracked_input_cancellations = mob_host_binding_authority_turn_rows_for_materialization(self.tracked_input_cancellations, member_key, generation, fence_token);
                self.materialized_generations.insert(member_key, generation);
                self.materialized_fences.insert(member_key, fence_token);
                self.materialized_sessions.insert(member_key, session_id);
                self.materialized_spec_digests.insert(member_key, spec_digest);
            }
            to Ready
            emit MaterializedMemberRecorded { member_key: member_key, generation: generation, fence_token: fence_token, session_id: session_id }
        }

        // =====================================================================
        // Release admission mirror (§19.L3)
        // =====================================================================

        transition ResolveReleaseAdmissionNotBound {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_bound" { self.binding_phases.contains_key(member_key.mob_id) == false }
            update {}
            to Ready
            emit ReleaseRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: HostAdmissionRejectKind::NotBound }
        }

        transition ResolveReleaseAdmissionAdmit {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "same_generation" { self.materialized_generations.get_cloned(member_key) == Some(generation) }
            guard "same_fence" { self.materialized_fences.get_cloned(member_key) == Some(fence_token) }
            update {}
            to Ready
            emit ReleaseAdmitted { member_key: member_key, generation: generation, fence_token: fence_token }
        }

        transition ResolveReleaseAdmissionStaleMaterialized {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "tuple_below_recorded" {
                generation < self.materialized_generations.get_cloned(member_key).get("value")
                || (generation == self.materialized_generations.get_cloned(member_key).get("value")
                    && fence_token < self.materialized_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit ReleaseRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: HostAdmissionRejectKind::StaleFence }
        }

        // A release tuple AHEAD of the recorded materialization names a
        // member state this host never produced — typed Unsupported, never
        // silently absorbed.
        transition ResolveReleaseAdmissionAheadMaterialized {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "tuple_above_recorded" {
                generation > self.materialized_generations.get_cloned(member_key).get("value")
                || (generation == self.materialized_generations.get_cloned(member_key).get("value")
                    && fence_token > self.materialized_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit ReleaseRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: HostAdmissionRejectKind::Unsupported }
        }

        transition ResolveReleaseAdmissionReplay {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "release_row" { self.release_generations.contains_key(member_key) == true }
            guard "same_generation" { self.release_generations.get_cloned(member_key) == Some(generation) }
            guard "same_fence" { self.release_fences.get_cloned(member_key) == Some(fence_token) }
            update {}
            to Ready
            emit ReleaseReplay { member_key: member_key, disposal: self.release_disposals.get_cloned(member_key).get("value") }
        }

        transition ResolveReleaseAdmissionStaleReleased {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "release_row" { self.release_generations.contains_key(member_key) == true }
            guard "tuple_below_released" {
                generation < self.release_generations.get_cloned(member_key).get("value")
                || (generation == self.release_generations.get_cloned(member_key).get("value")
                    && fence_token < self.release_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit ReleaseRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: HostAdmissionRejectKind::StaleFence }
        }

        transition ResolveReleaseAdmissionAheadReleased {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "release_row" { self.release_generations.contains_key(member_key) == true }
            guard "tuple_above_released" {
                generation > self.release_generations.get_cloned(member_key).get("value")
                || (generation == self.release_generations.get_cloned(member_key).get("value")
                    && fence_token > self.release_fences.get_cloned(member_key).get("value"))
            }
            update {}
            to Ready
            emit ReleaseRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: HostAdmissionRejectKind::Unsupported }
        }

        transition ResolveReleaseAdmissionUnknownMember {
            on input ResolveReleaseAdmission { member_key, generation, fence_token }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "no_materialized_row" { self.materialized_generations.contains_key(member_key) == false }
            guard "no_release_row" { self.release_generations.contains_key(member_key) == false }
            update {}
            to Ready
            emit ReleaseRejected { member_key: member_key, generation: generation, fence_token: fence_token, cause: HostAdmissionRejectKind::Unsupported }
        }

        // =====================================================================
        // Release recorder — moves the materialized row into release dedup
        // memory and prunes every turn-outcome row for that now-unowned
        // member identity.
        // =====================================================================

        transition RecordMemberReleaseAtRecordedTuple {
            on input RecordMemberRelease { member_key, generation, fence_token, disposal }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "bound" { self.binding_phases.contains_key(member_key.mob_id) == true }
            guard "materialized_row" { self.materialized_generations.contains_key(member_key) == true }
            guard "same_generation" { self.materialized_generations.get_cloned(member_key) == Some(generation) }
            guard "same_fence" { self.materialized_fences.get_cloned(member_key) == Some(fence_token) }
            update {
                self.turn_outcome_pending_window_starts = mob_host_binding_authority_turn_rows_after_release(self.turn_outcome_pending_window_starts, member_key, generation);
                self.turn_outcome_terminal_seqs = mob_host_binding_authority_turn_rows_after_release(self.turn_outcome_terminal_seqs, member_key, generation);
                self.turn_outcome_kinds = mob_host_binding_authority_turn_rows_after_release(self.turn_outcome_kinds, member_key, generation);
                self.turn_outcome_acknowledged = mob_host_binding_authority_turn_rows_after_release(self.turn_outcome_acknowledged, member_key, generation);
                self.tracked_input_cancellations = mob_host_binding_authority_turn_rows_after_release(self.tracked_input_cancellations, member_key, generation);
                self.release_generations.insert(member_key, generation);
                self.release_fences.insert(member_key, fence_token);
                self.release_disposals.insert(member_key, disposal);
                self.materialized_generations.remove(member_key);
                self.materialized_fences.remove(member_key);
                self.materialized_sessions.remove(member_key);
                self.materialized_spec_digests.remove(member_key);
            }
            to Ready
            emit MemberReleaseRecorded { member_key: member_key, disposal: disposal }
        }

        // =====================================================================
        // Turn-outcome journal (§18 O2) — durable Pending-before-effect,
        // then an atomic Pending → terminal move on the full turn key.
        // =====================================================================

        transition ReserveTurnOutcomePendingFresh {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "not_cancelled" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            guard "capacity_available" { mob_host_binding_authority_turn_occupancy(self.turn_outcome_pending_window_starts, self.turn_outcome_terminal_seqs, turn_key) < 256 }
            update {
                self.turn_outcome_pending_window_starts.insert(turn_key, window_start);
            }
            to Ready
            emit TurnOutcomePendingReserved { turn_key: turn_key, window_start: window_start }
        }

        transition ReserveTurnOutcomePendingReplay {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TurnOutcomePendingReplayed {
                turn_key: turn_key,
                window_start: self.turn_outcome_pending_window_starts.get_cloned(turn_key).get("value")
            }
        }

        transition ReserveTurnOutcomePendingTerminalReplay {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TurnOutcomePendingTerminalReplay { turn_key: turn_key }
        }

        // ACK removes the payload row but retains this exact-key tombstone.
        // Delayed request delivery therefore deduplicates without recreating
        // Pending or consulting a volatile runtime ledger.
        transition ReserveTurnOutcomePendingAcknowledgedReplay {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TurnOutcomePendingTerminalReplay { turn_key: turn_key }
        }

        // A cancel receipt is a durable negative/controlling terminal for
        // delivery admission. It therefore shares the existing terminal
        // replay effect: the shell returns exact dedup without touching the
        // volatile runtime ledger.
        transition ReserveTurnOutcomePendingCancelledReplay {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "cancelled" { self.tracked_input_cancellations.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TurnOutcomePendingTerminalReplay { turn_key: turn_key }
        }

        transition ReserveTurnOutcomePendingJournalFull {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "not_cancelled" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            guard "capacity_exhausted" { mob_host_binding_authority_turn_occupancy(self.turn_outcome_pending_window_starts, self.turn_outcome_terminal_seqs, turn_key) >= 256 }
            update {}
            to Ready
            emit TurnOutcomePendingJournalFull { turn_key: turn_key }
        }

        transition ReserveTurnOutcomePendingStale {
            on input ReserveTurnOutcomePending { turn_key, window_start }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == false }
            update {}
            to Ready
            emit TurnOutcomePendingStale { turn_key: turn_key }
        }

        transition CancelTurnOutcomePendingPresent {
            on input CancelTurnOutcomePending { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == true }
            update {
                self.turn_outcome_pending_window_starts.remove(turn_key);
            }
            to Ready
            emit TurnOutcomePendingCanceled { turn_key: turn_key }
        }

        transition CancelTurnOutcomePendingAbsent {
            on input CancelTurnOutcomePending { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            update {}
            to Ready
            emit TurnOutcomePendingCancelReplay { turn_key: turn_key }
        }

        transition RecordTurnOutcomeFresh {
            on input RecordTurnOutcome { turn_key, terminal_seq, outcome }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == true }
            guard "unrecorded" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "not_cancelled" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            update {
                self.turn_outcome_pending_window_starts.remove(turn_key);
                self.turn_outcome_terminal_seqs.insert(turn_key, terminal_seq);
                self.turn_outcome_kinds.insert(turn_key, outcome);
            }
            to Ready
            emit TurnOutcomeRecorded { turn_key: turn_key, terminal_seq: terminal_seq, outcome: outcome }
        }

        // Redelivery converges: the recorded row wins and is echoed back.
        transition RecordTurnOutcomeReplay {
            on input RecordTurnOutcome { turn_key, terminal_seq, outcome }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "recorded" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TurnOutcomeReplayed {
                turn_key: turn_key,
                terminal_seq: self.turn_outcome_terminal_seqs.get_cloned(turn_key).get("value"),
                outcome: self.turn_outcome_kinds.get_cloned(turn_key).get("value")
            }
        }

        // A watcher can finish after release or after a newer generation has
        // superseded its residency. That completion is an expected stale
        // no-op: it must never recreate an invisible durable journal row.
        transition RecordTurnOutcomeStale {
            on input RecordTurnOutcome { turn_key, terminal_seq, outcome }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == false }
            update {}
            to Ready
            emit TurnOutcomeStaleDropped { turn_key: turn_key }
        }

        // A duplicate/recovered watcher can finish after another watcher
        // already consumed Pending and the controller acknowledged its
        // terminal. Without a current Pending or terminal fact it has no
        // write authority and converges as an explicit no-op.
        transition RecordTurnOutcomeUnreserved {
            on input RecordTurnOutcome { turn_key, terminal_seq, outcome }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "unrecorded" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            update {}
            to Ready
            emit TurnOutcomeUnreservedDropped { turn_key: turn_key }
        }

        // Exact acknowledgement prunes both aligned machine facts. It is
        // intentionally independent from the durable event cursor because a
        // journal commit may lag the terminal event append.
        transition AcknowledgeTurnOutcomePresent {
            on input AcknowledgeTurnOutcome { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "mob_bound" { self.binding_phases.contains_key(turn_key.mob_id) == true }
            guard "recorded" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == true }
            update {
                self.turn_outcome_acknowledged.insert(turn_key, true);
                self.turn_outcome_terminal_seqs.remove(turn_key);
                self.turn_outcome_kinds.remove(turn_key);
            }
            to Ready
            emit TurnOutcomeAcknowledged { turn_key: turn_key }
        }

        // Unknown or already-acknowledged acknowledgements converge. Unknown
        // keys retain no negative memory (so an ACK racing ahead of record
        // cannot suppress it); acknowledged keys retain their existing exact
        // tombstone until member residency disposal.
        transition AcknowledgeTurnOutcomeAbsent {
            on input AcknowledgeTurnOutcome { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "mob_bound" { self.binding_phases.contains_key(turn_key.mob_id) == true }
            guard "unrecorded" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            update {}
            to Ready
            emit TurnOutcomeAckReplay { turn_key: turn_key }
        }

        // =================================================================
        // Exact tracked-input cancellation. The shell serializes this input
        // with delivery admission for the same key, so `runtime_input_present
        // == false` plus these authority guards is a real negative
        // certificate rather than a racy ledger snapshot.
        // =================================================================

        transition CancelTrackedInputExisting {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "cancel_recorded" { self.tracked_input_cancellations.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TrackedInputCancelReplay {
                turn_key: turn_key,
                outcome: self.tracked_input_cancellations.get_copied(turn_key).get("value")
            }
        }

        transition CancelTrackedInputTerminal {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TrackedInputCancelTerminal {
                turn_key: turn_key,
                terminal_seq: self.turn_outcome_terminal_seqs.get_copied(turn_key).get("value"),
                outcome: self.turn_outcome_kinds.get_copied(turn_key).get("value")
            }
        }

        // The payload terminal was already consumed, but the ACK tombstone
        // still proves that execution was possible. Never fabricate
        // `NoEffect`; cancellation converges as the conservative controlling
        // terminal instead.
        transition CancelTrackedInputAcknowledged {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == true }
            update {}
            to Ready
            emit TrackedInputCancelAcknowledgedReplay { turn_key: turn_key }
        }

        // With the exact-key admission gate held, an absent runtime binding
        // means neither an existing Pending reservation nor a delayed sender
        // can cross into runtime acceptance. Atomically consume Pending (when
        // present) and install the durable negative receipt.
        transition CancelTrackedInputNoEffect {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "runtime_absent" { runtime_input_present == false }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "not_cancel_recorded" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            update {
                self.turn_outcome_pending_window_starts.remove(turn_key);
                self.tracked_input_cancellations.insert(turn_key, TrackedInputCancelKind::NoEffect);
            }
            to Ready
            emit TrackedInputCancelNoEffect { turn_key: turn_key }
        }

        // Runtime acceptance is possible. Replace Pending with a durable
        // controlling tombstone before the shell starts quiescence so every
        // delayed Deliver is blocked, including after a host crash.
        transition CancelTrackedInputRequest {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "runtime_present" { runtime_input_present == true }
            guard "pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == true }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "not_cancel_recorded" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            update {
                self.turn_outcome_pending_window_starts.remove(turn_key);
                self.tracked_input_cancellations.insert(turn_key, TrackedInputCancelKind::Cancelling);
            }
            to Ready
            emit TrackedInputCancelRequested { turn_key: turn_key }
        }

        // A runtime idempotency row without host Pending/terminal/cancel
        // custody is contradictory. It cannot authorize either NoEffect or a
        // new controlling tombstone.
        transition CancelTrackedInputUnreserved {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "runtime_present" { runtime_input_present == true }
            guard "not_pending" { self.turn_outcome_pending_window_starts.contains_key(turn_key) == false }
            guard "not_terminal" { self.turn_outcome_terminal_seqs.contains_key(turn_key) == false }
            guard "not_acknowledged" { self.turn_outcome_acknowledged.contains_key(turn_key) == false }
            guard "not_cancel_recorded" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            update {}
            to Ready
            emit TrackedInputCancelUnreserved { turn_key: turn_key }
        }

        transition CancelTrackedInputStale {
            on input CancelTrackedInput { turn_key, runtime_input_present }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == false }
            update {}
            to Ready
            emit TrackedInputCancelStale { turn_key: turn_key }
        }

        transition CompleteTrackedInputCancelPending {
            on input CompleteTrackedInputCancel { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "cancelling" { self.tracked_input_cancellations.get_copied(turn_key) == Some(TrackedInputCancelKind::Cancelling) }
            update {
                self.tracked_input_cancellations.insert(turn_key, TrackedInputCancelKind::Cancelled);
            }
            to Ready
            emit TrackedInputCancelCompleted { turn_key: turn_key }
        }

        transition CompleteTrackedInputCancelReplay {
            on input CompleteTrackedInputCancel { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "terminal_cancel" {
                self.tracked_input_cancellations.get_copied(turn_key) == Some(TrackedInputCancelKind::NoEffect)
                || self.tracked_input_cancellations.get_copied(turn_key) == Some(TrackedInputCancelKind::Cancelled)
            }
            update {}
            to Ready
            emit TrackedInputCancelReplay {
                turn_key: turn_key,
                outcome: self.tracked_input_cancellations.get_copied(turn_key).get("value")
            }
        }

        transition CompleteTrackedInputCancelUnreserved {
            on input CompleteTrackedInputCancel { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == true }
            guard "no_cancel" { self.tracked_input_cancellations.contains_key(turn_key) == false }
            update {}
            to Ready
            emit TrackedInputCancelUnreserved { turn_key: turn_key }
        }

        transition CompleteTrackedInputCancelStale {
            on input CompleteTrackedInputCancel { turn_key }
            guard { self.lifecycle_phase == Phase::Ready }
            guard "not_current_materialized_residency" { mob_host_binding_authority_turn_key_is_current(self.materialized_generations, self.materialized_fences, turn_key) == false }
            update {}
            to Ready
            emit TrackedInputCancelStale { turn_key: turn_key }
        }
    }
        }

        impl MobHostBindingAuthorityAuthority {
            // Native reducer helpers. The Rust bodies live here (expanded into
            // both the catalog and production authorities); their TLA+
            // operator definitions live in
            // `meerkat-machine-codegen/src/artifacts.rs`
            // (`render_mob_host_binding_authority_native_helpers`) and the
            // names are registered in the native-helper allow-list in
            // `meerkat-machine-schema/src/machine.rs`.

            // Retains only the member rows whose key names a different mob —
            // RevokeHostBinding's mob-scoped clear (A14 isolation).
            fn mob_host_binding_authority_member_rows_without_mob<V: Clone>(
                rows: &std::collections::BTreeMap<MemberKey, V>,
                mob_id: &MobId,
            ) -> std::collections::BTreeMap<MemberKey, V> {
                rows.iter()
                    .filter(|(key, _)| key.mob_id != *mob_id)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }

            // Retains only the turn-journal rows whose key names a different
            // mob — the DEC-5 half of RevokeHostBinding's mob-scoped clear.
            fn mob_host_binding_authority_turn_rows_without_mob<V: Clone>(
                rows: &std::collections::BTreeMap<TurnKey, V>,
                mob_id: &MobId,
            ) -> std::collections::BTreeMap<TurnKey, V> {
                rows.iter()
                    .filter(|(key, _)| key.mob_id != *mob_id)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }

            // A release leaves no current materialized owner, so every row
            // for that exact mob + identity is pruned. Other identities and
            // other mobs remain isolated.
            fn mob_host_binding_authority_turn_rows_after_release<V: Clone>(
                rows: &std::collections::BTreeMap<TurnKey, V>,
                member_key: &MemberKey,
                _released_generation: &Generation,
            ) -> std::collections::BTreeMap<TurnKey, V> {
                rows.iter()
                    .filter(|(key, _)| {
                        key.mob_id != member_key.mob_id
                            || key.agent_identity != member_key.agent_identity
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }

            // A superseding materialization retires every journal row for an
            // older `(generation, fence)` residency of that member. A
            // same-generation higher fence is still a distinct authority.
            fn mob_host_binding_authority_turn_rows_for_materialization<V: Clone>(
                rows: &std::collections::BTreeMap<TurnKey, V>,
                member_key: &MemberKey,
                generation: &Generation,
                fence_token: &FenceToken,
            ) -> std::collections::BTreeMap<TurnKey, V> {
                rows.iter()
                    .filter(|(key, _)| {
                        key.mob_id != member_key.mob_id
                            || key.agent_identity != member_key.agent_identity
                            || (key.generation == *generation
                                && key.fence_token == *fence_token)
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }

            // Total ownership predicate for journal admission/replay and the
            // state invariant: exact mob + identity + current generation and
            // fence authority.
            fn mob_host_binding_authority_turn_key_is_current(
                materialized_generations: &std::collections::BTreeMap<MemberKey, Generation>,
                materialized_fences: &std::collections::BTreeMap<MemberKey, FenceToken>,
                turn_key: &TurnKey,
            ) -> bool {
                let member_key = MemberKey::new(
                    turn_key.mob_id.clone(),
                    turn_key.agent_identity.clone(),
                );
                materialized_generations.get(&member_key) == Some(&turn_key.generation)
                    && materialized_fences.get(&member_key) == Some(&turn_key.fence_token)
            }

            // Durable quota occupancy for one mob-scoped member. Pending and
            // terminal maps are invariant-disjoint, so the sum is exact.
            fn mob_host_binding_authority_turn_occupancy(
                pending: &std::collections::BTreeMap<TurnKey, u64>,
                terminal: &std::collections::BTreeMap<TurnKey, u64>,
                turn_key: &TurnKey,
            ) -> u64 {
                let pending_count = pending
                    .keys()
                    .filter(|key| {
                        key.mob_id == turn_key.mob_id
                            && key.agent_identity == turn_key.agent_identity
                    })
                    .count();
                let terminal_count = terminal
                    .keys()
                    .filter(|key| {
                        key.mob_id == turn_key.mob_id
                            && key.agent_identity == turn_key.agent_identity
                    })
                    .count();
                u64::try_from(pending_count.saturating_add(terminal_count)).unwrap_or(u64::MAX)
            }
        }
    };
}

crate::mob_host_binding_authority_dsl!("self", "catalog::dsl::mob_host_binding_authority");

// ---------------------------------------------------------------------------
// Value types. The DSL needs Ord + Hash + Clone (+ Default, because generated
// guard/emit code reads map values through `OptionValueExt::get`, which is
// `unwrap_or_default()` behind ordered guards). The production copy in
// `meerkat-mob` re-exports these and adds domain bridging.
// ---------------------------------------------------------------------------

/// Mob identity key. One authority serves many mobs (A14).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MobId(pub String);

impl<T: Into<String>> From<T> for MobId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Member agent identity within a mob roster.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct AgentIdentity(pub String);

impl<T: Into<String>> From<T> for AgentIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Comms peer identity string (identity-first, never a display name).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerId(pub String);

impl<T: Into<String>> From<T> for PeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Ed25519 public signing key of the bound supervisor.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerSigningKey(pub [u8; 32]);

impl From<[u8; 32]> for PeerSigningKey {
    fn from(key: [u8; 32]) -> Self {
        Self(key)
    }
}

/// Member session id minted on the member host.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SessionId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Durable input id of a delivered turn (§18 O2 idempotency component).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct InputId(pub String);

impl<T: Into<String>> From<T> for InputId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Member runtime generation (u64-ordered; guards compare tuples).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Member runtime fence token (u64-ordered; guards compare tuples).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Mob-scoped member key (A14): the composed `(mob_id, agent_identity)` map
/// key for materialized-member and dedup rows. Composed shell-side and passed
/// as an opaque typed payload (the `ExternalPeerKey` precedent); guards read
/// its fields for binding checks.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MemberKey {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
}

impl MemberKey {
    pub fn new(mob_id: MobId, agent_identity: AgentIdentity) -> Self {
        Self {
            mob_id,
            agent_identity,
        }
    }
}

/// Turn-outcome journal key (§18.9, mob-keyed per DEC-5):
/// `(mob_id, agent_identity, generation, fence_token, input_id)`. Identities are
/// mob-scoped names, so the journal namespace must carry the mob.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct TurnKey {
    pub mob_id: MobId,
    pub agent_identity: AgentIdentity,
    pub generation: Generation,
    pub fence_token: FenceToken,
    pub input_id: InputId,
}

impl TurnKey {
    pub fn new(
        mob_id: MobId,
        agent_identity: AgentIdentity,
        generation: Generation,
        fence_token: FenceToken,
        input_id: InputId,
    ) -> Self {
        Self {
            mob_id,
            agent_identity,
            generation,
            fence_token,
            input_id,
        }
    }
}

/// Binding phase for a bound mob. Presence in the map is the bound witness;
/// the single variant keeps the fact typed rather than a bare bool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum HostBindingPhase {
    #[default]
    Bound,
}

/// Host-addressed admission reject vocabulary (§6.3 + §18.9
/// `TurnDirectiveUnsupported`; `AddressMismatch` mirrors the wire
/// `BridgeRejectionCause` bind-time class).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum HostAdmissionRejectKind {
    #[default]
    NotBound,
    StaleSupervisor,
    SenderMismatch,
    InvalidBootstrapToken,
    AddressMismatch,
    StaleFence,
    AlreadyBound,
    Unsupported,
    TurnDirectiveUnsupported,
}

/// Materialize reject vocabulary: dedup classes (NotBound / StaleFence /
/// SpecDigestMismatch) plus the §14 R7 tier-2 preflight causes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MaterializeRejectKind {
    #[default]
    NotBound,
    StaleFence,
    SpecDigestMismatch,
    ModelUnresolvable,
    AuthBindingUnresolvable,
    EnvKeysMissing,
    McpCommandMissing,
    RealmBackendUnavailable,
    MemoryStoreUnavailable,
    EngineProtocolUnsupported,
}

/// Typed member-session disposal classes (§19.L3/L4). The wire reply's
/// `AlreadyArchived` folds into `Archived` for the machine fact — both mean
/// the durable terminal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberSessionDisposal {
    #[default]
    Archived,
    RuntimeReleasedOnlyHostOwned,
    RuntimeReleasedOnlyNoDurableSessions,
}

/// Terminal classification of a tracked remote turn (§18 O2). Pinned to the
/// flow step terminality vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowTurnOutcomeKind {
    #[default]
    Completed,
    Failed,
    Canceled,
}

/// Lifecycle-bounded durable tracked-input cancellation receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TrackedInputCancelKind {
    #[default]
    NoEffect,
    Cancelling,
    Cancelled,
}
