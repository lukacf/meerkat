use super::*;

pub(super) async fn device(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    bridge: &Bridge,
) -> Result<bool, ConnectError> {
    let (send, mut notices) = mpsc::channel(1);
    let mut shown = move |notice| {
        send.try_send(notice)
            .map_err(|_| io::Error::other("Provider presentation ended"))
    };
    let (cancel, mut cancelled) = watch::channel(false);
    let interrupt = async {
        tokio::select! { biased; _=bridge.closed()=>{},
        _=async { while !*cancelled.borrow_and_update() { if cancelled.changed().await.is_err(){break;} } }=>{} };
        Ok(())
    };
    tokio::pin!(interrupt);
    let operation = async {
        let mut presentation = SignInPresentation::new(&mut shown);
        device_login_presented(
            startup,
            target,
            &mut io::sink(),
            interrupt.as_mut(),
            Some(&mut presentation),
        )
        .await
    };
    tokio::pin!(operation);
    if bridge.retired() {
        return Ok(false);
    }
    if let Some(result) = operation.as_mut().now_or_never() {
        return result;
    }
    let notice = tokio::select! { biased; _=bridge.closed()=>{cancel.send_replace(true);return operation.await.map(|_|false)},
    result=operation.as_mut()=>return result, notice=notices.recv()=>notice };
    let prompt = match notice {
        Some(SignInNotice::Device {
            verification_uri,
            user_code,
        }) => json!({"kind":"device","verification_uri":verification_uri,"user_code":user_code}),
        _ => {
            cancel.send_replace(true);
            return Err(combined(
                invalid("Native device presentation unavailable"),
                operation.await.map(|_| ()),
            ));
        }
    };
    let response = bridge.ask(prompt);
    tokio::pin!(response);
    tokio::select! { biased;
        answer=response.as_mut()=>{cancel.send_replace(true);let joined=operation.await;
            match answer { Ok(Answer::Cancel)=>joined.map(|_|false),Ok(_)=>Err(combined(invalid("Device notices only accept cancellation"),joined.map(|_|()))),Err(error)=>Err(combined(error,joined.map(|_|()))) }},
        result=operation.as_mut()=>result,
    }
}

/// Transport parser only. Native flow authority still verifies and exchanges.
pub(super) fn callback(
    raw: &str,
    redirect: &str,
    state: &str,
) -> Result<(String, String), ConnectError> {
    let url = reqwest::Url::parse(raw).map_err(|_| invalid("Malformed callback return"))?;
    let expected = reqwest::Url::parse(redirect)
        .map_err(|_| invalid("Native callback profile unavailable"))?;
    if url.scheme() != expected.scheme()
        || url.host_str() != expected.host_str()
        || url.port_or_known_default() != expected.port_or_known_default()
        || url.path() != expected.path()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "Callback does not match the retained native redirect",
        ));
    }
    let mut fields = std::collections::BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if !matches!(
            key.as_ref(),
            "state" | "code" | "error" | "error_description"
        ) || fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(invalid("Ambiguous callback return"));
        }
    }
    if fields.get("state").map(String::as_str) != Some(state) {
        return Err(invalid("Callback state does not match this attempt"));
    }
    if fields.contains_key("error") {
        return Err(invalid("Provider refused browser authorization"));
    }
    if fields.contains_key("error_description") {
        return Err(invalid("Unexpected callback error description"));
    }
    let code = fields
        .remove("code")
        .filter(|s| !s.is_empty() && s.len() <= 8192 && !s.chars().any(char::is_control))
        .ok_or_else(|| invalid("Callback code unavailable"))?;
    Ok((state.to_owned(), code))
}

