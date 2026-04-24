use super::OptionValueExt;
use meerkat_machine_dsl::machine;

machine! {
    machine MobMachine {
        version: 1,
        rust: "self" / "catalog::dsl::mob_machine",

        state {
            lifecycle_phase: MobPhase,
            live_runtime_ids: Set<AgentRuntimeId>,
            externally_addressable_runtime_ids: Set<AgentRuntimeId>,
            runtime_fence_tokens: Map<AgentRuntimeId, FenceToken>,
            active_run_count: u64,
            pending_spawn_count: u64,
            coordinator_bound: bool,
            // Per-runtime lifecycle marker (Active vs Retiring). Tracks the
            // draining/retiring sub-state independently of the mob-level
            // lifecycle phase so the shell can decide whether to route fresh
            // work to a member while retire-drain is in flight.
            member_state_markers: Map<AgentRuntimeId, MobMemberState>,
            // Undirected wiring edges between agent identities. Stored as
            // ordered pairs (smaller identity first) wrapped in WiringEdge
            // so the DSL sees a single opaque key type.
            wiring_edges: Set<WiringEdge>,
            // Identity → current runtime binding. Survives within a
            // generation; respawn replaces the runtime id for the same
            // identity.
            identity_to_runtime: Map<AgentIdentity, AgentRuntimeId>,
            // Task board: the full MobTask payload is opaque at DSL level;
            // lifecycle fields below provide the guard-visible projection.
            tasks: Map<TaskId, MobTask>,
            // Projected status index: guards use these to reject unknown-id
            // updates and illegal status transitions (e.g. Completed→Pending).
            in_progress_task_ids: Set<TaskId>,
            completed_task_ids: Set<TaskId>,
            // W3-H / dogma #4: canonical identity→bridge-session map for
            // realtime WS observers. Key absence == Unbound; presence carries
            // the current bridge session id that identity's realtime channel
            // should pin to. Respawn updates the value atomically in the same
            // DSL transition that rebinds the runtime id.
            //
            // Track-B (R5): this map is the identity-level "member is bound to
            // *some* session" fact. Realtime WS observers were the first
            // consumer; the `RecomputeMobPeerOverlay` composition driver is
            // the second and reads the same map plus `wiring_edges` to
            // project peer endpoints onto live sessions.
            member_session_bindings: Map<AgentIdentity, SessionId>,
            // Runtime ids whose MeerkatMachine peer-ingress ownership must be
            // detached before this mob may route RequestRuntimeDestroy.
            pending_session_ingress_detach_runtime_ids: Set<AgentRuntimeId>,
            topology_epoch: u64,
        }

        init(Running) {
            live_runtime_ids = EmptySet,
            externally_addressable_runtime_ids = EmptySet,
            runtime_fence_tokens = EmptyMap,
            active_run_count = 0,
            pending_spawn_count = 0,
            coordinator_bound = true,
            member_state_markers = EmptyMap,
            wiring_edges = EmptySet,
            identity_to_runtime = EmptyMap,
            tasks = EmptyMap,
            in_progress_task_ids = EmptySet,
            completed_task_ids = EmptySet,
            member_session_bindings = EmptyMap,
            pending_session_ingress_detach_runtime_ids = EmptySet,
            topology_epoch = 0,
        }

        terminal [Destroyed]

        phase MobPhase {
            Running,
            Stopped,
            Completed,
            Destroyed,
        }

        input MobMachineInput {
            RunFlow,
            CancelFlow,
            FlowStatus,
            Spawn { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, bridge_session_id: SessionId, replacing: Option<SessionId> },
            Retire { agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, releasing: Option<SessionId>, session_id: SessionId },
            Respawn { agent_runtime_id: AgentRuntimeId },
            RetireAll,
            WireMembers { edge: WiringEdge },
            UnwireMembers { edge: WiringEdge },
            BindMemberSession { agent_identity: AgentIdentity, session_id: SessionId },
            RotateMemberSession { agent_identity: AgentIdentity, old_session_id: SessionId, new_session_id: SessionId },
            ReleaseMemberSession { agent_identity: AgentIdentity, session_id: SessionId },
            SessionIngressDetachedForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            SessionIngressDetachFailedForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId, reason: String },
            SubmitWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
            CancelWork { work_id: WorkId },
            CancelAllWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            Stop,
            Resume,
            Complete,
            Reset,
            Destroy,
            TaskCreate { task_id: TaskId, task_payload: MobTask },
            TaskUpdate { task_id: TaskId, new_status: TaskStatus },
            TaskList,
            TaskGet,
            McpServerStates,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            SubscribeAgentEvents,
            SubscribeAllAgentEvents,
            SubscribeMobEvents,
            PollEvents,
            ReplayAllEvents,
            RecordOperatorActionProvenance,
            GetMember,
            SetSpawnPolicy,
            Shutdown,
            ForceCancel,
        }

        surface_only [
            FlowStatus,
            TaskList,
            TaskGet,
            McpServerStates,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            CancelWork,
            PollEvents,
            ReplayAllEvents,
            GetMember
        ]

        signal MobMachineSignal {
            ObserveRuntimeReady { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RetireMember { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId },
            ObserveRuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ResetMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, session_id: SessionId },
            RespawnMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, session_id: SessionId },
            DestroyMob { session_id: SessionId },
            ObserveRuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            MarkCompleted,
            StartRun,
            FinishRun,
            BeginCleanup,
            FinishCleanup,
            InitializeOrchestrator,
            BindCoordinator,
            UnbindCoordinator,
            StageSpawn,
            CompleteSpawn,
            StartFlow,
            CompleteFlow,
            StopOrchestrator,
            ResumeOrchestrator,
            DestroyOrchestrator,
            ForceCancelMember,
            MemberPeerExposed,
            MemberTerminalized,
            OperationPeerTrusted,
            PeerInputAdmitted,
            CreateRun,
        }

        effect MobMachineEffect {
            RequestRuntimeBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: SessionId },
            RequestRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
            RequestRuntimeRetire { session_id: SessionId },
            RequestRuntimeDestroy { session_id: SessionId },
            EmitMemberLifecycleNotice { kind: Enum<MemberLifecycleKind> },
            EmitRunLifecycleNotice,
            EmitFlowRunNotice,
            AppendFailureLedger,
            FlowTerminalized,
            EscalateSupervisor,
            NotifyCoordinator,
            ExposePendingSpawn,
            EmitMemberTerminalNotice,
            AdmitPeerInput,
            EmitProgressNote,
            EmitTaskNotice,
            // Track-B (R5): canonical topology-change signals consumed by
            // the `RecomputeMobPeerOverlay` composition driver.
            //
            // - `WiringGraphChanged` fires when `wiring_edges` mutates.
            // - `MemberSessionBindingChanged` fires on every binding
            //   mutation (set, rotated, or released), carrying the
            //   before/after session ids as `Option<SessionId>`. Absence
            //   of a value means "no binding on this side of the
            //   transition": `old=None, new=Some` → set, `Some, Some` →
            //   rotate, `Some, None` → release.
            //
            // Both carry the post-transition `topology_epoch` so the
            // driver can linearize recomputes against the newest topology
            // snapshot.
            WiringGraphChanged { epoch: u64 },
            MemberSessionBindingChanged { epoch: u64, agent_identity: AgentIdentity, old_session_id: Option<SessionId>, new_session_id: Option<SessionId> },
            // D-wiring-observability (#27): pair-valued notice emitted from
            // `WireMembers`/`UnwireMembers` alongside `WiringGraphChanged`.
            // Unlike `WiringGraphChanged` (opaque epoch bump), this carries
            // the `WiringEdge` so external observers (event store,
            // telemetry) can reconstruct which identity pair was wired or
            // unwired. Separate from `EmitMemberLifecycleNotice` because
            // wiring is pair-valued, not per-member.
            EmitWiringLifecycleNotice { kind: Enum<WiringLifecycleKind>, edge: WiringEdge },
        }

        disposition RequestRuntimeBinding => routed [MeerkatMachine],
        disposition RequestRuntimeIngress => routed [MeerkatMachine],
        disposition RequestRuntimeRetire => routed [MeerkatMachine],
        disposition RequestRuntimeDestroy => routed [MeerkatMachine],
        disposition EmitMemberLifecycleNotice => external,
        disposition EmitRunLifecycleNotice => external,
        disposition EmitFlowRunNotice => external,
        disposition AppendFailureLedger => local,
        disposition FlowTerminalized => external,
        disposition EscalateSupervisor => external,
        disposition NotifyCoordinator => external,
        disposition ExposePendingSpawn => external,
        disposition EmitMemberTerminalNotice => external,
        disposition AdmitPeerInput => external,
        disposition EmitProgressNote => external,
        disposition EmitTaskNotice => external,
        disposition WiringGraphChanged => external,
        disposition MemberSessionBindingChanged => external,
        disposition EmitWiringLifecycleNotice => external,

        // =====================================================================
        // Invariants
        // =====================================================================

        // W3-H / dogma #4: "no zombie realtime binding" — every identity that
        // has a bound session must appear in `identity_to_runtime` (i.e. must
        // be an identity MobMachine has spawned). keys(bindings) ⊆
        // keys(identity_to_runtime). Paired with Retire's `remove` and
        // Spawn's guard/state consistency.
        invariant bindings_require_known_identity {
            for_all(id in self.member_session_bindings.keys(), self.identity_to_runtime.contains_key(id))
        }

        // =====================================================================
        // Track-B (R5): identity-level wiring mutations.
        //
        // `WireMembers`/`UnwireMembers` mutate `wiring_edges` at DSL
        // authority and bump `topology_epoch`. The `WiringGraphChanged`
        // effect lets the `RecomputeMobPeerOverlay` composition driver
        // linearize peer-overlay recomputation against graph changes.
        // =====================================================================

        transition WireMembersRunning {
            on input WireMembers { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_not_already_wired" { self.wiring_edges.contains(edge) == false }
            update {
                self.wiring_edges.insert(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Wired, edge: edge }
        }

        transition UnwireMembersRunning {
            on input UnwireMembers { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            update {
                self.wiring_edges.remove(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Unwired, edge: edge }
        }

        // =====================================================================
        // Track-B (R5): identity-level session-binding mutations.
        //
        // `BindMemberSession`/`RotateMemberSession`/`ReleaseMemberSession`
        // are the explicit-driven counterparts to the Spawn/Retire-coupled
        // binding updates. Each bumps `topology_epoch` and emits
        // `MemberSessionBindingChanged { epoch, agent_identity, old, new }`
        // so the composition driver can recompute overlay endpoints
        // keyed on the updated session.
        // =====================================================================

        transition BindMemberSessionRunning {
            on input BindMemberSession { agent_identity, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_has_runtime" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {
                self.member_session_bindings.insert(agent_identity, session_id);
                self.topology_epoch += 1;
            }
            to Running
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: None, new_session_id: Some(session_id) }
        }

        transition RotateMemberSessionRunning {
            on input RotateMemberSession { agent_identity, old_session_id, new_session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_has_runtime" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "old_session_id_matches_current" {
                self.member_session_bindings.get_cloned(agent_identity) == Some(old_session_id)
            }
            update {
                self.member_session_bindings.insert(agent_identity, new_session_id);
                self.topology_epoch += 1;
            }
            to Running
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(old_session_id), new_session_id: Some(new_session_id) }
        }

        transition ReleaseMemberSessionRunning {
            on input ReleaseMemberSession { agent_identity, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "session_id_matches_current" {
                self.member_session_bindings.get_cloned(agent_identity) == Some(session_id)
            }
            update {
                self.member_session_bindings.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(session_id), new_session_id: None }
        }

        // =====================================================================
        // Direct transitions
        // =====================================================================

        // Spawn is a Running self-loop: the real runtime starts in Running
        // and does not expose a durable pre-start top-level phase.
        //
        // W3-H: Spawn splits into Fresh (no prior realtime binding) and
        // Replacing (prior binding present, respawn-style rotation). Guards
        // check BOTH the input's `replacing` witness AND the state's
        // `member_session_bindings` key presence so the DSL enforces
        // caller/state consistency.
        transition SpawnRunningFresh {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            update {
                // Spawn is the "member joined live_runtime_ids" fact. The
                // pending_spawn_count lifecycle is owned by StageSpawn (+1)
                // and CompleteSpawn (-1) signals; Spawn itself leaves the
                // counter untouched. active_run_count is unrelated to spawn.
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: bridge_session_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: None, new_session_id: Some(bridge_session_id) }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition SpawnRunningReplacing {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "replacing_present" { replacing != None }
            update {
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: bridge_session_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(replacing.get("value")), new_session_id: Some(bridge_session_id) }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition ObserveRuntimeReady {
            on signal ObserveRuntimeReady { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }

        // SubmitWork formally requests runtime ingress on the bound member runtime.
        transition SubmitWorkRunningExternal {
            on input SubmitWork { agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "external_origin" { origin == WorkOrigin::External }
            guard "runtime_externally_addressable" { self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition SubmitWorkRunningInternal {
            on input SubmitWork { agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "internal_origin" { origin == WorkOrigin::Internal }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition RetireMember {
            on signal RetireMember { agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition ObserveRuntimeRetired {
            on signal ObserveRuntimeRetired { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ResetMember {
            on signal ResetMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Reset }
        }

        transition RespawnMember {
            on signal RespawnMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Respawned }
        }

        transition MarkCompleted {
            on signal MarkCompleted
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Completed
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Completed }
        }

        transition DestroyMob {
            on signal DestroyMob { session_id }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            guard "session_ingress_detaches_closed" { self.pending_session_ingress_detach_runtime_ids == EmptySet }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.member_state_markers = EmptyMap;
                self.pending_session_ingress_detach_runtime_ids = EmptySet;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit RequestRuntimeDestroy { session_id: session_id }
        }

        transition ObserveRuntimeDestroyed {
            on signal ObserveRuntimeDestroyed { agent_runtime_id, fence_token }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
                || self.lifecycle_phase == Phase::Destroyed
            }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.member_state_markers = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Destroyed }
        }

        // =====================================================================
        // Absorbed transitions: per-phase self-loops
        // =====================================================================

        // RecordOperatorActionProvenance: all 4 phases, no guard, no effect
        transition RecordOperatorActionProvenanceRunning {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition RecordOperatorActionProvenanceStopped {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition RecordOperatorActionProvenanceCompleted {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition RecordOperatorActionProvenanceDestroyed {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // SetSpawnPolicy: all 4 phases, no guard, no effect
        transition SetSpawnPolicyRunning {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SetSpawnPolicyStopped {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SetSpawnPolicyCompleted {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SetSpawnPolicyDestroyed {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Phase-changing transitions
        // =====================================================================

        // Stop: Running -> Stopped
        transition StopRunning {
            on input Stop
            guard { self.lifecycle_phase == Phase::Running }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        // Resume: Stopped -> Running
        transition ResumeStopped {
            on input Resume
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // Complete: Running -> Completed
        transition CompleteRunning {
            on input Complete
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Completed
            emit EmitRunLifecycleNotice
        }

        // Reset: Running|Stopped|Completed -> Running
        transition ResetToRunning {
            on input Reset
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Running self-loops (inputs)
        // =====================================================================

        // TaskCreate: real mutator. Rejects duplicate task ids via the
        // unknown-task guard's inverse (task id not already present).
        transition TaskCreateRunning {
            on input TaskCreate { task_id, task_payload }
            guard { self.lifecycle_phase == Phase::Running }
            guard "task_id_unused" { self.tasks.contains_key(task_id) == false }
            update {
                self.tasks.insert(task_id, task_payload);
            }
            to Running
            emit EmitTaskNotice
        }

        // TaskUpdate: status transition authority.
        //
        // Split into one transition per target status. Each enforces:
        //   * task_id must refer to an existing task (unknown ids rejected)
        //   * the source status is not a terminal one we disallow rolling
        //     back from (Completed → Pending / InProgress is rejected).
        //
        // The status-projection sets (`in_progress_task_ids`,
        // `completed_task_ids`) are the guard-visible truth; `tasks` carries
        // the opaque payload for shell-side projection.
        transition TaskUpdateRunningPending {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_pending" { new_status == TaskStatus::Pending }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            guard "not_completed" { self.completed_task_ids.contains(task_id) == false }
            update {
                self.in_progress_task_ids.remove(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunningInProgress {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_in_progress" { new_status == TaskStatus::InProgress }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            guard "not_completed" { self.completed_task_ids.contains(task_id) == false }
            update {
                self.in_progress_task_ids.insert(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunningCompleted {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_completed" { new_status == TaskStatus::Completed }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            update {
                self.in_progress_task_ids.remove(task_id);
                self.completed_task_ids.insert(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunningCancelled {
            on input TaskUpdate { task_id, new_status }
            guard { self.lifecycle_phase == Phase::Running }
            guard "target_cancelled" { new_status == TaskStatus::Cancelled }
            guard "task_known" { self.tasks.contains_key(task_id) == true }
            guard "not_completed" { self.completed_task_ids.contains(task_id) == false }
            update {
                self.in_progress_task_ids.remove(task_id);
            }
            to Running
            emit EmitTaskNotice
        }

        transition ForceCancelRunning {
            on input ForceCancel
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        // =====================================================================
        // Subscribe commands
        // =====================================================================

        // SubscribeAgentEvents: all 4 phases, guard: active_members_present
        transition SubscribeAgentEventsRunning {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Running
        }
        transition SubscribeAgentEventsStopped {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Stopped
        }
        transition SubscribeAgentEventsCompleted {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Completed }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Completed
        }
        transition SubscribeAgentEventsDestroyed {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Destroyed
        }

        // SubscribeAllAgentEvents: all 4 phases, no guard
        transition SubscribeAllAgentEventsRunning {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SubscribeAllAgentEventsStopped {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SubscribeAllAgentEventsCompleted {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SubscribeAllAgentEventsDestroyed {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // SubscribeMobEvents: all 4 phases, no guard
        transition SubscribeMobEventsRunning {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SubscribeMobEventsStopped {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SubscribeMobEventsCompleted {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SubscribeMobEventsDestroyed {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Shutdown: from any non-Destroyed state
        // =====================================================================

        transition ShutdownRunning {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ShutdownStopped {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ShutdownCompleted {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Completed }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Completed
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Signal-driven Running self-loops
        // =====================================================================

        transition CancelFlowRunning {
            on input CancelFlow
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        transition InitializeOrchestratorRunning {
            on signal InitializeOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit NotifyCoordinator
        }

        transition BindCoordinatorRunning {
            on signal BindCoordinator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit NotifyCoordinator
        }

        transition UnbindCoordinatorRunning {
            on signal UnbindCoordinator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = false;
            }
            to Running
            emit NotifyCoordinator
        }

        transition StageSpawnRunning {
            on signal StageSpawn
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.pending_spawn_count += 1;
            }
            to Running
            emit ExposePendingSpawn
        }

        transition StopOrchestratorRunning {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = false; }
            to Running
            emit NotifyCoordinator
        }
        transition StopOrchestratorStopped {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = false; }
            to Stopped
            emit NotifyCoordinator
        }
        transition StopOrchestratorCompleted {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = false; }
            to Completed
            emit NotifyCoordinator
        }

        transition ResumeOrchestratorRunning {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = true; }
            to Running
            emit NotifyCoordinator
        }
        transition ResumeOrchestratorStopped {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = true; }
            to Stopped
            emit NotifyCoordinator
        }
        transition ResumeOrchestratorCompleted {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = true; }
            to Completed
            emit NotifyCoordinator
        }

        transition DestroyOrchestratorRunning {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = false; }
            to Running
            emit NotifyCoordinator
        }
        transition DestroyOrchestratorStopped {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = false; }
            to Stopped
            emit NotifyCoordinator
        }
        transition DestroyOrchestratorCompleted {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = false; }
            to Completed
            emit NotifyCoordinator
        }

        transition ForceCancelMemberRunning {
            on signal ForceCancelMember
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberTerminalNotice
        }

        transition MemberPeerExposedRunning {
            on signal MemberPeerExposed
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        transition MemberTerminalizedRunning {
            on signal MemberTerminalized
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberTerminalNotice
        }

        transition OperationPeerTrustedRunning {
            on signal OperationPeerTrusted
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        transition PeerInputAdmittedRunning {
            on signal PeerInputAdmitted
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        // =====================================================================
        // BeginCleanup / FinishCleanup
        // =====================================================================

        transition BeginCleanupStopped {
            on signal BeginCleanup
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition BeginCleanupCompleted {
            on signal BeginCleanup
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition FinishCleanupStopped {
            on signal FinishCleanup
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition FinishCleanupCompleted {
            on signal FinishCleanup
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // RunFlow / StartFlow / CreateRun / StartRun
        // =====================================================================

        transition RunFlowRunning {
            on input RunFlow
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition StartFlowRunning {
            on signal StartFlow
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition CreateRunRunning {
            on signal CreateRun
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition StartRunRunning {
            on signal StartRun
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // CompleteFlow / FinishRun
        // =====================================================================
        //
        // The *Zero transitions accept `active_run_count == 0` as a
        // legitimate terminal convergence (see `cancel_all_flow_tasks`
        // in meerkat-mob::runtime::actor for why both destroy-driven
        // cancel and natural FlowFinished cleanup can race each other
        // to the same terminal state). Modeling convergence in the
        // authority keeps signal semantics machine-owned.

        transition CompleteFlowRunning {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count -= 1;
            }
            to Running
            emit FlowTerminalized
        }

        transition CompleteFlowRunningZero {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Running
            emit NotifyCoordinator
        }

        transition FinishRunRunning {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count -= 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition FinishRunRunningZero {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Running
            emit NotifyCoordinator
        }

        // =====================================================================
        // Retire / RetireAll
        // =====================================================================

        transition RetireRunningReleasing {
            on input Retire { agent_runtime_id, agent_identity, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_present" { releasing != None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_session_bindings.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
        }

        transition RetireRunningPreservingBinding {
            on input Retire { agent_runtime_id, agent_identity, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireRunningNoBinding {
            on input Retire { agent_runtime_id, agent_identity, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireStoppedReleasing {
            on input Retire { agent_runtime_id, agent_identity, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_present" { releasing != None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_session_bindings.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                self.topology_epoch += 1;
            }
            to Stopped
            emit RequestRuntimeRetire { session_id: session_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
        }

        transition SessionIngressDetachedForMobDestroyRunning {
            on input SessionIngressDetachedForMobDestroy { mob_id, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "mob_id_present" { mob_id == mob_id }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
            }
            to Running
        }

        transition SessionIngressDetachedForMobDestroyStopped {
            on input SessionIngressDetachedForMobDestroy { mob_id, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "mob_id_present" { mob_id == mob_id }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
            }
            to Stopped
        }

        transition SessionIngressDetachFailedForMobDestroyRunning {
            on input SessionIngressDetachFailedForMobDestroy { mob_id, agent_runtime_id, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "mob_id_present" { mob_id == mob_id }
            guard "reason_present" { reason == reason }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
        }

        transition SessionIngressDetachFailedForMobDestroyStopped {
            on input SessionIngressDetachFailedForMobDestroy { mob_id, agent_runtime_id, reason }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "mob_id_present" { mob_id == mob_id }
            guard "reason_present" { reason == reason }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {}
            to Stopped
        }

        transition RetireStoppedPreservingBinding {
            on input Retire { agent_runtime_id, agent_identity, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireStoppedNoBinding {
            on input Retire { agent_runtime_id, agent_identity, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireAllRunning {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
        }

        transition RetireAllStopped {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
        }

        // =====================================================================
        // CompleteSpawn
        // =====================================================================

        transition CompleteSpawnRunning {
            on signal CompleteSpawn
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "pending_spawns_present" { self.pending_spawn_count > 0 }
            update {
                self.pending_spawn_count -= 1;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        // =====================================================================
        // Destroy (input)
        // =====================================================================

        transition DestroyFromAny {
            on input Destroy
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            guard "session_ingress_detaches_closed" { self.pending_session_ingress_detach_runtime_ids == EmptySet }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.pending_session_ingress_detach_runtime_ids = EmptySet;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
        }

        // =====================================================================
        // Respawn (input, Running self-loop)
        // =====================================================================

        transition RespawnRunning {
            on input Respawn { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {}
            to Running
            emit ExposePendingSpawn
        }

        // =====================================================================
        // CancelAllWork
        // =====================================================================

        transition CancelAllWorkRunning {
            on input CancelAllWork { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

    }
}

// ---------------------------------------------------------------------------
// Stub types for compilation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentIdentity(pub String);
impl<T: Into<String>> From<T> for AgentIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentRuntimeId(pub String);
impl<T: Into<String>> From<T> for AgentRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MobId(pub String);
impl<T: Into<String>> From<T> for MobId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FenceToken(pub u64);
impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);
impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub String);
impl<T: Into<String>> From<T> for WorkId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub String);
impl<T: Into<String>> From<T> for TaskId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for bridge session id. Maps to
/// `meerkat_core::session::SessionId` — the bridge session a mob member is
/// attached to for the current runtime generation.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub String);
impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Task lifecycle status. DSL guards enumerate these directly
/// (`TaskStatus::Pending`, `TaskStatus::InProgress`,
/// `TaskStatus::Completed`, `TaskStatus::Cancelled`). `Completed` and
/// `Cancelled` are the two terminal statuses; neither may be transitioned
/// away from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

/// Opaque task payload carried through the DSL. The full domain type is
/// richer than what the DSL models; only `tasks.contains(id)` is observed in
/// guards. Field projection lives in shell code consuming the DSL state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MobTask {
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub owner: Option<AgentIdentity>,
    pub blocked_by: Vec<TaskId>,
}

/// Per-runtime lifecycle marker tracking whether a member is actively serving
/// work or draining toward retirement. Opaque to DSL guards — observed only
/// at the shell layer for work-routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobMemberState {
    #[default]
    Active,
    Retiring,
}

/// Typed work-origin classification. Closed mirror of the real
/// `meerkat-mob::ids::WorkOrigin` enum — the DSL uses this for guard-visible
/// truth on `SubmitWork` and `RequestRuntimeIngress`. The `Ingest` variant
/// exists only on the receiving side of the admission seam (the runtime
/// control-plane dispatch); mob-originated work only uses
/// `External`/`Internal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WorkOrigin {
    #[default]
    External,
    Internal,
    Ingest,
}

/// Typed member lifecycle notice kind for
/// `MobMachineEffect::EmitMemberLifecycleNotice`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberLifecycleKind {
    #[default]
    Spawned,
    Retiring,
    Retired,
    Reset,
    Respawned,
    Completed,
    Destroyed,
}

/// Typed wiring lifecycle notice kind for
/// `MobMachineEffect::EmitWiringLifecycleNotice`. Pair-valued (edge-keyed)
/// counterpart to `MemberLifecycleKind` (member-keyed). Emitted alongside
/// `WiringGraphChanged` by `WireMembers`/`UnwireMembers` transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WiringLifecycleKind {
    #[default]
    Wired,
    Unwired,
}

/// Undirected wiring edge between two identities. Callers MUST normalize
/// to `(smaller, larger)` before constructing so that edge equality is
/// independent of insertion order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WiringEdge {
    pub a: AgentIdentity,
    pub b: AgentIdentity,
}

impl WiringEdge {
    /// Constructs an edge, normalizing so `a <= b`.
    pub fn new(lhs: AgentIdentity, rhs: AgentIdentity) -> Self {
        if lhs <= rhs {
            Self { a: lhs, b: rhs }
        } else {
            Self { a: rhs, b: lhs }
        }
    }
}
