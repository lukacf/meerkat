use super::*;

// Failure-only local diagnostics. Callers supply fixed labels, never HTTP values.
fn status_read_refused(stage: &'static str, cause: &'static str) {
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr().lock(),
        "{{\"event\":\"toolkit_console_status_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}"
    );
}

pub(super) fn router(panel: ConsoleSetup) -> Router {
    Router::new()
        .route("/toolkit/setup", get(index))
        .route("/toolkit/setup/data", get(data).post(data_page))
        .route("/toolkit/setup/start", post(start))
        .route("/toolkit/setup/answer", post(answer))
        .route("/toolkit/setup/credentials", post(credentials))
        .route("/toolkit/setup/cancel-sign-in", post(cancel_sign_in))
        .route("/toolkit/setup/access/revoke", post(revoke_access))
        .route(
            "/toolkit/setup/recovery/revoke",
            post(recover_service_approval),
        )
        .route(
            "/toolkit/setup/callback",
            get(callback_landing).post(callback),
        )
        .route("/toolkit/setup/{asset}", get(asset))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(panel)
}
pub(super) fn status_disclosure_failure(
    error: meerkat_toolkit::host_lifetime::control::HostLifetimeError,
) -> StatusCode {
    match error {
        meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::FORBIDDEN,
    }
}
fn failure(status: StatusCode) -> Response {
    response(
        status,
        "application/json",
        "{\"error\":\"Protected setup is unavailable\"}",
    )
}
/// Why a setup answer route refused its request.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// A protected setup failure status.
    Status(StatusCode),
    /// The caller is not a verified, current person: nothing was answered.
    ApproverUnauthenticated,
}
impl From<StatusCode> for Refusal {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}
impl Refusal {
    pub(super) fn into_response(self) -> Response {
        match self {
            Self::Status(status) => failure(status),
            Self::ApproverUnauthenticated => response(
                StatusCode::FORBIDDEN,
                "application/json",
                json!({
                    "error": meerkat_mobkit::http_console::CONSOLE_APPROVER_UNAUTHENTICATED_KIND,
                    "message": "Setup answers are made only by a signed-in person; nothing was answered",
                })
                .to_string(),
            ),
        }
    }
}
/// A setup answer is made only by a verified, current person: the human the
/// request's verified token names, as the Console resolves it. A caller
/// without a token on an open Console, a service token, a token that names
/// no human and a shared-account token answer nothing, and so does a person
/// whose token no longer verifies (`None`, after an await).
pub(super) fn answering_person(viewer: Option<&ConsoleViewerContext>) -> Result<(), Refusal> {
    let person = viewer.is_some_and(|viewer| {
        !viewer.service_identity
            && viewer
                .human
                .as_ref()
                .is_some_and(|human| human.current().is_ok())
    });
    if person {
        Ok(())
    } else {
        Err(Refusal::ApproverUnauthenticated)
    }
}
fn response(status: StatusCode, content_type: &'static str, body: impl Into<String>) -> Response {
    (status, [("content-type", content_type), ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"), ("x-content-type-options", "nosniff"),
        ("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'"
    )], body.into()).into_response()
}
fn resolve_viewer(
    panel: &ConsoleSetup,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<ConsoleViewerContext, StatusCode> {
    if uri.query().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    panel
        .0
        .viewer
        .get()
        .and_then(|resolver| resolver.resolve(headers, uri))
        .ok_or(StatusCode::UNAUTHORIZED)
}
fn viewer(
    panel: &ConsoleSetup,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(ConsoleViewerContext, ConsoleLoginHandle), StatusCode> {
    let viewer = resolve_viewer(panel, headers, uri)?;
    let login = panel
        .0
        .binding
        .current_login(&viewer)
        .map_err(|_| StatusCode::FORBIDDEN)?;
    Ok((viewer, login))
}
fn mutation(panel: &ConsoleSetup, headers: &HeaderMap, uri: &Uri) -> Result<(), StatusCode> {
    if uri.query().is_some()
        || headers.get_all("origin").iter().count() != 1
        || headers.get("origin").and_then(|value| value.to_str().ok())
            != Some(panel.0.selection.origin.as_str())
        || headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
fn selected(
    panel: &ConsoleSetup,
    headers: HeaderMap,
    uri: Uri,
) -> Result<(Arc<Active>, Action), Refusal> {
    mutation(panel, &headers, &uri)?;
    let viewer = resolve_viewer(panel, &headers, &uri)?;
    // Only a verified, current person answers a setup prompt.
    answering_person(Some(&viewer))?;
    let login = panel
        .0
        .binding
        .current_login(&viewer)
        .map_err(|_| StatusCode::FORBIDDEN)?;
    // The caller answers only the prompt of its own login.
    let active = panel
        .0
        .active
        .of(&login)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::CONFLICT)?;
    let action = Action {
        viewer: panel
            .0
            .viewer
            .get()
            .ok_or(StatusCode::SERVICE_UNAVAILABLE)?
            .clone(),
        binding: panel.0.binding.clone(),
        headers,
        uri,
        login,
        task: active.task.clone(),
        identity: Arc::new(OnceLock::new()),
    };
    Ok((active, action))
}
/// After the route's awaits: the answer's carrier must still name a verified,
/// current person (a login that ended answers nothing), then the action's own
/// check (`checked`) and its exact presentation must still hold.
fn still_selected(
    panel: &ConsoleSetup,
    active: &Arc<Active>,
    action: &Action,
    checked: bool,
) -> Result<(), Refusal> {
    answering_person(action.viewer.resolve(&action.headers, &action.uri).as_ref())?;
    if !checked {
        return Err(StatusCode::FORBIDDEN.into());
    }
    action.current().ok_or(StatusCode::FORBIDDEN)?;
    if !panel
        .0
        .active
        .of(&action.login)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .is_some_and(|current| Arc::ptr_eq(&current, active))
    {
        return Err(StatusCode::CONFLICT.into());
    }
    Ok(())
}
async fn index(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(status) = viewer(&panel, &headers, &uri) {
        return failure(status);
    }
    response(
        StatusCode::OK,
        "text/html; charset=utf-8",
        include_str!("assets/index.html"),
    )
}
async fn asset(
    State(panel): State<ConsoleSetup>,
    Path(name): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(status) = viewer(&panel, &headers, &uri) {
        return failure(status);
    }
    let (kind, bytes) = match name.as_str() {
        "panel.js" => ("application/javascript", include_str!("assets/panel.js")),
        "view.js" => ("application/javascript", include_str!("assets/view.js")),
        "projection.js" => (
            "application/javascript",
            include_str!("assets/projection.js"),
        ),
        "style.css" => ("text/css", include_str!("assets/style.css")),
        _ => return failure(StatusCode::NOT_FOUND),
    };
    response(StatusCode::OK, kind, bytes)
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PageDirection {
    Refresh,
    Next,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageRequest {
    pub(super) page_id: u64,
    pub(super) direction: PageDirection,
}
pub(super) fn page_request(body: &[u8]) -> Result<PageRequest, StatusCode> {
    if body.len() > 256 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let value: PageRequest = serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if value.page_id == 0 || value.page_id > 9_007_199_254_740_991 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(value)
}
async fn data(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    data_from(panel, headers, uri, None).await
}
async fn data_page(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    // This fixed read POST uses the same-origin JSON gate, not an action grant.
    if let Err(status) = mutation(&panel, &headers, &uri) {
        return failure(status);
    }
    let request = match page_request(&body) {
        Ok(value) => value,
        Err(status) => return failure(status),
    };
    data_from(panel, headers, uri, Some(request)).await
}
// One read owns its selected presentation across the native await. Retirement
// removes that presentation; replacement cannot retarget the completed read.
// `current` reads the reader's login's presentation now, after the await.
pub(super) async fn read_status_with_review<T, P>(
    selected: Option<Arc<T>>,
    current: impl FnOnce() -> Result<Option<Arc<T>>, ()>,
    read: impl std::future::Future<Output = Result<P, StatusCode>>,
) -> Result<(P, Option<Arc<T>>), StatusCode> {
    let page = read.await?;
    let selected = match selected {
        Some(selected) => match current().map_err(|()| StatusCode::CONFLICT)? {
            None => None,
            Some(current) if Arc::ptr_eq(&current, &selected) => Some(selected),
            Some(_) => return Err(StatusCode::CONFLICT),
        },
        None => None,
    };
    Ok((page, selected))
}
async fn data_from(
    panel: ConsoleSetup,
    headers: HeaderMap,
    uri: Uri,
    request: Option<PageRequest>,
) -> Response {
    let (_, login) = match viewer(&panel, &headers, &uri) {
        Ok(value) => value,
        Err(status) => return failure(status),
    };
    if panel.reclaim_terminal().await.is_err() {
        status_read_refused("http.data.reclaim_terminal", "error");
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    }
    let current = || {
        viewer(&panel, &headers, &uri)
            .ok()
            .and_then(|(viewer, current)| login.same_owner(&current).then_some(viewer))
    };
    let previous = match request.as_ref() {
        None => None,
        Some(request) => match panel.0.pages.lock() {
            Ok(pages) => match pages
                .iter()
                .find(|page| page.id == request.page_id && page.login.same_owner(&login))
            {
                Some(page) => Some(page.value.clone()),
                None => return failure(StatusCode::CONFLICT),
            },
            Err(_) => {
                status_read_refused("http.data.previous_page_lock", "poisoned");
                return failure(StatusCode::SERVICE_UNAVAILABLE);
            }
        },
    };
    // Only the reader's own login's presentation is shown to it.
    let active = match panel.0.active.of(&login) {
        Ok(active) => active,
        Err(_) => {
            status_read_refused("http.data.active_lock", "poisoned");
            return failure(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    if let Some(active) = &active
        && (active
            .task
            .check_console_viewer(&current, &login)
            .await
            .is_err()
            || current().is_none())
    {
        return failure(StatusCode::FORBIDDEN);
    }
    let selected = request
        .as_ref()
        .zip(previous.as_ref())
        .map(|(request, previous)| {
            (
                previous.as_ref(),
                match request.direction {
                    PageDirection::Refresh => ConsolePageRead::Refresh,
                    PageDirection::Next => ConsolePageRead::Next,
                },
            )
        });
    let read = async {
        let page = match panel.0.binding.status_page(&current, selected).await {
            Ok(value) if value.same_login(&login) => Arc::new(value),
            Ok(_) => {
                status_read_refused("http.data.status_page", "wrong_login");
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            Err(error) => {
                use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
                status_read_refused(
                    "http.data.status_page",
                    match error {
                        HostLifetimeError::InvalidSelection => "InvalidSelection",
                        HostLifetimeError::Unavailable => "Unavailable",
                        HostLifetimeError::Denied => "Denied",
                        HostLifetimeError::WrongOwner => "WrongOwner",
                        HostLifetimeError::NotSettled => "NotSettled",
                    },
                );
                return Err(status_disclosure_failure(error));
            }
        };
        if current().is_none() {
            return Err(StatusCode::FORBIDDEN);
        }
        Ok(page)
    };
    let (page, active) =
        match read_status_with_review(active, || panel.0.active.of(&login).map_err(|_| ()), read)
            .await
        {
            Ok(value) => value,
            Err(status) => return failure(status),
        };
    let projection = page.projection();
    let review = if let Some(active) = active {
        let start = match active.start.lock() {
            Ok(slot) => !active.software && slot.is_some(),
            Err(_) => {
                status_read_refused("http.data.start_lock", "poisoned");
                return failure(StatusCode::SERVICE_UNAVAILABLE);
            }
        };
        let prompt = match active.prompt.lock() {
            Ok(slot) => slot
                .as_ref()
                .filter(|exchange| !exchange.response.is_closed())
                .map(|exchange| exchange.prompt.clone()),
            Err(_) => {
                status_read_refused("http.data.prompt_lock", "poisoned");
                return failure(StatusCode::SERVICE_UNAVAILABLE);
            }
        };
        let credential_input = match active.credentials.lock() {
            Ok(slot) => slot.as_ref().map(CredentialDelivery::projection),
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        let mut review = json!({"review_id":active.id, "can_start":start, "prompt":prompt});
        if let Some(input) = credential_input {
            review["credential_input"] = input;
        }
        Some(review)
    } else {
        None
    };
    if current().is_none() {
        return failure(StatusCode::FORBIDDEN);
    }
    let id = match panel
        .0
        .next
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
            (id < 9_007_199_254_740_991).then_some(id + 1)
        }) {
        Ok(id) => id,
        Err(_) => {
            status_read_refused("http.data.page_id", "exhausted");
            return failure(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    let mut envelope = json!({"projection":projection,"review":review,
        "page":{"page_id":id,"first":page.is_first(),"has_next":page.has_next()}});
    if let Some(mut value) = page.access_projection() {
        if page.access().is_some() {
            value["offer_id"] = json!(id);
        }
        envelope["application_access"] = value;
    }
    if let Some(mut value) = page.recovery_projection() {
        if page.recovery().is_some() {
            value["offer_id"] = json!(id);
        }
        envelope["operator_recovery"] = value;
    }
    if current().is_none() {
        return failure(StatusCode::FORBIDDEN);
    }
    if let Err(error) = panel.0.binding.check_status_page(&page, &current) {
        return failure(status_disclosure_failure(error));
    }
    let mut pages = match panel.0.pages.lock() {
        Ok(value) => value,
        Err(_) => {
            status_read_refused("http.data.offer_pages_lock", "poisoned");
            return failure(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    if replace_status_offer(
        &mut pages,
        &login,
        request.as_ref().map(|request| request.page_id),
        Some((id, page)),
    )
    .is_err()
    {
        return failure(StatusCode::CONFLICT);
    }
    response(StatusCode::OK, "application/json", envelope.to_string())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RevokeAccess {
    pub(super) offer_id: u64,
    pub(super) row_id: String,
}
// The actual mounted dispatch uses the exact artifact policy. Unknown fields,
// alternate principals and native grant coordinates never reach its owner.
pub(super) fn access_request(
    selection: &ConsoleSetupSelection,
    body: &[u8],
) -> Result<RevokeAccess, StatusCode> {
    if selection.access_management != Some(ConsoleAccessPolicy::RequesterIssuedLeaves) {
        return Err(StatusCode::FORBIDDEN);
    }
    bounded_offer_request(body)
}
pub(super) fn recovery_request(
    selection: &ConsoleSetupSelection,
    body: &[u8],
) -> Result<RevokeAccess, StatusCode> {
    if selection.recovery_management
        != Some(ConsoleRecoveryPolicy::OperatorUnpublishedServiceApprovals)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    bounded_offer_request(body)
}
fn bounded_offer_request(body: &[u8]) -> Result<RevokeAccess, StatusCode> {
    if body.len() > 1024 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let value: RevokeAccess = serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if value.offer_id == 0
        || value.offer_id > 9_007_199_254_740_991
        || value.row_id.len() != 64
        || !value
            .row_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(value)
}
async fn revoke_access(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    if let Err(status) = mutation(&panel, &headers, &uri) {
        return failure(status);
    }
    let request = match access_request(&panel.0.selection, &body) {
        Ok(value) => value,
        Err(status) => return failure(status),
    };
    let (_, login) = match viewer(&panel, &headers, &uri) {
        Ok(value) => value,
        Err(status) => return failure(status),
    };
    let page = match panel.0.pages.lock() {
        Ok(slot) => match slot
            .iter()
            .find(|offer| offer.id == request.offer_id && offer.login.same_owner(&login))
        {
            Some(offer) => offer.value.clone(),
            None => return failure(StatusCode::CONFLICT),
        },
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Some(offer) = page.access() else {
        return failure(StatusCode::CONFLICT);
    };
    let resolver = match panel.0.viewer.get() {
        Some(value) => value,
        None => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let native = || resolver.resolve(&headers, &uri);
    // This path uses no Active setup request and remains available for retained
    // completed sources. The one-use offer owns any entered outcome if reply is lost.
    match panel
        .0
        .binding
        .revoke_application_access(offer, &request.row_id, &native)
        .await
    {
        Ok(value) => response(StatusCode::OK, "application/json", value.to_string()),
        Err(_) => failure(StatusCode::FORBIDDEN),
    }
}
// An opaque same-login offer selects already-retained intent. No request body
// can supply a principal, native grant coordinate, approval or connector config.
async fn recover_service_approval(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    if let Err(status) = mutation(&panel, &headers, &uri) {
        return failure(status);
    }
    let request = match recovery_request(&panel.0.selection, &body) {
        Ok(value) => value,
        Err(status) => return failure(status),
    };
    let (_, login) = match viewer(&panel, &headers, &uri) {
        Ok(value) => value,
        Err(status) => return failure(status),
    };
    let page = match panel.0.pages.lock() {
        Ok(slot) => match slot
            .iter()
            .find(|offer| offer.id == request.offer_id && offer.login.same_owner(&login))
        {
            Some(offer) => offer.value.clone(),
            None => return failure(StatusCode::CONFLICT),
        },
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Some(offer) = page.recovery() else {
        return failure(StatusCode::CONFLICT);
    };
    let resolver = match panel.0.viewer.get() {
        Some(value) => value,
        None => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let native = || resolver.resolve(&headers, &uri);
    match panel
        .0
        .binding
        .recover_service_approval(offer, &request.row_id, &native)
        .await
    {
        Ok(value) => response(StatusCode::OK, "application/json", value.to_string()),
        Err(_) => failure(StatusCode::FORBIDDEN),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    review_id: u64,
}
async fn start(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    if body.len() > 1024 {
        return failure(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let request: Start = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST),
    };
    let (active, action) = match selected(&panel, headers, uri) {
        Ok(value) => value,
        Err(refusal) => return refusal.into_response(),
    };
    if request.review_id != active.id || active.software {
        return failure(StatusCode::CONFLICT);
    }
    let checked = action.check().await.is_ok();
    if let Err(refusal) = still_selected(&panel, &active, &action, checked) {
        if !checked {
            let _ = panel.reclaim_terminal().await;
        }
        return refusal.into_response();
    }
    let mut start = match active.start.lock() {
        Ok(start) => start,
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    if start.is_none() {
        return failure(StatusCode::CONFLICT);
    }
    if active.accept_action(action).is_err() {
        return failure(StatusCode::FORBIDDEN);
    }
    if start
        .take()
        .expect("checked pending start")
        .send(StartDisposition::Start)
        .is_err()
    {
        return failure(StatusCode::CONFLICT);
    }
    response(
        StatusCode::ACCEPTED,
        "application/json",
        "{\"state\":\"forwarded\"}",
    )
}
async fn answer(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let (active, action) = match selected(&panel, headers, uri) {
        Ok(value) => value,
        Err(refusal) => return refusal.into_response(),
    };
    let checked = action.check().await.is_ok();
    if let Err(refusal) = still_selected(&panel, &active, &action, checked) {
        return refusal.into_response();
    }
    // Credential approvals use one combined protected answer. The generic
    // prompt carrier never accepts, records or forwards submitted values.
    if active.credential_review {
        return failure(StatusCode::CONFLICT);
    }
    let result = match active.prompt.lock() {
        Ok(mut prompt) => answer_retained_authorized(active.id, &mut prompt, &body, || {
            active.accept_action(action)
        }),
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    if result.is_err() {
        return failure(StatusCode::CONFLICT);
    }
    response(
        StatusCode::ACCEPTED,
        "application/json",
        "{\"state\":\"forwarded\"}",
    )
}
async fn credentials(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let (active, action) = match selected(&panel, headers, uri) {
        Ok(value) => value,
        Err(refusal) => return refusal.into_response(),
    };
    let checked = action.check().await.is_ok();
    if let Err(refusal) = still_selected(&panel, &active, &action, checked) {
        return refusal.into_response();
    }
    let result = {
        let mut credentials = match active.credentials.lock() {
            Ok(slot) => slot,
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        let mut prompt = match active.prompt.lock() {
            Ok(slot) => slot,
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        answer_retained_credentials(active.id, &mut credentials, &mut prompt, &body, || {
            active.accept_action(action)
        })
    };
    if result.is_err() {
        return failure(StatusCode::CONFLICT);
    }
    // Forwarding is neither recorded approval nor a native mutation result.
    response(
        StatusCode::ACCEPTED,
        "application/json",
        "{\"state\":\"forwarded\"}",
    )
}
async fn cancel_sign_in(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let (active, action) = match selected(&panel, headers, uri) {
        Ok(value) => value,
        Err(refusal) => return refusal.into_response(),
    };
    let checked = action.check().await.is_ok();
    if let Err(refusal) = still_selected(&panel, &active, &action, checked) {
        return refusal.into_response();
    }
    // Serialize with callback delivery. A completed delivery remains owned by
    // the account driver; cancellation can only signal its retained notice.
    let result = {
        let mut callback = match active.callback.lock() {
            Ok(slot) => slot,
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        let mut prompt = match active.prompt.lock() {
            Ok(slot) => slot,
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        crate::installed_service_review::account_request::console::cancel_retained_sign_in_authorized(
            active.id, &mut prompt, &mut callback, &body, || active.accept_action(action))
    };
    if result.is_err() {
        return failure(StatusCode::CONFLICT);
    }
    // This acknowledges a request only. The existing account owner joins any
    // entered completion and preserves a result that won the cancellation race.
    response(
        StatusCode::ACCEPTED,
        "application/json",
        "{\"state\":\"forwarded\"}",
    )
}
async fn callback(
    State(panel): State<ConsoleSetup>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    let (active, action) = match selected(&panel, headers, uri) {
        Ok(value) => value,
        Err(refusal) => return refusal.into_response(),
    };
    let checked = action.check().await.is_ok();
    if let Err(refusal) = still_selected(&panel, &active, &action, checked) {
        return refusal.into_response();
    }
    let received = {
        let mut slot = match active.callback.lock() {
            Ok(slot) => slot,
            Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
        };
        let Some(callback) = slot.as_mut() else {
            return failure(StatusCode::CONFLICT);
        };
        match callback.deliver_json_authorized(&body, || active.accept_action(action.clone())) {
            Ok(received) => {
                slot.take();
                received
            }
            Err(_) => return failure(StatusCode::CONFLICT),
        }
    };
    // Losing this HTTP response cannot cancel or repeat the driver-owned work.
    let observed = received.await;
    if action.check().await.is_err() {
        return failure(StatusCode::FORBIDDEN);
    }
    match observed {
        Ok(CallbackResult::Completed) => response(
            StatusCode::OK,
            "application/json",
            "{\"state\":\"completed\"}",
        ),
        Ok(CallbackResult::Refused) => failure(StatusCode::CONFLICT),
        Ok(CallbackResult::CleanupUnconfirmed) | Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE),
    }
}
async fn callback_landing(OriginalUri(uri): OriginalUri) -> Response {
    if uri.query().is_some_and(|query| query.len() > 16 * 1024) {
        return failure(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mut result = response(
        StatusCode::OK,
        "text/html; charset=utf-8",
        include_str!("assets/callback.html"),
    );
    result.headers_mut().insert("content-security-policy", axum::http::HeaderValue::from_static(
        "default-src 'none'; script-src 'sha256-AlzvtVcRKoCs4IRppBW6S8w1qxYZGgAzUD3jY+7iSOM='; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
    result
}
