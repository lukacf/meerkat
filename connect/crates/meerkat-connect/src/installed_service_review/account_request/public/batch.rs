//! A bounded alternative on the existing public request tool. The helper
//! chooses every reconciled job; the host authenticates the same waiting task.
use super::*;
use toolkit_mobkit::private_originals::PublicServiceBatchEntry;
use toolkit_mobkit::installed_host::operator_setup::MAX_PUBLIC_SERVICE_BATCH;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchMemberProposal { job_id: String, operations: Vec<String> }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchProposal {
    services: Vec<BatchMemberProposal>,
    agent: String,
    #[serde(deserialize_with = "explicit_lifetime")]
    expires_in_ms: Option<u64>,
    tool_policy: ServiceToolPolicySelection,
}

struct Member {
    selected: PublicServiceSelection,
    configuration: McpServerConfig,
    selection: PublicSelection,
}
impl Member {
    fn current(&self) -> bool {
        self.selected.public_configuration(&self.selection.operations).ok().as_ref() == Some(&self.configuration)
    }
}

pub(super) struct InstalledPublicBatchRequest {
    members: Vec<Member>,
    expires_in_ms: Option<u64>,
    policy: ServiceToolPolicySelection,
    pub(super) task: Option<InstalledServiceTask>,
    entry: ReviewedEntryTicket,
    current: CurrentRequest,
    pub(super) response: oneshot::Sender<PublicReply>,
}

fn invalid_batch() -> ToolError {
    ToolError::InvalidArguments { name: SETUP_PUBLIC_SERVICE_REQUEST.into(), reason:
        "Provide services with 2-8 distinct reconciled public job_ids and exact raw operations, one intended agent, explicit expires_in_ms and one tool_policy covering the complete set. Duplicate jobs or MCP targets are refused.".into() }
}

fn parse_batch(raw: &str) -> Result<BatchProposal, ToolError> {
    if raw.len() > 64 * 1024 { return Err(invalid_batch()); }
    serde_json::from_str::<crate::forge::UniqueJson>(raw).map_err(|_| invalid_batch())?;
    let value: Value = serde_json::from_str(raw).map_err(|_| invalid_batch())?;
    if value.get("expires_in_ms").is_none() { return Err(invalid_batch()); }
    let proposal: BatchProposal = serde_json::from_str(raw).map_err(|_| invalid_batch())?;
    if proposal.services.len() < 2 || proposal.services.len() > MAX_PUBLIC_SERVICE_BATCH
        || proposal.tool_policy.catalog.is_none()
        || proposal.services.iter().map(|member| &member.job_id).collect::<BTreeSet<_>>().len() != proposal.services.len()
    { return Err(invalid_batch()); }
    // Preserve the existing singleton validation for each exact job/operation
    // proposal. This does not enter review or construct an approval record.
    for member in &proposal.services {
        parse(&serde_json::to_string(&json!({"job_id":member.job_id,"operations":member.operations,
            "agent":proposal.agent,"expires_in_ms":proposal.expires_in_ms})).map_err(|_| invalid_batch())?)?;
    }
    Ok(proposal)
}