pub(super) async fn browser(
    startup: &mut SessionStartup,
    target: &ResolvedConnectionTarget,
    request: &CreateSessionModelResolutionRequest,
    identity: OAuthProviderIdentity,
    transport: BrowserTransport,
    bridge: &Bridge,
) -> Result<bool, ConnectError> {
    let authority = startup.provider_auth_runtime_authority().await?;
    let service = HostAuthService::from_factory(&startup.factory, authority.clone())
        .map_err(|e| ConnectError::provider_setup("Open native browser authentication", e))?;
    let target_ref = host_target(target, identity);
    let (path, host, ports): (&str, &str, &[u16]) = match identity {
        OAuthProviderIdentity::OpenAiChatGpt => ("/auth/callback", "localhost", &[1455, 1457]),
        OAuthProviderIdentity::AnthropicClaudeAi => ("/callback", "localhost", &[0]),
        OAuthProviderIdentity::GoogleCodeAssist => ("/callback", "127.0.0.1", &[0]),
        _ => return Err(invalid("No native browser transport for this identity")),
    };
    let binding = bind_loopback_callback_with_redirect(path, host, ports)
        .await
        .map_err(|e| ConnectError::provider_setup("Bind native callback", e))?;
    let redirect = binding.redirect_url.clone();
    if bridge.retired() {
        binding
            .cancel()
            .await
            .map_err(|e| ConnectError::provider_setup("Close native callback", e))?;
        return Ok(false);
    }
    // Do not race/drop a started native login_start future either.
    let start = match service
        .login_start(&startup.config, &target_ref, redirect.clone())
        .await
    {
        Ok(start) => start,
        Err(error) => {
            return Err(combined(
                ConnectError::provider_setup("Start native browser flow", error),
                binding
                    .cancel()
                    .await
                    .map_err(|e| ConnectError::provider_setup("Close native callback", e)),
            ));
        }
    };
    let flow = authority.oauth_flow_authority();
    let expire = || {
        flow.expire(
            &start.state,
            &target.credential_identity,
            identity.into(),
            &redirect,
        )
        .map_err(|e| ConnectError::provider_setup("Expire native browser attempt", e))
    };
    let handle = binding.expect_state(start.state.clone());
    let selected = select_callback(
        handle,
        &redirect,
        &start.state,
        &start.authorize_url,
        transport,
        bridge,
    )
    .await;
    let (state, code) = match selected {
        Ok(Some(value)) => value,
        Ok(None) => {
            expire()?;
            return Ok(false);
        }
        Err(error) => return Err(combined(error, expire())),
    };
    if bridge.retired() {
        expire()?;
        return Ok(false);
    }
    if let Err(error) = current(startup, target, request).await {
        return Err(combined(error, expire()));
    }
    if bridge.retired() {
        expire()?;
        return Ok(false);
    }
    // Once entered, this exact native exchange owns mutation and is joined even
    // if the viewer expires or the parent disappears. No detached retry.
    service
        .login_complete(&startup.config, &target_ref, redirect, state, code)
        .await
        .map_err(|e| ConnectError::provider_setup("Complete native browser authentication", e))?;
    Ok(!bridge.retired())
}

/// Exact production winner/transport join boundary, separately testable without
/// issuing a provider exchange. It consumes one actual native handle.
pub(super) async fn select_callback(
    mut handle: meerkat_auth_core::auth_oauth::LoopbackHandle,
    redirect: &str,
    state: &str,
    authorization_url: &str,
    transport: BrowserTransport,
    bridge: &Bridge,
) -> Result<Option<(String, String)>, ConnectError> {
    let deadline = tokio::time::Instant::now() + CALLBACK_WINDOW;
    let response=bridge.ask(json!({"kind":if transport==BrowserTransport::ManualReturn {"browser_manual"} else {"browser"},
        "authorization_url":authorization_url,"redirect":redirect}));
    tokio::pin!(response);
    let selected = tokio::select! { biased;
        _=bridge.closed()=>Ok(None),
        _=tokio::time::sleep_until(deadline)=>Err(invalid("Native callback window expired")),
        answer=response.as_mut()=>match answer {
            Ok(Answer::Cancel)=>Ok(None),
            Ok(Answer::Callback {url}) if transport==BrowserTransport::ManualReturn=>callback(&url,redirect,state).map(Some),
            Ok(_)=>Err(invalid("Unexpected browser response")),Err(error)=>Err(error),
        },
        result=handle.wait_until(deadline)=>result.map(|callback|Some((callback.state,callback.code)))
            .map_err(|e|ConnectError::provider_setup("Receive native callback",e)),
    };
    let closed = handle
        .close()
        .await
        .map_err(|e| ConnectError::provider_setup("Join native callback transport", e));
    match selected {
        Ok(value) => {
            let _ = closed?;
            Ok(value)
        }
        Err(error) => Err(combined(error, closed.map(|_| ()))),
    }
}
