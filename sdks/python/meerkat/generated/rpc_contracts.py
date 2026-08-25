"""Generated RPC method contracts for meerkat-sdk.

Source: artifacts/schemas/rpc-methods.json
"""

from __future__ import annotations

from collections.abc import Awaitable
from typing import Any, Literal, Protocol, overload

from .types import (
    ActivateInstructionParams,
    ApprovalDecideParams,
    ApprovalGetParams,
    ApprovalListParams,
    ApprovalListResult,
    ApprovalRecord,
    ApprovalRequestParams,
    ArchiveSessionParams,
    ArtifactDownloadParams,
    ArtifactDownloadResult,
    ArtifactIdParams,
    ArtifactListParams,
    ArtifactListResult,
    ArtifactRecord,
    AttentionListRequest,
    AttentionListResult,
    BindingIdParams,
    BlobGetParams,
    BlobPayload,
    BridgeLiveControlOutcome,
    CapabilitiesResponse,
    CommsPeersParams,
    CommsPeersResult,
    CommsSendParams,
    CommsSendResult,
    ConfigEnvelope,
    ConfigPatchParams,
    ConfigSetParams,
    ConfigWriteResult,
    CreateProfileParams,
    CreateScheduleRequest,
    DeferredCreateResult,
    DeviceCompleteParams,
    DeviceStartParams,
    EventsLatestCursorParams,
    EventsLatestCursorResult,
    EventsListSinceParams,
    EventsListSinceResult,
    EventsSnapshotParams,
    EventsSnapshotResult,
    ExportAtifParams,
    ForkSessionAtParams,
    ForkSessionReplaceParams,
    GoalStatusRequest,
    GoalStatusResult,
    HelpRequest,
    HelpResponse,
    InjectSystemContextParams,
    InjectSystemContextResult,
    InstructionActivationReadPage,
    InstructionActivationReceipt,
    InterruptParams,
    InterruptResult,
    JobsArtifactsParams,
    JobsArtifactsResult,
    JobsCancelParams,
    JobsCancelResult,
    JobsGetParams,
    JobsGetResult,
    JobsHealthResult,
    JobsListParams,
    JobsListResult,
    JobsProgressParams,
    JobsProgressResult,
    JobsResultParams,
    JobsResultResult,
    JobsRetryParams,
    JobsRetryResult,
    JobsSubscribeParams,
    JobsSubscribeResult,
    JobsUnsubscribeParams,
    JobsUnsubscribeResult,
    ListSchedulesParams,
    ListSessionTranscriptRevisionsParams,
    ListSessionsParams,
    ListSessionsResult,
    LiveChannelParams,
    LiveCloseResult,
    LiveCommitInputParams,
    LiveCommitInputResult,
    LiveInterruptResult,
    LiveOpenParams,
    LiveOpenResult,
    LiveRefreshResult,
    LiveSendInputParams,
    LiveSendInputResult,
    LiveStatusResult,
    LiveTruncateParams,
    LiveTruncateResult,
    LiveWebrtcAnswerParams,
    LiveWebrtcAnswerResult,
    LoginCompleteParams,
    LoginStartParams,
    McpAddParams,
    McpLiveOpResponse,
    McpReloadParams,
    McpRemoveParams,
    MobAdoptMemberIdentityDeclarationParams,
    MobAdoptMemberIdentityDeclarationResult,
    MobAppendSystemContextParams,
    MobAppendSystemContextResult,
    MobApplyMemberToolDeclarationParams,
    MobApplyMemberToolDeclarationResult,
    MobBindHostParams,
    MobBindHostResult,
    MobCancelAllWorkParams,
    MobCancelAllWorkResult,
    MobCancelWorkParams,
    MobCancelWorkResult,
    MobConcludeObjectiveParams,
    MobConcludeObjectiveResult,
    MobCreateParams,
    MobCreateResult,
    MobDestroyResult,
    MobEnsureMemberParams,
    MobEnsureMemberResult,
    MobEventsParams,
    MobEventsResult,
    MobFlowCancelParams,
    MobFlowCancelResult,
    MobFlowRunParams,
    MobFlowRunResult,
    MobFlowStatusParams,
    MobFlowStatusResult,
    MobFlowsResult,
    MobForceCancelResult,
    MobForkHelperParams,
    MobGrantScopesParams,
    MobGrantScopesResult,
    MobGrantsResult,
    MobHardCancelParams,
    MobHardCancelResult,
    MobHelperResult,
    MobHostsResult,
    MobIdParams,
    MobIngressInteractionParams,
    MobIngressInteractionResult,
    MobLifecycleParams,
    MobLifecycleResult,
    MobListMembersMatchingParams,
    MobListMembersMatchingResult,
    MobListResult,
    MobMemberHistoryParams,
    MobMemberHistoryResult,
    MobMemberLiveChannelParams,
    MobMemberLiveControlParams,
    MobMemberLiveOpenParams,
    MobMemberLiveStatusParams,
    MobMemberParams,
    MobMemberSendParams,
    MobMemberSendResult,
    MobMemberStatusResult,
    MobMemberToolDeclarationParams,
    MobMemberToolDeclarationResult,
    MobMembersResult,
    MobProfileCreateParams,
    MobProfileDeleteParams,
    MobProfileDeleteResult,
    MobProfileListResult,
    MobProfileLookupResult,
    MobProfileNameParams,
    MobProfileUpdateParams,
    MobReconcileParams,
    MobReconcileResult,
    MobResolveIdentityConvergenceBlockParams,
    MobResolveIdentityConvergenceBlockResult,
    MobRespawnParams,
    MobRespawnResult,
    MobRetireResult,
    MobRevokeHostParams,
    MobRevokeHostResult,
    MobRevokeScopesParams,
    MobRevokeScopesResult,
    MobRotateSupervisorResult,
    MobRouteInstallsResult,
    MobRunParams,
    MobRunResult,
    MobRunResultParams,
    MobSnapshotResult,
    MobSpawnHelperParams,
    MobSpawnManyParams,
    MobSpawnManyResult,
    MobSpawnParams,
    MobSpawnResult,
    MobStatusResult,
    MobStreamCloseParams,
    MobStreamCloseResult,
    MobStreamOpenParams,
    MobStreamOpenResult,
    MobSubmitWorkParams,
    MobSubmitWorkResult,
    MobTurnStartParams,
    MobUnwireParams,
    MobUnwireResult,
    MobWaitMembersResult,
    MobWaitParams,
    MobWireMembersBatchParams,
    MobWireMembersBatchResult,
    MobWireParams,
    MobWireResult,
    MobkitJobCancelAckParams,
    MobkitJobCheckpointParams,
    MobkitJobCompleteParams,
    MobkitJobFailParams,
    MobkitJobHeartbeatParams,
    MobkitJobMutationResult,
    MobkitJobProgressParams,
    ModelsCatalogResponse,
    MonitorsStartParams,
    MonitorsStartResult,
    ProvisionApiKeyParams,
    ReadInstructionActivationsParams,
    ReadSessionHistoryParams,
    ReadSessionParams,
    ReadSessionTranscriptRevisionParams,
    ReadyWorkFilter,
    RealmIdParams,
    RestoreSessionTranscriptRevisionParams,
    RewriteSessionTranscriptParams,
    RuntimeAcceptResult,
    RuntimeHostCapabilities,
    RuntimeHostHealth,
    RuntimeHostInfo,
    Schedule,
    ScheduleIdParams,
    ScheduleListResult,
    ScheduleOccurrencesParams,
    ScheduleOccurrencesResult,
    ScheduleToolCallParams,
    ScheduleToolsResult,
    ServerCapabilities,
    SessionExternalEventParams,
    SessionForkResult,
    SessionInputStateParams,
    SessionInputStateResult,
    SessionPeerResponseTerminalParams,
    SessionStreamCloseParams,
    SessionStreamCloseResult,
    SessionStreamOpenParams,
    SessionStreamOpenResult,
    SessionTranscriptRewriteResult,
    SkillListResponse,
    SystemPromptUpdateResult,
    ToolsRegisterParams,
    ToolsRegisterResult,
    UpdateScheduleParams,
    UpdateSystemPromptParams,
    WireAuthProfileCleared,
    WireAuthProfileCreated,
    WireAuthProfileDetail,
    WireAuthProfilesList,
    WireAuthStatusDetail,
    WireDeviceCompleteResult,
    WireDeviceStart,
    WireLoginReady,
    WireLoginStart,
    WireProvisionApiKeyResult,
    WireRealmConnectionSet,
    WireRealmList,
    WireRunResult,
    WireSessionHistory,
    WireSessionInfo,
    WireSessionTranscriptRevision,
    WireSessionTranscriptRevisionList,
    WorkEventsResult,
    WorkGraphEventFilter,
    WorkGraphIdParams,
    WorkGraphSnapshot,
    WorkGraphSnapshotFilter,
    WorkItem,
    WorkItemFilter,
    WorkItemsResult,
)