pub(super) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>,
    context: &ToolDispatchContext, plan: Option<&ResolvedToolExecutionPlan>,
) -> Result<ToolDispatchOutcome, ToolError> {
    if context.origin_session_id() != Some(&owner.session)
        || context.work_authorization().is_none() || context.retained_work().is_none()
    { return Err(ToolError::access_denied(call.name)); }
    let mut current = context.check_tool_authorization(call, plan)?.unwrap_or_else(|| context.clone());
    let proposal = parse_batch(call.args.get())?;
    let mut members = Vec::with_capacity(proposal.services.len());
    for member in proposal.services {
        let selected = select_public_job(&owner.checked, &owner.realm, &owner.session, &member.job_id)?;
        let operations: BTreeSet<_> = member.operations.into_iter().collect();
        if !operations.is_subset(&selected.public_operations().map_err(|_| unavailable())?) {
            return Err(invalid_batch());
        }
        let configuration = selected.public_configuration(&operations).map_err(|_| unavailable())?;
        require_console_tool_policy(call.name, &configuration, &operations, Some(&proposal.tool_policy))?;
        members.push(Member { selected, configuration, selection: PublicSelection {
            operations, agent: proposal.agent.clone(), expires_in_ms: proposal.expires_in_ms,
            tool_policy: None,
        } });
    }
    members.sort_by(|left, right| left.configuration.name.cmp(&right.configuration.name));
    if members.windows(2).any(|pair| pair[0].configuration.name == pair[1].configuration.name) {
        return Err(invalid_batch());
    }
    let task = owner.tasks.as_ref().ok_or_else(|| ToolError::access_denied(call.name))?
        .prepare_public_service(call, &current, plan).await
        .map_err(|_| unavailable())?.ok_or_else(|| ToolError::access_denied(call.name))?;
    require_captured_consumer(call.name, &proposal.agent, &task)?;
    // This new set shape deliberately uses one real protected Console review.
    // Existing singleton application policy behavior remains its own path.
    if task.console_login().is_none() || members.iter().any(|member| !member.current()) {
        return Err(ToolError::access_denied(call.name));
    }
    if let Some(refreshed) = current.check_tool_authorization(call, plan)? { current = refreshed; }
    let entry = current.reviewed_entry_ticket(call, plan)?;
    let (checks, mut requests) = mpsc::channel::<oneshot::Sender<bool>>(1);
    let (response, receive) = oneshot::channel();
    owner.public_requests.as_ref().ok_or_else(unavailable)?.try_send(InstalledPublicRequest::Batch(
        InstalledPublicBatchRequest { members, expires_in_ms: proposal.expires_in_ms, policy: proposal.tool_policy,
            task: Some(task), entry, current: CurrentRequest(checks), response }
    )).map_err(|_| unavailable())?;
    tokio::pin!(receive);
    let response = loop {
        tokio::select! { biased;
            request = requests.recv() => {
                let Some(request) = request else { break receive.await; };
                match current.check_tool_authorization(call, plan) {
                    Ok(refreshed) => { if let Some(refreshed) = refreshed { current = refreshed; } let _ = request.send(true); }
                    Err(error) => { let _ = request.send(false); return Err(error); }
                }
            }
            response = receive.as_mut() => break response,
        }
    };
    current.check_tool_authorization(call, plan)?;
    serde_json::to_string(&response.map_err(|_| unavailable())?)
        .map(|body| ToolResult::new(call.id.into(), body, false).into()).map_err(|_| unavailable())
}

impl InstalledAccountRequests {
    pub(super) async fn present_public_batch_console<F: Future<Output = ()>>(
        &mut self, mut request: InstalledPublicBatchRequest,
        presenter: &mut Option<ApprovalPresenter>, bridge: PromptBridge,
        refresh: &console::Refresh<'_>, decision: &console::CaptureDecision<'_>, closed: F,
    ) -> Result<bool, ConnectError> {
        let owner_closed = AtomicBool::new(false);
        let result = async {
            if request.response.is_closed() { return Ok(true); }
            let login = request.task.as_ref().and_then(|task| task.console_login()).cloned()
                .ok_or_else(|| refused("The public service set has no retained Console login"))?;
            let reservation = login.reserve().map_err(|_| refused("The Console login refused public service set entry"))?;
            let checked_refresh = || -> futures_util::future::BoxFuture<'static, Result<(), ConnectError>> {
                let login = login.clone();
                let current = login.check().map_err(|_| refused("The Console login is unavailable"));
                let refreshed = current.is_ok().then(|| refresh());
                Box::pin(async move {
                    current?;
                    refreshed.expect("a current login selected refresh").await?;
                    login.check().map_err(|_| refused("The Console login is unavailable"))?;
                    Ok(())
                })
            };
            let refresh: &console::Refresh<'_> = &checked_refresh;
            let result = async {
                let closing = async { tokio::select! { biased;
                    _ = closed => { owner_closed.store(true, Ordering::SeqCst); }, _ = login.closed() => {},
                }};
                tokio::pin!(closing);
                if !console::checkpoint(&request.current, &mut request.response, &bridge, refresh, closing.as_mut()).await? {
                    return Ok(request.response.is_closed());
                }
                if request.members.iter().any(|member| !member.current()) { return Err(refused("A selected public installation changed")); }
                let custody = request.entry.enter().map_err(|_| refused("The public service set cannot enter"))?;
                let (cancel_review, cancellation) = ReviewCancellation::channel();
                let task = request.task.take().ok_or_else(|| refused("The waiting task is unavailable"))?;
                let reviewed = console::drive_review_cancellable(
                    self.review.review_public_batch(&request.members, request.expires_in_ms, request.policy,
                        task, &request.current, refresh, decision, cancellation),
                    &self.review.approvals, presenter, &bridge, refresh, &mut request.response,
                    closing.as_mut(), Some(&login), Some(cancel_review), Some(&owner_closed),
                ).await?;
                let Some(reply) = reviewed else { return Ok(request.response.is_closed()); };
                if !console::checkpoint(&request.current, &mut request.response, &bridge, refresh, closing.as_mut()).await? {
                    return Ok(request.response.is_closed());
                }
                if request.members.iter().any(|member| !member.current()) { return Err(refused("A public installation changed after review")); }
                let _ = request.response.send(reply);
                drop(custody);
                Ok(true)
            }.await;
            if result.is_ok() { reservation.settled(); }
            result
        }.await;
        if owner_closed.load(Ordering::SeqCst) || presenter.is_none() {
            let retired = self.review.retire();
            drop(presenter.take());
            return match result {
                Ok(_) => retired.map(|()| false),
                Err(error) => present::join_cleanup(Err(error), retired).map(|()| false),
            };
        }
        result.map(|_| true)
    }
}

