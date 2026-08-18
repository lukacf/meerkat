// Generated RPC method contracts for @rkat/sdk.
// Source: artifacts/schemas/rpc-methods.json
import type {
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
  MobAppendSystemContextParams,
  MobAppendSystemContextResult,
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
} from "./types.js";

export interface RpcMethodContracts {
  "initialize": {
    params: Record<string, never>;
    result: (ServerCapabilities) & Record<string, unknown>;
  };
  "tools/register": {
    params: ToolsRegisterParams;
    result: (ToolsRegisterResult) & Record<string, unknown>;
  };
  "jobs/get": {
    params: JobsGetParams;
    result: (JobsGetResult) & Record<string, unknown>;
  };
  "jobs/list": {
    params: JobsListParams;
    result: (JobsListResult) & Record<string, unknown>;
  };
  "jobs/cancel": {
    params: JobsCancelParams;
    result: (JobsCancelResult) & Record<string, unknown>;
  };
  "jobs/progress": {
    params: JobsProgressParams;
    result: (JobsProgressResult) & Record<string, unknown>;
  };
  "jobs/result": {
    params: JobsResultParams;
    result: (JobsResultResult) & Record<string, unknown>;
  };
  "jobs/artifacts": {
    params: JobsArtifactsParams;
    result: (JobsArtifactsResult) & Record<string, unknown>;
  };
  "jobs/retry": {
    params: JobsRetryParams;
    result: (JobsRetryResult) & Record<string, unknown>;
  };
  "jobs/health": {
    params: Record<string, never>;
    result: (JobsHealthResult) & Record<string, unknown>;
  };
  "monitors/start": {
    params: MonitorsStartParams;
    result: (MonitorsStartResult) & Record<string, unknown>;
  };
  "jobs/subscribe": {
    params: JobsSubscribeParams;
    result: (JobsSubscribeResult) & Record<string, unknown>;
  };
  "jobs/unsubscribe": {
    params: JobsUnsubscribeParams;
    result: (JobsUnsubscribeResult) & Record<string, unknown>;
  };
  "mobkit/jobs/heartbeat": {
    params: MobkitJobHeartbeatParams;
    result: (MobkitJobMutationResult) & Record<string, unknown>;
  };
  "mobkit/jobs/progress": {
    params: MobkitJobProgressParams;
    result: (MobkitJobMutationResult) & Record<string, unknown>;
  };
  "mobkit/jobs/checkpoint": {
    params: MobkitJobCheckpointParams;
    result: (MobkitJobMutationResult) & Record<string, unknown>;
  };
  "mobkit/jobs/complete": {
    params: MobkitJobCompleteParams;
    result: (MobkitJobMutationResult) & Record<string, unknown>;
  };
  "mobkit/jobs/fail": {
    params: MobkitJobFailParams;
    result: (MobkitJobMutationResult) & Record<string, unknown>;
  };
  "mobkit/jobs/cancel_ack": {
    params: MobkitJobCancelAckParams;
    result: (MobkitJobMutationResult) & Record<string, unknown>;
  };
  "session/create": {
    params: Record<string, unknown>;
    result: (WireRunResult | DeferredCreateResult) & Record<string, unknown>;
  };
  "session/list": {
    params: ListSessionsParams;
    result: (ListSessionsResult) & Record<string, unknown>;
  };
  "session/read": {
    params: ReadSessionParams;
    result: (WireSessionInfo) & Record<string, unknown>;
  };
  "session/history": {
    params: ReadSessionHistoryParams;
    result: (WireSessionHistory) & Record<string, unknown>;
  };
  "session/export_atif": {
    params: ExportAtifParams;
    result: unknown;
  };
  "session/fork_at": {
    params: ForkSessionAtParams;
    result: (SessionForkResult) & Record<string, unknown>;
  };
  "session/fork_replace": {
    params: ForkSessionReplaceParams;
    result: (SessionForkResult) & Record<string, unknown>;
  };
  "session/rewrite_transcript": {
    params: RewriteSessionTranscriptParams;
    result: (SessionTranscriptRewriteResult) & Record<string, unknown>;
  };
  "session/update_system_prompt": {
    params: UpdateSystemPromptParams;
    result: (SystemPromptUpdateResult) & Record<string, unknown>;
  };
  "session/transcript_revision": {
    params: ReadSessionTranscriptRevisionParams;
    result: (WireSessionTranscriptRevision) & Record<string, unknown>;
  };
  "session/transcript_revisions": {
    params: ListSessionTranscriptRevisionsParams;
    result: (WireSessionTranscriptRevisionList) & Record<string, unknown>;
  };
  "session/restore_transcript_revision": {
    params: RestoreSessionTranscriptRevisionParams;
    result: (SessionTranscriptRewriteResult) & Record<string, unknown>;
  };
  "session/archive": {
    params: ArchiveSessionParams;
    result: unknown;
  };
  "turn/start": {
    params: Record<string, unknown>;
    result: (WireRunResult) & Record<string, unknown>;
  };
  "turn/interrupt": {
    params: InterruptParams;
    result: (InterruptResult) & Record<string, unknown>;
  };
  "config/get": {
    params: Record<string, never>;
    result: (ConfigEnvelope) & Record<string, unknown>;
  };
  "config/set": {
    params: ConfigSetParams;
    result: (ConfigWriteResult) & Record<string, unknown>;
  };
  "config/patch": {
    params: ConfigPatchParams;
    result: (ConfigWriteResult) & Record<string, unknown>;
  };
  "capabilities/get": {
    params: Record<string, never>;
    result: (CapabilitiesResponse) & Record<string, unknown>;
  };
  "runtime/host_info": {
    params: Record<string, never>;
    result: (RuntimeHostInfo) & Record<string, unknown>;
  };
  "runtime/capabilities": {
    params: Record<string, never>;
    result: (RuntimeHostCapabilities) & Record<string, unknown>;
  };
  "runtime/health": {
    params: Record<string, never>;
    result: (RuntimeHostHealth) & Record<string, unknown>;
  };
  "approval/request": {
    params: ApprovalRequestParams;
    result: (ApprovalRecord) & Record<string, unknown>;
  };
  "approval/list": {
    params: ApprovalListParams;
    result: (ApprovalListResult) & Record<string, unknown>;
  };
  "approval/get": {
    params: ApprovalGetParams;
    result: (ApprovalRecord) & Record<string, unknown>;
  };
  "approval/decide": {
    params: ApprovalDecideParams;
    result: (ApprovalRecord) & Record<string, unknown>;
  };
  "models/catalog": {
    params: Record<string, never>;
    result: (ModelsCatalogResponse) & Record<string, unknown>;
  };
  "auth/profile/list": {
    params: RealmIdParams;
    result: (WireAuthProfilesList) & Record<string, unknown>;
  };
  "auth/profile/get": {
    params: BindingIdParams;
    result: (WireAuthProfileDetail) & Record<string, unknown>;
  };
  "auth/profile/create": {
    params: CreateProfileParams;
    result: (WireAuthProfileCreated) & Record<string, unknown>;
  };
  "auth/profile/delete": {
    params: BindingIdParams;
    result: (WireAuthProfileCleared) & Record<string, unknown>;
  };
  "auth/login/start": {
    params: LoginStartParams;
    result: (WireLoginStart) & Record<string, unknown>;
  };
  "auth/login/complete": {
    params: LoginCompleteParams;
    result: (WireLoginReady) & Record<string, unknown>;
  };
  "auth/login/device_start": {
    params: DeviceStartParams;
    result: (WireDeviceStart) & Record<string, unknown>;
  };
  "auth/login/device_complete": {
    params: DeviceCompleteParams;
    result: (WireDeviceCompleteResult) & Record<string, unknown>;
  };
  "auth/login/provision_api_key": {
    params: ProvisionApiKeyParams;
    result: (WireProvisionApiKeyResult) & Record<string, unknown>;
  };
  "auth/status/get": {
    params: BindingIdParams;
    result: (WireAuthStatusDetail) & Record<string, unknown>;
  };
  "auth/logout": {
    params: BindingIdParams;
    result: (WireAuthProfileCleared) & Record<string, unknown>;
  };
  "realm/list": {
    params: Record<string, never>;
    result: (WireRealmList) & Record<string, unknown>;
  };
  "realm/get": {
    params: RealmIdParams;
    result: (WireRealmConnectionSet) & Record<string, unknown>;
  };
  "help/ask": {
    params: HelpRequest;
    result: (HelpResponse) & Record<string, unknown>;
  };
  "blob/get": {
    params: BlobGetParams;
    result: (BlobPayload) & Record<string, unknown>;
  };
  "artifact/list": {
    params: ArtifactListParams;
    result: (ArtifactListResult) & Record<string, unknown>;
  };
  "artifact/get": {
    params: ArtifactIdParams;
    result: (ArtifactRecord) & Record<string, unknown>;
  };
  "artifact/download": {
    params: ArtifactDownloadParams;
    result: (ArtifactDownloadResult) & Record<string, unknown>;
  };
  "session/external_event": {
    params: SessionExternalEventParams;
    result: (RuntimeAcceptResult) & Record<string, unknown>;
  };
  "session/peer_response_terminal": {
    params: SessionPeerResponseTerminalParams;
    result: (RuntimeAcceptResult) & Record<string, unknown>;
  };
  "session/inject_context": {
    params: InjectSystemContextParams;
    result: (InjectSystemContextResult) & Record<string, unknown>;
  };
  "session/input_status": {
    params: SessionInputStateParams;
    result: (SessionInputStateResult) & Record<string, unknown>;
  };
  "events/latest_cursor": {
    params: EventsLatestCursorParams;
    result: (EventsLatestCursorResult) & Record<string, unknown>;
  };
  "events/list_since": {
    params: EventsListSinceParams;
    result: (EventsListSinceResult) & Record<string, unknown>;
  };
  "events/snapshot": {
    params: EventsSnapshotParams;
    result: (EventsSnapshotResult) & Record<string, unknown>;
  };
  "session/stream_open": {
    params: SessionStreamOpenParams;
    result: (SessionStreamOpenResult) & Record<string, unknown>;
  };
  "session/stream_close": {
    params: SessionStreamCloseParams;
    result: (SessionStreamCloseResult) & Record<string, unknown>;
  };
  "schedule/create": {
    params: CreateScheduleRequest;
    result: (Schedule) & Record<string, unknown>;
  };
  "schedule/get": {
    params: ScheduleIdParams;
    result: (Schedule) & Record<string, unknown>;
  };
  "schedule/list": {
    params: ListSchedulesParams;
    result: (ScheduleListResult) & Record<string, unknown>;
  };
  "schedule/update": {
    params: UpdateScheduleParams;
    result: (Schedule) & Record<string, unknown>;
  };
  "schedule/pause": {
    params: ScheduleIdParams;
    result: (Schedule) & Record<string, unknown>;
  };
  "schedule/resume": {
    params: ScheduleIdParams;
    result: (Schedule) & Record<string, unknown>;
  };
  "schedule/delete": {
    params: ScheduleIdParams;
    result: (Schedule) & Record<string, unknown>;
  };
  "schedule/occurrences": {
    params: ScheduleOccurrencesParams;
    result: (ScheduleOccurrencesResult) & Record<string, unknown>;
  };
  "schedule/tools": {
    params: Record<string, never>;
    result: (ScheduleToolsResult) & Record<string, unknown>;
  };
  "schedule/call": {
    params: ScheduleToolCallParams;
    result: unknown;
  };
  "workgraph/get": {
    params: WorkGraphIdParams;
    result: (WorkItem) & Record<string, unknown>;
  };
  "workgraph/list": {
    params: WorkItemFilter;
    result: (WorkItemsResult) & Record<string, unknown>;
  };
  "workgraph/ready": {
    params: ReadyWorkFilter;
    result: (WorkItemsResult) & Record<string, unknown>;
  };
  "workgraph/snapshot": {
    params: WorkGraphSnapshotFilter;
    result: (WorkGraphSnapshot) & Record<string, unknown>;
  };
  "workgraph/events": {
    params: WorkGraphEventFilter;
    result: (WorkEventsResult) & Record<string, unknown>;
  };
  "workgraph/goal/status": {
    params: GoalStatusRequest;
    result: (GoalStatusResult) & Record<string, unknown>;
  };
  "workgraph/attention/list": {
    params: AttentionListRequest;
    result: (AttentionListResult) & Record<string, unknown>;
  };
  "skills/list": {
    params: Record<string, never>;
    result: (SkillListResponse) & Record<string, unknown>;
  };
  "live/open": {
    params: LiveOpenParams;
    result: (LiveOpenResult) & Record<string, unknown>;
  };
  "live/status": {
    params: LiveChannelParams;
    result: (LiveStatusResult) & Record<string, unknown>;
  };
  "live/close": {
    params: LiveChannelParams;
    result: (LiveCloseResult) & Record<string, unknown>;
  };
  "live/send_input": {
    params: LiveSendInputParams;
    result: (LiveSendInputResult) & Record<string, unknown>;
  };
  "live/commit_input": {
    params: LiveCommitInputParams;
    result: (LiveCommitInputResult) & Record<string, unknown>;
  };
  "live/interrupt": {
    params: LiveChannelParams;
    result: (LiveInterruptResult) & Record<string, unknown>;
  };
  "live/truncate": {
    params: LiveTruncateParams;
    result: (LiveTruncateResult) & Record<string, unknown>;
  };
  "live/refresh": {
    params: LiveChannelParams;
    result: (LiveRefreshResult) & Record<string, unknown>;
  };
  "live/webrtc/answer": {
    params: LiveWebrtcAnswerParams;
    result: (LiveWebrtcAnswerResult) & Record<string, unknown>;
  };
  "mob/create": {
    params: MobCreateParams;
    result: (MobCreateResult) & Record<string, unknown>;
  };
  "mob/list": {
    params: Record<string, never>;
    result: (MobListResult) & Record<string, unknown>;
  };
  "mob/status": {
    params: MobIdParams;
    result: (MobStatusResult) & Record<string, unknown>;
  };
  "mob/lifecycle": {
    params: MobLifecycleParams;
    result: (MobLifecycleResult) & Record<string, unknown>;
  };
  "mob/spawn": {
    params: MobSpawnParams;
    result: (MobSpawnResult) & Record<string, unknown>;
  };
  "mob/spawn_many": {
    params: MobSpawnManyParams;
    result: (MobSpawnManyResult) & Record<string, unknown>;
  };
  "mob/ensure_member": {
    params: MobEnsureMemberParams;
    result: (MobEnsureMemberResult) & Record<string, unknown>;
  };
  "mob/reconcile": {
    params: MobReconcileParams;
    result: (MobReconcileResult) & Record<string, unknown>;
  };
  "mob/list_members_matching": {
    params: MobListMembersMatchingParams;
    result: (MobListMembersMatchingResult) & Record<string, unknown>;
  };
  "mob/retire": {
    params: MobMemberParams;
    result: (MobRetireResult) & Record<string, unknown>;
  };
  "mob/respawn": {
    params: MobRespawnParams;
    result: (MobRespawnResult) & Record<string, unknown>;
  };
  "mob/wire": {
    params: MobWireParams;
    result: (MobWireResult) & Record<string, unknown>;
  };
  "mob/wire_members_batch": {
    params: MobWireMembersBatchParams;
    result: (MobWireMembersBatchResult) & Record<string, unknown>;
  };
  "mob/unwire": {
    params: MobUnwireParams;
    result: (MobUnwireResult) & Record<string, unknown>;
  };
  "mob/members": {
    params: MobIdParams;
    result: (MobMembersResult) & Record<string, unknown>;
  };
  "mob/events": {
    params: MobEventsParams;
    result: (MobEventsResult) & Record<string, unknown>;
  };
  "mob/member_send": {
    params: MobMemberSendParams;
    result: (MobMemberSendResult) & Record<string, unknown>;
  };
  "mob/ingress_interaction": {
    params: MobIngressInteractionParams;
    result: (MobIngressInteractionResult) & Record<string, unknown>;
  };
  "mob/append_system_context": {
    params: MobAppendSystemContextParams;
    result: (MobAppendSystemContextResult) & Record<string, unknown>;
  };
  "mob/flows": {
    params: MobIdParams;
    result: (MobFlowsResult) & Record<string, unknown>;
  };
  "mob/flow_run": {
    params: MobFlowRunParams;
    result: (MobFlowRunResult) & Record<string, unknown>;
  };
  "mob/run": {
    params: MobRunParams;
    result: (MobFlowRunResult) & Record<string, unknown>;
  };
  "mob/flow_status": {
    params: MobFlowStatusParams;
    result: (MobFlowStatusResult) & Record<string, unknown>;
  };
  "mob/run_result": {
    params: MobRunResultParams;
    result: (MobRunResult) & Record<string, unknown>;
  };
  "mob/flow_cancel": {
    params: MobFlowCancelParams;
    result: (MobFlowCancelResult) & Record<string, unknown>;
  };
  "mob/spawn_helper": {
    params: MobSpawnHelperParams;
    result: (MobHelperResult) & Record<string, unknown>;
  };
  "mob/fork_helper": {
    params: MobForkHelperParams;
    result: (MobHelperResult) & Record<string, unknown>;
  };
  "mob/force_cancel": {
    params: MobMemberParams;
    result: (MobForceCancelResult) & Record<string, unknown>;
  };
  "mob/turn_start": {
    params: MobTurnStartParams;
    result: (WireRunResult) & Record<string, unknown>;
  };
  "mob/member_status": {
    params: MobMemberParams;
    result: (MobMemberStatusResult) & Record<string, unknown>;
  };
  "mob/snapshot": {
    params: MobIdParams;
    result: (MobSnapshotResult) & Record<string, unknown>;
  };
  "mob/destroy": {
    params: MobIdParams;
    result: (MobDestroyResult) & Record<string, unknown>;
  };
  "mob/rotate_supervisor": {
    params: MobIdParams;
    result: (MobRotateSupervisorResult) & Record<string, unknown>;
  };
  "mob/submit_work": {
    params: MobSubmitWorkParams;
    result: (MobSubmitWorkResult) & Record<string, unknown>;
  };
  "mob/conclude_objective": {
    params: MobConcludeObjectiveParams;
    result: (MobConcludeObjectiveResult) & Record<string, unknown>;
  };
  "mob/cancel_work": {
    params: MobCancelWorkParams;
    result: (MobCancelWorkResult) & Record<string, unknown>;
  };
  "mob/cancel_all_work": {
    params: MobCancelAllWorkParams;
    result: (MobCancelAllWorkResult) & Record<string, unknown>;
  };
  "mob/wait_kickoff": {
    params: MobWaitParams;
    result: (MobWaitMembersResult) & Record<string, unknown>;
  };
  "mob/wait_ready": {
    params: MobWaitParams;
    result: (MobWaitMembersResult) & Record<string, unknown>;
  };
  "mob/profile/create": {
    params: MobProfileCreateParams;
    result: (MobProfileLookupResult) & Record<string, unknown>;
  };
  "mob/profile/get": {
    params: MobProfileNameParams;
    result: (MobProfileLookupResult) & Record<string, unknown>;
  };
  "mob/profile/list": {
    params: Record<string, never>;
    result: (MobProfileListResult) & Record<string, unknown>;
  };
  "mob/profile/update": {
    params: MobProfileUpdateParams;
    result: (MobProfileLookupResult) & Record<string, unknown>;
  };
  "mob/profile/delete": {
    params: MobProfileDeleteParams;
    result: (MobProfileDeleteResult) & Record<string, unknown>;
  };
  "mob/stream_open": {
    params: MobStreamOpenParams;
    result: (MobStreamOpenResult) & Record<string, unknown>;
  };
  "mob/stream_close": {
    params: MobStreamCloseParams;
    result: (MobStreamCloseResult) & Record<string, unknown>;
  };
  "mob/grant_scopes": {
    params: MobGrantScopesParams;
    result: (MobGrantScopesResult) & Record<string, unknown>;
  };
  "mob/revoke_scopes": {
    params: MobRevokeScopesParams;
    result: (MobRevokeScopesResult) & Record<string, unknown>;
  };
  "mob/grants": {
    params: MobIdParams;
    result: (MobGrantsResult) & Record<string, unknown>;
  };
  "mob/member_history": {
    params: MobMemberHistoryParams;
    result: (MobMemberHistoryResult) & Record<string, unknown>;
  };
  "mob/hosts": {
    params: MobIdParams;
    result: (MobHostsResult) & Record<string, unknown>;
  };
  "mob/route_installs": {
    params: MobIdParams;
    result: (MobRouteInstallsResult) & Record<string, unknown>;
  };
  "mob/bind_host": {
    params: MobBindHostParams;
    result: (MobBindHostResult) & Record<string, unknown>;
  };
  "mob/revoke_host": {
    params: MobRevokeHostParams;
    result: (MobRevokeHostResult) & Record<string, unknown>;
  };
  "mob/hard_cancel_member": {
    params: MobHardCancelParams;
    result: (MobHardCancelResult) & Record<string, unknown>;
  };
  "mob/member_live_open": {
    params: MobMemberLiveOpenParams;
    result: (LiveOpenResult) & Record<string, unknown>;
  };
  "mob/member_live_close": {
    params: MobMemberLiveChannelParams;
    result: (LiveCloseResult) & Record<string, unknown>;
  };
  "mob/member_live_status": {
    params: MobMemberLiveStatusParams;
    result: (LiveStatusResult) & Record<string, unknown>;
  };
  "mob/member_live_control": {
    params: MobMemberLiveControlParams;
    result: (BridgeLiveControlOutcome) & Record<string, unknown>;
  };
  "mcp/add": {
    params: McpAddParams;
    result: (McpLiveOpResponse) & Record<string, unknown>;
  };
  "mcp/remove": {
    params: McpRemoveParams;
    result: (McpLiveOpResponse) & Record<string, unknown>;
  };
  "mcp/reload": {
    params: McpReloadParams;
    result: (McpLiveOpResponse) & Record<string, unknown>;
  };
  "comms/send": {
    params: CommsSendParams;
    result: (CommsSendResult) & Record<string, unknown>;
  };
  "comms/peers": {
    params: CommsPeersParams;
    result: (CommsPeersResult) & Record<string, unknown>;
  };
}

export type RpcMethodName = keyof RpcMethodContracts;
export type RpcMethodParams<M extends RpcMethodName> =
  RpcMethodContracts[M]["params"];
export type RpcMethodResult<M extends RpcMethodName> =
  RpcMethodContracts[M]["result"];