class RpcRequest(Protocol):
    @overload
    def __call__(
        self,
        method: Literal["initialize"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[ServerCapabilities]: ...

    @overload
    def __call__(
        self,
        method: Literal["tools/register"],
        params: ToolsRegisterParams,
        /,
    ) -> Awaitable[ToolsRegisterResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/get"],
        params: JobsGetParams,
        /,
    ) -> Awaitable[JobsGetResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/list"],
        params: JobsListParams,
        /,
    ) -> Awaitable[JobsListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/cancel"],
        params: JobsCancelParams,
        /,
    ) -> Awaitable[JobsCancelResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/progress"],
        params: JobsProgressParams,
        /,
    ) -> Awaitable[JobsProgressResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/result"],
        params: JobsResultParams,
        /,
    ) -> Awaitable[JobsResultResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/artifacts"],
        params: JobsArtifactsParams,
        /,
    ) -> Awaitable[JobsArtifactsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/retry"],
        params: JobsRetryParams,
        /,
    ) -> Awaitable[JobsRetryResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/health"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[JobsHealthResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["monitors/start"],
        params: MonitorsStartParams,
        /,
    ) -> Awaitable[MonitorsStartResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/subscribe"],
        params: JobsSubscribeParams,
        /,
    ) -> Awaitable[JobsSubscribeResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["jobs/unsubscribe"],
        params: JobsUnsubscribeParams,
        /,
    ) -> Awaitable[JobsUnsubscribeResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mobkit/jobs/heartbeat"],
        params: MobkitJobHeartbeatParams,
        /,
    ) -> Awaitable[MobkitJobMutationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mobkit/jobs/progress"],
        params: MobkitJobProgressParams,
        /,
    ) -> Awaitable[MobkitJobMutationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mobkit/jobs/checkpoint"],
        params: MobkitJobCheckpointParams,
        /,
    ) -> Awaitable[MobkitJobMutationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mobkit/jobs/complete"],
        params: MobkitJobCompleteParams,
        /,
    ) -> Awaitable[MobkitJobMutationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mobkit/jobs/fail"],
        params: MobkitJobFailParams,
        /,
    ) -> Awaitable[MobkitJobMutationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mobkit/jobs/cancel_ack"],
        params: MobkitJobCancelAckParams,
        /,
    ) -> Awaitable[MobkitJobMutationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/create"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[WireRunResult | DeferredCreateResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/list"],
        params: ListSessionsParams,
        /,
    ) -> Awaitable[ListSessionsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/read"],
        params: ReadSessionParams,
        /,
    ) -> Awaitable[WireSessionInfo]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/history"],
        params: ReadSessionHistoryParams,
        /,
    ) -> Awaitable[WireSessionHistory]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/export_atif"],
        params: ExportAtifParams,
        /,
    ) -> Awaitable[Any]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/fork_at"],
        params: ForkSessionAtParams,
        /,
    ) -> Awaitable[SessionForkResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/fork_replace"],
        params: ForkSessionReplaceParams,
        /,
    ) -> Awaitable[SessionForkResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/rewrite_transcript"],
        params: RewriteSessionTranscriptParams,
        /,
    ) -> Awaitable[SessionTranscriptRewriteResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/update_system_prompt"],
        params: UpdateSystemPromptParams,
        /,
    ) -> Awaitable[SystemPromptUpdateResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/activate_instruction"],
        params: ActivateInstructionParams,
        /,
    ) -> Awaitable[InstructionActivationReceipt]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/instruction_activations"],
        params: ReadInstructionActivationsParams,
        /,
    ) -> Awaitable[InstructionActivationReadPage]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/transcript_revision"],
        params: ReadSessionTranscriptRevisionParams,
        /,
    ) -> Awaitable[WireSessionTranscriptRevision]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/transcript_revisions"],
        params: ListSessionTranscriptRevisionsParams,
        /,
    ) -> Awaitable[WireSessionTranscriptRevisionList]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/restore_transcript_revision"],
        params: RestoreSessionTranscriptRevisionParams,
        /,
    ) -> Awaitable[SessionTranscriptRewriteResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/archive"],
        params: ArchiveSessionParams,
        /,
    ) -> Awaitable[Any]: ...

    @overload
    def __call__(
        self,
        method: Literal["turn/start"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[WireRunResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["turn/interrupt"],
        params: InterruptParams,
        /,
    ) -> Awaitable[InterruptResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["config/get"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[ConfigEnvelope]: ...

    @overload
    def __call__(
        self,
        method: Literal["config/set"],
        params: ConfigSetParams,
        /,
    ) -> Awaitable[ConfigWriteResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["config/patch"],
        params: ConfigPatchParams,
        /,
    ) -> Awaitable[ConfigWriteResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["capabilities/get"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[CapabilitiesResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["runtime/host_info"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[RuntimeHostInfo]: ...

    @overload
    def __call__(
        self,
        method: Literal["runtime/capabilities"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[RuntimeHostCapabilities]: ...

    @overload
    def __call__(
        self,
        method: Literal["runtime/health"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[RuntimeHostHealth]: ...

    @overload
    def __call__(
        self,
        method: Literal["approval/request"],
        params: ApprovalRequestParams,
        /,
    ) -> Awaitable[ApprovalRecord]: ...

    @overload
    def __call__(
        self,
        method: Literal["approval/list"],
        params: ApprovalListParams,
        /,
    ) -> Awaitable[ApprovalListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["approval/get"],
        params: ApprovalGetParams,
        /,
    ) -> Awaitable[ApprovalRecord]: ...

    @overload
    def __call__(
        self,
        method: Literal["approval/decide"],
        params: ApprovalDecideParams,
        /,
    ) -> Awaitable[ApprovalRecord]: ...

    @overload
    def __call__(
        self,
        method: Literal["models/catalog"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[ModelsCatalogResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/profile/list"],
        params: RealmIdParams,
        /,
    ) -> Awaitable[WireAuthProfilesList]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/profile/get"],
        params: BindingIdParams,
        /,
    ) -> Awaitable[WireAuthProfileDetail]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/profile/create"],
        params: CreateProfileParams,
        /,
    ) -> Awaitable[WireAuthProfileCreated]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/profile/delete"],
        params: BindingIdParams,
        /,
    ) -> Awaitable[WireAuthProfileCleared]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/login/start"],
        params: LoginStartParams,
        /,
    ) -> Awaitable[WireLoginStart]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/login/complete"],
        params: LoginCompleteParams,
        /,
    ) -> Awaitable[WireLoginReady]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/login/device_start"],
        params: DeviceStartParams,
        /,
    ) -> Awaitable[WireDeviceStart]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/login/device_complete"],
        params: DeviceCompleteParams,
        /,
    ) -> Awaitable[WireDeviceCompleteResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/login/provision_api_key"],
        params: ProvisionApiKeyParams,
        /,
    ) -> Awaitable[WireProvisionApiKeyResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/status/get"],
        params: BindingIdParams,
        /,
    ) -> Awaitable[WireAuthStatusDetail]: ...

    @overload
    def __call__(
        self,
        method: Literal["auth/logout"],
        params: BindingIdParams,
        /,
    ) -> Awaitable[WireAuthProfileCleared]: ...

    @overload
    def __call__(
        self,
        method: Literal["realm/list"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[WireRealmList]: ...

    @overload
    def __call__(
        self,
        method: Literal["realm/get"],
        params: RealmIdParams,
        /,
    ) -> Awaitable[WireRealmConnectionSet]: ...

    @overload
    def __call__(
        self,
        method: Literal["help/ask"],
        params: HelpRequest,
        /,
    ) -> Awaitable[HelpResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["blob/get"],
        params: BlobGetParams,
        /,
    ) -> Awaitable[BlobPayload]: ...

    @overload
    def __call__(
        self,
        method: Literal["artifact/list"],
        params: ArtifactListParams,
        /,
    ) -> Awaitable[ArtifactListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["artifact/get"],
        params: ArtifactIdParams,
        /,
    ) -> Awaitable[ArtifactRecord]: ...

    @overload
    def __call__(
        self,
        method: Literal["artifact/download"],
        params: ArtifactDownloadParams,
        /,
    ) -> Awaitable[ArtifactDownloadResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/external_event"],
        params: SessionExternalEventParams,
        /,
    ) -> Awaitable[RuntimeAcceptResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/peer_response_terminal"],
        params: SessionPeerResponseTerminalParams,
        /,
    ) -> Awaitable[RuntimeAcceptResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/inject_context"],
        params: InjectSystemContextParams,
        /,
    ) -> Awaitable[InjectSystemContextResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/input_status"],
        params: SessionInputStateParams,
        /,
    ) -> Awaitable[SessionInputStateResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["events/latest_cursor"],
        params: EventsLatestCursorParams,
        /,
    ) -> Awaitable[EventsLatestCursorResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["events/list_since"],
        params: EventsListSinceParams,
        /,
    ) -> Awaitable[EventsListSinceResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["events/snapshot"],
        params: EventsSnapshotParams,
        /,
    ) -> Awaitable[EventsSnapshotResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/stream_open"],
        params: SessionStreamOpenParams,
        /,
    ) -> Awaitable[SessionStreamOpenResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["session/stream_close"],
        params: SessionStreamCloseParams,
        /,
    ) -> Awaitable[SessionStreamCloseResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/create"],
        params: CreateScheduleRequest,
        /,
    ) -> Awaitable[Schedule]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/get"],
        params: ScheduleIdParams,
        /,
    ) -> Awaitable[Schedule]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/list"],
        params: ListSchedulesParams,
        /,
    ) -> Awaitable[ScheduleListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/update"],
        params: UpdateScheduleParams,
        /,
    ) -> Awaitable[Schedule]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/pause"],
        params: ScheduleIdParams,
        /,
    ) -> Awaitable[Schedule]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/resume"],
        params: ScheduleIdParams,
        /,
    ) -> Awaitable[Schedule]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/delete"],
        params: ScheduleIdParams,
        /,
    ) -> Awaitable[Schedule]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/occurrences"],
        params: ScheduleOccurrencesParams,
        /,
    ) -> Awaitable[ScheduleOccurrencesResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/tools"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[ScheduleToolsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["schedule/call"],
        params: ScheduleToolCallParams,
        /,
    ) -> Awaitable[Any]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/get"],
        params: WorkGraphIdParams,
        /,
    ) -> Awaitable[WorkItem]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/list"],
        params: WorkItemFilter,
        /,
    ) -> Awaitable[WorkItemsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/ready"],
        params: ReadyWorkFilter,
        /,
    ) -> Awaitable[WorkItemsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/snapshot"],
        params: WorkGraphSnapshotFilter,
        /,
    ) -> Awaitable[WorkGraphSnapshot]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/events"],
        params: WorkGraphEventFilter,
        /,
    ) -> Awaitable[WorkEventsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/goal/status"],
        params: GoalStatusRequest,
        /,
    ) -> Awaitable[GoalStatusResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["workgraph/attention/list"],
        params: AttentionListRequest,
        /,
    ) -> Awaitable[AttentionListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["skills/list"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[SkillListResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/open"],
        params: LiveOpenParams,
        /,
    ) -> Awaitable[LiveOpenResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/status"],
        params: LiveChannelParams,
        /,
    ) -> Awaitable[LiveStatusResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/close"],
        params: LiveChannelParams,
        /,
    ) -> Awaitable[LiveCloseResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/send_input"],
        params: LiveSendInputParams,
        /,
    ) -> Awaitable[LiveSendInputResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/commit_input"],
        params: LiveCommitInputParams,
        /,
    ) -> Awaitable[LiveCommitInputResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/interrupt"],
        params: LiveChannelParams,
        /,
    ) -> Awaitable[LiveInterruptResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/truncate"],
        params: LiveTruncateParams,
        /,
    ) -> Awaitable[LiveTruncateResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/refresh"],
        params: LiveChannelParams,
        /,
    ) -> Awaitable[LiveRefreshResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["live/webrtc/answer"],
        params: LiveWebrtcAnswerParams,
        /,
    ) -> Awaitable[LiveWebrtcAnswerResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/create"],
        params: MobCreateParams,
        /,
    ) -> Awaitable[MobCreateResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/list"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[MobListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/status"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobStatusResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/lifecycle"],
        params: MobLifecycleParams,
        /,
    ) -> Awaitable[MobLifecycleResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/spawn"],
        params: MobSpawnParams,
        /,
    ) -> Awaitable[MobSpawnResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/spawn_many"],
        params: MobSpawnManyParams,
        /,
    ) -> Awaitable[MobSpawnManyResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/ensure_member"],
        params: MobEnsureMemberParams,
        /,
    ) -> Awaitable[MobEnsureMemberResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/reconcile"],
        params: MobReconcileParams,
        /,
    ) -> Awaitable[MobReconcileResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/list_members_matching"],
        params: MobListMembersMatchingParams,
        /,
    ) -> Awaitable[MobListMembersMatchingResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/retire"],
        params: MobMemberParams,
        /,
    ) -> Awaitable[MobRetireResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/respawn"],
        params: MobRespawnParams,
        /,
    ) -> Awaitable[MobRespawnResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/wire"],
        params: MobWireParams,
        /,
    ) -> Awaitable[MobWireResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/wire_members_batch"],
        params: MobWireMembersBatchParams,
        /,
    ) -> Awaitable[MobWireMembersBatchResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/unwire"],
        params: MobUnwireParams,
        /,
    ) -> Awaitable[MobUnwireResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/members"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobMembersResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_tool_declaration"],
        params: MobMemberToolDeclarationParams,
        /,
    ) -> Awaitable[MobMemberToolDeclarationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/apply_member_tool_declaration"],
        params: MobApplyMemberToolDeclarationParams,
        /,
    ) -> Awaitable[MobApplyMemberToolDeclarationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/adopt_member_identity_declaration"],
        params: MobAdoptMemberIdentityDeclarationParams,
        /,
    ) -> Awaitable[MobAdoptMemberIdentityDeclarationResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/resolve_identity_convergence_block"],
        params: MobResolveIdentityConvergenceBlockParams,
        /,
    ) -> Awaitable[MobResolveIdentityConvergenceBlockResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/events"],
        params: MobEventsParams,
        /,
    ) -> Awaitable[MobEventsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_send"],
        params: MobMemberSendParams,
        /,
    ) -> Awaitable[MobMemberSendResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/ingress_interaction"],
        params: MobIngressInteractionParams,
        /,
    ) -> Awaitable[MobIngressInteractionResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/append_system_context"],
        params: MobAppendSystemContextParams,
        /,
    ) -> Awaitable[MobAppendSystemContextResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/flows"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobFlowsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/flow_run"],
        params: MobFlowRunParams,
        /,
    ) -> Awaitable[MobFlowRunResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/run"],
        params: MobRunParams,
        /,
    ) -> Awaitable[MobFlowRunResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/flow_status"],
        params: MobFlowStatusParams,
        /,
    ) -> Awaitable[MobFlowStatusResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/run_result"],
        params: MobRunResultParams,
        /,
    ) -> Awaitable[MobRunResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/flow_cancel"],
        params: MobFlowCancelParams,
        /,
    ) -> Awaitable[MobFlowCancelResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/spawn_helper"],
        params: MobSpawnHelperParams,
        /,
    ) -> Awaitable[MobHelperResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/fork_helper"],
        params: MobForkHelperParams,
        /,
    ) -> Awaitable[MobHelperResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/force_cancel"],
        params: MobMemberParams,
        /,
    ) -> Awaitable[MobForceCancelResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/turn_start"],
        params: MobTurnStartParams,
        /,
    ) -> Awaitable[WireRunResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_status"],
        params: MobMemberParams,
        /,
    ) -> Awaitable[MobMemberStatusResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/snapshot"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobSnapshotResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/destroy"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobDestroyResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/rotate_supervisor"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobRotateSupervisorResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/submit_work"],
        params: MobSubmitWorkParams,
        /,
    ) -> Awaitable[MobSubmitWorkResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/conclude_objective"],
        params: MobConcludeObjectiveParams,
        /,
    ) -> Awaitable[MobConcludeObjectiveResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/cancel_work"],
        params: MobCancelWorkParams,
        /,
    ) -> Awaitable[MobCancelWorkResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/cancel_all_work"],
        params: MobCancelAllWorkParams,
        /,
    ) -> Awaitable[MobCancelAllWorkResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/wait_kickoff"],
        params: MobWaitParams,
        /,
    ) -> Awaitable[MobWaitMembersResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/wait_ready"],
        params: MobWaitParams,
        /,
    ) -> Awaitable[MobWaitMembersResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/profile/create"],
        params: MobProfileCreateParams,
        /,
    ) -> Awaitable[MobProfileLookupResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/profile/get"],
        params: MobProfileNameParams,
        /,
    ) -> Awaitable[MobProfileLookupResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/profile/list"],
        params: dict[str, Any],
        /,
    ) -> Awaitable[MobProfileListResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/profile/update"],
        params: MobProfileUpdateParams,
        /,
    ) -> Awaitable[MobProfileLookupResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/profile/delete"],
        params: MobProfileDeleteParams,
        /,
    ) -> Awaitable[MobProfileDeleteResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/stream_open"],
        params: MobStreamOpenParams,
        /,
    ) -> Awaitable[MobStreamOpenResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/stream_close"],
        params: MobStreamCloseParams,
        /,
    ) -> Awaitable[MobStreamCloseResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/grant_scopes"],
        params: MobGrantScopesParams,
        /,
    ) -> Awaitable[MobGrantScopesResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/revoke_scopes"],
        params: MobRevokeScopesParams,
        /,
    ) -> Awaitable[MobRevokeScopesResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/grants"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobGrantsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_history"],
        params: MobMemberHistoryParams,
        /,
    ) -> Awaitable[MobMemberHistoryResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/hosts"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobHostsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/route_installs"],
        params: MobIdParams,
        /,
    ) -> Awaitable[MobRouteInstallsResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/bind_host"],
        params: MobBindHostParams,
        /,
    ) -> Awaitable[MobBindHostResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/revoke_host"],
        params: MobRevokeHostParams,
        /,
    ) -> Awaitable[MobRevokeHostResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/hard_cancel_member"],
        params: MobHardCancelParams,
        /,
    ) -> Awaitable[MobHardCancelResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_live_open"],
        params: MobMemberLiveOpenParams,
        /,
    ) -> Awaitable[LiveOpenResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_live_close"],
        params: MobMemberLiveChannelParams,
        /,
    ) -> Awaitable[LiveCloseResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_live_status"],
        params: MobMemberLiveStatusParams,
        /,
    ) -> Awaitable[LiveStatusResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["mob/member_live_control"],
        params: MobMemberLiveControlParams,
        /,
    ) -> Awaitable[BridgeLiveControlOutcome]: ...

    @overload
    def __call__(
        self,
        method: Literal["mcp/add"],
        params: McpAddParams,
        /,
    ) -> Awaitable[McpLiveOpResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["mcp/remove"],
        params: McpRemoveParams,
        /,
    ) -> Awaitable[McpLiveOpResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["mcp/reload"],
        params: McpReloadParams,
        /,
    ) -> Awaitable[McpLiveOpResponse]: ...

    @overload
    def __call__(
        self,
        method: Literal["comms/send"],
        params: CommsSendParams,
        /,
    ) -> Awaitable[CommsSendResult]: ...

    @overload
    def __call__(
        self,
        method: Literal["comms/peers"],
        params: CommsPeersParams,
        /,
    ) -> Awaitable[CommsPeersResult]: ...
