//! Live request ownership. Physical persistence and invocation are realized by
//! the runtime only after the generated successor has committed.

use super::OptionValueExt;

#[macro_export]
macro_rules! live_request_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        pub use meerkat_core::ToolMutationClass;
        pub use meerkat_core::execution_scope::ScopedEffectKind;
        pub use meerkat_core::execution_scope::ScopedTokenAccountingStatus;
        pub use meerkat_core::live_execution::request::LiveRequestEvidenceKind;
        pub use meerkat_core::live_execution::request::LiveRequestCancellationReason;

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveRequestPhase {
            #[default]
            Reserved,
            Admitted,
            Running,
            Suspended,
            Terminal,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum LiveSourceRefusal {
            #[default]
            Empty,
            Gap,
            Budget,
            Permission,
            IngressClosed,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveEffectPhase {
            #[default]
            Claimed,
            Succeeded,
            Failed,
            Cancelled,
            Unknown,
            NotStarted,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveCompletionCreditSchema {
            #[default]
            #[serde(rename = "completion_credits_v1")]
            V1,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveRecoveryInputPhase {
            #[default]
            Queued,
            Staged,
            Applied,
            Terminal,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveRecoveryApplicationEvidence {
            #[default]
            Unobserved,
            NotApplicable,
            NotApplied,
            Applied,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveInputRecoveryPurpose {
            #[default]
            NormalizeColdInput,
            ObserveUnfinishedInput,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum LiveInputRecoveryDisposition {
            #[default]
            HoldUnresolvedRun,
            NoBoundRun,
            AppliedBoundary,
            HoldOutstandingEffects,
            HoldCallbackApplication,
            RuntimePending,
        }

        meerkat_machine_dsl::machine! {
            machine LiveRequestMachine {
                version: 25,
                rust: $rust_crate / $rust_module,

                state {
                    lifecycle_phase: LiveRequestOwnerPhase,
                    ingress_open: bool,
                    grant_id: String,
                    grant_generation: u64,
                    grant_expiry: u64,
                    grant_revoked: bool,
                    executor_binding: String,
                    grant_record: String,
                    grant_profile_revision: String,
                    grant_evidence: Set<Enum<LiveRequestEvidenceKind>>,
                    grant_mutations: Set<Enum<ToolMutationClass>>,
                    grant_tools_restricted: bool,
                    grant_tools: Set<String>,
                    grant_max_requests: u64,
                    grant_max_concurrent_requests: u64,
                    grant_max_effects: u64,
                    grant_max_tokens: u64,
                    grant_max_duration_ms: u64,
                    grant_admitted_requests: Set<String>,
                    grant_active_requests: Set<String>,
                    request_ids: Set<String>,
                    request_phases: Map<String, Enum<LiveRequestPhase>>,
                    request_sources: Map<String, String>,
                    source_requests: Map<String, String>,
                    source_refusals: Map<String, Enum<LiveSourceRefusal>>,
                    source_refusal_payloads: Map<String, String>,
                    request_payloads: Map<String, String>,
                    request_evidence: Map<String, Enum<LiveRequestEvidenceKind>>,
                    request_grants: Map<String, String>,
                    request_grant_records: Map<String, String>,
                    request_generations: Map<String, u64>,
                    request_executors: Map<String, String>,
                    request_inputs: Map<String, String>,
                    request_admission_commits: Map<String, String>,
                    request_ingress_generations: Map<String, u64>,
                    admitted_requests: Set<String>,
                    request_completion_obligations: Set<String>,
                    request_credit_records: Map<String, u64>,
                    request_credit_bytes: Map<String, u64>,
                    request_credit_snapshot_ceiling: Map<String, u64>,
                    request_credit_spent_records: Map<String, u64>,
                    request_credit_spent_bytes: Map<String, u64>,
                    request_terminal_sequences: Map<String, u64>,
                    request_terminal_digests: Map<String, String>,
                    request_ordinary_completion_digests: Map<String, String>,
                    request_runs: Map<String, String>,
                    run_requests: Map<String, String>,
                    run_inputs: Map<String, String>,
                    run_admission_commits: Map<String, String>,
                    run_scopes: Map<String, String>,
                    scope_runs: Map<String, String>,
                    bound_requests: Set<String>,
                    run_scope_records: Map<String, String>,
                    run_callback_records: Map<String, String>,
                    run_callback_receipts: Map<String, String>,
                    run_callback_claims: Map<String, Set<String>>,
                    run_callback_sequences: Map<String, u64>,
                    run_callback_digests: Map<String, String>,
                    run_predecessors: Map<String, String>,
                    run_ordinals: Map<String, u64>,
                    run_successors: Map<String, String>,
                    run_continuation_inputs: Map<String, String>,
                    run_continuation_admission_commits: Map<String, String>,
                    run_continuation_result_digests: Map<String, String>,
                    run_continuation_stage_credits: Map<String, u64>,
                    run_callback_application_claimed: Map<String, bool>,
                    request_parents: Map<String, String>,
                    cancelled_requests: Set<String>,
                    remaining_effects: Map<String, u64>,
                    request_known_tokens: Map<String, u64>,
                    claim_ids: Set<String>,
                    claim_requests: Map<String, String>,
                    claim_runs: Map<String, String>,
                    claim_targets: Map<String, String>,
                    claim_records: Map<String, String>,
                    claim_effects: Map<String, String>,
                    claim_kinds: Map<String, Enum<ScopedEffectKind>>,
                    claim_chains: Map<String, String>,
                    claim_attempts: Map<String, u64>,
                    claim_retry_eligible: Map<String, bool>,
                    chain_latest_claims: Map<String, String>,
                    claim_known_tokens: Map<String, u64>,
                    claim_accounting_status: Map<String, Enum<ScopedTokenAccountingStatus>>,
                    claim_accounting_records: Map<String, String>,
                    claim_policy_revisions: Map<String, String>,
                    claim_phases: Map<String, Enum<LiveEffectPhase>>,
                    spent_effects: Set<String>,
                    completion_credit_schema: Enum<LiveCompletionCreditSchema>,
                    claim_credit_records: Map<String, u64>,
                    claim_credit_bytes: Map<String, u64>,
                    claim_credit_minimum_record_charge: Map<String, u64>,
                    claim_credit_maximum_record_charge: Map<String, u64>,
                    claim_credit_snapshot_ceiling: Map<String, u64>,
                    claim_credit_spent_records: Map<String, u64>,
                    claim_credit_spent_bytes: Map<String, u64>,
                    claim_terminal_sequences: Map<String, u64>,
                    claim_terminal_digests: Map<String, String>,
                    source_cancellations: Map<String, Enum<LiveRequestCancellationReason>>,
                }

                init(Ready) {
                    ingress_open = false,
                    grant_id = "",
                    grant_generation = 0,
                    grant_expiry = 0,
                    grant_revoked = true,
                    executor_binding = "",
                    grant_record = "",
                    grant_profile_revision = "",
                    grant_evidence = EmptySet,
                    grant_mutations = EmptySet,
                    grant_tools_restricted = true,
                    grant_tools = EmptySet,
                    grant_max_requests = 0,
                    grant_max_concurrent_requests = 0,
                    grant_max_effects = 0,
                    grant_max_tokens = 0,
                    grant_max_duration_ms = 0,
                    grant_admitted_requests = EmptySet,
                    grant_active_requests = EmptySet,
                    request_ids = EmptySet,
                    request_phases = EmptyMap,
                    request_sources = EmptyMap,
                    source_requests = EmptyMap,
                    source_refusals = EmptyMap,
                    source_refusal_payloads = EmptyMap,
                    request_payloads = EmptyMap,
                    request_evidence = EmptyMap,
                    request_grants = EmptyMap,
                    request_grant_records = EmptyMap,
                    request_generations = EmptyMap,
                    request_executors = EmptyMap,
                    request_inputs = EmptyMap,
                    request_admission_commits = EmptyMap,
                    request_ingress_generations = EmptyMap,
                    admitted_requests = EmptySet,
                    request_completion_obligations = EmptySet,
                    request_credit_records = EmptyMap,
                    request_credit_bytes = EmptyMap,
                    request_credit_snapshot_ceiling = EmptyMap,
                    request_credit_spent_records = EmptyMap,
                    request_credit_spent_bytes = EmptyMap,
                    request_terminal_sequences = EmptyMap,
                    request_terminal_digests = EmptyMap,
                    request_ordinary_completion_digests = EmptyMap,
                    request_runs = EmptyMap,
                    run_requests = EmptyMap,
                    run_inputs = EmptyMap,
                    run_admission_commits = EmptyMap,
                    run_scopes = EmptyMap,
                    scope_runs = EmptyMap,
                    bound_requests = EmptySet,
                    run_scope_records = EmptyMap,
                    run_callback_records = EmptyMap,
                    run_callback_receipts = EmptyMap,
                    run_callback_claims = EmptyMap,
                    run_callback_sequences = EmptyMap,
                    run_callback_digests = EmptyMap,
                    run_predecessors = EmptyMap,
                    run_ordinals = EmptyMap,
                    run_successors = EmptyMap,
                    run_continuation_inputs = EmptyMap,
                    run_continuation_admission_commits = EmptyMap,
                    run_continuation_result_digests = EmptyMap,
                    run_continuation_stage_credits = EmptyMap,
                    run_callback_application_claimed = EmptyMap,
                    request_parents = EmptyMap,
                    cancelled_requests = EmptySet,
                    remaining_effects = EmptyMap,
                    request_known_tokens = EmptyMap,
                    claim_ids = EmptySet,
                    claim_requests = EmptyMap,
                    claim_runs = EmptyMap,
                    claim_targets = EmptyMap,
                    claim_records = EmptyMap,
                    claim_effects = EmptyMap,
                    claim_kinds = EmptyMap,
                    claim_chains = EmptyMap,
                    claim_attempts = EmptyMap,
                    claim_retry_eligible = EmptyMap,
                    chain_latest_claims = EmptyMap,
                    claim_known_tokens = EmptyMap,
                    claim_accounting_status = EmptyMap,
                    claim_accounting_records = EmptyMap,
                    claim_policy_revisions = EmptyMap,
                    claim_phases = EmptyMap,
                    spent_effects = EmptySet,
                    completion_credit_schema = LiveCompletionCreditSchema::V1,
                    claim_credit_records = EmptyMap,
                    claim_credit_bytes = EmptyMap,
                    claim_credit_minimum_record_charge = EmptyMap,
                    claim_credit_maximum_record_charge = EmptyMap,
                    claim_credit_snapshot_ceiling = EmptyMap,
                    claim_credit_spent_records = EmptyMap,
                    claim_credit_spent_bytes = EmptyMap,
                    claim_terminal_sequences = EmptyMap,
                    claim_terminal_digests = EmptyMap,
                    source_cancellations = EmptyMap,
                }

                terminal []

                phase LiveRequestOwnerPhase { Ready }

                input LiveRequestInput {
                    Activate {
                        grant_id: String,
                        generation: u64,
                        expires_at: u64,
                        executor: String,
                        record: String,
                        profile_revision: String,
                        evidence: Set<Enum<LiveRequestEvidenceKind>>,
                        mutations: Set<Enum<ToolMutationClass>>,
                        tools_restricted: bool,
                        tools: Set<String>,
                        max_requests: u64,
                        max_concurrent_requests: u64,
                        max_effects: u64,
                        max_tokens: u64,
                        max_duration_ms: u64,
                        now: u64,
                    },
                    Reserve {
                        request_id: String,
                        source: String,
                        payload: String,
                        evidence: Enum<LiveRequestEvidenceKind>,
                        profile_revision: String,
                        parent_scope: String,
                        grant_id: String,
                        generation: u64,
                        executor: String,
                        now: u64,
                        credit_records: u64,
                        credit_bytes: u64,
                        snapshot_ceiling: u64,
                        content_complete: bool,
                        content_discontinuous: bool,
                        content_empty: bool,
                        content_fits: bool,
                    },
                    Admit {
                        request_id: String,
                        source: String,
                        payload: String,
                        input_id: String,
                        admission_commit: String,
                        profile_revision: String,
                        ingress_generation: u64,
                        source_ingress_open: bool,
                        credit_records: u64,
                        credit_bytes: u64,
                        snapshot_ceiling: u64,
                        now: u64,
                    },
                    ObserveAdmission {
                        request_id: String,
                        source: String,
                        payload: String,
                        input_id: String,
                        admission_commit: String,
                        grant_id: String,
                        generation: u64,
                        executor: String,
                        ingress_generation: u64,
                    },
                    Stage {
                        request_id: String,
                        input_id: String,
                        admission_commit: String,
                        run_id: String,
                        scope_id: String,
                        scope_record: String,
                        executor: String,
                        profile_revision: String,
                        now: u64,
                    },
                    RestoreScope {
                        request_id: String,
                        input_id: String,
                        admission_commit: String,
                        run_id: String,
                        scope_id: String,
                        scope_record: String,
                        parent_scope: String,
                        executor: String,
                        profile_revision: String,
                        now: u64,
                    },
                    AdmitCallbackContinuation {
                        request_id: String, run_id: String, callback_record: String,
                        result_digest: String, input_id: String, admission_commit: String,
                        executor: String, profile_revision: String,
                        stage_credit_bytes: u64, now: u64,
                    },
                    ObserveCallbackContinuation {
                        request_id: String, run_id: String, callback_record: String,
                        result_digest: String, input_id: String, admission_commit: String,
                    },
                    StageCallbackContinuation {
                        request_id: String, previous_run_id: String,
                        input_id: String, admission_commit: String, result_digest: String,
                        run_id: String, scope_id: String, scope_record: String,
                        executor: String, profile_revision: String, now: u64,
                    },
                    ClaimCallbackApplication {
                        request_id: String, run_id: String, input_id: String,
                        admission_commit: String, scope_id: String, scope_record: String,
                        callback_record: String, result_digest: String,
                        executor: String, profile_revision: String, now: u64,
                    },
                    ClaimEffect {
                        request_id: String,
                        input_id: String,
                        admission_commit: String,
                        run_id: String,
                        scope_id: String,
                        scope_record: String,
                        parent_scope: String,
                        executor: String,
                        claim_id: String,
                        claim_record: String,
                        effect_id: String,
                        chain_id: String,
                        attempt: u64,
                        target: String,
                        kind: Enum<ScopedEffectKind>,
                        tool: String,
                        mutation: Enum<ToolMutationClass>,
                        profile_revision: String,
                        policy_revision: String,
                        policy_permits: bool,
                        credit_schema: Enum<LiveCompletionCreditSchema>,
                        credit_records: u64,
                        credit_bytes: u64,
                        minimum_record_charge: u64,
                        maximum_record_charge: u64,
                        snapshot_ceiling: u64,
                        available_records: u64,
                        available_bytes: u64,
                        now: u64,
                    },
                    SettleEffect {
                        claim_id: String,
                        request_id: String,
                        run_id: String,
                        target: String,
                        outcome: Enum<LiveEffectPhase>,
                        completion_records: u64,
                        completion_bytes: u64,
                        completion_sequence: u64,
                        completion_digest: String,
                        local_noninvocation_proven: bool,
                        token_accounting_status: Enum<ScopedTokenAccountingStatus>,
                        token_accounting_record: String,
                        observed_tokens: u64,
                    },
                    Cancel { request_id: String },
                    Revoke { grant_id: String, generation: u64 },
                    FenceExecutor { executor: String },
                    CloseIngress {},
                    Suspend {
                        request_id: String, run_id: String, callback_record: String,
                        ordinary_completion_digest: String, callback_claims: Set<String>,
                        spent_records: Map<String, u64>, spent_bytes: Map<String, u64>,
                        completion_sequence: u64, completion_digest: String
                    },
                    ObserveCallbackSuspension {
                        request_id: String, run_id: String, callback_record: String,
                        ordinary_completion_digest: String, callback_claims: Set<String>,
                        completion_digest: String
                    },
                    Complete {
                        request_id: String,
                        run_id: String,
                        input_id: String,
                        ordinary_completion_digest: String,
                        completion_records: u64,
                        completion_bytes: u64,
                        completion_sequence: u64,
                        completion_digest: String,
                    },
                    ObserveRequestCompletion {
                        request_id: String,
                        run_id: String,
                        input_id: String,
                        ordinary_completion_digest: String,
                    },
                    ObserveEffectSettlement {
                        claim_id: String,
                        request_id: String,
                        run_id: String,
                        target: String,
                        outcome: Enum<LiveEffectPhase>,
                        completion_digest: String,
                    },
                    ResolveModelAttempt {
                        request_id: String,
                        run_id: String,
                        scope_id: String,
                        chain_id: String,
                        now: u64,
                    },
                    ResolveInputRecovery {
                        request_id: String,
                        input_id: String,
                        run_id: String,
                        source: String,
                        observed_phase: Enum<LiveRecoveryInputPhase>,
                        boundary_committed: bool,
                        application_evidence: Enum<LiveRecoveryApplicationEvidence>,
                        purpose: Enum<LiveInputRecoveryPurpose>,
                        runtime_run_current: bool,
                    },
                    CompleteRunless {
                        request_id: String,
                        input_id: String,
                        ordinary_completion_digest: String,
                        completion_records: u64,
                        completion_bytes: u64,
                        completion_sequence: u64,
                        completion_digest: String,
                    },
                    ObserveRunlessCompletion {
                        request_id: String,
                        input_id: String,
                        ordinary_completion_digest: String,
                    },
                    CancelSource {
                        source: String,
                        reason: Enum<LiveRequestCancellationReason>,
                    },
                    CompleteUnstagedContinuation {
                        request_id: String, previous_run_id: String, input_id: String,
                        ordinary_completion_digest: String,
                        completion_records: u64, completion_bytes: u64,
                        completion_sequence: u64, completion_digest: String,
                    },
                    ObserveUnstagedContinuationCompletion {
                        request_id: String, previous_run_id: String, input_id: String,
                        ordinary_completion_digest: String,
                    },
                    ObserveCancelledCallback {
                        request_id: String, run_id: String, callback_record: String,
                        ordinary_completion_digest: String,
                        callback_claims: Set<String>, completion_digest: String,
                        reason: Enum<LiveRequestCancellationReason>,
                    },
                }

                effect LiveRequestEffect {
                    ActivationChanged { grant_id: String, generation: u64, record: String },
                    SourceReserved { request_id: String, source: String, payload: String },
                    SourceRefused { source: String, payload: String, reason: Enum<LiveSourceRefusal> },
                    InputAdmitted { request_id: String, input_id: String, admission_commit: String },
                    AdmissionObserved { request_id: String, input_id: String, admission_commit: String },
                    CallbackContinuationAdmitted {
                        request_id: String, run_id: String, input_id: String,
                        admission_commit: String, result_digest: String,
                    },
                    CallbackContinuationObserved {
                        request_id: String, run_id: String, input_id: String,
                        admission_commit: String, result_digest: String,
                    },
                    CallbackApplicationClaimed {
                        request_id: String, run_id: String, scope_id: String,
                        callback_record: String, result_digest: String,
                    },
                    RunScopeBound { request_id: String, run_id: String, scope_id: String },
                    ScopeRestored { request_id: String, run_id: String, scope_id: String },
                    ModelAttemptResolved { chain_id: String, attempt: u64 },
                    ModelTokenBudgetExhausted { used: u64, limit: u64 },
                    RequestCompletionObserved {
                        request_id: String,
                        completion_sequence: u64,
                        completion_digest: String,
                    },
                    EffectStartClaimed {
                        claim_id: String,
                        request_id: String,
                        run_id: String,
                        target: String,
                    },
                    EffectSettled { claim_id: String, outcome: Enum<LiveEffectPhase> },
                    RequestCancellationRequired { request_id: String },
                    GrantRevoked { grant_id: String, generation: u64 },
                    ExecutorFenced { executor: String },
                    IngressClosed {},
                    RequestSuspended { request_id: String, run_id: String },
                    RequestCompleted { request_id: String, run_id: String },
                    EffectSettlementObserved {
                        claim_id: String,
                        outcome: Enum<LiveEffectPhase>,
                        completion_sequence: u64,
                        completion_digest: String,
                    },
                    InputRecoveryResolved {
                        request_id: String,
                        input_id: String,
                        run_id: String,
                        disposition: Enum<LiveInputRecoveryDisposition>,
                    },
                    RunlessRequestCompleted { request_id: String, input_id: String },
                    SourceCancellationRetained {
                        source: String,
                        reason: Enum<LiveRequestCancellationReason>,
                    },
                    CancelledCallbackHeld {
                        request_id: String, run_id: String,
                        reason: Enum<LiveRequestCancellationReason>,
                    },
                }

                disposition ActivationChanged => local seam SurfaceResultAlignment,
                disposition CancelledCallbackHeld => local seam SurfaceResultAlignment,
                disposition SourceReserved => local seam SurfaceResultAlignment,
                disposition SourceRefused => local seam SurfaceResultAlignment,
                disposition InputAdmitted => local seam SurfaceResultAlignment,
                disposition AdmissionObserved => local seam SurfaceResultAlignment,
                disposition CallbackContinuationAdmitted => local seam SurfaceResultAlignment,
                disposition CallbackContinuationObserved => local seam SurfaceResultAlignment,
                disposition CallbackApplicationClaimed => external handoff scoped_callback_application seam OwnerRealizationPlusFeedback,
                disposition RunScopeBound => local seam SurfaceResultAlignment,
                disposition ScopeRestored => local seam SurfaceResultAlignment,
                disposition ModelAttemptResolved => local seam SurfaceResultAlignment,
                disposition ModelTokenBudgetExhausted => local seam SurfaceResultAlignment,
                disposition EffectStartClaimed => external handoff scoped_effect_start seam OwnerRealizationPlusFeedback,
                disposition EffectSettled => local seam SurfaceResultAlignment,
                disposition EffectSettlementObserved => local seam SurfaceResultAlignment,
                disposition RequestCancellationRequired => external handoff live_request_cancel seam OwnerRealizationPlusFeedback,
                disposition SourceCancellationRetained => local seam SurfaceResultAlignment,
                disposition GrantRevoked => local seam SurfaceResultAlignment,
                disposition ExecutorFenced => local seam SurfaceResultAlignment,
                disposition IngressClosed => local seam SurfaceResultAlignment,
                disposition RequestSuspended => local seam SurfaceResultAlignment,
                disposition RequestCompleted => local seam SurfaceResultAlignment,
                disposition RequestCompletionObserved => local seam SurfaceResultAlignment,
                disposition InputRecoveryResolved => local seam SurfaceResultAlignment,
                disposition RunlessRequestCompleted => local seam SurfaceResultAlignment,

                transition ResolveUnboundInputRecovery {
                    on input ResolveInputRecovery {
                        request_id, input_id, run_id, source, observed_phase,
                        boundary_committed, application_evidence, purpose, runtime_run_current
                    }
                    guard {
                        self.admitted_requests.contains(request_id)
                        && self.request_sources.get_cloned(request_id) == Some(source)
                        && run_id == ""
                        && observed_phase == LiveRecoveryInputPhase::Queued
                        && (self.request_inputs.get_cloned(request_id) == Some(input_id)
                            || !for_all(previous in self.run_continuation_inputs.keys(),
                                self.run_requests.get_cloned(previous) != Some(request_id)
                                || self.run_continuation_inputs.get_cloned(previous) != Some(input_id)))
                        && for_all(bound in self.run_inputs.keys(),
                            self.run_inputs.get_cloned(bound) != Some(input_id))
                    }
                    update {}
                    to Ready
                    emit InputRecoveryResolved {
                        request_id: request_id, input_id: input_id, run_id: run_id,
                        disposition: LiveInputRecoveryDisposition::NoBoundRun
                    }
                }

                transition ResolveBoundInputRecovery {
                    on input ResolveInputRecovery {
                        request_id, input_id, run_id, source, observed_phase,
                        boundary_committed, application_evidence, purpose, runtime_run_current
                    }
                    guard {
                        self.admitted_requests.contains(request_id)
                        && self.request_sources.get_cloned(request_id) == Some(source)
                        && run_id != ""
                        && self.run_requests.get_cloned(run_id) == Some(request_id)
                        && self.run_inputs.get_cloned(run_id) == Some(input_id)
                        && self.run_scopes.contains_key(run_id)
                        && self.run_scope_records.contains_key(run_id)
                    }
                    update {}
                    to Ready
                    emit InputRecoveryResolved {
                        request_id: request_id, input_id: input_id, run_id: run_id,
                        disposition: if observed_phase == LiveRecoveryInputPhase::Applied
                            && boundary_committed {
                            LiveInputRecoveryDisposition::AppliedBoundary
                        } else {
                            if purpose == LiveInputRecoveryPurpose::ObserveUnfinishedInput
                                && runtime_run_current
                                && observed_phase == LiveRecoveryInputPhase::Staged
                                && self.request_runs.get_cloned(request_id) == Some(run_id) {
                                LiveInputRecoveryDisposition::RuntimePending
                            } else {
                                if self.run_callback_application_claimed.get_cloned(run_id) == Some(true)
                                    && application_evidence != LiveRecoveryApplicationEvidence::Applied {
                                    LiveInputRecoveryDisposition::HoldCallbackApplication
                                } else {
                                    if !for_all(claim in self.claim_runs.keys(),
                                        self.claim_runs.get_cloned(claim) != Some(run_id)
                                        || self.claim_phases.get_cloned(claim) != Some(LiveEffectPhase::Claimed)) {
                                        LiveInputRecoveryDisposition::HoldOutstandingEffects
                                    } else {
                                        LiveInputRecoveryDisposition::HoldUnresolvedRun
                                    }
                                }
                            }
                        }
                    }
                }

                transition ActivateFreshGrant {
                    on input Activate {
                        grant_id, generation, expires_at, executor, record, profile_revision,
                        evidence, mutations, tools_restricted, tools, max_requests,
                        max_concurrent_requests, max_effects, max_tokens, max_duration_ms, now
                    }
                    guard {
                        grant_id != "" && executor != ""
                        && record != "" && profile_revision != ""
                        && generation > self.grant_generation
                        && expires_at > now
                        && evidence.len() > 0
                        && max_requests > 0
                        && max_concurrent_requests > 0
                        && max_concurrent_requests <= max_requests
                        && max_effects > 0 && max_tokens > 0 && max_duration_ms > 0
                    }
                    update {
                        self.grant_id = grant_id;
                        self.grant_generation = generation;
                        self.grant_expiry = expires_at;
                        self.grant_revoked = false;
                        self.executor_binding = executor;
                        self.grant_record = record;
                        self.grant_profile_revision = profile_revision;
                        self.grant_evidence = evidence;
                        self.grant_mutations = mutations;
                        self.grant_tools_restricted = tools_restricted;
                        self.grant_tools = tools;
                        self.grant_max_requests = max_requests;
                        self.grant_max_concurrent_requests = max_concurrent_requests;
                        self.grant_max_effects = max_effects;
                        self.grant_max_tokens = max_tokens;
                        self.grant_max_duration_ms = max_duration_ms;
                        self.grant_admitted_requests = EmptySet;
                        self.grant_active_requests = EmptySet;
                        self.ingress_open = true;
                    }
                    to Ready
                    emit ActivationChanged { grant_id: grant_id, generation: generation, record: record }
                }

                transition ReserveNewSource {
                    on input Reserve {
                        request_id, source, payload, evidence, profile_revision, parent_scope,
                        grant_id, generation, executor, now,
                        credit_records, credit_bytes, snapshot_ceiling,
                        content_complete, content_discontinuous, content_empty, content_fits
                    }
                    guard {
                        self.ingress_open && self.grant_revoked == false
                        && now < self.grant_expiry
                        && request_id != "" && source != "" && payload != ""
                        && grant_id == self.grant_id
                        && generation == self.grant_generation
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                        && self.grant_evidence.contains(evidence)
                        && self.request_ids.contains(request_id) == false
                        && self.source_requests.contains_key(source) == false
                        && self.source_refusals.contains_key(source) == false
                        && self.source_cancellations.contains_key(source) == false
                        && credit_records > 0 && credit_bytes > 0 && snapshot_ceiling > 0
                        && content_complete && !content_discontinuous && !content_empty && content_fits
                    }
                    update {
                        self.request_ids.insert(request_id);
                        self.request_phases.insert(request_id, LiveRequestPhase::Reserved);
                        self.request_sources.insert(request_id, source);
                        self.source_requests.insert(source, request_id);
                        self.request_payloads.insert(request_id, payload);
                        self.request_evidence.insert(request_id, evidence);
                        self.request_grants.insert(request_id, grant_id);
                        self.request_grant_records.insert(request_id, self.grant_record);
                        self.request_generations.insert(request_id, generation);
                        self.request_executors.insert(request_id, executor);
                        self.request_parents.insert(request_id, parent_scope);
                        self.remaining_effects.insert(request_id, self.grant_max_effects);
                        self.request_known_tokens.insert(request_id, 0);
                        self.request_credit_records.insert(request_id, credit_records);
                        self.request_completion_obligations.insert(request_id);
                        self.request_credit_bytes.insert(request_id, credit_bytes);
                        self.request_credit_snapshot_ceiling.insert(request_id, snapshot_ceiling);
                        self.request_credit_spent_records.insert(request_id, 0);
                        self.request_credit_spent_bytes.insert(request_id, 0);
                        self.request_terminal_sequences.insert(request_id, 0);
                        self.request_terminal_digests.insert(request_id, "");
                        self.request_ordinary_completion_digests.insert(request_id, "");
                    }
                    to Ready
                    emit SourceReserved { request_id: request_id, source: source, payload: payload }
                }

                transition RefuseNewSource {
                    on input Reserve {
                        request_id, source, payload, evidence, profile_revision, parent_scope,
                        grant_id, generation, executor, now,
                        credit_records, credit_bytes, snapshot_ceiling,
                        content_complete, content_discontinuous, content_empty, content_fits
                    }
                    guard {
                        source != "" && payload != "" && content_complete
                        && !self.source_requests.contains_key(source)
                        && !self.source_refusals.contains_key(source)
                        && !self.source_cancellations.contains_key(source)
                        && (!self.ingress_open || content_discontinuous || !content_fits || content_empty
                            || self.grant_revoked || now >= self.grant_expiry
                            || grant_id != self.grant_id || generation != self.grant_generation
                            || executor != self.executor_binding
                            || profile_revision != self.grant_profile_revision
                            || !self.grant_evidence.contains(evidence))
                    }
                    update {
                        self.source_refusal_payloads.insert(source, payload);
                        self.source_refusals.insert(source,
                            if !self.ingress_open && self.grant_generation > 0 { LiveSourceRefusal::IngressClosed }
                            else { if content_discontinuous { LiveSourceRefusal::Gap }
                            else { if !content_fits { LiveSourceRefusal::Budget }
                            else { if content_empty { LiveSourceRefusal::Empty }
                            else { LiveSourceRefusal::Permission } } } });
                    }
                    to Ready
                    emit SourceRefused {
                        source: source, payload: payload,
                        reason: self.source_refusals.get_cloned(source).get("value")
                    }
                }

                transition AdmitReservedInput {
                    on input Admit {
                        request_id, source, payload, input_id, admission_commit, profile_revision,
                        ingress_generation, source_ingress_open, credit_records, credit_bytes, snapshot_ceiling, now
                    }
                    guard {
                        self.ingress_open && source_ingress_open && self.grant_revoked == false
                        && now < self.grant_expiry
                        && self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") == LiveRequestPhase::Reserved
                        && self.cancelled_requests.contains(request_id) == false
                        && self.request_sources.get_cloned(request_id).get("value") == source
                        && self.request_payloads.get_cloned(request_id).get("value") == payload
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id).get("value") == self.grant_generation
                        && self.request_executors.get_cloned(request_id).get("value") == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                        && input_id != "" && admission_commit != "" && ingress_generation > 0
                        && credit_records > 0 && credit_bytes > 0 && snapshot_ceiling > 0
                        && self.request_credit_records.get_cloned(request_id) == Some(credit_records)
                        && self.request_credit_bytes.get_cloned(request_id) == Some(credit_bytes)
                        && self.request_credit_snapshot_ceiling.get_cloned(request_id) == Some(snapshot_ceiling)
                        && self.grant_admitted_requests.len() < self.grant_max_requests
                        && self.grant_active_requests.len() < self.grant_max_concurrent_requests
                        && for_all(admitted in self.admitted_requests,
                            self.request_inputs.get_cloned(admitted).get("value") != input_id
                            && self.request_admission_commits.get_cloned(admitted).get("value") != admission_commit)
                        && for_all(run in self.run_requests.keys(),
                            self.run_continuation_inputs.get_cloned(run) != Some(input_id)
                            && self.run_continuation_admission_commits.get_cloned(run) != Some(admission_commit))
                    }
                    update {
                        self.request_inputs.insert(request_id, input_id);
                        self.request_admission_commits.insert(request_id, admission_commit);
                        self.request_ingress_generations.insert(request_id, ingress_generation);
                        self.admitted_requests.insert(request_id);
                        self.grant_admitted_requests.insert(request_id);
                        self.grant_active_requests.insert(request_id);
                        self.request_phases.insert(request_id, LiveRequestPhase::Admitted);
                    }
                    to Ready
                    emit InputAdmitted { request_id: request_id, input_id: input_id, admission_commit: admission_commit }
                }

                transition ObserveCommittedAdmission {
                    on input ObserveAdmission {
                        request_id, source, payload, input_id, admission_commit,
                        grant_id, generation, executor, ingress_generation
                    }
                    guard {
                        self.admitted_requests.contains(request_id)
                        && self.request_sources.get_cloned(request_id).get("value") == source
                        && self.request_payloads.get_cloned(request_id).get("value") == payload
                        && self.request_inputs.get_cloned(request_id).get("value") == input_id
                        && self.request_admission_commits.get_cloned(request_id).get("value") == admission_commit
                        && self.request_grants.get_cloned(request_id).get("value") == grant_id
                        && self.request_generations.get_cloned(request_id).get("value") == generation
                        && self.request_executors.get_cloned(request_id).get("value") == executor
                        && self.request_ingress_generations.get_cloned(request_id).get("value") == ingress_generation
                    }
                    update {}
                    to Ready
                    emit AdmissionObserved { request_id: request_id, input_id: input_id, admission_commit: admission_commit }
                }

                transition StageAdmittedRun {
                    on input Stage {
                        request_id, input_id, admission_commit, run_id, scope_id, scope_record, executor,
                        profile_revision, now
                    }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") == LiveRequestPhase::Admitted
                        && self.cancelled_requests.contains(request_id) == false
                        && self.request_inputs.get_cloned(request_id).get("value") == input_id
                        && self.request_admission_commits.get_cloned(request_id).get("value") == admission_commit
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id).get("value") == self.grant_generation
                        && self.request_executors.get_cloned(request_id).get("value") == executor
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                        && run_id != "" && scope_id != "" && scope_record != ""
                        && self.run_requests.contains_key(run_id) == false
                        && self.scope_runs.contains_key(scope_id) == false
                    }
                    update {
                        self.request_runs.insert(request_id, run_id);
                        self.run_requests.insert(run_id, request_id);
                        self.run_inputs.insert(run_id, input_id);
                        self.run_admission_commits.insert(run_id, admission_commit);
                        self.run_scopes.insert(run_id, scope_id);
                        self.scope_runs.insert(scope_id, run_id);
                        self.bound_requests.insert(request_id);
                        self.run_scope_records.insert(run_id, scope_record);
                        self.run_callback_records.insert(run_id, "");
                        self.run_callback_receipts.insert(run_id, "");
                        self.run_callback_claims.insert(run_id, EmptySet);
                        self.run_callback_sequences.insert(run_id, 0);
                        self.run_callback_digests.insert(run_id, "");
                        self.run_predecessors.insert(run_id, "");
                        self.run_ordinals.insert(run_id, 0);
                        self.run_successors.insert(run_id, "");
                        self.run_continuation_inputs.insert(run_id, "");
                        self.run_continuation_admission_commits.insert(run_id, "");
                        self.run_continuation_result_digests.insert(run_id, "");
                        self.run_continuation_stage_credits.insert(run_id, 0);
                        self.run_callback_application_claimed.insert(run_id, false);
                        self.request_phases.insert(request_id, LiveRequestPhase::Running);
                    }
                    to Ready
                    emit RunScopeBound { request_id: request_id, run_id: run_id, scope_id: scope_id }
                }

                transition AdmitExactCallbackContinuation {
                    on input AdmitCallbackContinuation {
                        request_id, run_id, callback_record, result_digest, input_id,
                        admission_commit, executor, profile_revision, stage_credit_bytes, now
                    }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Suspended)
                        && self.cancelled_requests.contains(request_id) == false
                        && self.request_runs.get_cloned(request_id) == Some(run_id)
                        && self.run_requests.get_cloned(run_id) == Some(request_id)
                        && callback_record != ""
                        && self.run_callback_records.get_cloned(run_id) == Some(callback_record)
                        && self.run_continuation_inputs.get_cloned(run_id) == Some("")
                        && self.run_successors.get_cloned(run_id) == Some("")
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id) == Some(self.grant_generation)
                        && self.request_executors.get_cloned(request_id) == Some(executor)
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                        && result_digest.len() == 64 && input_id != "" && admission_commit != ""
                        && stage_credit_bytes > 0
                        && for_all(request in self.admitted_requests,
                            self.request_inputs.get_cloned(request) != Some(input_id)
                            && self.request_admission_commits.get_cloned(request) != Some(admission_commit))
                        && for_all(run in self.run_requests.keys(),
                            self.run_continuation_inputs.get_cloned(run) != Some(input_id)
                            && self.run_continuation_admission_commits.get_cloned(run) != Some(admission_commit))
                    }
                    update {
                        self.run_continuation_inputs.insert(run_id, input_id);
                        self.run_continuation_admission_commits.insert(run_id, admission_commit);
                        self.run_continuation_result_digests.insert(run_id, result_digest);
                        self.run_continuation_stage_credits.insert(run_id, stage_credit_bytes);
                    }
                    to Ready
                    emit CallbackContinuationAdmitted {
                        request_id: request_id, run_id: run_id, input_id: input_id,
                        admission_commit: admission_commit, result_digest: result_digest
                    }
                }

                transition ObserveExactCallbackContinuation {
                    on input ObserveCallbackContinuation {
                        request_id, run_id, callback_record, result_digest, input_id, admission_commit
                    }
                    guard {
                        self.run_requests.get_cloned(run_id) == Some(request_id)
                        && callback_record != "" && input_id != "" && admission_commit != ""
                        && result_digest.len() == 64
                        && self.run_callback_records.get_cloned(run_id) == Some(callback_record)
                        && self.run_continuation_inputs.get_cloned(run_id) == Some(input_id)
                        && self.run_continuation_admission_commits.get_cloned(run_id) == Some(admission_commit)
                        && self.run_continuation_result_digests.get_cloned(run_id) == Some(result_digest)
                    }
                    update {}
                    to Ready
                    emit CallbackContinuationObserved {
                        request_id: request_id, run_id: run_id, input_id: input_id,
                        admission_commit: admission_commit, result_digest: result_digest
                    }
                }

                transition StageExactCallbackContinuation {
                    on input StageCallbackContinuation {
                        request_id, previous_run_id, input_id, admission_commit, result_digest,
                        run_id, scope_id, scope_record, executor, profile_revision, now
                    }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Suspended)
                        && self.cancelled_requests.contains(request_id) == false
                        && self.request_runs.get_cloned(request_id) == Some(previous_run_id)
                        && self.run_requests.get_cloned(previous_run_id) == Some(request_id)
                        && self.run_successors.get_cloned(previous_run_id) == Some("")
                        && self.run_continuation_inputs.get_cloned(previous_run_id) == Some(input_id)
                        && self.run_continuation_admission_commits.get_cloned(previous_run_id) == Some(admission_commit)
                        && self.run_continuation_result_digests.get_cloned(previous_run_id) == Some(result_digest)
                        && self.run_continuation_stage_credits.get_cloned(previous_run_id).get("value") > 0
                        && self.run_ordinals.get_cloned(previous_run_id).get("value") < u64::MAX
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id) == Some(self.grant_generation)
                        && self.request_executors.get_cloned(request_id) == Some(executor)
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                        && input_id != "" && admission_commit != "" && result_digest.len() == 64
                        && run_id != "" && scope_id != "" && scope_record != ""
                        && self.run_requests.contains_key(run_id) == false
                        && self.scope_runs.contains_key(scope_id) == false
                    }
                    update {
                        self.request_runs.insert(request_id, run_id);
                        self.run_requests.insert(run_id, request_id);
                        self.run_inputs.insert(run_id, input_id);
                        self.run_admission_commits.insert(run_id, admission_commit);
                        self.run_scopes.insert(run_id, scope_id);
                        self.scope_runs.insert(scope_id, run_id);
                        self.run_scope_records.insert(run_id, scope_record);
                        self.run_callback_records.insert(run_id, "");
                        self.run_callback_receipts.insert(run_id, "");
                        self.run_callback_claims.insert(run_id, EmptySet);
                        self.run_callback_sequences.insert(run_id, 0);
                        self.run_callback_digests.insert(run_id, "");
                        self.run_predecessors.insert(run_id, previous_run_id);
                        self.run_ordinals.insert(run_id, self.run_ordinals.get_cloned(previous_run_id).get("value") + 1);
                        self.run_successors.insert(previous_run_id, run_id);
                        self.run_successors.insert(run_id, "");
                        self.run_continuation_inputs.insert(run_id, "");
                        self.run_continuation_admission_commits.insert(run_id, "");
                        self.run_continuation_result_digests.insert(run_id, "");
                        self.run_continuation_stage_credits.insert(previous_run_id, 0);
                        self.run_continuation_stage_credits.insert(run_id, 0);
                        self.run_callback_application_claimed.insert(run_id, false);
                        self.request_phases.insert(request_id, LiveRequestPhase::Running);
                    }
                    to Ready
                    emit RunScopeBound { request_id: request_id, run_id: run_id, scope_id: scope_id }
                }

                transition ClaimExactCallbackApplication {
                    on input ClaimCallbackApplication {
                        request_id, run_id, input_id, admission_commit, scope_id, scope_record,
                        callback_record, result_digest, executor, profile_revision, now
                    }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Running)
                        && self.cancelled_requests.contains(request_id) == false
                        && self.request_runs.get_cloned(request_id) == Some(run_id)
                        && self.run_inputs.get_cloned(run_id) == Some(input_id)
                        && self.run_admission_commits.get_cloned(run_id) == Some(admission_commit)
                        && self.run_scopes.get_cloned(run_id) == Some(scope_id)
                        && self.run_scope_records.get_cloned(run_id) == Some(scope_record)
                        && self.run_predecessors.get_cloned(run_id) != Some("")
                        && self.run_callback_records.get_cloned(self.run_predecessors.get_cloned(run_id).get("value"))
                            == Some(callback_record)
                        && self.run_continuation_result_digests.get_cloned(self.run_predecessors.get_cloned(run_id).get("value"))
                            == Some(result_digest)
                        && self.run_callback_application_claimed.get_cloned(run_id) == Some(false)
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id).get("value") == self.grant_generation
                        && self.request_executors.get_cloned(request_id) == Some(executor)
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                    }
                    update { self.run_callback_application_claimed.insert(run_id, true); }
                    to Ready
                    emit CallbackApplicationClaimed {
                        request_id: request_id, run_id: run_id, scope_id: scope_id,
                        callback_record: callback_record, result_digest: result_digest
                    }
                }

                transition RestoreExactRunningScope {
                    on input RestoreScope {
                        request_id, input_id, admission_commit, run_id, scope_id, scope_record, parent_scope,
                        executor, profile_revision, now
                    }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") == LiveRequestPhase::Running
                        && self.cancelled_requests.contains(request_id) == false
                        && self.run_inputs.get_cloned(run_id) == Some(input_id)
                        && self.run_admission_commits.get_cloned(run_id) == Some(admission_commit)
                        && self.request_runs.get_cloned(request_id).get("value") == run_id
                        && self.run_scopes.get_cloned(run_id).get("value") == scope_id
                        && self.run_scope_records.get_cloned(run_id).get("value") == scope_record
                        && self.request_parents.get_cloned(request_id).get("value") == parent_scope
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id).get("value") == self.grant_generation
                        && self.request_executors.get_cloned(request_id).get("value") == executor
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                    }
                    update {}
                    to Ready
                    emit ScopeRestored { request_id: request_id, run_id: run_id, scope_id: scope_id }
                }

                transition ClaimExactCurrentEffect {
                    on input ClaimEffect {
                        request_id, input_id, admission_commit, run_id, scope_id, scope_record, parent_scope,
                        executor, claim_id, claim_record, effect_id, chain_id, attempt, target, kind, tool, mutation,
                        profile_revision, policy_revision, policy_permits,
                        credit_schema, credit_records, credit_bytes,
                        minimum_record_charge, maximum_record_charge, snapshot_ceiling,
                        available_records, available_bytes, now
                    }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") == LiveRequestPhase::Running
                        && self.cancelled_requests.contains(request_id) == false
                        && self.run_inputs.get_cloned(run_id) == Some(input_id)
                        && self.run_admission_commits.get_cloned(run_id) == Some(admission_commit)
                        && self.request_runs.get_cloned(request_id).get("value") == run_id
                        && self.run_scopes.get_cloned(run_id).get("value") == scope_id
                        && self.run_scope_records.get_cloned(run_id).get("value") == scope_record
                        && self.request_parents.get_cloned(request_id).get("value") == parent_scope
                        && self.request_grants.get_cloned(request_id).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request_id).get("value") == self.grant_generation
                        && self.request_executors.get_cloned(request_id).get("value") == executor
                        && executor == self.executor_binding
                        && profile_revision == self.grant_profile_revision
                        && (kind != ScopedEffectKind::ToolDispatch
                            || (tool != ""
                                && self.grant_mutations.contains(mutation)
                                && (self.grant_tools_restricted == false
                                    || self.grant_tools.contains(tool))))
                        && (kind == ScopedEffectKind::ToolDispatch || tool == "")
                        && (kind != ScopedEffectKind::DescendantAdmission
                            || (mutation == ToolMutationClass::Mutating
                                && self.grant_mutations.contains(mutation)))
                        && (kind != ScopedEffectKind::ModelComputation
                            || self.request_known_tokens.get_cloned(request_id).get("value") < self.grant_max_tokens)
                        && self.remaining_effects.get_cloned(request_id).get("value") > 0
                        && claim_id != "" && claim_record != "" && effect_id != "" && target != ""
                        && policy_revision != "" && policy_permits
                        && self.claim_ids.contains(claim_id) == false
                        && self.spent_effects.contains(effect_id) == false
                        && (self.run_predecessors.get_cloned(run_id) == Some("")
                            || self.run_callback_application_claimed.get_cloned(run_id) == Some(true))
                        && chain_id != "" && attempt <= 4294967295
                        && (kind == ScopedEffectKind::ModelComputation
                            || (chain_id == effect_id && attempt == 0))
                        && ((self.chain_latest_claims.contains_key(chain_id) == false && attempt == 0)
                            || (kind == ScopedEffectKind::ModelComputation
                                && self.chain_latest_claims.contains_key(chain_id)
                                && self.claim_kinds.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value"))
                                    == Some(ScopedEffectKind::ModelComputation)
                                && self.claim_requests.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") == request_id
                                && self.claim_runs.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") == run_id
                                && self.claim_retry_eligible.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value")
                                && self.claim_attempts.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") < 4294967295
                                && attempt == self.claim_attempts.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") + 1))
                        && credit_schema == self.completion_credit_schema
                        && credit_records > 1 && credit_records <= available_records
                        && minimum_record_charge > 0
                        && maximum_record_charge >= minimum_record_charge
                        && snapshot_ceiling > 0 && snapshot_ceiling <= available_bytes
                        && credit_records <= (available_bytes - snapshot_ceiling) / maximum_record_charge
                        && credit_bytes == credit_records * maximum_record_charge
                    }
                    update {
                        self.claim_ids.insert(claim_id);
                        self.claim_requests.insert(claim_id, request_id);
                        self.claim_runs.insert(claim_id, run_id);
                        self.claim_targets.insert(claim_id, target);
                        self.claim_records.insert(claim_id, claim_record);
                        self.claim_effects.insert(claim_id, effect_id);
                        self.claim_kinds.insert(claim_id, kind);
                        self.claim_chains.insert(claim_id, chain_id);
                        self.claim_attempts.insert(claim_id, attempt);
                        self.claim_retry_eligible.insert(claim_id, false);
                        self.chain_latest_claims.insert(chain_id, claim_id);
                        self.claim_known_tokens.insert(claim_id, 0);
                        self.claim_accounting_status.insert(claim_id, ScopedTokenAccountingStatus::Pending);
                        self.claim_accounting_records.insert(claim_id, "");
                        self.claim_policy_revisions.insert(claim_id, policy_revision);
                        self.claim_phases.insert(claim_id, LiveEffectPhase::Claimed);
                        self.spent_effects.insert(effect_id);
                        self.remaining_effects.insert(
                            request_id, self.remaining_effects.get_cloned(request_id).get("value") - 1
                        );
                        self.claim_credit_records.insert(claim_id, credit_records);
                        self.claim_credit_bytes.insert(claim_id, credit_bytes);
                        self.claim_credit_minimum_record_charge.insert(claim_id, minimum_record_charge);
                        self.claim_credit_maximum_record_charge.insert(claim_id, maximum_record_charge);
                        self.claim_credit_snapshot_ceiling.insert(claim_id, snapshot_ceiling);
                        self.claim_credit_spent_records.insert(claim_id, 0);
                        self.claim_credit_spent_bytes.insert(claim_id, 0);
                        self.claim_terminal_sequences.insert(claim_id, 0);
                        self.claim_terminal_digests.insert(claim_id, "");
                    }
                    to Ready
                    emit EffectStartClaimed {
                        claim_id: claim_id, request_id: request_id, run_id: run_id, target: target
                    }
                }

                transition SettleClaimedEffect {
                    on input SettleEffect {
                        claim_id, request_id, run_id, target, outcome,
                        completion_records, completion_bytes, completion_sequence, completion_digest,
                        local_noninvocation_proven, token_accounting_status, token_accounting_record, observed_tokens
                    }
                    guard {
                        self.claim_ids.contains(claim_id)
                        && (self.claim_phases.get_cloned(claim_id).get("value") == LiveEffectPhase::Claimed
                            || (self.claim_phases.get_cloned(claim_id).get("value") == LiveEffectPhase::Unknown
                                && outcome != LiveEffectPhase::Unknown && outcome != LiveEffectPhase::NotStarted))
                        && self.claim_requests.get_cloned(claim_id).get("value") == request_id
                        && self.claim_runs.get_cloned(claim_id).get("value") == run_id
                        && self.claim_targets.get_cloned(claim_id).get("value") == target
                        && outcome != LiveEffectPhase::Claimed
                        && (outcome != LiveEffectPhase::NotStarted || local_noninvocation_proven)
                        && completion_records > 0
                        && completion_records <= self.claim_credit_records.get_cloned(claim_id).get("value")
                            - self.claim_credit_spent_records.get_cloned(claim_id).get("value")
                        && completion_bytes <= self.claim_credit_bytes.get_cloned(claim_id).get("value")
                            - self.claim_credit_spent_bytes.get_cloned(claim_id).get("value")
                        && completion_bytes >= completion_records
                            * self.claim_credit_minimum_record_charge.get_cloned(claim_id).get("value")
                        && completion_bytes <= completion_records
                            * self.claim_credit_maximum_record_charge.get_cloned(claim_id).get("value")
                        && (outcome != LiveEffectPhase::Unknown
                            || (completion_records < self.claim_credit_records.get_cloned(claim_id).get("value")
                                    - self.claim_credit_spent_records.get_cloned(claim_id).get("value")
                                && completion_sequence < u64::MAX))
                        && completion_sequence > self.claim_terminal_sequences.get_cloned(claim_id).get("value")
                        && completion_digest.len() == 64
                        && token_accounting_record != ""
                        && token_accounting_status != ScopedTokenAccountingStatus::Pending
                        && ((token_accounting_status == ScopedTokenAccountingStatus::NotApplicable
                                && (self.claim_kinds.get_cloned(claim_id) != Some(ScopedEffectKind::ModelComputation)
                                    || outcome == LiveEffectPhase::NotStarted))
                            || (token_accounting_status != ScopedTokenAccountingStatus::NotApplicable
                                && self.claim_kinds.get_cloned(claim_id) == Some(ScopedEffectKind::ModelComputation)
                                && outcome != LiveEffectPhase::NotStarted))
                        && (token_accounting_status == ScopedTokenAccountingStatus::Measured
                            || token_accounting_status == ScopedTokenAccountingStatus::Disputed
                            || observed_tokens == 0)
                    }
                    update {
                        self.request_known_tokens.insert(request_id,
                            if observed_tokens <= self.claim_known_tokens.get_cloned(claim_id).get("value") {
                                self.request_known_tokens.get_cloned(request_id).get("value")
                            } else {
                                if observed_tokens - self.claim_known_tokens.get_cloned(claim_id).get("value")
                                    > u64::MAX - self.request_known_tokens.get_cloned(request_id).get("value") {
                                    u64::MAX
                                } else {
                                    self.request_known_tokens.get_cloned(request_id).get("value")
                                        + (observed_tokens - self.claim_known_tokens.get_cloned(claim_id).get("value"))
                                }
                            });
                        self.claim_known_tokens.insert(claim_id,
                            if observed_tokens > self.claim_known_tokens.get_cloned(claim_id).get("value") {
                                observed_tokens
                            } else { self.claim_known_tokens.get_cloned(claim_id).get("value") });
                        self.claim_accounting_status.insert(claim_id, token_accounting_status);
                        self.claim_accounting_records.insert(claim_id, token_accounting_record);
                        self.claim_retry_eligible.insert(claim_id,
                            self.claim_phases.get_cloned(claim_id).get("value") == LiveEffectPhase::Claimed
                            && (outcome == LiveEffectPhase::Succeeded || outcome == LiveEffectPhase::Failed));
                        self.claim_phases.insert(claim_id, outcome);
                        self.claim_credit_spent_records.insert(claim_id,
                            self.claim_credit_spent_records.get_cloned(claim_id).get("value") + completion_records);
                        self.claim_credit_spent_bytes.insert(claim_id,
                            self.claim_credit_spent_bytes.get_cloned(claim_id).get("value") + completion_bytes);
                        self.claim_terminal_sequences.insert(claim_id, completion_sequence);
                        self.claim_terminal_digests.insert(claim_id, completion_digest);
                    }
                    to Ready
                    emit EffectSettled { claim_id: claim_id, outcome: outcome }
                }

                    transition ResolveFreshModelAttempt {
                        on input ResolveModelAttempt { request_id, run_id, scope_id, chain_id, now }
                        guard {
                            self.grant_revoked == false && now < self.grant_expiry
                            && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Running)
                            && self.cancelled_requests.contains(request_id) == false
                            && self.request_generations.get_cloned(request_id) == Some(self.grant_generation)
                            && self.request_executors.get_cloned(request_id).get("value") == self.executor_binding
                            && self.request_runs.get_cloned(request_id) == Some(run_id)
                            && self.run_scopes.get_cloned(run_id) == Some(scope_id)
                            && chain_id != "" && self.chain_latest_claims.contains_key(chain_id) == false
                            && self.request_known_tokens.get_cloned(request_id).get("value") < self.grant_max_tokens
                        }
                        update {}
                        to Ready
                        emit ModelAttemptResolved { chain_id: chain_id, attempt: 0 }
                    }

                    transition ResolveConclusiveModelSuccessor {
                        on input ResolveModelAttempt { request_id, run_id, scope_id, chain_id, now }
                        guard {
                            self.grant_revoked == false && now < self.grant_expiry
                            && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Running)
                            && self.cancelled_requests.contains(request_id) == false
                            && self.request_generations.get_cloned(request_id) == Some(self.grant_generation)
                            && self.request_executors.get_cloned(request_id).get("value") == self.executor_binding
                            && self.request_runs.get_cloned(request_id) == Some(run_id)
                            && self.run_scopes.get_cloned(run_id) == Some(scope_id)
                            && chain_id != "" && self.chain_latest_claims.contains_key(chain_id)
                            && self.claim_kinds.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value"))
                                == Some(ScopedEffectKind::ModelComputation)
                            && self.claim_requests.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") == request_id
                            && self.claim_runs.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") == run_id
                            && self.claim_retry_eligible.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value")
                            && self.claim_attempts.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") < 4294967295
                            && self.request_known_tokens.get_cloned(request_id).get("value") < self.grant_max_tokens
                        }
                        update {}
                        to Ready
                        emit ModelAttemptResolved {
                            chain_id: chain_id,
                            attempt: self.claim_attempts.get_cloned(self.chain_latest_claims.get_cloned(chain_id).get("value")).get("value") + 1
                        }
                    }
                transition ResolveExhaustedModelTokenBudget {
                    on input ResolveModelAttempt { request_id, run_id, scope_id, chain_id, now }
                    guard {
                        self.grant_revoked == false && now < self.grant_expiry
                        && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Running)
                        && self.cancelled_requests.contains(request_id) == false
                        && self.request_generations.get_cloned(request_id) == Some(self.grant_generation)
                        && self.request_executors.get_cloned(request_id).get("value") == self.executor_binding
                        && self.request_runs.get_cloned(request_id) == Some(run_id)
                        && self.run_scopes.get_cloned(run_id) == Some(scope_id)
                        && chain_id != ""
                        && self.request_known_tokens.get_cloned(request_id).get("value") >= self.grant_max_tokens
                    }
                    update {}
                    to Ready
                    emit ModelTokenBudgetExhausted {
                        used: self.request_known_tokens.get_cloned(request_id).get("value"),
                        limit: self.grant_max_tokens
                    }
                }

                transition ObserveExactEffectSettlement {
                    on input ObserveEffectSettlement {
                        claim_id, request_id, run_id, target, outcome, completion_digest
                    }
                    guard {
                        self.claim_ids.contains(claim_id)
                        && outcome != LiveEffectPhase::Claimed
                        && self.claim_phases.get_cloned(claim_id).get("value") == outcome
                        && self.claim_requests.get_cloned(claim_id).get("value") == request_id
                        && self.claim_runs.get_cloned(claim_id).get("value") == run_id
                        && self.claim_targets.get_cloned(claim_id).get("value") == target
                        && self.claim_terminal_digests.get_cloned(claim_id).get("value") == completion_digest
                    }
                    update {}
                    to Ready
                    emit EffectSettlementObserved {
                        claim_id: claim_id,
                        outcome: outcome,
                        completion_sequence: self.claim_terminal_sequences.get_cloned(claim_id).get("value"),
                        completion_digest: completion_digest
                    }
                }

                transition CancelKnownRequest {
                    on input Cancel { request_id }
                    guard {
                        self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") != LiveRequestPhase::Terminal
                        && self.request_phases.get_cloned(request_id) != Some(LiveRequestPhase::Reserved)
                    }
                    update { self.cancelled_requests.insert(request_id); }
                    to Ready
                    emit RequestCancellationRequired { request_id: request_id }
                }

                transition CancelUnadmittedRequest {
                    on input Cancel { request_id }
                    guard {
                        self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Reserved)
                    }
                    update {
                        self.cancelled_requests.insert(request_id);
                        self.request_completion_obligations.remove(request_id);
                    }
                    to Ready
                    emit RequestCancellationRequired { request_id: request_id }
                }

                transition CancelUnreservedSource {
                    on input CancelSource { source, reason }
                    guard {
                        source != ""
                        && self.source_requests.contains_key(source) == false
                        && self.source_cancellations.contains_key(source) == false
                    }
                    update { self.source_cancellations.insert(source, reason); }
                    to Ready
                    emit SourceCancellationRetained { source: source, reason: reason }
                }

                transition CancelReservedSource {
                    on input CancelSource { source, reason }
                    guard {
                        self.source_requests.contains_key(source)
                        && self.source_cancellations.contains_key(source) == false
                        && self.request_phases.get_cloned(self.source_requests.get_cloned(source).get("value")) != Some(LiveRequestPhase::Terminal)
                        && self.request_phases.get_cloned(self.source_requests.get_cloned(source).get("value")) != Some(LiveRequestPhase::Reserved)
                    }
                    update {
                        self.source_cancellations.insert(source, reason);
                        self.cancelled_requests.insert(self.source_requests.get_cloned(source).get("value"));
                    }
                    to Ready
                    emit SourceCancellationRetained { source: source, reason: reason }
                    emit RequestCancellationRequired {
                        request_id: self.source_requests.get_cloned(source).get("value")
                    }
                }

                transition CancelUnadmittedSource {
                    on input CancelSource { source, reason }
                    guard {
                        self.source_requests.contains_key(source)
                        && self.source_cancellations.contains_key(source) == false
                        && self.request_phases.get_cloned(self.source_requests.get_cloned(source).get("value")) == Some(LiveRequestPhase::Reserved)
                    }
                    update {
                        self.source_cancellations.insert(source, reason);
                        self.cancelled_requests.insert(self.source_requests.get_cloned(source).get("value"));
                        self.request_completion_obligations.remove(self.source_requests.get_cloned(source).get("value"));
                    }
                    to Ready
                    emit SourceCancellationRetained { source: source, reason: reason }
                    emit RequestCancellationRequired {
                        request_id: self.source_requests.get_cloned(source).get("value")
                    }
                }

                transition CancelCompletedSource {
                    on input CancelSource { source, reason }
                    guard {
                        self.source_requests.contains_key(source)
                        && self.source_cancellations.contains_key(source) == false
                        && self.request_phases.get_cloned(self.source_requests.get_cloned(source).get("value")) == Some(LiveRequestPhase::Terminal)
                    }
                    update {
                        self.source_cancellations.insert(source, reason);
                        self.cancelled_requests.insert(self.source_requests.get_cloned(source).get("value"));
                    }
                    to Ready
                    emit SourceCancellationRetained { source: source, reason: reason }
                }

                transition ObserveSourceCancellation {
                    on input CancelSource { source, reason }
                    guard {
                        self.source_cancellations.get_cloned(source) == Some(reason)
                        && (self.source_requests.contains_key(source) == false
                            || self.request_phases.get_cloned(self.source_requests.get_cloned(source).get("value")) == Some(LiveRequestPhase::Terminal))
                    }
                    update {}
                    to Ready
                    emit SourceCancellationRetained {
                        source: source,
                        reason: reason
                    }
                }

                transition ObservePendingSourceCancellation {
                    on input CancelSource { source, reason }
                    guard {
                        self.source_cancellations.get_cloned(source) == Some(reason)
                        && self.source_requests.contains_key(source)
                        && self.request_phases.get_cloned(self.source_requests.get_cloned(source).get("value")) != Some(LiveRequestPhase::Terminal)
                    }
                    update {}
                    to Ready
                    emit SourceCancellationRetained { source: source, reason: reason }
                    emit RequestCancellationRequired {
                        request_id: self.source_requests.get_cloned(source).get("value")
                    }
                }

                transition RevokeCurrentGrant {
                    on input Revoke { grant_id, generation }
                    guard {
                        grant_id == self.grant_id && generation == self.grant_generation
                        && generation > 0
                    }
                    update {
                        self.grant_revoked = true;
                        self.ingress_open = false;
                    }
                    to Ready
                    emit GrantRevoked { grant_id: grant_id, generation: generation }
                }

                transition FenceExecutorBinding {
                    on input FenceExecutor { executor }
                    guard { executor != "" && executor != self.executor_binding }
                    update {
                        self.executor_binding = executor;
                        self.grant_revoked = true;
                        self.ingress_open = false;
                    }
                    to Ready
                    emit ExecutorFenced { executor: executor }
                }

                transition CloseRequestIngress {
                    on input CloseIngress {}
                    guard { self.ingress_open }
                    update { self.ingress_open = false; }
                    to Ready
                    emit IngressClosed {}
                }

                transition ObserveClosedRequestIngress {
                    on input CloseIngress {}
                    guard { !self.ingress_open }
                    update {}
                    to Ready
                    emit IngressClosed {}
                }

                transition SuspendRunningRequest {
                    on input Suspend {
                        request_id, run_id, callback_record, ordinary_completion_digest,
                        callback_claims, spent_records, spent_bytes,
                        completion_sequence, completion_digest
                    }
                    guard {
                        self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") == LiveRequestPhase::Running
                        && self.request_runs.get_cloned(request_id).get("value") == run_id
                        && self.run_callback_records.get_cloned(run_id) == Some("")
                        && callback_record != "" && ordinary_completion_digest.len() == 64
                        && completion_digest.len() == 64 && completion_sequence > 0
                        && callback_claims.len() > 0
                        && spent_records.keys() == self.claim_ids
                        && spent_bytes.keys() == self.claim_ids
                        && for_all(claim in callback_claims,
                            self.claim_ids.contains(claim)
                            && self.claim_runs.get_cloned(claim) == Some(run_id)
                            && self.claim_requests.get_cloned(claim) == Some(request_id)
                            && self.claim_kinds.get_cloned(claim) == Some(ScopedEffectKind::ToolDispatch)
                            && (self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Unknown)
                                || self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Succeeded)
                                || self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Failed)
                                || self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Cancelled)))
                        && for_all(claim in self.claim_ids,
                            (callback_claims.contains(claim)
                                && self.claim_credit_spent_records.get_cloned(claim).get("value")
                                    < self.claim_credit_records.get_cloned(claim).get("value")
                                && spent_records.get_cloned(claim).get("value")
                                    == self.claim_credit_spent_records.get_cloned(claim).get("value") + 1
                                && spent_bytes.get_cloned(claim).get("value")
                                    > self.claim_credit_spent_bytes.get_cloned(claim).get("value")
                                && spent_bytes.get_cloned(claim).get("value")
                                    <= self.claim_credit_bytes.get_cloned(claim).get("value")
                                && spent_bytes.get_cloned(claim).get("value")
                                    - self.claim_credit_spent_bytes.get_cloned(claim).get("value")
                                    >= self.claim_credit_minimum_record_charge.get_cloned(claim).get("value")
                                && spent_bytes.get_cloned(claim).get("value")
                                    - self.claim_credit_spent_bytes.get_cloned(claim).get("value")
                                    <= self.claim_credit_maximum_record_charge.get_cloned(claim).get("value"))
                            || (callback_claims.contains(claim) == false
                                && spent_records.get_cloned(claim) == self.claim_credit_spent_records.get_cloned(claim)
                                && spent_bytes.get_cloned(claim) == self.claim_credit_spent_bytes.get_cloned(claim)))
                        && for_all(claim in self.claim_ids,
                            self.claim_requests.get_cloned(claim).get("value") != request_id
                            || self.claim_phases.get_cloned(claim).get("value") != LiveEffectPhase::Claimed)
                    }
                    update {
                        self.run_callback_records.insert(run_id, callback_record);
                        self.run_callback_receipts.insert(run_id, ordinary_completion_digest);
                        self.run_callback_claims.insert(run_id, callback_claims);
                        self.run_callback_sequences.insert(run_id, completion_sequence);
                        self.run_callback_digests.insert(run_id, completion_digest);
                        self.claim_credit_spent_records = spent_records;
                        self.claim_credit_spent_bytes = spent_bytes;
                        self.request_phases.insert(request_id, LiveRequestPhase::Suspended);
                    }
                    to Ready
                    emit RequestSuspended { request_id: request_id, run_id: run_id }
                }

                transition ObserveExactCallbackSuspension {
                    on input ObserveCallbackSuspension {
                        request_id, run_id, callback_record, ordinary_completion_digest,
                        callback_claims, completion_digest
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Suspended)
                        && self.request_runs.get_cloned(request_id) == Some(run_id)
                        && self.run_callback_records.get_cloned(run_id) == Some(callback_record)
                        && self.run_callback_receipts.get_cloned(run_id) == Some(ordinary_completion_digest)
                        && self.run_callback_claims.get_cloned(run_id) == Some(callback_claims)
                        && self.run_callback_digests.get_cloned(run_id) == Some(completion_digest)
                    }
                    update {}
                    to Ready
                    emit RequestSuspended { request_id: request_id, run_id: run_id }
                }

                transition ObserveExactCancelledCallbackHold {
                    on input ObserveCancelledCallback {
                        request_id, run_id, callback_record, ordinary_completion_digest,
                        callback_claims, completion_digest, reason
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Suspended)
                        && self.cancelled_requests.contains(request_id)
                        && self.request_runs.get_cloned(request_id) == Some(run_id)
                        && self.run_requests.get_cloned(run_id) == Some(request_id)
                        && self.run_continuation_inputs.get_cloned(run_id) == Some("")
                        && self.run_successors.get_cloned(run_id) == Some("")
                        && self.request_sources.contains_key(request_id)
                        && self.source_cancellations.get_cloned(self.request_sources.get_cloned(request_id).get("value")) == Some(reason)
                        && callback_record != ""
                        && self.run_callback_records.get_cloned(run_id) == Some(callback_record)
                        && self.run_callback_receipts.get_cloned(run_id) == Some(ordinary_completion_digest)
                        && self.run_callback_claims.get_cloned(run_id) == Some(callback_claims)
                        && self.run_callback_digests.get_cloned(run_id) == Some(completion_digest)
                    }
                    update {}
                    to Ready
                    emit CancelledCallbackHeld {
                        request_id: request_id, run_id: run_id,
                        reason: reason
                    }
                }

                transition CompleteRunningRequest {
                    on input Complete {
                        request_id, run_id, input_id, ordinary_completion_digest,
                        completion_records, completion_bytes, completion_sequence, completion_digest
                    }
                    guard {
                        self.request_ids.contains(request_id)
                        && self.request_phases.get_cloned(request_id).get("value") == LiveRequestPhase::Running
                        && self.request_runs.get_cloned(request_id).get("value") == run_id
                        && self.run_inputs.get_cloned(run_id) == Some(input_id)
                        && ordinary_completion_digest.len() == 64
                        && completion_digest.len() == 64 && completion_sequence > 0
                        && completion_records == 1
                        && completion_records <= self.request_credit_records.get_cloned(request_id).get("value")
                        && completion_bytes > 0
                        && completion_bytes <= self.request_credit_bytes.get_cloned(request_id).get("value")
                        && for_all(claim in self.claim_ids,
                            self.claim_requests.get_cloned(claim).get("value") != request_id
                            || self.claim_phases.get_cloned(claim).get("value") != LiveEffectPhase::Claimed)
                    }
                    update {
                        self.request_phases.insert(request_id, LiveRequestPhase::Terminal);
                        self.grant_active_requests.remove(request_id);
                        self.request_completion_obligations.remove(request_id);
                        self.request_credit_spent_records.insert(request_id, completion_records);
                        self.request_credit_spent_bytes.insert(request_id, completion_bytes);
                        self.request_terminal_sequences.insert(request_id, completion_sequence);
                        self.request_terminal_digests.insert(request_id, completion_digest);
                        self.request_ordinary_completion_digests.insert(request_id, ordinary_completion_digest);
                    }
                    to Ready
                    emit RequestCompleted { request_id: request_id, run_id: run_id }
                }

                transition ObserveCompletedRequest {
                    on input ObserveRequestCompletion {
                        request_id, run_id, input_id, ordinary_completion_digest
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Terminal)
                        && self.request_runs.get_cloned(request_id) == Some(run_id)
                        && self.run_inputs.get_cloned(run_id) == Some(input_id)
                        && self.request_ordinary_completion_digests.get_cloned(request_id) == Some(ordinary_completion_digest)
                    }
                    update {}
                    to Ready
                    emit RequestCompletionObserved {
                        request_id: request_id,
                        completion_sequence: self.request_terminal_sequences.get_cloned(request_id).get("value"),
                        completion_digest: self.request_terminal_digests.get_cloned(request_id).get("value")
                    }
                }

                transition CompleteRunlessRequest {
                    on input CompleteRunless {
                        request_id, input_id, ordinary_completion_digest,
                        completion_records, completion_bytes,
                        completion_sequence, completion_digest
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Admitted)
                        && !self.bound_requests.contains(request_id)
                        && self.request_inputs.get_cloned(request_id) == Some(input_id)
                        && ordinary_completion_digest.len() == 64
                        && completion_digest.len() == 64
                        && completion_sequence > 0
                        && completion_records == 1
                        && completion_records <= self.request_credit_records.get_cloned(request_id).get("value")
                        && completion_bytes > 0
                        && completion_bytes <= self.request_credit_bytes.get_cloned(request_id).get("value")
                        && for_all(claim in self.claim_ids,
                            self.claim_requests.get_cloned(claim).get("value") != request_id)
                    }
                    update {
                        self.request_phases.insert(request_id, LiveRequestPhase::Terminal);
                        self.grant_active_requests.remove(request_id);
                        self.request_completion_obligations.remove(request_id);
                        self.request_credit_spent_records.insert(request_id, completion_records);
                        self.request_credit_spent_bytes.insert(request_id, completion_bytes);
                        self.request_terminal_sequences.insert(request_id, completion_sequence);
                        self.request_terminal_digests.insert(request_id, completion_digest);
                        self.request_ordinary_completion_digests.insert(request_id, ordinary_completion_digest);
                    }
                    to Ready
                    emit RunlessRequestCompleted { request_id: request_id, input_id: input_id }
                }

                transition ObserveCompletedRunlessRequest {
                    on input ObserveRunlessCompletion {
                        request_id, input_id, ordinary_completion_digest
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Terminal)
                        && !self.bound_requests.contains(request_id)
                        && self.request_inputs.get_cloned(request_id) == Some(input_id)
                        && self.request_ordinary_completion_digests.get_cloned(request_id) == Some(ordinary_completion_digest)
                    }
                    update {}
                    to Ready
                    emit RequestCompletionObserved {
                        request_id: request_id,
                        completion_sequence: self.request_terminal_sequences.get_cloned(request_id).get("value"),
                        completion_digest: self.request_terminal_digests.get_cloned(request_id).get("value")
                    }
                }

                transition CompleteUnstagedCallbackContinuation {
                    on input CompleteUnstagedContinuation {
                        request_id, previous_run_id, input_id, ordinary_completion_digest,
                        completion_records, completion_bytes, completion_sequence, completion_digest
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Suspended)
                        && self.request_runs.get_cloned(request_id) == Some(previous_run_id)
                        && self.run_requests.get_cloned(previous_run_id) == Some(request_id)
                        && input_id != ""
                        && self.run_continuation_inputs.get_cloned(previous_run_id) == Some(input_id)
                        && self.run_successors.get_cloned(previous_run_id) == Some("")
                        && self.run_callback_records.get_cloned(previous_run_id).get("value") != ""
                        && ordinary_completion_digest.len() == 64
                        && completion_digest.len() == 64 && completion_sequence > 0
                        && completion_records == 1
                        && completion_records <= self.request_credit_records.get_cloned(request_id).get("value")
                        && completion_bytes > 0
                        && completion_bytes <= self.request_credit_bytes.get_cloned(request_id).get("value")
                        && for_all(claim in self.claim_ids,
                            self.claim_requests.get_cloned(claim).get("value") != request_id
                            || self.claim_phases.get_cloned(claim).get("value") != LiveEffectPhase::Claimed)
                    }
                    update {
                        self.request_phases.insert(request_id, LiveRequestPhase::Terminal);
                        self.grant_active_requests.remove(request_id);
                        self.request_completion_obligations.remove(request_id);
                        self.request_credit_spent_records.insert(request_id, completion_records);
                        self.request_credit_spent_bytes.insert(request_id, completion_bytes);
                        self.request_terminal_sequences.insert(request_id, completion_sequence);
                        self.request_terminal_digests.insert(request_id, completion_digest);
                        self.request_ordinary_completion_digests.insert(request_id, ordinary_completion_digest);
                    }
                    to Ready
                    emit RunlessRequestCompleted { request_id: request_id, input_id: input_id }
                }

                transition ObserveCompletedUnstagedCallbackContinuation {
                    on input ObserveUnstagedContinuationCompletion {
                        request_id, previous_run_id, input_id, ordinary_completion_digest
                    }
                    guard {
                        self.request_phases.get_cloned(request_id) == Some(LiveRequestPhase::Terminal)
                        && self.request_runs.get_cloned(request_id) == Some(previous_run_id)
                        && self.run_requests.get_cloned(previous_run_id) == Some(request_id)
                        && input_id != ""
                        && self.run_continuation_inputs.get_cloned(previous_run_id) == Some(input_id)
                        && self.run_successors.get_cloned(previous_run_id) == Some("")
                        && self.request_ordinary_completion_digests.get_cloned(request_id) == Some(ordinary_completion_digest)
                    }
                    update {}
                    to Ready
                    emit RequestCompletionObserved {
                        request_id: request_id,
                        completion_sequence: self.request_terminal_sequences.get_cloned(request_id).get("value"),
                        completion_digest: self.request_terminal_digests.get_cloned(request_id).get("value")
                    }
                }

                invariant refused_sources_never_mint_requests {
                    self.source_refusals.keys() == self.source_refusal_payloads.keys()
                    && for_all(source in self.source_refusals.keys(),
                        source != "" && !self.source_requests.contains_key(source)
                        && self.source_refusal_payloads.get_cloned(source).get("value") != "")
                }

                invariant request_completion_obligations_have_exact_lifetimes {
                    for_all(request in self.request_completion_obligations,
                        self.request_ids.contains(request))
                    && for_all(request in self.request_ids,
                        self.request_completion_obligations.contains(request)
                        == (self.request_phases.get_cloned(request) != Some(LiveRequestPhase::Terminal)
                            && (self.request_phases.get_cloned(request) != Some(LiveRequestPhase::Reserved)
                                || !self.cancelled_requests.contains(request))))
                }

                invariant request_completion_credits_are_complete {
                    self.request_credit_records.keys() == self.request_ids
                    && self.request_credit_bytes.keys() == self.request_ids
                    && self.request_credit_snapshot_ceiling.keys() == self.request_ids
                    && self.request_credit_spent_records.keys() == self.request_ids
                    && self.request_credit_spent_bytes.keys() == self.request_ids
                    && self.request_terminal_sequences.keys() == self.request_ids
                    && self.request_terminal_digests.keys() == self.request_ids
                    && self.request_ordinary_completion_digests.keys() == self.request_ids
                    && for_all(request in self.request_ids,
                        self.request_credit_records.get_cloned(request).get("value") > 0
                        && self.request_credit_bytes.get_cloned(request).get("value") > 0
                        && self.request_credit_snapshot_ceiling.get_cloned(request).get("value") > 0
                        && self.request_credit_spent_records.get_cloned(request).get("value")
                            <= self.request_credit_records.get_cloned(request).get("value")
                        && self.request_credit_spent_bytes.get_cloned(request).get("value")
                            <= self.request_credit_bytes.get_cloned(request).get("value")
                        && if self.request_phases.get_cloned(request) == Some(LiveRequestPhase::Terminal) {
                            self.request_credit_spent_records.get_cloned(request) == Some(1)
                            && self.request_credit_spent_bytes.get_cloned(request).get("value") > 0
                            && self.request_terminal_sequences.get_cloned(request).get("value") > 0
                            && self.request_terminal_digests.get_cloned(request).get("value").len() == 64
                            && self.request_ordinary_completion_digests.get_cloned(request).get("value").len() == 64
                        } else {
                            self.request_credit_spent_records.get_cloned(request) == Some(0)
                            && self.request_credit_spent_bytes.get_cloned(request) == Some(0)
                            && self.request_terminal_sequences.get_cloned(request) == Some(0)
                            && self.request_terminal_digests.get_cloned(request).get("value") == ""
                            && self.request_ordinary_completion_digests.get_cloned(request).get("value") == ""
                        })
                }

                invariant request_record_fields_have_one_owner {
                    self.request_phases.keys() == self.request_ids
                    && self.request_sources.keys() == self.request_ids
                    && self.request_payloads.keys() == self.request_ids
                    && self.request_evidence.keys() == self.request_ids
                    && self.request_grants.keys() == self.request_ids
                    && self.request_grant_records.keys() == self.request_ids
                    && self.request_generations.keys() == self.request_ids
                    && self.request_executors.keys() == self.request_ids
                    && self.request_parents.keys() == self.request_ids
                    && self.remaining_effects.keys() == self.request_ids
                    && self.source_requests.keys().len() == self.request_ids.len()
                }

                invariant activated_grant_is_complete_and_bounded {
                    self.grant_generation == 0
                    || (self.grant_id != ""
                        && self.grant_record != ""
                        && self.grant_profile_revision != ""
                        && self.grant_evidence.len() > 0
                        && self.grant_max_requests > 0
                        && self.grant_max_concurrent_requests > 0
                        && self.grant_max_concurrent_requests <= self.grant_max_requests
                        && self.grant_max_effects > 0
                        && self.grant_max_tokens > 0
                        && self.grant_max_duration_ms > 0
                        && self.grant_admitted_requests.len() <= self.grant_max_requests
                        && self.grant_active_requests.len() <= self.grant_max_concurrent_requests)
                }

                invariant active_grant_requests_have_exact_admission {
                    for_all(request in self.grant_admitted_requests,
                        self.admitted_requests.contains(request)
                        && self.request_grants.get_cloned(request).get("value") == self.grant_id
                        && self.request_generations.get_cloned(request).get("value") == self.grant_generation)
                    && for_all(request in self.admitted_requests,
                        (self.request_grants.get_cloned(request).get("value") != self.grant_id
                            || self.request_generations.get_cloned(request).get("value") != self.grant_generation)
                        || self.grant_admitted_requests.contains(request))
                    && for_all(request in self.grant_active_requests,
                        self.grant_admitted_requests.contains(request)
                        && self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Terminal)
                    && for_all(request in self.grant_admitted_requests,
                        self.grant_active_requests.contains(request)
                            == (self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Terminal))
                }

                invariant current_requests_retain_the_activated_ceiling {
                    for_all(request in self.request_ids,
                        (self.request_grants.get_cloned(request).get("value") != self.grant_id
                            || self.request_generations.get_cloned(request).get("value") != self.grant_generation)
                        || (self.request_grant_records.get_cloned(request).get("value") == self.grant_record
                            && exists(evidence in self.grant_evidence,
                                self.request_evidence.get_cloned(request) == Some(evidence))
                            && self.remaining_effects.get_cloned(request).get("value") <= self.grant_max_effects))
                }

                invariant admission_and_run_scope_fields_are_complete {
                    self.request_inputs.keys() == self.admitted_requests
                    && self.request_admission_commits.keys() == self.admitted_requests
                    && self.request_ingress_generations.keys() == self.admitted_requests
                    && self.request_runs.keys() == self.bound_requests
                    && self.run_inputs.keys() == self.run_requests.keys()
                    && self.run_admission_commits.keys() == self.run_requests.keys()
                    && self.run_scopes.keys() == self.run_requests.keys()
                    && self.run_scope_records.keys() == self.run_requests.keys()
                    && self.run_callback_records.keys() == self.run_requests.keys()
                    && self.run_callback_receipts.keys() == self.run_requests.keys()
                    && self.run_callback_claims.keys() == self.run_requests.keys()
                    && self.run_callback_sequences.keys() == self.run_requests.keys()
                    && self.run_callback_digests.keys() == self.run_requests.keys()
                    && self.run_predecessors.keys() == self.run_requests.keys()
                    && self.run_ordinals.keys() == self.run_requests.keys()
                    && self.run_successors.keys() == self.run_requests.keys()
                    && self.run_continuation_inputs.keys() == self.run_requests.keys()
                    && self.run_continuation_admission_commits.keys() == self.run_requests.keys()
                    && self.run_continuation_result_digests.keys() == self.run_requests.keys()
                    && self.run_continuation_stage_credits.keys() == self.run_requests.keys()
                    && self.run_callback_application_claimed.keys() == self.run_requests.keys()
                    && self.scope_runs.keys().len() == self.run_requests.keys().len()
                }

                invariant request_identity_joins_are_exact {
                    for_all(request in self.request_ids,
                        request != ""
                        && self.request_sources.get_cloned(request).get("value") != ""
                        && self.source_requests.contains_key(self.request_sources.get_cloned(request).get("value"))
                        && self.source_requests.get_cloned(self.request_sources.get_cloned(request).get("value")).get("value") == request
                        && self.request_payloads.get_cloned(request).get("value") != ""
                        && self.request_grants.get_cloned(request).get("value") != ""
                        && self.request_grant_records.get_cloned(request).get("value") != ""
                        && self.request_generations.get_cloned(request).get("value") > 0
                        && self.request_generations.get_cloned(request).get("value") <= self.grant_generation
                        && self.request_executors.get_cloned(request).get("value") != ""
                        && (self.admitted_requests.contains(request)
                            == (self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Reserved))
                        && (self.bound_requests.contains(request)
                            || (self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Running
                                && self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Suspended))
                        && (!self.bound_requests.contains(request)
                            || (self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Reserved
                                && self.request_phases.get_cloned(request).get("value") != LiveRequestPhase::Admitted)))
                    && for_all(request in self.cancelled_requests, self.request_ids.contains(request))
                    && for_all(source in self.source_cancellations.keys(),
                        source != ""
                        && (self.source_requests.contains_key(source) == false
                            || self.cancelled_requests.contains(self.source_requests.get_cloned(source).get("value"))))
                    && for_all(request in self.admitted_requests,
                        self.request_ids.contains(request)
                        && self.request_inputs.get_cloned(request).get("value") != ""
                        && self.request_admission_commits.get_cloned(request).get("value") != ""
                        && self.request_ingress_generations.get_cloned(request).get("value") > 0
                        && for_all(other in self.admitted_requests,
                            request == other
                            || (self.request_inputs.get_cloned(request).get("value")
                                != self.request_inputs.get_cloned(other).get("value")
                                && self.request_admission_commits.get_cloned(request).get("value")
                                    != self.request_admission_commits.get_cloned(other).get("value"))))
                    && for_all(request in self.bound_requests,
                        self.admitted_requests.contains(request)
                        && self.request_runs.get_cloned(request).get("value") != ""
                        && self.run_requests.contains_key(self.request_runs.get_cloned(request).get("value"))
                        && self.run_requests.get_cloned(self.request_runs.get_cloned(request).get("value")).get("value") == request)
                    && for_all(run in self.run_requests.keys(),
                        run != ""
                        && self.bound_requests.contains(self.run_requests.get_cloned(run).get("value"))
                        && self.run_inputs.get_cloned(run).get("value") != ""
                        && self.run_admission_commits.get_cloned(run).get("value") != ""
                        && self.run_scopes.get_cloned(run).get("value") != ""
                        && self.run_scope_records.get_cloned(run).get("value") != ""
                        && self.scope_runs.contains_key(self.run_scopes.get_cloned(run).get("value"))
                        && self.scope_runs.get_cloned(self.run_scopes.get_cloned(run).get("value")).get("value") == run)
                }

                invariant callback_continuations_form_one_exact_acyclic_run_chain {
                    for_all(run in self.run_requests.keys(),
                        ((self.run_predecessors.get_cloned(run) == Some("")
                            && self.run_ordinals.get_cloned(run) == Some(0)
                            && self.run_inputs.get_cloned(run)
                                == self.request_inputs.get_cloned(self.run_requests.get_cloned(run).get("value"))
                            && self.run_admission_commits.get_cloned(run)
                                == self.request_admission_commits.get_cloned(self.run_requests.get_cloned(run).get("value")))
                        || (self.run_predecessors.get_cloned(run) != Some("")
                            && self.run_requests.contains_key(self.run_predecessors.get_cloned(run).get("value"))
                            && self.run_requests.get_cloned(self.run_predecessors.get_cloned(run).get("value"))
                                == self.run_requests.get_cloned(run)
                            && self.run_successors.get_cloned(self.run_predecessors.get_cloned(run).get("value")) == Some(run)
                            && self.run_ordinals.get_cloned(self.run_predecessors.get_cloned(run).get("value")).get("value") < u64::MAX
                            && self.run_ordinals.get_cloned(run).get("value")
                                == self.run_ordinals.get_cloned(self.run_predecessors.get_cloned(run).get("value")).get("value") + 1
                            && self.run_inputs.get_cloned(run)
                                == self.run_continuation_inputs.get_cloned(self.run_predecessors.get_cloned(run).get("value"))
                            && self.run_admission_commits.get_cloned(run)
                                == self.run_continuation_admission_commits.get_cloned(self.run_predecessors.get_cloned(run).get("value"))))
                        && ((self.run_successors.get_cloned(run) == Some(""))
                            == (self.request_runs.get_cloned(self.run_requests.get_cloned(run).get("value")) == Some(run)))
                        && for_all(other in self.run_requests.keys(),
                            run == other
                            || (self.run_inputs.get_cloned(run) != self.run_inputs.get_cloned(other)
                                && self.run_admission_commits.get_cloned(run) != self.run_admission_commits.get_cloned(other))))
                }

                invariant callback_continuation_admission_is_complete_and_one_shot {
                    for_all(run in self.run_requests.keys(),
                        ((self.run_continuation_inputs.get_cloned(run) == Some("")
                            && self.run_continuation_admission_commits.get_cloned(run) == Some("")
                            && self.run_continuation_result_digests.get_cloned(run) == Some("")
                            && self.run_continuation_stage_credits.get_cloned(run) == Some(0)
                            && self.run_successors.get_cloned(run) == Some(""))
                        || (self.run_continuation_inputs.get_cloned(run) != Some("")
                            && self.run_continuation_admission_commits.get_cloned(run) != Some("")
                            && self.run_continuation_result_digests.get_cloned(run).get("value").len() == 64
                            && self.run_callback_records.get_cloned(run) != Some("")
                            && ((self.run_successors.get_cloned(run) == Some("")
                                    && self.run_continuation_stage_credits.get_cloned(run).get("value") > 0)
                                || (self.run_successors.get_cloned(run) != Some("")
                                    && self.run_continuation_stage_credits.get_cloned(run) == Some(0)
                                    && self.run_predecessors.get_cloned(self.run_successors.get_cloned(run).get("value")) == Some(run)))
                            && for_all(request in self.admitted_requests,
                                self.run_continuation_inputs.get_cloned(run) != self.request_inputs.get_cloned(request)
                                && self.run_continuation_admission_commits.get_cloned(run) != self.request_admission_commits.get_cloned(request))
                            && for_all(other in self.run_requests.keys(),
                                run == other
                                || (self.run_continuation_inputs.get_cloned(run) != self.run_continuation_inputs.get_cloned(other)
                                    && self.run_continuation_admission_commits.get_cloned(run) != self.run_continuation_admission_commits.get_cloned(other))))))
                }

                invariant callback_application_claims_require_continuation_lineage {
                    for_all(run in self.run_requests.keys(),
                        self.run_callback_application_claimed.get_cloned(run) == Some(false)
                        || (self.run_predecessors.get_cloned(run) != Some("")
                            && self.run_callback_records.get_cloned(self.run_predecessors.get_cloned(run).get("value")) != Some("")
                            && self.run_continuation_result_digests.get_cloned(self.run_predecessors.get_cloned(run).get("value"))
                                .get("value").len() == 64))
                    && for_all(claim in self.claim_ids,
                        self.run_predecessors.get_cloned(self.claim_runs.get_cloned(claim).get("value")) == Some("")
                        || self.run_callback_application_claimed.get_cloned(self.claim_runs.get_cloned(claim).get("value")) == Some(true))
                }

                invariant claims_retain_exact_identity_and_spent_effects {
                    self.claim_requests.keys() == self.claim_ids
                    && self.claim_runs.keys() == self.claim_ids
                    && self.claim_targets.keys() == self.claim_ids
                    && self.claim_records.keys() == self.claim_ids
                    && self.claim_effects.keys() == self.claim_ids
                    && self.claim_kinds.keys() == self.claim_ids
                    && self.claim_chains.keys() == self.claim_ids
                    && self.claim_attempts.keys() == self.claim_ids
                    && self.claim_retry_eligible.keys() == self.claim_ids
                    && self.claim_policy_revisions.keys() == self.claim_ids
                    && self.claim_phases.keys() == self.claim_ids
                    && self.spent_effects.len() == self.claim_ids.len()
                    && self.claim_credit_records.keys() == self.claim_ids
                    && self.claim_credit_bytes.keys() == self.claim_ids
                    && self.claim_credit_minimum_record_charge.keys() == self.claim_ids
                    && self.claim_credit_maximum_record_charge.keys() == self.claim_ids
                    && self.claim_credit_snapshot_ceiling.keys() == self.claim_ids
                    && self.claim_credit_spent_records.keys() == self.claim_ids
                    && self.claim_credit_spent_bytes.keys() == self.claim_ids
                    && self.claim_terminal_sequences.keys() == self.claim_ids
                    && self.claim_terminal_digests.keys() == self.claim_ids
                }

                invariant observed_token_accounting_is_complete {
                    self.request_known_tokens.keys() == self.request_ids
                    && self.claim_known_tokens.keys() == self.claim_ids
                    && self.claim_accounting_status.keys() == self.claim_ids
                    && self.claim_accounting_records.keys() == self.claim_ids
                    && for_all(claim in self.claim_ids,
                        self.claim_known_tokens.get_cloned(claim).get("value")
                            <= self.request_known_tokens.get_cloned(self.claim_requests.get_cloned(claim).get("value")).get("value")
                        && (self.claim_kinds.get_cloned(claim) == Some(ScopedEffectKind::ModelComputation)
                            || self.claim_known_tokens.get_cloned(claim) == Some(0))
                        && ((self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Claimed)
                                && self.claim_known_tokens.get_cloned(claim) == Some(0)
                                && self.claim_accounting_status.get_cloned(claim) == Some(ScopedTokenAccountingStatus::Pending)
                                && self.claim_accounting_records.get_cloned(claim).get("value") == "")
                            || (self.claim_phases.get_cloned(claim) != Some(LiveEffectPhase::Claimed)
                                && self.claim_accounting_status.get_cloned(claim) != Some(ScopedTokenAccountingStatus::Pending)
                                && self.claim_accounting_records.get_cloned(claim).get("value") != "")))
                }

                invariant effect_attempt_chains_are_exact {
                    for_all(claim in self.claim_ids,
                        self.claim_chains.get_cloned(claim).get("value") != ""
                        && self.claim_attempts.get_cloned(claim).get("value") <= 4294967295
                        && self.chain_latest_claims.contains_key(self.claim_chains.get_cloned(claim).get("value"))
                        && self.claim_attempts.get_cloned(claim).get("value")
                            <= self.claim_attempts.get_cloned(self.chain_latest_claims.get_cloned(self.claim_chains.get_cloned(claim).get("value")).get("value")).get("value")
                        && (self.claim_attempts.get_cloned(claim).get("value") == 0
                            || exists(previous in self.claim_ids,
                                self.claim_chains.get_cloned(previous) == self.claim_chains.get_cloned(claim)
                                && self.claim_attempts.get_cloned(previous).get("value") == self.claim_attempts.get_cloned(claim).get("value") - 1
                                && self.claim_retry_eligible.get_cloned(previous).get("value")))
                        && for_all(other in self.claim_ids,
                            self.claim_chains.get_cloned(other) != self.claim_chains.get_cloned(claim)
                            || (self.claim_requests.get_cloned(other) == self.claim_requests.get_cloned(claim)
                                && self.claim_runs.get_cloned(other) == self.claim_runs.get_cloned(claim)
                                && self.claim_kinds.get_cloned(other) == self.claim_kinds.get_cloned(claim)
                                && (other == claim || self.claim_attempts.get_cloned(other) != self.claim_attempts.get_cloned(claim))))
                        && (self.claim_retry_eligible.get_cloned(claim).get("value") == false
                            || (self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Succeeded)
                                || self.claim_phases.get_cloned(claim) == Some(LiveEffectPhase::Failed)))
                        && (self.claim_kinds.get_cloned(claim) == Some(ScopedEffectKind::ModelComputation)
                            || (self.claim_chains.get_cloned(claim) == self.claim_effects.get_cloned(claim)
                                && self.claim_attempts.get_cloned(claim) == Some(0))))
                    && for_all(chain in self.chain_latest_claims.keys(),
                        self.claim_ids.contains(self.chain_latest_claims.get_cloned(chain).get("value"))
                        && self.claim_chains.get_cloned(self.chain_latest_claims.get_cloned(chain).get("value")) == Some(chain))
                }

                invariant completion_credits_are_bounded_and_settlement_is_exact {
                    self.completion_credit_schema == LiveCompletionCreditSchema::V1
                    && for_all(claim in self.claim_ids,
                        self.claim_credit_records.get_cloned(claim).get("value") > 1
                        && self.claim_credit_minimum_record_charge.get_cloned(claim).get("value") > 0
                        && self.claim_credit_maximum_record_charge.get_cloned(claim).get("value")
                            >= self.claim_credit_minimum_record_charge.get_cloned(claim).get("value")
                        && self.claim_credit_records.get_cloned(claim).get("value")
                            <= self.claim_credit_bytes.get_cloned(claim).get("value")
                                / self.claim_credit_maximum_record_charge.get_cloned(claim).get("value")
                        && self.claim_credit_records.get_cloned(claim).get("value")
                            * self.claim_credit_maximum_record_charge.get_cloned(claim).get("value")
                            == self.claim_credit_bytes.get_cloned(claim).get("value")
                        && self.claim_credit_snapshot_ceiling.get_cloned(claim).get("value") > 0
                        && self.claim_credit_spent_records.get_cloned(claim).get("value")
                            <= self.claim_credit_records.get_cloned(claim).get("value")
                        && self.claim_credit_spent_bytes.get_cloned(claim).get("value")
                            <= self.claim_credit_bytes.get_cloned(claim).get("value")
                        && self.claim_credit_spent_bytes.get_cloned(claim).get("value")
                            >= self.claim_credit_spent_records.get_cloned(claim).get("value")
                                * self.claim_credit_minimum_record_charge.get_cloned(claim).get("value")
                        && self.claim_credit_spent_bytes.get_cloned(claim).get("value")
                            <= self.claim_credit_spent_records.get_cloned(claim).get("value")
                                * self.claim_credit_maximum_record_charge.get_cloned(claim).get("value")
                        && (self.claim_phases.get_cloned(claim).get("value") != LiveEffectPhase::Unknown
                            || (self.claim_credit_spent_records.get_cloned(claim).get("value")
                                    < self.claim_credit_records.get_cloned(claim).get("value")
                                && self.claim_terminal_sequences.get_cloned(claim).get("value")
                                    < u64::MAX))
                        && ((self.claim_phases.get_cloned(claim).get("value") == LiveEffectPhase::Claimed
                                && self.claim_credit_spent_records.get_cloned(claim).get("value") == 0
                                && self.claim_terminal_sequences.get_cloned(claim).get("value") == 0
                                && self.claim_terminal_digests.get_cloned(claim).get("value") == "")
                            || (self.claim_phases.get_cloned(claim).get("value") != LiveEffectPhase::Claimed
                                && self.claim_credit_spent_records.get_cloned(claim).get("value") > 0
                                && self.claim_terminal_sequences.get_cloned(claim).get("value") > 0
                                && self.claim_terminal_digests.get_cloned(claim).get("value").len() == 64)))
                }

                invariant claim_identity_joins_are_exact {
                    for_all(claim in self.claim_ids,
                        claim != ""
                        && self.bound_requests.contains(self.claim_requests.get_cloned(claim).get("value"))
                        && self.run_requests.contains_key(self.claim_runs.get_cloned(claim).get("value"))
                        && self.run_requests.get_cloned(self.claim_runs.get_cloned(claim).get("value")).get("value")
                            == self.claim_requests.get_cloned(claim).get("value")
                        && self.claim_targets.get_cloned(claim).get("value") != ""
                        && self.claim_policy_revisions.get_cloned(claim).get("value") != ""
                        && self.claim_effects.get_cloned(claim).get("value") != ""
                        && self.spent_effects.contains(self.claim_effects.get_cloned(claim).get("value"))
                        && for_all(other in self.claim_ids,
                            claim == other
                            || self.claim_effects.get_cloned(claim).get("value")
                                != self.claim_effects.get_cloned(other).get("value")))
                }

                invariant callback_suspensions_retain_exact_run_membership {
                    for_all(run in self.run_requests.keys(),
                        (self.run_callback_records.get_cloned(run) == Some("")
                            && self.run_callback_receipts.get_cloned(run) == Some("")
                            && self.run_callback_claims.get_cloned(run).get("value").len() == 0
                            && self.run_callback_sequences.get_cloned(run) == Some(0)
                            && self.run_callback_digests.get_cloned(run) == Some(""))
                        || (self.run_callback_records.get_cloned(run).get("value") != ""
                            && self.run_callback_receipts.get_cloned(run).get("value").len() == 64
                            && self.run_callback_claims.get_cloned(run).get("value").len() > 0
                            && self.run_callback_sequences.get_cloned(run).get("value") > 0
                            && self.run_callback_digests.get_cloned(run).get("value").len() == 64
                            && for_all(claim in self.run_callback_claims.get_cloned(run).get("value"),
                                self.claim_ids.contains(claim)
                                && self.claim_runs.get_cloned(claim) == Some(run)
                                && self.claim_kinds.get_cloned(claim) == Some(ScopedEffectKind::ToolDispatch)
                                && self.claim_credit_spent_records.get_cloned(claim).get("value") >= 2
                                && self.claim_phases.get_cloned(claim) != Some(LiveEffectPhase::Claimed)
                                && self.claim_phases.get_cloned(claim) != Some(LiveEffectPhase::NotStarted))))
                    && for_all(request in self.bound_requests,
                        self.request_phases.get_cloned(request) != Some(LiveRequestPhase::Suspended)
                        || self.run_callback_records.get_cloned(self.request_runs.get_cloned(request).get("value")).get("value") != "")
                }

                invariant open_ingress_requires_a_live_grant {
                    self.ingress_open == false
                    || (self.grant_revoked == false && self.grant_generation > 0
                        && self.grant_id != "" && self.executor_binding != "")
                }
            }
        }
    };
}

live_request_catalog_machine_dsl!("self", "catalog::dsl::live_request");