impl InstalledServiceReview {
    async fn review_public_batch(&self, members: &[Member], expires_in_ms: Option<u64>, policy: ServiceToolPolicySelection,
        task: InstalledServiceTask, current: &CurrentRequest, refresh: &console::Refresh<'_>,
        capture_decision: &console::CaptureDecision<'_>, cancellation: ReviewCancellation,
    ) -> Result<PublicReply, ConnectError> {
        let check = || -> Result<(), ConnectError> {
            self.approvals.check_session(&self.session)?;
            cancellation.check()?;
            if members.iter().any(|member| !member.current()) { return Err(refused("The selected public service set changed")); }
            Ok(())
        };
        check()?; refresh().await?;
        if !current.check().await { return Err(refused("The public service set is no longer current")); }
        let mut entries = Vec::with_capacity(members.len());
        let mut documents = Vec::with_capacity(members.len());
        for member in members {
            let document = public_review_body(&member.selected, &member.configuration, &member.selection)?;
            let mut request = batch_request(&member.selected, document.clone());
            request.proposed_action.summary = "Review one member of the public service set".into();
            documents.push(document);
            entries.push(PublicServiceBatchEntry { configuration: member.configuration.clone(),
                operations: member.selection.operations.iter().cloned().map(toolkit_package::Id::new)
                    .collect::<Result<_, _>>().map_err(|_| refused("The public operations are invalid"))?, request });
        }
        let request = batch_request(&members.first().ok_or_else(|| refused("The public set is empty"))?.selected,
            json!({"phase":"installed_public_service_batch_access","services":documents,
                "tool_policy":policy,"notice":"One exact set, one fresh instruction only after every required attachment is current and Ready. Partial effects are retained and reconciled individually; approval is not readiness or rollback."}));
        let prepared = task.prepare_public_batch(entries, expires_in_ms, request, policy).await
            .map_err(|_| refused("The waiting task cannot be offered for this public service set"))?;
        check()?;
        let record = self.approvals.review_access_cancellable(prepared.request().clone(), cancellation.cancelled(),
            |id| cancellation.approval_created(id)).await?;
        let decision_current = capture_decision()?;
        check()?;
        if record.status == ApprovalStatus::Denied { return Ok(PublicReply { state: PublicState::Denied,
            access_reviewed: true, grant_issued: false, fresh_instruction_retained: false, ready: false }); }
        if record.status != ApprovalStatus::Approved { return Err(refused("The public set has no matching decision")); }
        if !current.check().await { return Err(refused("The public service set is no longer current")); }
        refresh().await?; check()?;
        let selected: Vec<_> = members.iter().map(|member| (member.selected.clone(), member.configuration.clone(), member.selection.operations.clone())).collect();
        let approvals = self.approvals.clone(); let session = self.session.clone(); let cancelled = cancellation.clone();
        let retained = prepared.approve(record, move || approvals.check_session(&session).is_ok()
            && cancelled.check().is_ok() && decision_current()
            && selected.iter().all(|(selection, config, operations)| selection.public_configuration(operations).ok().as_ref() == Some(config)))
            .await.map_err(|_| refused("The approved public service set could not be retained"))?;
        check()?;
        Ok(PublicReply { state: PublicState::Reviewed, access_reviewed: true, grant_issued: retained,
            fresh_instruction_retained: retained, ready: false })
    }
}

fn batch_request(selected: &PublicServiceSelection, body: Value) -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("connect-installed-setup-agent").expect("fixed principal"),
        owner: ApprovalOwnerRef::Session { session_id: selected.session().clone() },
        resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-installed-public-service-set") },
        proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
            summary: "Review this exact public service set for the task".into(), body: Some(body.clone()) },
        risk: ApprovalRisk::Medium, request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((SystemTime::now() + Duration::from_secs(120)).into()),
        metadata: Default::default(), request_provenance: None,
    }
}

#[cfg(test)]
mod tests;
