//! The kickoff a seated member's incarnation owes, started once the member
//! is live.
//!
//! MobMachine records the kickoff a member owes (`member_kickoff_pending`):
//! the rotation's terminal step records it on the next incarnation, which it
//! seats Dormant, and a restart recovers it from the journal. The owed
//! kickoff makes the member's boot verdict Wake, so the machine's own wake
//! materializes it; nothing here wakes a member. Each time the actor turns
//! to wait for its next command (the first time included, after the startup
//! readiness pass settled the members a start materialized), this starts
//! the kickoff of each member that owes one and is live: the kickoff is
//! marked starting (record before send), its input is admitted on the
//! member's session under the incarnation's kickoff key, and its outcome
//! resolves the kickoff as a spawn's kickoff outcome does. The key makes the
//! admission idempotent, so the kickoff runs exactly once on the
//! incarnation's session.
//!
//! A kickoff a crash left starting (its starting row was journaled, then its
//! admission, its run or its waiter was lost) is owed too, in both runtime
//! modes, and the machine's boot verdict wakes its member for it as well. It
//! is not marked again: its input is admitted under the same key, which
//! re-attaches to the input the runtime still holds (in flight, or terminal
//! with its receipt) and admits it only when there is none.

use super::*;

impl MobActor {
    /// Start the kickoff of every member that owes one and is live (see the
    /// module docs): a kickoff still pending, and one a crash left starting
    /// with no waiter (its journal row was written, its admission or its
    /// waiter was lost). A member whose kickoff cannot start yet (a stopped
    /// or quiescing mob, a member still waking, retiring or spawning) keeps
    /// it owed for a later turn of the loop.
    #[cfg(feature = "runtime-adapter")]
    pub(super) async fn start_owed_kickoffs(&mut self) {
        let candidates = {
            let state = self.dsl_authority.state();
            if (state.member_kickoff_pending.is_empty() && state.member_kickoff_starting.is_empty())
                || state.lifecycle_phase != mob_dsl::MobPhase::Running
                || state.placed_completion_lifecycle_quiescing
                || self.admission_closed.is_some()
            {
                return;
            }
            state
                .member_kickoff_pending
                .iter()
                .chain(state.member_kickoff_starting.iter())
                .filter(|member| {
                    matches!(
                        state.member_materialization.get(*member),
                        Some(mob_dsl::MemberMaterializationPhase::Live)
                    )
                })
                .map(|member| AgentIdentity::from(member.0.as_str()))
                .collect::<Vec<_>>()
        };
        if candidates.is_empty() {
            return;
        }
        // A kickoff whose waiter runs is in hand: its outcome resolves it.
        let owed = {
            let waiters = self.autonomous_initial_turns.lock().await;
            candidates
                .into_iter()
                .filter(|identity| !waiters.contains_key(identity))
                .collect::<Vec<_>>()
        };
        for identity in owed {
            if let Err(error) = Box::pin(self.start_owed_kickoff(&identity)).await {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %identity,
                    %error,
                    "the member's owed kickoff did not start; it stays owed"
                );
            }
        }
    }

    /// Start one live member's owed kickoff, or leave it owed when another
    /// lane holds the member now: a placed member's kickoff rides its host's
    /// lane, a retiring member's kickoff is cancelled by its retirement, a
    /// spawn activation (or its cleanup) in flight owns its own kickoff, and
    /// a host that holds the member's run starts re-arms it at its release.
    ///
    /// A pending kickoff is marked starting first (record before send). A
    /// starting one (a crash came between that row and the admission, or
    /// during the kickoff's run) is not marked again: its input is admitted
    /// under the same incarnation key, which re-attaches to the input the
    /// runtime already holds (in flight, or terminal with its receipt) and
    /// admits it only when there is none. Either way it runs once.
    #[cfg(feature = "runtime-adapter")]
    async fn start_owed_kickoff(&mut self, identity: &AgentIdentity) -> Result<(), MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let pending = self
            .dsl_authority
            .state()
            .member_kickoff_pending
            .contains(&dsl_identity);
        if super::super::member_runtime_is_host_owned(self.dsl_authority.state(), identity)
            || self.retirements.contains_key(identity)
            || !self.spawn_cleanup_settled_for(identity)
            || self
                .autonomous_initial_turns
                .lock()
                .await
                .contains_key(identity)
        {
            return Ok(());
        }
        let Some(adapter) = self.runtime_adapter.clone() else {
            return Ok(());
        };
        let (entry, prompt) = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(identity).cloned() else {
                return Ok(());
            };
            let prompt = roster.owed_kickoff_prompt(identity).cloned();
            (entry, prompt)
        };
        let session_id = entry
            .member_ref
            .bridge_session_id()
            .cloned()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "the member '{identity}' that owes a kickoff is not session-backed"
                ))
            })?;
        // The re-attach after a hold is the hold's release's (as for a
        // kickoff a Shutdown kept): an admission now would be live work the
        // hold's Shutdown could not keep.
        if super::super::stop_report::HostRunStartHoldReason::ALL
            .into_iter()
            .any(|reason| adapter.registration_run_start_hold_staged(&session_id, reason.runtime()))
        {
            return Ok(());
        }
        let prompt = prompt.unwrap_or_else(|| {
            ContentInput::from(self.fallback_spawn_prompt(&entry.role, identity))
        });
        if pending {
            // Record before send: the kickoff is starting before its input
            // is admitted, so a restart never admits it a second time.
            self.apply_kickoff_input(
                identity,
                mob_dsl::MobMachineInput::KickoffMarkStarting {
                    member_id: dsl_identity,
                },
                "owed_kickoff_mark_starting",
            )
            .await?;
        }
        let completion = match machine_kickoff_turn_metadata(self.dsl_authority.state(), identity) {
            Ok(turn_metadata) => {
                let input = owed_kickoff_input(&entry.agent_runtime_id, prompt, turn_metadata);
                match adapter
                    .accept_input_with_completion(&session_id, input)
                    .await
                {
                    Ok((outcome, handle)) => {
                        KickoffCompletion::of_admission(&adapter, &session_id, &outcome, handle)
                    }
                    Err(error) => Some(KickoffCompletion::Refused(error.to_string())),
                }
            }
            Err(error) => Some(KickoffCompletion::Refused(error.to_string())),
        };
        // The waiter resolves the kickoff from its outcome, a refused
        // admission included, exactly as a spawn's kickoff waiter does.
        let waiter =
            spawn_kickoff_outcome_waiter(self.command_tx.clone(), identity.clone(), completion);
        self.autonomous_initial_turns
            .lock()
            .await
            .insert(identity.clone(), waiter);
        tracing::debug!(
            mob_id = %self.definition.id,
            agent_identity = %identity,
            resumed = !pending,
            "started the member's owed kickoff"
        );
        Ok(())
    }
}

/// The owed kickoff's input: the kickoff prompt under the incarnation's
/// kickoff key and the machine-owned kickoff turn metadata, as a spawn's
/// kickoff input carries them.
#[cfg(feature = "runtime-adapter")]
fn owed_kickoff_input(
    agent_runtime_id: &AgentRuntimeId,
    content: ContentInput,
    turn_metadata: Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
) -> meerkat_runtime::Input {
    use meerkat_runtime::{Input, InputHeader, PromptInput};

    Input::Prompt(PromptInput {
        injected_context: Vec::new(),
        header: InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::InputOrigin::Operator,
            durability: meerkat_runtime::InputDurability::Durable,
            visibility: meerkat_runtime::InputVisibility::default(),
            idempotency_key: Some(local_kickoff_idempotency_key(agent_runtime_id)),
            supersession_key: None,
            correlation_id: None,
        },
        content,
        typed_turn_appends: Vec::new(),
        turn_metadata,
    })
}
