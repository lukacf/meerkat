use super::OptionValueExt;
use meerkat_machine_dsl::machine;

machine! {
    machine MeerkatMachine {
        version: 1,
        rust: "self" / "catalog::dsl::meerkat_machine",

        state {
            lifecycle_phase: MeerkatPhase,
            session_id: Option<SessionId>,
            active_runtime_id: Option<AgentRuntimeId>,
            active_fence_token: Option<FenceToken>,
            current_run_id: Option<RunId>,
            pre_run_phase: Option<String>,
            silent_intent_overrides: Set<String>,
            // Realtime-attachment authority state — per-session binding-state
            // machine with monotonic authority epochs for provider-callback
            // validation. See docs/architecture/realtime-259-port-plan.md.
            realtime_intent_present: bool,
            realtime_binding_state: Enum<RealtimeBindingState>,
            realtime_binding_authority_epoch: Option<u64>,
            realtime_reattach_required: bool,
            realtime_next_authority_epoch: u64,
            // Live-topology reconfigure phase — temporarily blocks realtime
            // publishes/attaches while an LLM-identity swap is in progress.
            live_topology_phase: Enum<LiveTopologyPhase>,
            // MCP server connection authority. Keyed on the configured server
            // name (McpServerId). The runtime reads this map to drive the
            // deterministic [MCP_PENDING] system-notice toggle and to gate
            // tool availability as servers complete their handshakes.
            mcp_server_states: Map<McpServerId, Enum<McpServerState>>,

            // --- Peer interaction lifecycle (W1-A / issue #264) ---
            //
            // Outbound request lifecycle: the shell inserts on send, advances
            // on progress/terminal arrival, and removes on terminal so
            // projection consumers (subscriber / stream registries) drop
            // associated channels deterministically via the
            // `PeerInteractionCleanup` effect.
            pending_peer_requests: Map<PeerCorrelationId, Enum<OutboundPeerRequestState>>,
            // Inbound mirror for requests we owe a reply to.
            inbound_peer_requests: Map<PeerCorrelationId, Enum<InboundPeerRequestState>>,

            // --- Session-context advancement (W2-E / issue #264) ---
            //
            // Monotonic watermark in milliseconds of the last canonical
            // session-context mutation the shell reported via
            // `AdvanceSessionContext`. Advancing transitions emit
            // `SessionContextAdvanced` so the realtime projection consumer
            // refreshes from a typed effect instead of polling a watch
            // channel.
            last_session_context_updated_at_ms: u64,

            // --- Interaction stream lifecycle (U6 / dogma #5) ---
            //
            // Shell-side `interaction_stream_registry` becomes a pure
            // projection of channels keyed on these sets. Reservation TTL is
            // shell-owned mechanics; entry into / exit from each lifecycle
            // state is DSL truth. Lifecycle is encoded as two disjoint sets
            // (matching the same pattern used elsewhere in this DSL for
            // tagged-union state without map value comparison) — Reserved
            // sits in `reserved_interaction_streams`, Attached in
            // `attached_interaction_streams`, terminal states leave both
            // sets and emit `InteractionStreamCleanup`.
            reserved_interaction_streams: Set<PeerCorrelationId>,
            attached_interaction_streams: Set<PeerCorrelationId>,

            // --- Realtime product-turn lifecycle (U9 / dogma #4) ---
            //
            // Closed-phase encoding of the product-session turn lifecycle:
            // `Idle` → `AwaitingProgress` → {`Committed`, `OutputStarted`}
            // → `Preemptible` → `Idle`. Replaces the shell-local boolean
            // triple (`product_turn_in_flight`, `product_turn_committed`,
            // `product_output_started`) in the realtime-WS dispatcher.
            realtime_product_turn_phase: Enum<RealtimeProductTurnPhase>,

            // --- Realtime projection freshness (dogma round 2, U-C / dogma #1, #3, #13, #20) ---
            //
            // Closed-set encoding of the realtime provider session's
            // projection-freshness relative to canonical session truth.
            // Replaces the shell-local `ProjectionFreshness` enum +
            // observer queue in `meerkat-rpc::realtime_ws`. The shell
            // fires `RealtimeProjectionAdvanceObserved` /
            // `RealtimeProjectionRefreshed` / `RealtimeProjectionReset`
            // inputs; the DSL decides the resulting freshness.
            realtime_projection_freshness: Enum<RealtimeProjectionFreshness>,
            realtime_projection_frontier_ms: u64,

            // --- Realtime reconnect policy (dogma round 2, U-C / dogma #1, #3, #18, #20) ---
            //
            // Typed classification of what a clean provider-session close
            // means for the realtime channel's reconnect behavior. Replaces
            // the shell-local boolean pair (`client_has_submitted_input`,
            // `last_turn_terminally_completed`).
            realtime_reconnect_policy: Enum<RealtimeReconnectPolicy>,

            // --- Peer-ingress transport capability ownership (W2-G / issue #264) ---
            //
            // Tracks which subsystem owns the peer-ingress transport capability
            // for this session. `Unattached` is the initial state; session
            // standalone paths move to `SessionOwned`; mob provisioners move to
            // `MobOwned` (promotion from `SessionOwned` is allowed, but silent
            // downgrade from `MobOwned` → `SessionOwned` is rejected by the
            // `AttachSessionIngress` guard — the s71 regression class closed
            // structurally). `peer_ingress_comms_runtime_id` and
            // `peer_ingress_mob_id` are populated iff the kind variant names
            // them; the `peer_ingress_owner_consistency` invariant enforces
            // pairing.
            peer_ingress_owner_kind: Enum<PeerIngressOwnerKind>,
            peer_ingress_comms_runtime_id: Option<CommsRuntimeId>,
            peer_ingress_mob_id: Option<MobId>,

            // --- Supervisor-bridge authorization (Wave 3 D Row 21) ---
            //
            // Canonical authorization fact for the supervisor-bridge
            // command surface. Previously lived as an
            // `Option<AuthorizedSupervisorState>` on the comms drain
            // task's stack; the companion trust edge was router-owned,
            // so the authorization discriminant had split ownership.
            // The DSL now owns both the kind and the canonical binding
            // (`peer_id` + `name` + `address` + `epoch`); the trust edge
            // in the router stays in lock-step via shell-side
            // `add_trusted_peer` / `remove_trusted_peer` calls that only
            // run after the DSL mutator accepts the matching
            // `BindSupervisor` / `AuthorizeSupervisor` / `RevokeSupervisor`
            // transition. The `supervisor_binding_consistency` invariant
            // enforces that the companion fields are populated exactly
            // when `supervisor_binding_kind == Bound`.
            supervisor_binding_kind: Enum<SupervisorBindingKind>,
            supervisor_bound_name: Option<String>,
            supervisor_bound_peer_id: Option<String>,
            supervisor_bound_address: Option<String>,
            supervisor_bound_epoch: Option<u64>,

            // --- Track-B (R5): peer-projection state ---
            //
            // The identity-level wiring graph owned by `MobMachine` is
            // projected onto endpoint-level peer sets here. This session
            // sees its own "local" endpoint (what it publishes as its
            // address to the comms layer), a set of "direct" peer
            // endpoints (non-mob peering — e.g. standalone product
            // sessions wiring to each other), and a "mob overlay" set
            // computed by the `RecomputeMobPeerOverlay` composition
            // driver from `wiring_edges` + `member_session_bindings`.
            //
            // The effective trust set is the union `direct ∪ overlay`.
            // It is NOT stored separately here — `effective` is a pure
            // read-side projection (dogma #11: "derived projections
            // never authoritative"). The comms reconciliation handler
            // consumes `CommsTrustReconcileRequested` and reads the
            // snapshot to compute the effective set on receipt.
            //
            // Two independent epoch namespaces:
            //
            // * `peer_projection_epoch` advances on every Track-B
            //   peer-projection mutation (direct add/remove + overlay
            //   apply). Carried in `PeerProjectionChanged` /
            //   `CommsTrustReconcileRequested` so the comms
            //   reconciliation handler can linearize trust-store
            //   reconciles.
            //
            // * `mob_overlay_epoch` is the overlay-specific watermark
            //   the `stale_overlay_epoch` guard reads. The
            //   `RecomputeMobPeerOverlay` driver supplies the overlay
            //   epoch; it threads the driver's monotonically
            //   increasing per-overlay counter (the driver in turn
            //   threads `MobMachine.topology_epoch` through that
            //   counter). Keeping this namespace separate from
            //   `peer_projection_epoch` prevents direct-endpoint
            //   mutations (which legitimately bump
            //   `peer_projection_epoch`) from accidentally racing
            //   ahead of the overlay guard and locking out future
            //   overlay dispatches.
            local_endpoint: Option<PeerEndpoint>,
            direct_peer_endpoints: Set<PeerEndpoint>,
            mob_overlay_peer_endpoints: Set<PeerEndpoint>,
            peer_projection_epoch: u64,
            mob_overlay_epoch: u64,
        }

        init(Initializing) {
            session_id = None,
            active_runtime_id = None,
            active_fence_token = None,
            current_run_id = None,
            pre_run_phase = None,
            silent_intent_overrides = EmptySet,
            realtime_intent_present = false,
            realtime_binding_state = RealtimeBindingState::Unbound,
            realtime_binding_authority_epoch = None,
            realtime_reattach_required = false,
            realtime_next_authority_epoch = 1,
            live_topology_phase = LiveTopologyPhase::Idle,
            mcp_server_states = EmptyMap,
            pending_peer_requests = EmptyMap,
            inbound_peer_requests = EmptyMap,
            last_session_context_updated_at_ms = 0,
            reserved_interaction_streams = EmptySet,
            attached_interaction_streams = EmptySet,
            realtime_product_turn_phase = RealtimeProductTurnPhase::Idle,
            realtime_projection_freshness = RealtimeProjectionFreshness::Clean,
            realtime_projection_frontier_ms = 0,
            realtime_reconnect_policy = RealtimeReconnectPolicy::CleanExit,
            peer_ingress_owner_kind = PeerIngressOwnerKind::Unattached,
            peer_ingress_comms_runtime_id = None,
            peer_ingress_mob_id = None,
            supervisor_binding_kind = SupervisorBindingKind::Unbound,
            supervisor_bound_name = None,
            supervisor_bound_peer_id = None,
            supervisor_bound_address = None,
            supervisor_bound_epoch = None,

            // Track-B (R5): peer-projection state — initialised empty.
            local_endpoint = None,
            direct_peer_endpoints = EmptySet,
            mob_overlay_peer_endpoints = EmptySet,
            peer_projection_epoch = 0,
            mob_overlay_epoch = 0,
        }

        terminal [Destroyed]

        phase MeerkatPhase {
            Initializing,
            Idle,
            Attached,
            Running,
            Retired,
            Stopped,
            Destroyed,
        }

        input MeerkatMachineInput {
            // Direct inputs
            RegisterSession { session_id: SessionId },
            UnregisterSession { session_id: SessionId },
            ReconfigureSessionLlmIdentity {
                previous_identity: SessionLlmIdentity,
                previous_visibility_state: SessionToolVisibilityState,
                previous_capability_surface: Option<SessionLlmCapabilitySurface>,
                previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
                target_identity: SessionLlmIdentity,
                target_capability_surface: SessionLlmCapabilitySurface,
                next_visibility_state: SessionToolVisibilityState,
                next_capability_base_filter: ToolFilter,
                next_active_visibility_revision: u64,
                tool_visibility_delta: SessionToolVisibilityDelta,
            },
            PrepareBindings { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            SetPeerIngressContext { keep_alive: bool },
            NotifyDrainExited { reason: String },
            InterruptCurrentRun,
            CancelAfterBoundary,
            StagePersistentFilter { filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness> },
            RequestDeferredTools { names: Set<String>, witnesses: Map<String, ToolVisibilityWitness> },
            PublishCommittedVisibleSet {
                active_filter: ToolFilter,
                staged_filter: ToolFilter,
                active_requested_deferred_names: Set<String>,
                staged_requested_deferred_names: Set<String>,
                active_visibility_revision: u64,
                staged_visibility_revision: u64,
            },
            Recover,
            Retire,
            Reset,
            StopRuntimeExecutor,
            Destroy,
            // Absorbed inputs
            EnsureSessionWithExecutor { session_id: SessionId },
            SetSilentIntents { session_id: SessionId, intents: Set<String> },
            ContainsSession { session_id: SessionId },
            SessionHasExecutor { session_id: SessionId },
            SessionHasComms { session_id: SessionId },
            OpsLifecycleRegistry { session_id: SessionId },
            InputState { session_id: SessionId, input_id: InputId },
            ListActiveInputs { session_id: SessionId },
            Abort { session_id: SessionId },
            AbortAll,
            Wait { session_id: SessionId },
            Ingest { runtime_id: AgentRuntimeId, work_id: WorkId, origin: Enum<WorkOrigin> },
            PublishEvent { kind: String },
            RuntimeState { runtime_id: String },
            RuntimeRealtimeAttachmentStatus { session_id: SessionId },
            LoadBoundaryReceipt { runtime_id: String, sequence: u64 },
            AcceptWithCompletion { input_id: InputId, request_immediate_processing: bool, interrupt_yielding: bool, wake_if_idle: bool, run_id: RunId },
            AcceptWithoutWake { input_id: InputId },
            Prepare { session_id: SessionId, run_id: RunId },
            Commit { input_id: InputId, run_id: RunId },
            Fail { run_id: RunId },
            Recycle,
            // Realtime-attachment inputs.
            ProjectRealtimeIntent { present: bool },
            BeginRealtimeBinding,
            ReplaceRealtimeBinding,
            DetachRealtimeBinding,
            RequireRealtimeReattach,
            PublishRealtimeSignal { authority_epoch: u64, next_binding_state: Enum<RealtimeBindingState> },
            // Ops-barrier satisfaction feedback input (wired to the
            // `ops_barrier_satisfaction` handoff protocol on the compat
            // `mob_bundle` composition). Carries the exact operation ids
            // whose completion released the outstanding barrier so the
            // turn-state authority can observe barrier closure.
            OpsBarrierSatisfied { operation_ids: Set<OperationId> },
            // MCP server lifecycle inputs. The shell translates MCP connection
            // events into these inputs so the DSL owns authoritative per-server
            // state; tool availability and the `[MCP_PENDING]` notice are then
            // pure reads off `self.mcp_server_states`.
            McpServerConnectPending { server_id: McpServerId },
            McpServerConnected { server_id: McpServerId },
            McpServerFailed { server_id: McpServerId, error: String },
            McpServerDisconnected { server_id: McpServerId },
            McpServerReload { server_id: McpServerId },
            // Peer interaction lifecycle inputs (W1-A). Fire on outbound send,
            // response arrival (progress or terminal), timeout, inbound
            // request arrival, and inbound reply completion.
            PeerRequestSent { corr_id: PeerCorrelationId, to: String },
            PeerResponseProgressArrived { corr_id: PeerCorrelationId },
            PeerResponseTerminalArrived { corr_id: PeerCorrelationId, disposition: Enum<PeerTerminalDisposition> },
            PeerRequestTimedOut { corr_id: PeerCorrelationId },
            PeerRequestReceived { corr_id: PeerCorrelationId },
            PeerResponseReplied { corr_id: PeerCorrelationId },
            // Session-context advancement input (W2-E / issue #264).
            AdvanceSessionContext { updated_at_ms: u64 },
            // Interaction stream lifecycle inputs (U6 / dogma #5).
            InteractionStreamReserved { corr_id: PeerCorrelationId },
            InteractionStreamAttached { corr_id: PeerCorrelationId },
            InteractionStreamCompleted { corr_id: PeerCorrelationId },
            InteractionStreamExpired { corr_id: PeerCorrelationId },
            InteractionStreamClosedEarly { corr_id: PeerCorrelationId },
            // Realtime product-turn lifecycle inputs (U9 / dogma #4). Fired
            // from the realtime-WS dispatcher on every observed provider-
            // session event; idempotent transitions are guard-rejected.
            ProductTurnInFlight,
            ProductTurnCommitted,
            ProductOutputStarted,
            ProductTurnInterrupted,
            ProductTurnTerminal,
            // Realtime projection freshness inputs (dogma round 2, U-C /
            // dogma #1, #3, #13, #20).
            RealtimeProjectionAdvanceObserved { advanced_at_ms: u64 },
            RealtimeProjectionRefreshed { observed_ms: u64 },
            RealtimeProjectionReset { baseline_ms: u64 },
            // Realtime reconnect-policy inputs (dogma round 2, U-C /
            // dogma #1, #3, #18, #20).
            ClassifyRealtimeClientInputSubmitted,
            ClassifyRealtimeMidTurnActivity,
            ClassifyRealtimeTurnTerminated,
            // Live-topology reconfigure inputs.
            BeginLiveTopologyReconfigure { authority_epoch: u64 },
            MarkLiveTopologyDetached,
            ApplyLiveTopologyIdentity,
            ApplyLiveTopologyVisibility,
            CompleteLiveTopology,
            AbortLiveTopologyBeforeDetach,
            FailLiveTopologyAfterDetach,
            // Peer-ingress transport capability ownership (W2-G).
            //
            // `AttachSessionIngress` only succeeds from `Unattached`:
            // transitioning from `MobOwned` back to `SessionOwned` would be a
            // silent transport downgrade and is rejected structurally.
            // `AttachMobIngress` allows promotion from `Unattached` or
            // `SessionOwned` (mob provisioning takes over a session-attached
            // drain). `DetachIngress` clears any active ownership.
            AttachSessionIngress { comms_runtime_id: CommsRuntimeId },
            AttachMobIngress { comms_runtime_id: CommsRuntimeId, mob_id: MobId },
            DetachIngress,
            // Supervisor-bridge authorization (Wave 3 D Row 21).
            //
            // `BindSupervisor` establishes the initial binding from
            // `Unbound`. `AuthorizeSupervisor` rotates an already-`Bound`
            // binding — the shell enforces the "new supervisor must be
            // authorized by the current supervisor" gate via
            // sender-authentication on the incoming request before
            // firing this input. `RevokeSupervisor` tears the binding
            // down and returns to `Unbound`; the `epoch` and `peer_id`
            // must match the current binding so a stale revoke cannot
            // clear a rotated binding.
            BindSupervisor {
                name: String,
                peer_id: String,
                address: String,
                epoch: u64,
            },
            AuthorizeSupervisor {
                name: String,
                peer_id: String,
                address: String,
                epoch: u64,
            },
            RevokeSupervisor {
                peer_id: String,
                epoch: u64,
            },

            // =====================================================================
            // Track-B (R5): peer-projection inputs.
            //
            // Drive the MeerkatMachine's endpoint-level peer sets. The
            // `RecomputeMobPeerOverlay` composition driver emits
            // `ApplyMobPeerOverlay` to each bound session based on the
            // identity-level wiring graph owned by MobMachine; direct
            // peering surfaces emit `PublishLocalEndpoint` /
            // `AddDirectPeerEndpoint` / `RemoveDirectPeerEndpoint` when
            // they establish or tear down non-mob peering.
            //
            // - `PublishLocalEndpoint { endpoint }` / `ClearLocalEndpoint`:
            //   session self-endpoint publication. `Clear` guarded on
            //   `local_endpoint != None`.
            // - `AddDirectPeerEndpoint { endpoint }` / `RemoveDirectPeerEndpoint { endpoint }`:
            //   non-mob peering. Add guarded on not-already-direct;
            //   Remove guarded on present-in-direct. Both bump
            //   `peer_projection_epoch` and emit
            //   `CommsTrustReconcileRequested`.
            // - `ApplyMobPeerOverlay { epoch, endpoints }`: replaces
            //   `mob_overlay_peer_endpoints` wholesale and sets
            //   `peer_projection_epoch = epoch`. Guarded by
            //   `stale_overlay_epoch` — rejects when
            //   `epoch < self.peer_projection_epoch` so out-of-order
            //   overlay applications cannot clobber a newer topology.
            // =====================================================================
            PublishLocalEndpoint {
                endpoint: PeerEndpoint,
            },
            ClearLocalEndpoint,
            AddDirectPeerEndpoint {
                endpoint: PeerEndpoint,
            },
            RemoveDirectPeerEndpoint {
                endpoint: PeerEndpoint,
            },
            ApplyMobPeerOverlay {
                epoch: u64,
                endpoints: Set<PeerEndpoint>,
            },
        }

        surface_only [
            ContainsSession,
            SessionHasExecutor,
            SessionHasComms,
            OpsLifecycleRegistry,
            InputState,
            ListActiveInputs,
            RuntimeState,
            RuntimeRealtimeAttachmentStatus,
            LoadBoundaryReceipt,
            Recover
        ]

        signal MeerkatMachineSignal {
            Initialize,
            BoundaryApplied { revision: u64 },
            DrainQueuedRun { run_id: RunId },
            StartConversationRun,
            StartImmediateAppend,
            StartImmediateContext,
            ClassifyExternalEnvelope,
            ClassifyPlainEvent,
            EnsureDrainRunning,
            StageAdd,
            StageRemove,
            StageReload,
            ApplySurfaceBoundary,
            PendingSucceeded,
            PendingFailed,
            CallStarted,
            CallFinished,
            FinalizeRemovalClean,
            FinalizeRemovalForced,
            SnapshotAligned,
            ShutdownSurface,
        }

        effect MeerkatMachineEffect {
            RuntimeBound { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RequestCancellationAtBoundary,
            WakeInterrupt,
            CommittedVisibleSetPublished { revision: u64 },
            // `kind` is a closed classifier of runtime lifecycle markers;
            // `detail` stays `String` because it's a free-form diagnostic
            // message paired with the kind.
            RuntimeNotice { kind: Enum<RuntimeNoticeKind>, detail: String },
            // Absorbed effects
            ResolveAdmission,
            SubmitAdmittedIngressEffect,
            SubmitRunPrimitive,
            ResolveCompletionAsTerminated,
            ApplyControlPlaneCommand,
            InitiateRecycle,
            IngressAccepted,
            PostAdmissionSignal { signal: String },
            ReadyForRun,
            InputLifecycleNotice,
            CompletionResolved,
            IngressNotice,
            SilentIntentApplied,
            CheckCompaction,
            RecordTerminalOutcome,
            RecordRunAssociation,
            RecordBoundarySequence,
            SubmitOpEvent,
            NotifyOpWatcher,
            ExposeOperationPeer,
            RetainTerminalRecord,
            EvictCompletedRecord,
            CompletionProduced { seq: u64, operation_id: OperationId, kind: OperationKind },
            // Wait-all barrier satisfaction. The handoff protocol's
            // obligation payload (wait_request_id + operation_ids) lives
            // on the compat `OpsBarrierBridgeMachine`'s mirror effect so
            // the DSL-macro-constrained canonical effect can stay
            // fieldless without expanding the DSL syntax. See
            // `meerkat-machine-schema/src/compat/ops_barrier_bridge.rs`.
            WaitAllSatisfied,
            CollectCompletedResult,
            EnqueueClassifiedEntry,
            SpawnDrainTask,
            ScheduleSurfaceCompletion,
            RefreshVisibleSurfaceSet,
            EmitExternalToolDelta,
            CloseSurfaceConnection,
            RejectSurfaceCall,
            // Realtime-attachment effects.
            RealtimeIntentProjected { present: bool },
            RealtimeBindingRotated { authority_epoch: u64 },
            // MCP server lifecycle effects. Emitted on each per-server state
            // transition so the shell can toggle the `[MCP_PENDING]` system
            // notice and refresh tool availability deterministically.
            McpServerStateChanged { server_id: McpServerId, new_state: Enum<McpServerState> },
            McpServerReloadRequested { server_id: McpServerId },
            // Peer interaction lifecycle effects (W1-A). Emitted on every
            // lifecycle-advancing transition so the shell can drop channels
            // (`subscriber_registry`, `interaction_stream_registry`) keyed on
            // `corr_id` — the map handles become a pure projection of these
            // effects.
            PeerInteractionStateChanged { corr_id: PeerCorrelationId, new_state: Enum<OutboundPeerRequestState> },
            PeerInteractionCleanup { corr_id: PeerCorrelationId },
            InboundPeerInteractionStateChanged { corr_id: PeerCorrelationId, new_state: Enum<InboundPeerRequestState> },
            // Session-context advancement effect (W2-E / issue #264). Emitted
            // on every transition that advances canonical session-context
            // truth. The realtime projection consumer installs a typed
            // observer on the session's DSL handle to drive a typed
            // `ProjectionFreshness` state.
            SessionContextAdvanced { updated_at_ms: u64 },
            // Interaction stream lifecycle effects (U6 / dogma #5).
            InteractionStreamStateChanged { corr_id: PeerCorrelationId, new_state: Enum<InteractionStreamState> },
            InteractionStreamCleanup { corr_id: PeerCorrelationId },
            // Realtime product-turn phase change effect (U9 / dogma #4).
            RealtimeProductTurnPhaseChanged { new_phase: Enum<RealtimeProductTurnPhase> },
            // Realtime projection freshness + reconnect policy change
            // effects (dogma round 2, U-C / dogma #1, #3, #13, #18, #20).
            RealtimeProjectionFreshnessChanged {
                new_freshness: Enum<RealtimeProjectionFreshness>,
                frontier_ms: u64,
            },
            RealtimeReconnectPolicyChanged { new_policy: Enum<RealtimeReconnectPolicy> },
            // Live-topology reconfigure effects.
            LiveTopologyPhaseChanged,
            // --- Track-B (R5): peer-projection effects ---
            //
            // `LocalEndpointChanged` fires on publish/clear so the comms
            // layer can re-advertise the session's self-endpoint.
            //
            // `PeerProjectionChanged` is the declarative "the effective
            // peer set was recomputed" signal. Carries the
            // post-transition `peer_projection_epoch` so consumers can
            // linearize observations.
            //
            // `CommsTrustReconcileRequested` fires whenever the
            // effective peer set (direct ∪ overlay) could have
            // changed. The comms runtime reconciliation handler
            // (Commit 4) reads the snapshot on receipt, computes the
            // effective set via `direct_peer_endpoints ∪
            // mob_overlay_peer_endpoints`, and diffs against its
            // own "applied trust store" view to compute add/remove
            // deltas. The DSL emits "reconcile needed at epoch N";
            // the shell does the mechanical trust-store diff.
            LocalEndpointChanged { endpoint: Option<PeerEndpoint> },
            PeerProjectionChanged { peer_projection_epoch: u64 },
            CommsTrustReconcileRequested { peer_projection_epoch: u64 },
        }

        // =====================================================================
        // Effect dispositions
        // =====================================================================

        disposition RuntimeBound => routed [MobMachine],
        disposition RuntimeRetired => routed [MobMachine],
        disposition RuntimeDestroyed => routed [MobMachine],
        disposition RequestCancellationAtBoundary => local,
        disposition WakeInterrupt => local,
        disposition CommittedVisibleSetPublished => external,
        disposition RuntimeNotice => external,
        // Absorbed effect dispositions
        disposition ResolveAdmission => local,
        disposition SubmitAdmittedIngressEffect => local,
        disposition SubmitRunPrimitive => local,
        disposition ResolveCompletionAsTerminated => local,
        disposition ApplyControlPlaneCommand => local,
        disposition InitiateRecycle => local,
        disposition IngressAccepted => external,
        disposition PostAdmissionSignal => local,
        disposition ReadyForRun => local,
        disposition InputLifecycleNotice => external,
        disposition CompletionResolved => local,
        disposition IngressNotice => external,
        disposition SilentIntentApplied => external,
        disposition CheckCompaction => local,
        disposition RecordTerminalOutcome => local,
        disposition RecordRunAssociation => local,
        disposition RecordBoundarySequence => local,
        disposition SubmitOpEvent => local,
        disposition NotifyOpWatcher => local,
        disposition ExposeOperationPeer => local,
        disposition RetainTerminalRecord => local,
        disposition EvictCompletedRecord => local,
        disposition CompletionProduced => local,
        disposition WaitAllSatisfied => local,
        disposition CollectCompletedResult => local,
        disposition EnqueueClassifiedEntry => local,
        disposition SpawnDrainTask => local,
        disposition ScheduleSurfaceCompletion => local,
        disposition RefreshVisibleSurfaceSet => external,
        disposition EmitExternalToolDelta => external,
        disposition CloseSurfaceConnection => local,
        disposition RejectSurfaceCall => external,
        disposition RealtimeIntentProjected => external,
        disposition RealtimeBindingRotated => external,
        disposition McpServerStateChanged => external,
        disposition McpServerReloadRequested => external,
        disposition PeerInteractionStateChanged => external,
        disposition PeerInteractionCleanup => external,
        disposition InboundPeerInteractionStateChanged => external,
        disposition SessionContextAdvanced => external,
        disposition InteractionStreamStateChanged => external,
        disposition InteractionStreamCleanup => external,
        disposition RealtimeProductTurnPhaseChanged => external,
        disposition RealtimeProjectionFreshnessChanged => external,
        disposition RealtimeReconnectPolicyChanged => external,
        disposition LiveTopologyPhaseChanged => external,
        disposition LocalEndpointChanged => external,
        disposition PeerProjectionChanged => external,
        disposition CommsTrustReconcileRequested => external,

        // =====================================================================
        // Invariants
        // =====================================================================

        invariant fence_requires_bound_runtime {
            self.active_fence_token == None || self.active_runtime_id != None
        }

        invariant running_has_current_run {
            self.lifecycle_phase != Phase::Running || self.current_run_id != None
        }

        invariant current_run_only_while_running_or_retired {
            self.current_run_id == None
            || self.lifecycle_phase == Phase::Running
            || self.lifecycle_phase == Phase::Retired
        }

        // Realtime binding state + authority epoch must stay in lockstep.
        // Unbound iff no epoch; any active binding phase must carry Some(epoch).
        // Prevents Unbound+Some(epoch) and BindingReady+None from being
        // representable as a derived TLC fact.
        invariant realtime_binding_epoch_consistency {
            (self.realtime_binding_state == RealtimeBindingState::Unbound)
            == (self.realtime_binding_authority_epoch == None)
        }

        // Peer-ingress owner companion fields must stay in lockstep with the
        // kind variant. `Unattached` carries no companions; `SessionOwned`
        // carries only a comms runtime id; `MobOwned` carries both. The
        // catalog DSL encodes the tagged-union discipline across three DSL
        // fields; silent transitions that leave the comms runtime id behind
        // become impossible to serialize.
        invariant peer_ingress_owner_consistency {
            (self.peer_ingress_owner_kind == PeerIngressOwnerKind::Unattached
                && self.peer_ingress_comms_runtime_id == None
                && self.peer_ingress_mob_id == None)
            || (self.peer_ingress_owner_kind == PeerIngressOwnerKind::SessionOwned
                && self.peer_ingress_comms_runtime_id != None
                && self.peer_ingress_mob_id == None)
            || (self.peer_ingress_owner_kind == PeerIngressOwnerKind::MobOwned
                && self.peer_ingress_comms_runtime_id != None
                && self.peer_ingress_mob_id != None)
        }

        // Supervisor-binding tagged-union discipline (Wave 3 D Row 21).
        // `Unbound` carries no companions; `Bound` carries all four
        // (`name`, `peer_id`, `address`, `epoch`). A half-populated
        // binding is structurally unrepresentable — the split-ownership
        // regression class is closed by construction.
        invariant supervisor_binding_consistency {
            (self.supervisor_binding_kind == SupervisorBindingKind::Unbound
                && self.supervisor_bound_name == None
                && self.supervisor_bound_peer_id == None
                && self.supervisor_bound_address == None
                && self.supervisor_bound_epoch == None)
            || (self.supervisor_binding_kind == SupervisorBindingKind::Bound
                && self.supervisor_bound_name != None
                && self.supervisor_bound_peer_id != None
                && self.supervisor_bound_address != None
                && self.supervisor_bound_epoch != None)
        }


        // =====================================================================
        // Direct transitions
        // =====================================================================

        // 1. Initialize: Initializing → Idle
        transition Initialize {
            on signal Initialize
            guard { self.lifecycle_phase == Phase::Initializing }
            update {}
            to Idle
        }

        // 2. RegisterSession: per-phase self-loop, no guard
        transition RegisterSession {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RegisterSession { session_id }
            update {
                self.session_id = Some(session_id);
            }
            to Idle
        }

        // 3. UnregisterSession: per-phase → Idle (NOT a self-loop, goes to Idle)
        // Cannot use per_phase because target is always Idle, not source phase.
        transition UnregisterSessionIdle {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition UnregisterSessionAttached {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition UnregisterSessionRunning {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition UnregisterSessionRetired {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition UnregisterSessionStopped {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }

        // 4. ReconfigureSessionLlmIdentity: Attached + Running self-loops
        transition ReconfigureSessionLlmIdentityAttached {
            on input ReconfigureSessionLlmIdentity {
                previous_identity, previous_visibility_state,
                previous_capability_surface, previous_capability_surface_status,
                target_identity, target_capability_surface,
                next_visibility_state, next_capability_base_filter,
                next_active_visibility_revision, tool_visibility_delta
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {}
            to Attached
        }
        transition ReconfigureSessionLlmIdentityRunning {
            on input ReconfigureSessionLlmIdentity {
                previous_identity, previous_visibility_state,
                previous_capability_surface, previous_capability_surface_status,
                target_identity, target_capability_surface,
                next_visibility_state, next_capability_base_filter,
                next_active_visibility_revision, tool_visibility_delta
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {}
            to Running
        }

        // 5. StagePersistentFilter: per-phase self-loop, guard session_registered
        transition StagePersistentFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StagePersistentFilter { filter, witnesses }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 6. RequestDeferredTools: per-phase self-loop, guard session_registered
        transition RequestDeferredTools {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequestDeferredTools { names, witnesses }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 7. PrepareBindings: different source→target mappings per phase
        // Initializing → Initializing (no guard, emits RuntimeBound)
        transition PrepareBindingsInitializing {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Initializing }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Initializing
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Idle → Attached
        transition PrepareBindingsIdle {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Attached → Attached
        transition PrepareBindingsAttached {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Running → Running
        transition PrepareBindingsRunning {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Running
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Retired → Retired
        transition PrepareBindingsRetired {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Retired
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Stopped → Stopped (inline in hand-written catalog)
        transition PrepareBindingsStopped {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Stopped
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 8. SetPeerIngressContext: per-phase self-loop, guard session_registered
        transition SetPeerIngressContext {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SetPeerIngressContext { keep_alive }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 9. NotifyDrainExited: per-phase self-loop, guard session_registered, emit RuntimeNotice
        transition NotifyDrainExited {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input NotifyDrainExited { reason }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit RuntimeNotice { kind: RuntimeNoticeKind::Drain, detail: "drain exited" }
        }

        // 10. InterruptCurrentRun: Attached + Running self-loops
        transition InterruptCurrentRunAttached {
            on input InterruptCurrentRun
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit WakeInterrupt
            emit RequestCancellationAtBoundary
        }
        transition InterruptCurrentRun {
            on input InterruptCurrentRun
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit WakeInterrupt
            emit RequestCancellationAtBoundary
        }

        // 11. CancelAfterBoundary: Attached + Running self-loops
        transition CancelAfterBoundaryAttached {
            on input CancelAfterBoundary
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit RequestCancellationAtBoundary
        }
        transition CancelAfterBoundary {
            on input CancelAfterBoundary
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit RequestCancellationAtBoundary
        }

        // 12. BoundaryAppliedPublish: Running self-loop (signal)
        transition BoundaryAppliedPublish {
            on signal BoundaryApplied { revision }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit CommittedVisibleSetPublished { revision: revision }
        }

        // 13. PublishCommittedVisibleSet: per-phase self-loop, complex guards
        transition PublishCommittedVisibleSetIdle {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Idle
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetAttached {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Attached
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetRunning {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Running
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetRetired {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Retired
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetStopped {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Stopped
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }

        // 14. Retire: from [Idle, Attached, Running] → Retired
        transition RetireRequestedFromIdle {
            on input Retire
            guard {
                self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Running
            }
            update {}
            to Retired
            emit RuntimeRetired { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 15. Reset: from [Initializing, Idle, Attached, Retired] → Idle
        transition Reset {
            on input Reset
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Retired
            }
            update {
                self.current_run_id = None;
                self.active_fence_token = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Idle
            emit RuntimeNotice { kind: RuntimeNoticeKind::Reset, detail: "runtime reset" }
        }

        // 16. StopRuntimeExecutor: different behavior per phase
        // Unbound (Initializing, Idle, Retired) → Stopped
        transition StopRuntimeExecutorUnbound {
            on input StopRuntimeExecutor
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Retired
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Stop, detail: "runtime executor stopped" }
        }
        // Attached → Attached (self-loop)
        transition StopRuntimeExecutorAttached {
            on input StopRuntimeExecutor
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Attached
            emit RuntimeNotice { kind: RuntimeNoticeKind::Stop, detail: "runtime executor stopped" }
        }
        // Running → Running (self-loop)
        transition StopRuntimeExecutorRunning {
            on input StopRuntimeExecutor
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Running
            emit RuntimeNotice { kind: RuntimeNoticeKind::Stop, detail: "runtime executor stopped" }
        }

        // 17. Destroy: from all non-Destroyed → Destroyed
        transition Destroy {
            on input Destroy
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Retired
                || self.lifecycle_phase == Phase::Stopped
            }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Destroyed
            emit RuntimeDestroyed { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // =====================================================================
        // Absorbed transitions
        // =====================================================================

        // 18. EnsureSessionWithExecutor
        // Idle → Attached (phase change)
        transition EnsureSessionWithExecutorIdle {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            update {}
            to Attached
        }
        // Attached, Running: self-loop
        transition EnsureSessionWithExecutorAttached {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
        }
        transition EnsureSessionWithExecutorRunning {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        // Retired, Stopped: self-loop
        transition EnsureSessionWithExecutorRetired {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {}
            to Retired
        }
        transition EnsureSessionWithExecutorStopped {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }

        // 19. SetSilentIntents: per-phase, guard session_registered
        // Idle, Attached, Running, Retired: update intents
        transition SetSilentIntentsIdle {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Idle
        }
        transition SetSilentIntentsAttached {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Attached
        }
        transition SetSilentIntentsRunning {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Running
        }
        transition SetSilentIntentsRetired {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Retired
        }
        // Stopped: no-op (no update)
        transition SetSilentIntentsStopped {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_registered" { self.session_id != None }
            update {}
            to Stopped
        }

        // 20. Abort: per-phase self-loop, guard session_registered
        transition Abort {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input Abort { session_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 20b. Wait: per-phase self-loop, guard session_registered
        transition Wait {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input Wait { session_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 21. AbortAll: per-phase self-loop, no guard
        transition AbortAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortAll
            update {}
            to Idle
        }

        // 22. EnsureDrainRunning: Attached/Running self-loops, emit SpawnDrainTask
        transition EnsureDrainRunningAttached {
            on signal EnsureDrainRunning
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SpawnDrainTask
        }
        transition EnsureDrainRunningRunning {
            on signal EnsureDrainRunning
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit SpawnDrainTask
        }

        // 23. Ingest: Idle/Attached/Running self-loops, emit ResolveAdmission
        transition Ingest {
            per_phase [Idle, Attached, Running]
            on input Ingest { runtime_id, work_id, origin }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit ResolveAdmission
        }

        // 24. PublishEvent: per-phase self-loop, emit IngressNotice
        transition PublishEvent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PublishEvent { kind }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit IngressNotice
        }

        // 25. AcceptWithCompletion: complex, multiple variants per phase.
        //
        // The `wake_if_idle` flag is the machine-owned truth for "this
        // input must wake the runtime loop when it reaches idle" (e.g.
        // peer_response_terminal queued while the session is running).
        // Idle/Attached queued arms already emit WakeLoop
        // unconditionally — the caller is about to wake — so
        // wake_if_idle is ignored in those guards. The Running+Queued
        // path splits on it: passive (no signal) vs WakeIfIdle (emit
        // WakeLoop for the next idle reach).
        //
        // Idle + queued (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionIdleQueued {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Idle
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "WakeLoop" }
        }
        // Idle + immediate (immediate=true, interrupt_yielding=false)
        transition AcceptWithCompletionIdleImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Idle
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "RequestImmediateProcessing" }
        }
        // Attached + immediate → Running (phase change!)
        transition AcceptWithCompletionAttachedImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("attached");
            }
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "RequestImmediateProcessing" }
            emit SubmitRunPrimitive
        }
        // Attached + queued (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionAttachedQueued {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Attached
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "WakeLoop" }
        }
        // Running + queued passive (immediate=false, interrupt_yielding=false, wake_if_idle=false)
        transition AcceptWithCompletionRunningQueuedPassive {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            guard "wake_if_idle" { wake_if_idle == false }
            update {}
            to Running
            emit IngressAccepted
        }
        // Running + queued wake-if-idle (immediate=false, interrupt_yielding=false, wake_if_idle=true)
        //
        // The input is staged for the next run boundary *and* the machine
        // records a pending `WakeLoop` so the runtime loop observes the
        // wake on its first idle re-check. Drives the turn-driven
        // realtime async-peer-response path: an operator that called
        // send_request and is waiting must not strand the response on
        // durable context alone — the admission signal becomes the
        // authority that schedules the next turn.
        transition AcceptWithCompletionRunningQueuedWakeIfIdle {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            guard "wake_if_idle" { wake_if_idle == true }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "WakeLoop" }
        }
        // Running + interrupt_yielding (immediate=false, interrupt_yielding=true)
        transition AcceptWithCompletionRunningInterruptYielding {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == true }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "InterruptYielding" }
        }
        // Running + immediate (immediate=true, interrupt_yielding=false)
        transition AcceptWithCompletionRunningImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "RequestImmediateProcessing" }
        }

        // 26. AcceptWithoutWake: Idle/Attached/Running self-loops
        transition AcceptWithoutWake {
            per_phase [Idle, Attached, Running]
            on input AcceptWithoutWake { input_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit IngressAccepted
        }

        // 27. ClassifyExternalEnvelope/ClassifyPlainEvent: Attached/Running, emit EnqueueClassifiedEntry
        transition ClassifyExternalEnvelopeAttached {
            on signal ClassifyExternalEnvelope
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
        }
        transition ClassifyExternalEnvelopeRunning {
            on signal ClassifyExternalEnvelope
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EnqueueClassifiedEntry
        }
        transition ClassifyPlainEventAttached {
            on signal ClassifyPlainEvent
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
        }
        transition ClassifyPlainEventRunning {
            on signal ClassifyPlainEvent
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EnqueueClassifiedEntry
        }

        // 28. Prepare: Idle→Running, Attached→Running
        transition PrepareIdle {
            on input Prepare { session_id, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("idle");
            }
            to Running
            emit SubmitRunPrimitive
        }
        transition PrepareAttached {
            on input Prepare { session_id, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("attached");
            }
            to Running
            emit SubmitRunPrimitive
        }

        // 29. DrainQueuedRun: Retired→Running (signal)
        transition DrainQueuedRunRetired {
            on signal DrainQueuedRun { run_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("retired");
            }
            to Running
            emit SubmitRunPrimitive
        }

        // 30. StartConversationRun/StartImmediateAppend/StartImmediateContext:
        //     Attached self-loops (signals), emit SubmitRunPrimitive
        transition StartConversationRunAttached {
            on signal StartConversationRun
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SubmitRunPrimitive
        }
        transition StartImmediateAppendAttached {
            on signal StartImmediateAppend
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SubmitRunPrimitive
        }
        transition StartImmediateContextAttached {
            on signal StartImmediateContext
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SubmitRunPrimitive
        }

        // 31. Commit: Running → Idle/Attached/Retired (guard pre_run_phase + run_id match)
        transition CommitRunningToIdle {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some("idle") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition CommitRunningToAttached {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some("attached") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
        }
        transition CommitRunningToRetired {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some("retired") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
        }

        // 32. Fail: Running → Idle/Attached/Retired (guard pre_run_phase + run_id match)
        transition FailRunningToIdle {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some("idle") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
            emit RecordTerminalOutcome
        }
        transition FailRunningToAttached {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some("attached") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
            emit RecordTerminalOutcome
        }
        transition FailRunningToRetired {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some("retired") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
            emit RecordTerminalOutcome
        }

        // 33. MCP surface signals: Attached/Running per-phase, emit EmitExternalToolDelta (or other)
        transition StageAddAttached {
            on signal StageAdd
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition StageAddRunning {
            on signal StageAdd
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition StageRemoveAttached {
            on signal StageRemove
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition StageRemoveRunning {
            on signal StageRemove
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition StageReloadAttached {
            on signal StageReload
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition StageReloadRunning {
            on signal StageReload
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition ApplySurfaceBoundaryAttached {
            on signal ApplySurfaceBoundary
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit ScheduleSurfaceCompletion
        }
        transition ApplySurfaceBoundaryRunning {
            on signal ApplySurfaceBoundary
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit ScheduleSurfaceCompletion
        }
        transition PendingSucceededAttached {
            on signal PendingSucceeded
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition PendingSucceededRunning {
            on signal PendingSucceeded
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition PendingFailedAttached {
            on signal PendingFailed
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition PendingFailedRunning {
            on signal PendingFailed
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition CallStartedAttached {
            on signal CallStarted
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
        }
        transition CallStartedRunning {
            on signal CallStarted
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
        }
        transition CallFinishedAttached {
            on signal CallFinished
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
        }
        transition CallFinishedRunning {
            on signal CallFinished
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
        }
        transition FinalizeRemovalCleanAttached {
            on signal FinalizeRemovalClean
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition FinalizeRemovalCleanRunning {
            on signal FinalizeRemovalClean
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition FinalizeRemovalForcedAttached {
            on signal FinalizeRemovalForced
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition FinalizeRemovalForcedRunning {
            on signal FinalizeRemovalForced
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition SnapshotAlignedAttached {
            on signal SnapshotAligned
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition SnapshotAlignedRunning {
            on signal SnapshotAligned
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition ShutdownSurfaceAttached {
            on signal ShutdownSurface
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition ShutdownSurfaceRunning {
            on signal ShutdownSurface
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }

        // 34. Recycle: from Idle/Retired → Idle, from Attached → Attached
        transition RecycleFromIdleOrRetired {
            on input Recycle
            guard {
                self.lifecycle_phase == Phase::Idle || self.lifecycle_phase == Phase::Retired
            }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.active_fence_token = None;
                self.current_run_id = None;
            }
            to Idle
            emit InitiateRecycle
        }
        transition RecycleFromAttached {
            on input Recycle
            guard { self.lifecycle_phase == Phase::Attached }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.active_fence_token = None;
                self.current_run_id = None;
            }
            to Attached
            emit InitiateRecycle
        }

        // =====================================================================
        // Realtime-attachment transitions
        // =====================================================================

        transition ProjectRealtimeIntent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ProjectRealtimeIntent { present }
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_intent_present = present;
            }
            to Idle
            emit RealtimeIntentProjected { present: present }
        }

        transition BeginRealtimeBinding {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BeginRealtimeBinding
            guard "session_registered" { self.session_id != None }
            guard "no_topology_reconfigure_in_progress" { self.live_topology_phase == LiveTopologyPhase::Idle }
            update {
                self.realtime_binding_state = RealtimeBindingState::BindingNotReady;
                self.realtime_binding_authority_epoch = Some(self.realtime_next_authority_epoch);
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
            emit RealtimeBindingRotated { authority_epoch: self.realtime_binding_authority_epoch.get("value") }
        }

        transition ReplaceRealtimeBinding {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ReplaceRealtimeBinding
            guard "session_registered" { self.session_id != None }
            guard "no_topology_reconfigure_in_progress" { self.live_topology_phase == LiveTopologyPhase::Idle }
            update {
                self.realtime_binding_state = RealtimeBindingState::ReplacementPending;
                self.realtime_binding_authority_epoch = Some(self.realtime_next_authority_epoch);
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
            emit RealtimeBindingRotated { authority_epoch: self.realtime_binding_authority_epoch.get("value") }
        }

        transition DetachRealtimeBinding {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DetachRealtimeBinding
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
        }

        transition RequireRealtimeReattach {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequireRealtimeReattach
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = true;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
        }

        transition PublishRealtimeSignal {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PublishRealtimeSignal { authority_epoch, next_binding_state }
            guard "authority_matches_current" { self.realtime_binding_authority_epoch == Some(authority_epoch) }
            guard "no_topology_reconfigure_in_progress" { self.live_topology_phase == LiveTopologyPhase::Idle }
            guard "valid_next_state" {
                next_binding_state == RealtimeBindingState::BindingNotReady
                || next_binding_state == RealtimeBindingState::BindingReady
                || next_binding_state == RealtimeBindingState::ReplacementPending
            }
            update {
                self.realtime_binding_state = next_binding_state;
                self.realtime_reattach_required = false;
            }
            to Idle
        }

        // =====================================================================
        // MCP server lifecycle transitions
        // =====================================================================
        //
        // Each MCP server is keyed by its configured `McpServerId` in the
        // `mcp_server_states` map. The shell translates incoming connection
        // events into these inputs; each input rewrites that key's state and
        // emits `McpServerStateChanged` so downstream consumers (the
        // `[MCP_PENDING]` system-notice toggle and tool-availability filter)
        // can stay pure reads off DSL state.

        transition McpServerConnectPending {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerConnectPending { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::PendingConnect);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::PendingConnect }
        }

        transition McpServerConnected {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerConnected { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::Connected);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::Connected }
        }

        transition McpServerFailed {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerFailed { server_id, error }
            guard "session_registered" { self.session_id != None }
            update {
                // The catalog DSL cannot carry payloads on enum state
                // variants, so the error detail lives on the input and the
                // emitted effect; state just records the `Failed` category.
                self.mcp_server_states.insert(server_id, McpServerState::Failed);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::Failed }
        }

        transition McpServerDisconnected {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerDisconnected { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::Disconnected);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::Disconnected }
        }

        transition McpServerReload {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerReload { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                // Reload moves the server back to PendingConnect; the shell is
                // expected to tear down the prior connection and drive a fresh
                // Connected / Failed transition on completion.
                self.mcp_server_states.insert(server_id, McpServerState::PendingConnect);
            }
            to Idle
            emit McpServerReloadRequested { server_id: server_id }
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::PendingConnect }
        }

        // =====================================================================
        // Peer interaction lifecycle transitions (W1-A / issue #264)
        // =====================================================================

        transition PeerRequestSent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerRequestSent { corr_id, to }
            guard "not_already_pending" { !self.pending_peer_requests.contains_key(corr_id) }
            update {
                self.pending_peer_requests.insert(corr_id, OutboundPeerRequestState::Sent);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::Sent }
        }

        transition PeerResponseProgressArrived {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseProgressArrived { corr_id }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            update {
                self.pending_peer_requests.insert(corr_id, OutboundPeerRequestState::AcceptedProgress);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::AcceptedProgress }
        }

        transition PeerResponseTerminalArrivedCompleted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseTerminalArrived { corr_id, disposition }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            guard "completed" { disposition == PeerTerminalDisposition::Completed }
            update {
                self.pending_peer_requests.remove(corr_id);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::Completed }
            emit PeerInteractionCleanup { corr_id: corr_id }
        }

        transition PeerResponseTerminalArrivedFailed {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseTerminalArrived { corr_id, disposition }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            guard "failed" { disposition == PeerTerminalDisposition::Failed }
            update {
                self.pending_peer_requests.remove(corr_id);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::Failed }
            emit PeerInteractionCleanup { corr_id: corr_id }
        }

        transition PeerRequestTimedOut {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerRequestTimedOut { corr_id }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            update {
                self.pending_peer_requests.remove(corr_id);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::TimedOut }
            emit PeerInteractionCleanup { corr_id: corr_id }
        }

        transition PeerRequestReceived {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerRequestReceived { corr_id }
            guard "not_already_inbound" { !self.inbound_peer_requests.contains_key(corr_id) }
            update {
                self.inbound_peer_requests.insert(corr_id, InboundPeerRequestState::Received);
            }
            to Idle
            emit InboundPeerInteractionStateChanged { corr_id: corr_id, new_state: InboundPeerRequestState::Received }
        }

        transition PeerResponseReplied {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseReplied { corr_id }
            guard "inbound_exists" { self.inbound_peer_requests.contains_key(corr_id) }
            update {
                self.inbound_peer_requests.remove(corr_id);
            }
            to Idle
            emit InboundPeerInteractionStateChanged { corr_id: corr_id, new_state: InboundPeerRequestState::Replied }
        }

        // Session-context advancement (W2-E / issue #264). Monotonic guard
        // filters out duplicate or out-of-order ticks at the DSL layer so
        // callers fire unconditionally after any mutation.
        transition AdvanceSessionContext {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AdvanceSessionContext { updated_at_ms }
            guard "monotonic" { updated_at_ms > self.last_session_context_updated_at_ms }
            update {
                self.last_session_context_updated_at_ms = updated_at_ms;
            }
            to Idle
            emit SessionContextAdvanced { updated_at_ms: updated_at_ms }
        }

        // =====================================================================
        // Interaction stream lifecycle transitions (U6 / dogma #5)
        // =====================================================================

        transition InteractionStreamReserved {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamReserved { corr_id }
            guard "not_reserved" { !self.reserved_interaction_streams.contains(corr_id) }
            guard "not_attached" { !self.attached_interaction_streams.contains(corr_id) }
            update {
                self.reserved_interaction_streams.insert(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Reserved }
        }

        transition InteractionStreamAttached {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamAttached { corr_id }
            guard "is_reserved" { self.reserved_interaction_streams.contains(corr_id) }
            update {
                self.reserved_interaction_streams.remove(corr_id);
                self.attached_interaction_streams.insert(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Attached }
        }

        transition InteractionStreamCompleted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamCompleted { corr_id }
            guard "is_attached" { self.attached_interaction_streams.contains(corr_id) }
            update {
                self.attached_interaction_streams.remove(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Completed }
            emit InteractionStreamCleanup { corr_id: corr_id }
        }

        transition InteractionStreamExpired {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamExpired { corr_id }
            guard "is_reserved" { self.reserved_interaction_streams.contains(corr_id) }
            update {
                self.reserved_interaction_streams.remove(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Expired }
            emit InteractionStreamCleanup { corr_id: corr_id }
        }

        transition InteractionStreamClosedEarly {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamClosedEarly { corr_id }
            guard "is_attached" { self.attached_interaction_streams.contains(corr_id) }
            update {
                self.attached_interaction_streams.remove(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::ClosedEarly }
            emit InteractionStreamCleanup { corr_id: corr_id }
        }

        // =====================================================================
        // Realtime product-turn lifecycle transitions (U9 / dogma #4)
        // =====================================================================

        transition ProductTurnInFlight {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnInFlight
            guard "only_from_idle" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::AwaitingProgress;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::AwaitingProgress }
        }

        transition ProductTurnCommittedFromAwaiting {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnCommitted
            guard "from_awaiting" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::AwaitingProgress
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Committed;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Committed }
        }

        transition ProductTurnCommittedFromOutput {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnCommitted
            guard "from_output_started" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::OutputStarted
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Preemptible;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Preemptible }
        }

        transition ProductOutputStartedFromAwaiting {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductOutputStarted
            guard "from_awaiting" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::AwaitingProgress
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::OutputStarted;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::OutputStarted }
        }

        transition ProductOutputStartedFromCommitted {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductOutputStarted
            guard "from_committed" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Committed
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Preemptible;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Preemptible }
        }

        transition ProductTurnInterruptedFromPreemptible {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnInterrupted
            guard "from_preemptible" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Preemptible
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Committed;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Committed }
        }

        transition ProductTurnInterruptedFromOutput {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnInterrupted
            guard "from_output_started" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::OutputStarted
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::AwaitingProgress;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::AwaitingProgress }
        }

        transition ProductTurnTerminal {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnTerminal
            guard "not_already_idle" {
                self.realtime_product_turn_phase != RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Idle;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Idle }
        }

        // =====================================================================
        // Realtime projection freshness (dogma round 2, U-C / dogma #1, #3, #13, #20)
        // =====================================================================

        transition RealtimeProjectionAdvanceDuringTurn {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionAdvanceObserved { advanced_at_ms }
            guard "monotonic" { advanced_at_ms > self.realtime_projection_frontier_ms }
            guard "turn_in_flight" {
                self.realtime_product_turn_phase != RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::StaleDeferred;
                self.realtime_projection_frontier_ms = advanced_at_ms;
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::StaleDeferred,
                frontier_ms: advanced_at_ms
            }
        }

        transition RealtimeProjectionAdvanceWhileIdle {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionAdvanceObserved { advanced_at_ms }
            guard "monotonic" { advanced_at_ms > self.realtime_projection_frontier_ms }
            guard "turn_idle" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::StaleImmediate;
                self.realtime_projection_frontier_ms = advanced_at_ms;
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::StaleImmediate,
                frontier_ms: advanced_at_ms
            }
        }

        transition RealtimeProjectionRefreshed {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionRefreshed { observed_ms }
            guard "not_behind_frontier" {
                observed_ms >= self.realtime_projection_frontier_ms
            }
            guard "actually_changing" {
                self.realtime_projection_freshness != RealtimeProjectionFreshness::Clean
                || observed_ms > self.realtime_projection_frontier_ms
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::Clean;
                if observed_ms > self.realtime_projection_frontier_ms {
                    self.realtime_projection_frontier_ms = observed_ms;
                }
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::Clean,
                frontier_ms: self.realtime_projection_frontier_ms
            }
        }

        transition RealtimeProjectionReset {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionReset { baseline_ms }
            guard "actually_changing" {
                self.realtime_projection_freshness != RealtimeProjectionFreshness::Clean
                || baseline_ms > self.realtime_projection_frontier_ms
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::Clean;
                if baseline_ms > self.realtime_projection_frontier_ms {
                    self.realtime_projection_frontier_ms = baseline_ms;
                }
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::Clean,
                frontier_ms: self.realtime_projection_frontier_ms
            }
        }

        // =====================================================================
        // Realtime reconnect policy (dogma round 2, U-C / dogma #1, #3, #18, #20)
        // =====================================================================

        transition ClassifyRealtimeClientInputSubmitted {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ClassifyRealtimeClientInputSubmitted
            guard "not_already_reattach" {
                self.realtime_reconnect_policy != RealtimeReconnectPolicy::ReattachAndRecover
            }
            update {
                self.realtime_reconnect_policy = RealtimeReconnectPolicy::ReattachAndRecover;
            }
            to Idle
            emit RealtimeReconnectPolicyChanged {
                new_policy: RealtimeReconnectPolicy::ReattachAndRecover
            }
        }

        transition ClassifyRealtimeMidTurnActivity {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ClassifyRealtimeMidTurnActivity
            guard "not_already_reattach" {
                self.realtime_reconnect_policy != RealtimeReconnectPolicy::ReattachAndRecover
            }
            update {
                self.realtime_reconnect_policy = RealtimeReconnectPolicy::ReattachAndRecover;
            }
            to Idle
            emit RealtimeReconnectPolicyChanged {
                new_policy: RealtimeReconnectPolicy::ReattachAndRecover
            }
        }

        transition ClassifyRealtimeTurnTerminated {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ClassifyRealtimeTurnTerminated
            guard "actually_changing" {
                self.realtime_reconnect_policy != RealtimeReconnectPolicy::CleanExit
                || self.realtime_projection_freshness == RealtimeProjectionFreshness::StaleDeferred
            }
            update {
                self.realtime_reconnect_policy = RealtimeReconnectPolicy::CleanExit;
                if self.realtime_projection_freshness == RealtimeProjectionFreshness::StaleDeferred {
                    self.realtime_projection_freshness = RealtimeProjectionFreshness::StaleImmediate;
                }
            }
            to Idle
            emit RealtimeReconnectPolicyChanged {
                new_policy: RealtimeReconnectPolicy::CleanExit
            }
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: self.realtime_projection_freshness,
                frontier_ms: self.realtime_projection_frontier_ms
            }
        }

        // =====================================================================
        // Live-topology reconfigure transitions
        // =====================================================================

        transition BeginLiveTopologyReconfigure {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BeginLiveTopologyReconfigure { authority_epoch }
            guard "session_registered" { self.session_id != None }
            guard "authority_matches_current" { self.realtime_binding_authority_epoch == Some(authority_epoch) }
            guard "topology_idle" { self.live_topology_phase == LiveTopologyPhase::Idle }
            update {
                self.live_topology_phase = LiveTopologyPhase::Reconfiguring;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition MarkLiveTopologyDetached {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input MarkLiveTopologyDetached
            guard "session_registered" { self.session_id != None }
            guard "topology_reconfiguring" { self.live_topology_phase == LiveTopologyPhase::Reconfiguring }
            // DSL-native "safe to detach now" guard.
            //
            // **Catalog/runtime divergence (intentional):** the runtime DSL
            // (`meerkat-runtime/src/meerkat_machine/dsl.rs`) guards on the
            // richer `turn_phase ∈ {Ready, DrainingBoundary, Completed,
            // Failed, Cancelled}` set, which permits detach at the natural
            // draining boundary mid-run. The catalog DSL is the TLC-facing
            // twin and does not model `turn_phase`; it conservatively
            // approximates "safe to detach" as "no run currently open"
            // (`current_run_id == None`). This is a strict over-
            // approximation: any TLC trace that exercises this transition
            // under the catalog guard is also admissible under the runtime
            // guard, so invariants proven here hold in production.
            guard "no_active_run" { self.current_run_id == None }
            update {
                self.live_topology_phase = LiveTopologyPhase::Detached;
                // Compose the detach into the binding authority.
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition ApplyLiveTopologyIdentity {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ApplyLiveTopologyIdentity
            guard "session_registered" { self.session_id != None }
            guard "topology_detached" { self.live_topology_phase == LiveTopologyPhase::Detached }
            update {
                self.live_topology_phase = LiveTopologyPhase::HostIdentityApplied;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition ApplyLiveTopologyVisibility {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ApplyLiveTopologyVisibility
            guard "session_registered" { self.session_id != None }
            guard "host_identity_applied" { self.live_topology_phase == LiveTopologyPhase::HostIdentityApplied }
            update {
                self.live_topology_phase = LiveTopologyPhase::HostVisibilityApplied;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition CompleteLiveTopology {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CompleteLiveTopology
            guard "session_registered" { self.session_id != None }
            guard "host_visibility_applied" { self.live_topology_phase == LiveTopologyPhase::HostVisibilityApplied }
            update {
                self.live_topology_phase = LiveTopologyPhase::Idle;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition AbortLiveTopologyBeforeDetach {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortLiveTopologyBeforeDetach
            guard "session_registered" { self.session_id != None }
            guard "topology_reconfiguring" { self.live_topology_phase == LiveTopologyPhase::Reconfiguring }
            update {
                self.live_topology_phase = LiveTopologyPhase::Idle;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition FailLiveTopologyAfterDetach {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input FailLiveTopologyAfterDetach
            guard "session_registered" { self.session_id != None }
            guard "topology_past_detach" {
                self.live_topology_phase == LiveTopologyPhase::Detached
                || self.live_topology_phase == LiveTopologyPhase::HostIdentityApplied
                || self.live_topology_phase == LiveTopologyPhase::HostVisibilityApplied
            }
            update {
                self.live_topology_phase = LiveTopologyPhase::Idle;
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = true;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        // =====================================================================
        // Peer-ingress transport capability ownership (W2-G)
        // =====================================================================

        // AttachSessionIngress: only valid from `Unattached`. Rejects
        // `MobOwned` → `SessionOwned` silent downgrades by construction.
        transition AttachSessionIngress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AttachSessionIngress { comms_runtime_id }
            guard "session_registered" { self.session_id != None }
            guard "owner_is_unattached" {
                self.peer_ingress_owner_kind == PeerIngressOwnerKind::Unattached
            }
            update {
                self.peer_ingress_owner_kind = PeerIngressOwnerKind::SessionOwned;
                self.peer_ingress_comms_runtime_id = Some(comms_runtime_id);
                self.peer_ingress_mob_id = None;
            }
            to Idle
        }

        // AttachMobIngress: valid from `Unattached` or `SessionOwned`.
        // Mob provisioning is allowed to take over a session-owned drain
        // (the spec's promotion case).
        transition AttachMobIngress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AttachMobIngress { comms_runtime_id, mob_id }
            guard "session_registered" { self.session_id != None }
            guard "owner_allows_mob_attach" {
                self.peer_ingress_owner_kind == PeerIngressOwnerKind::Unattached
                || self.peer_ingress_owner_kind == PeerIngressOwnerKind::SessionOwned
            }
            update {
                self.peer_ingress_owner_kind = PeerIngressOwnerKind::MobOwned;
                self.peer_ingress_comms_runtime_id = Some(comms_runtime_id);
                self.peer_ingress_mob_id = Some(mob_id);
            }
            to Idle
        }

        // DetachIngress: clear any active ownership back to `Unattached`.
        transition DetachIngress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DetachIngress
            guard "session_registered" { self.session_id != None }
            guard "owner_is_attached" {
                self.peer_ingress_owner_kind != PeerIngressOwnerKind::Unattached
            }
            update {
                self.peer_ingress_owner_kind = PeerIngressOwnerKind::Unattached;
                self.peer_ingress_comms_runtime_id = None;
                self.peer_ingress_mob_id = None;
            }
            to Idle
        }

        // =====================================================================
        // Supervisor-bridge authorization (Wave 3 D Row 21)
        // =====================================================================

        transition BindSupervisor {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BindSupervisor { name, peer_id, address, epoch }
            guard "supervisor_unbound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Unbound
            }
            update {
                self.supervisor_binding_kind = SupervisorBindingKind::Bound;
                self.supervisor_bound_name = Some(name);
                self.supervisor_bound_peer_id = Some(peer_id);
                self.supervisor_bound_address = Some(address);
                self.supervisor_bound_epoch = Some(epoch);
            }
            to Idle
        }

        transition AuthorizeSupervisor {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AuthorizeSupervisor { name, peer_id, address, epoch }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            update {
                self.supervisor_bound_name = Some(name);
                self.supervisor_bound_peer_id = Some(peer_id);
                self.supervisor_bound_address = Some(address);
                self.supervisor_bound_epoch = Some(epoch);
            }
            to Idle
        }

        transition RevokeSupervisor {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RevokeSupervisor { peer_id, epoch }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            guard "peer_id_matches_current" {
                self.supervisor_bound_peer_id == Some(peer_id)
            }
            guard "epoch_matches_current" {
                self.supervisor_bound_epoch == Some(epoch)
            }
            update {
                self.supervisor_binding_kind = SupervisorBindingKind::Unbound;
                self.supervisor_bound_name = None;
                self.supervisor_bound_peer_id = None;
                self.supervisor_bound_address = None;
                self.supervisor_bound_epoch = None;
            }
            to Idle
        }

        // =====================================================================
        // Ops-barrier satisfaction (handoff feedback input)
        // =====================================================================
        //
        // Delivered by the `ops_barrier_satisfaction` handoff protocol's
        // submit helper once the realizing owner observes that every
        // `operation_id` in the outstanding barrier has completed. The
        // transition is a phase-preserving self-loop: the barrier is a
        // correlation fact, not a phase mutation, and the runtime
        // turn-state authority consumes the feedback through its own
        // reducer via `TurnStateHandle::ops_barrier_satisfied(...)`.
        //
        // Keeping the transition present (rather than marking the input
        // `surface_only`) lets the schema enforce the input-vs-transition
        // coverage contract and gives TLC something concrete to cover.

        transition OpsBarrierSatisfiedAttached {
            on input OpsBarrierSatisfied { operation_ids }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
        }
        transition OpsBarrierSatisfiedRunning {
            on input OpsBarrierSatisfied { operation_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
        }

        // =====================================================================
        // Track-B (R5): peer-projection transitions.
        //
        // Active session phases (`Idle`, `Attached`, `Running`) accept
        // peer-projection mutations via `per_phase [...]`. Non-active
        // phases (`Initializing`, `Retired`, `Stopped`, `Destroyed`)
        // do not — the session is not participating in peer trust
        // decisions outside the active phases.
        //
        // `per_phase` uses the DSL's multi-phase self-loop construct
        // that expands into one transition per listed phase at macro
        // expansion. Transitions are "to" the same phase they fired
        // from.
        // =====================================================================

        // PublishLocalEndpoint — no effective-peer-set change.
        //
        // The `to Idle` is filler syntax: `per_phase` expansion
        // overrides `to_phase` to each listed phase (self-loop), but
        // the parser still requires the `to <Phase>` clause.
        transition PublishLocalEndpoint {
            per_phase [Idle, Attached, Running]
            on input PublishLocalEndpoint { endpoint }
            update {
                self.local_endpoint = Some(endpoint);
            }
            to Idle
            emit LocalEndpointChanged { endpoint: Some(endpoint) }
        }

        // ClearLocalEndpoint — guarded on Some(endpoint) being present.
        transition ClearLocalEndpoint {
            per_phase [Idle, Attached, Running]
            on input ClearLocalEndpoint
            guard "local_endpoint_present" { self.local_endpoint != None }
            update {
                self.local_endpoint = None;
            }
            to Idle
            emit LocalEndpointChanged { endpoint: None }
        }

        // AddDirectPeerEndpoint — direct set update, epoch bump,
        // reconcile effect.
        transition AddDirectPeerEndpoint {
            per_phase [Idle, Attached, Running]
            on input AddDirectPeerEndpoint { endpoint }
            guard "endpoint_not_already_direct" { self.direct_peer_endpoints.contains(endpoint) == false }
            update {
                self.direct_peer_endpoints.insert(endpoint);
                self.peer_projection_epoch += 1;
            }
            to Idle
            emit PeerProjectionChanged { peer_projection_epoch: self.peer_projection_epoch }
            emit CommsTrustReconcileRequested { peer_projection_epoch: self.peer_projection_epoch }
        }

        // RemoveDirectPeerEndpoint — direct set removal.
        transition RemoveDirectPeerEndpoint {
            per_phase [Idle, Attached, Running]
            on input RemoveDirectPeerEndpoint { endpoint }
            guard "endpoint_present_in_direct" { self.direct_peer_endpoints.contains(endpoint) == true }
            update {
                self.direct_peer_endpoints.remove(endpoint);
                self.peer_projection_epoch += 1;
            }
            to Idle
            emit PeerProjectionChanged { peer_projection_epoch: self.peer_projection_epoch }
            emit CommsTrustReconcileRequested { peer_projection_epoch: self.peer_projection_epoch }
        }

        // ApplyMobPeerOverlay — stale-epoch guard on the overlay-
        // specific `mob_overlay_epoch`, wholesale overlay replacement.
        // The guard uses `>` (strictly-newer) rather than `>=` so
        // repeat application of the same epoch is rejected (the
        // driver's epoch counter already suppresses no-op dispatches;
        // any re-delivery at the same epoch is a retry-bug on the
        // transport layer, not a legitimate state change).
        //
        // Both epochs bump on a successful apply: `mob_overlay_epoch`
        // tracks overlay-specific freshness (used by the guard),
        // `peer_projection_epoch` tracks general effective-set changes
        // (carried in `CommsTrustReconcileRequested`).
        transition ApplyMobPeerOverlay {
            per_phase [Idle, Attached, Running]
            on input ApplyMobPeerOverlay { epoch, endpoints }
            guard "stale_overlay_epoch" { epoch > self.mob_overlay_epoch }
            update {
                self.mob_overlay_peer_endpoints = endpoints;
                self.mob_overlay_epoch = epoch;
                self.peer_projection_epoch += 1;
            }
            to Idle
            emit PeerProjectionChanged { peer_projection_epoch: self.peer_projection_epoch }
            emit CommsTrustReconcileRequested { peer_projection_epoch: self.peer_projection_epoch }
        }
    }
}

// =====================================================================
// Stub types for compilation
// =====================================================================

macro_rules! stub_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub String);
        impl<T: Into<String>> From<T> for $name {
            fn from(s: T) -> Self {
                Self(s.into())
            }
        }
    };
}

stub_newtype!(SessionId);
stub_newtype!(AgentRuntimeId);
stub_newtype!(FenceToken);
stub_newtype!(RunId);
stub_newtype!(InputId);
stub_newtype!(WorkId);
stub_newtype!(OperationId);
stub_newtype!(SessionLlmIdentity);
stub_newtype!(SessionToolVisibilityState);
stub_newtype!(SessionLlmCapabilitySurface);
/// Typed capability-surface resolution status. Closed mirror of
/// `meerkat_runtime::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus` —
/// replaces the former `stub_newtype!` stringly wrapper the catalog DSL
/// carried for the two-state discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionLlmCapabilitySurfaceStatus {
    Resolved,
    #[default]
    Unresolved,
}
stub_newtype!(SessionToolVisibilityDelta);
stub_newtype!(ToolFilter);
stub_newtype!(ToolVisibilityWitness);
stub_newtype!(Generation);

pub use crate::types::{CommsRuntimeId, McpServerId, MobId, PeerCorrelationId};

/// Typed outbound peer-request state (catalog DSL twin).
///
/// Unit variants only: failure reason and terminal disposition travel on the
/// companion fields of the DSL input / effect, not in the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OutboundPeerRequestState {
    #[default]
    Sent,
    AcceptedProgress,
    Completed,
    Failed,
    TimedOut,
}

/// Typed inbound peer-request state (catalog DSL twin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InboundPeerRequestState {
    #[default]
    Received,
    Replied,
}

/// Typed terminal disposition for an outbound peer response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerTerminalDisposition {
    #[default]
    Completed,
    Failed,
}

/// Typed lifecycle state of an interaction stream reservation (catalog DSL twin).
///
/// Owns whether a reserved subscriber/stream channel is still claimable
/// (`Reserved`), live with an attached consumer (`Attached`), or terminal
/// (`Completed`, `Expired`, `ClosedEarly`). The shell-side
/// `interaction_stream_registry` projects channels off this map; TTL is
/// shell-owned mechanics, while every state transition is DSL truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InteractionStreamState {
    #[default]
    Reserved,
    Attached,
    Completed,
    Expired,
    ClosedEarly,
}

/// Per-server MCP connection lifecycle state.
///
/// Unit variants only: the catalog DSL (TLC-facing twin) does not model
/// variant payloads in state. The `Failed` category is carried through the
/// `McpServerFailed` input and the `McpServerStateChanged` effect with an
/// `error: String` companion field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum McpServerState {
    #[default]
    PendingConnect,
    Connected,
    Failed,
    Disconnected,
}

/// Per-session realtime binding-state lifecycle.
///
/// Unit variants only — this enum models the binding-state machine as a
/// closed set of phases carried inside `MeerkatMachine` state. The default
/// (`Unbound`) is paired with `realtime_binding_authority_epoch == None`
/// by the `realtime_binding_epoch_consistency` invariant.
///
/// Default serde tagging reuses the variant names as string values
/// (`"Unbound"`, `"BindingNotReady"`, `"BindingReady"`, `"ReplacementPending"`)
/// to preserve wire-format compatibility with earlier stringly-typed clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeBindingState {
    #[default]
    Unbound,
    BindingNotReady,
    BindingReady,
    ReplacementPending,
}

/// Product-turn lifecycle phase for a provider-managed realtime session
/// (U9 / dogma #4).
///
/// Unit variants — collapses the old shell-local boolean triple
/// (`product_turn_in_flight`, `product_turn_committed`,
/// `product_output_started`) into a closed five-phase lifecycle owned
/// by the DSL. The `Preemptible` phase corresponds exactly to the "both
/// committed and output-started" conjunction that the realtime shell
/// used to compute via `should_preempt_product_turn_on_input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeProductTurnPhase {
    #[default]
    Idle,
    AwaitingProgress,
    Committed,
    OutputStarted,
    Preemptible,
}

/// Realtime provider-session projection freshness (dogma round 2, U-C /
/// dogma #1, #3, #13, #20).
///
/// Catalog twin of [`crate::meerkat_machine::dsl::RealtimeProjectionFreshness`].
/// Unit variants — the frontier watermark is carried in the companion
/// state field `realtime_projection_frontier_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeProjectionFreshness {
    #[default]
    Clean,
    StaleDeferred,
    StaleImmediate,
}

/// Realtime reconnect-policy classification (dogma round 2, U-C /
/// dogma #1, #3, #18, #20).
///
/// Catalog twin of [`crate::meerkat_machine::dsl::RealtimeReconnectPolicy`].
/// Replaces the shell-local boolean pair (`client_has_submitted_input`,
/// `last_turn_terminally_completed`) with a typed classification owned by
/// the DSL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeReconnectPolicy {
    #[default]
    CleanExit,
    ReattachAndRecover,
}

/// Peer-ingress transport capability ownership kind (W2-G / issue #264).
///
/// Unit variants — the DSL's tagged-union encoding pairs this kind with the
/// companion fields `peer_ingress_comms_runtime_id` and
/// `peer_ingress_mob_id`; the `peer_ingress_owner_consistency` invariant
/// enforces pairing. Silent downgrade `MobOwned` → `SessionOwned` is
/// impossible by construction: `AttachSessionIngress` only fires from
/// `Unattached`, and `AttachMobIngress` permits promotion from
/// `Unattached` or `SessionOwned` but never the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressOwnerKind {
    #[default]
    Unattached,
    SessionOwned,
    MobOwned,
}

/// Supervisor-bridge authorization kind (Wave 3 D Row 21).
///
/// Paired with `supervisor_bound_{name, peer_id, address, epoch}` in DSL
/// state; `supervisor_binding_consistency` enforces pairing. Rotation is
/// structural: `BindSupervisor` requires `Unbound`; `AuthorizeSupervisor`
/// requires `Bound`; `RevokeSupervisor` requires `Bound` and returns to
/// `Unbound`. Before Wave 3 D this fact lived as an `Option<AuthorizedSupervisorState>`
/// on the comms drain task's stack — the identity and epoch of the
/// authorized supervisor were helper-local while the corresponding trust
/// edge was router-owned. Moving the authorization discriminant + epoch
/// into DSL state collapses that split ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SupervisorBindingKind {
    #[default]
    Unbound,
    Bound,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OperationKind {
    #[default]
    ToolCall,
    Completion,
}

/// Live-topology reconfigure phase (catalog DSL twin of the runtime
/// `LiveTopologyPhase`). Closed set of phases the LLM-identity-swap pipeline
/// progresses through; the `realtime_binding_*` invariants pair it with the
/// binding authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveTopologyPhase {
    #[default]
    Idle,
    Reconfiguring,
    Detached,
    HostIdentityApplied,
    HostVisibilityApplied,
}

/// Typed work-lane origin for [`MeerkatMachineInput::Ingest`]. Structural
/// mirror of `MobMachine.WorkOrigin` so the cross-machine composition seam
/// binds on a single typed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WorkOrigin {
    #[default]
    External,
    Internal,
    /// Canonical admission entrypoint fired by the runtime control plane
    /// with no surface-level transport or work-lane label.
    Ingest,
}

/// Typed runtime notice classifier for the `RuntimeNotice` effect (catalog
/// DSL twin). Closed set of per-transition runtime lifecycle markers
/// emitted by the runtime-control plane so the shell dispatcher matches
/// exhaustively on a typed discriminant instead of comparing string
/// literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeNoticeKind {
    #[default]
    Drain,
    Reset,
    Stop,
    Exit,
    Recover,
}

/// Track-B (R5) / wave-c C-6r: typed declarative peer endpoint descriptor.
///
/// Carries the three fields the comms runtime needs to install a
/// trusted peer: `name` (human-readable display slug), `peer_id`
/// (Ed25519 pubkey-derived UUIDv5), `address` (transport URL).
/// Wave-c C-6r retypes the fields from bare `String` to typed
/// newtypes — `PeerName`, `PeerId`, `PeerAddress` — mirroring the
/// runtime-side twin at
/// `meerkat-runtime/src/meerkat_machine/dsl.rs::PeerEndpoint`. The
/// two copies are required to stay structurally equivalent;
/// the `peer_endpoint_structural_equivalence` tripwire in this
/// crate's test harness asserts typed fields at both sites.
///
/// Equality is component-wise. `PartialOrd` + `Ord` enable
/// `BTreeSet<PeerEndpoint>` storage; `Hash` is retained for
/// `HashSet` use at call sites that prefer it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
}

impl PeerEndpoint {
    pub fn new(
        name: impl Into<PeerName>,
        peer_id: impl Into<PeerId>,
        address: impl Into<PeerAddress>,
    ) -> Self {
        Self {
            name: name.into(),
            peer_id: peer_id.into(),
            address: address.into(),
        }
    }
}

/// Schema-local newtype for a peer display name. Mirrors the shape
/// that `meerkat_core::comms::PeerName` exposes across the core seam;
/// the schema catalog keeps a DSL-local copy so validation can see a
/// consistent opaque-struct shape without taking a dependency on
/// `meerkat-core`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerName(pub String);

impl<T: Into<String>> From<T> for PeerName {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl PeerName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Schema-local newtype for the canonical routing identity of a peer
/// (the UUIDv5-derived `PeerId` carried by `meerkat_core::comms::PeerId`).
/// Stored as a slug string so `MachineSchema` validation sees a stable
/// opaque shape; call sites convert to/from the core typed form at the
/// runtime seam.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerId(pub String);

impl<T: Into<String>> From<T> for PeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl PeerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Schema-local newtype for a peer transport endpoint URL. Mirrors the
/// shape of `meerkat_core::comms::PeerAddress` at the core seam; keeps
/// the DSL-local schema free of a meerkat-core dependency.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerAddress(pub String);

impl<T: Into<String>> From<T> for PeerAddress {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl PeerAddress {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// =====================================================================
// Tests
// =====================================================================
