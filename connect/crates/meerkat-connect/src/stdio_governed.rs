//! Protocol 7's governed next-service startup. All entered work stays in the
//! existing Opened worker until its actual close and cancellation joins finish.
use super::*;
use crate::{
    service_preparation::private_native::PrivateNativeSetup,
    service_probe::{ProbeAccess, ServiceSelection},
    terminal_connector::private_service::Admission,
};
use meerkat::governed_realm::{
    open_stock_realm_persistence_for_host,
    stock::{
        StockPrincipals,
        issuance::{DEFAULT_STOCK_AGENT, stock_agent_principal},
    },
};

pub(super) async fn open(
    mut startup: SessionStartup,
    model: meerkat::CreateSessionModelResolutionRequest,
    bridge: PromptBridge,
    selection: ServiceSelection,
    version: u8,
    adapter: Adapter,
) -> Result<Opened, OpenFailure> {
    let host = selection.host().clone();
    if version != 7
        || !adapter.is_services()
        || startup.storage.locator != host.binding.realm
        || bridge.retired()
        || !host.lifetime.current(host.binding.expires_at_ms)
    {
        return Err("invalid_binding".into());
    }
    let storage = startup.storage.clone();
    let session = meerkat::Session::new();
    let artifacts = crate::resources::reserve_setup_artifacts(&storage)
        .map_err(|error| OpenFailure::native("unavailable", error))?;
    let actor =
        ApprovalPrincipalId::new(host.binding.viewer_id.clone()).map_err(|_| "invalid_binding")?;
    let expires = std::time::UNIX_EPOCH
        .checked_add(Duration::from_millis(host.binding.expires_at_ms))
        .ok_or("invalid_binding")?;
    let (approvals, mut approval_presenter) =
        LocalApprovals::create_browser(&artifacts, actor, expires)
            .map_err(|error| OpenFailure::native("unavailable", error))?;
    approvals
        .bind_session(session.id())
        .map_err(|error| OpenFailure::native("invalid_binding", error))?;
    let approvals = Arc::new(approvals);
    let setup = PrivateNativeSetup::new(
        session.id().clone(),
        storage.locator.clone(),
        approvals.clone(),
    );
    let admission =
        Admission::new(host.binding.expires_at_ms, bridge.retired.clone()).with_host(Some(&host));
    let result = async {
        check(&bridge)?;
        #[cfg(target_os = "linux")]
        if let Some(input) = selection.selected_input() {
            setup.bind_selected_input(input)?;
        }
        if meerkat_store::read_realm_posture_in(
            &storage.locator.state_root,
            storage.locator.realm.as_str(),
        )
        .await
        .map_err(|error| ConnectError::native("Read governed next-service posture", error))?
            != meerkat_store::RealmPosture::Governed
        {
            return Err(ended());
        }
        let resources = tokio::task::spawn_blocking(crate::resources::InstalledResources::locate)
            .await
            .map_err(|error| {
                ConnectError::native("Join next-service resource verification", error)
            })??;
        check(&bridge)?;
        let interrupted = async {
            bridge.closed().await;
            Ok(())
        };
        let (helper, _prepared_artifacts) = setup.prepare(resources, &storage, interrupted).await?;
        check(&bridge)?;
        startup
            .review_private_helper(
                &model,
                session.id(),
                &helper,
                &setup,
                &approvals,
                &mut approval_presenter,
                &bridge,
                &admission,
                &artifacts,
            )
            .await?;
        check(&bridge)?;
        let principals = StockPrincipals::for_local_operator(&storage.locator.realm)
            .map_err(|error| ConnectError::native("Identify private setup owner", error))?;
        let access = Arc::new(ProbeAccess {
            lifetime: host.lifetime.clone(),
            expires_at_ms: host.binding.expires_at_ms,
            caller: host.binding.caller.clone(),
            owner: principals.operator,
            executor: stock_agent_principal(&storage.locator.realm, DEFAULT_STOCK_AGENT)
                .map_err(|error| ConnectError::native("Identify private setup executor", error))?,
            session: session.id().clone(),
        });
        let (_, persistence) = open_stock_realm_persistence_for_host(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            storage.locator.realm.as_str(),
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout.clone()),
            DEFAULT_STOCK_AGENT,
            host.binding.ingress.clone(),
            access,
        )
        .await
        .map_err(|error| ConnectError::native("Open authenticated next-service setup", error))?;
        let stock = persistence.stock_governance().cloned().ok_or_else(ended)?;
        host.lifetime.join(stock.authorization_publication())?;
        startup.persistence = Some(persistence);
        check(&bridge)?;
        let model = crate::provider_setup::private::present(&mut startup, model, bridge.clone())
            .await?
            .ok_or_else(ended)?;
        check(&bridge)?;
        let plain = crate::terminal_connector::TerminalConnectors::open(&mut startup).await?;
        let connector = plain.private_control();
        // Capture the clean factory before installing this single-use adapter
        // composer; a later probe must not retain or reconstruct this setup.
        let capture = startup.capture_probe_source(model.clone(), session.id().clone());
        let auth = startup.connector_auth.as_ref().ok_or_else(ended)?;
        startup.factory = setup.configure(
            startup
                .factory
                .clone()
                .mcp_call_context_provider(auth.context_provider.clone()),
            plain.private_services_dispatcher_factory(),
        )?;
        let options = setup.options(
            SessionOptions {
                model,
                ..Default::default()
            },
            &helper,
        )?;
        check(&bridge)?;
        let opened = startup
            .materialize_private_services(options, session, &setup, &helper)
            .await?;
        let published = (|| {
            check(&bridge)?;
            let (lazy, presenter) = setup.take_composition()?;
            let capabilities = capabilities(
                version,
                adapter,
                false,
                &lazy.declared_tools(),
                Some(&presenter),
            )
            .map_err(|_| ended())?;
            Ok::<_, ConnectError>((lazy, presenter, capabilities))
        })();
        match published {
            Ok((lazy, presenter, capabilities)) => Ok(Opened {
                session: Arc::new(opened),
                approvals: approvals.clone(),
                _lazy: Arc::new(lazy),
                approval_presenter,
                forge_presenter: None,
                paired_host_presenter: None,
                secret_presenter: None,
                service_presenter: Some((presenter, storage)),
                capabilities,
                connector: Some(connector),
                plain: Some(plain),
                probe_capture: Some(capture),
            }),
            Err(primary) => Err(match opened.close().await {
                Ok(()) => primary,
                Err(cleanup) => ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                },
            }),
        }
    }
    .await;
    match result {
        Ok(opened) => Ok(opened),
        Err(primary) => Err(OpenFailure::native(
            "connector_unavailable",
            match setup.close().await {
                Ok(()) => primary,
                Err(cleanup) => ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                },
            },
        )),
    }
}

fn check(bridge: &PromptBridge) -> Result<(), ConnectError> {
    if bridge.retired() {
        Err(ended())
    } else {
        Ok(())
    }
}
