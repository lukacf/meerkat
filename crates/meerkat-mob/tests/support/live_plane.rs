//! Phase 6b live-plane fixture half (work item C6): the REAL member-host
//! live plane composed over a spawned `HostDaemonFixture`, mirroring the
//! `rkat mob host` composition shape (mob_host.rs:553-590) — live-ws
//! `TcpListener` plus the `ServiceLiveProjection` four-role sink, the
//! `LiveAdapterHost`, `LiveWsState`, `serve_live_ws_listener`, the injected
//! S4 scripted realtime factory, and the `ServiceMemberLiveHost`
//! machine-slot install.
//!
//! Deliberately NOT declared inside `support/mod.rs`: that module is
//! `#[path]`-included by `tests/integration/tests/e2e_fast_lane.rs` and by
//! meerkat-cli's e2e-system root, neither of which carries meerkat-live /
//! facade live-feature dev-dependencies. Live test roots declare
//! `#[path = "support/live_plane.rs"] mod live_support;` beside
//! `mod support;`.
//!
//! Cross-lane seams consumed BY NAME (ADJ-P6B-1/-4/-16; owned by the
//! live-pipeline lane, verified against its in-worktree shapes):
//!   * `meerkat::surface::ServiceMemberLiveHost` +
//!     `ServiceMemberLiveHostConfig` — the facade-owned `MemberLiveHost`
//!     impl (DEC-P6B-L10), isolated in ONE install fn below.
//!   * `MeerkatMachine::set_member_live_host` — the machine shell slot
//!     (the `set_member_observation_host` precedent).
//!   * `meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory` —
//!     the shared deterministic realtime fake (facade feature
//!     `test-realtime-fixtures`, ADJ-P6B-4): `new()` supports OpenAI,
//!     `supporting_no_provider()` drives the B18 row, `open_count()` /
//!     `adapters()[..].commands()` are the observation surface.

#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

pub use meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory;

use crate::support::HostDaemonFixture;

/// Handle over one composed fixture live plane. Dropping (or `shutdown`)
/// aborts the WS serve task; the machine-slot install lives as long as the
/// fixture's runtime adapter.
pub struct FixtureLivePlane {
    /// Advertised ws base URL — the bind-ceremony fact AND the base every
    /// minted bootstrap URL must start with (seam S5).
    pub advertise: String,
    /// The REAL bound listener address (`127.0.0.1:<port>`).
    pub local_addr: std::net::SocketAddr,
    /// S4 scripted factory handle (open-count / observation assertions).
    pub factory: Arc<ScriptedRealtimeSessionFactory>,
    serve_task: tokio::task::JoinHandle<()>,
}

impl FixtureLivePlane {
    pub fn shutdown(&self) {
        self.serve_task.abort();
    }
}

impl Drop for FixtureLivePlane {
    fn drop(&mut self) {
        self.serve_task.abort();
    }
}

/// Default scripted factory: supports OpenAI (the one realtime catalog
/// row's provider) and serves unlimited deterministic sessions.
pub fn scripted_realtime_factory() -> Arc<ScriptedRealtimeSessionFactory> {
    Arc::new(ScriptedRealtimeSessionFactory::default())
}

/// A factory supporting NO provider — the B18
/// `LiveAdapterUnavailable{provider}` row (T-C11).
pub fn unsupporting_realtime_factory() -> Arc<ScriptedRealtimeSessionFactory> {
    Arc::new(ScriptedRealtimeSessionFactory::supporting_no_provider())
}

/// Bind the fixture live-ws listener FIRST (loopback, port 0) so its real
/// address can feed both the advertise knob and the fixture's
/// `HostFixtureOptions.live_endpoint` bind fact before the daemon spawns.
pub async fn bind_live_listener() -> (tokio::net::TcpListener, std::net::SocketAddr) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fixture live-ws listener");
    let local_addr = listener.local_addr().expect("live listener local addr");
    (listener, local_addr)
}

/// Compose the live plane over `fixture` and install the member live host
/// on the machine slot. `advertise: None` ⇒ advertise the REAL bound
/// listener (`ws://{local_addr}`) so returned bootstraps are connectable
/// (T-C2); `Some(base)` pins a distinguishable non-listener base (T-C1's
/// verbatim-passthrough proof). The caller must have spawned the fixture
/// with `live_endpoint = Some(<same advertise value>)` so the bind fact and
/// the serving plane agree.
pub async fn install_live_plane(
    fixture: &HostDaemonFixture,
    listener: tokio::net::TcpListener,
    advertise: Option<String>,
    factory: Arc<ScriptedRealtimeSessionFactory>,
) -> FixtureLivePlane {
    let local_addr = listener.local_addr().expect("live listener local addr");
    let advertise = advertise.unwrap_or_else(|| format!("ws://{local_addr}"));
    let service = Arc::clone(
        fixture
            .member_concrete_service
            .as_ref()
            .expect("live plane requires the PersistentTemp member substrate"),
    );
    let adapter = Arc::clone(
        fixture
            .member_runtime_adapter
            .as_ref()
            .expect("live plane requires a member runtime adapter"),
    );

    // The mob_host.rs four-role composition: ONE ServiceLiveProjection Arc
    // cloned into every role.
    let projection = Arc::new(meerkat::surface::ServiceLiveProjection::new(
        Arc::clone(&service),
        Arc::clone(&adapter),
    ));
    let projection_sink: Arc<dyn meerkat_live::LiveProjectionSink> = projection.clone();
    let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = projection.clone();
    let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = projection.clone();
    let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = projection;
    let host = Arc::new(
        meerkat_live::LiveAdapterHost::new(projection_sink)
            .with_tool_timeout(meerkat_live::DEFAULT_LIVE_TOOL_TIMEOUT),
    );
    // DEC-P6B-L11 (§16.2 tool parity): the daemon composition installs the
    // owning-session tool dispatcher; the fixture mirrors it.
    host.set_live_tool_dispatcher(Arc::new(meerkat::surface::ServiceLiveToolDispatcher::new(
        Arc::clone(&service),
    )));
    let ws_state = Arc::new(meerkat_live::LiveWsState::new(
        Arc::clone(&host),
        close_feedback,
        status_feedback,
        token_authority,
    ));
    let ws_state_task = Arc::clone(&ws_state);
    let serve_task = tokio::spawn(async move {
        let _ = meerkat_live::serve_live_ws_listener(listener, ws_state_task).await;
    });

    install_member_live_host(
        &service,
        &adapter,
        host,
        ws_state,
        advertise.clone(),
        Arc::clone(&factory) as Arc<dyn meerkat_client::RealtimeSessionFactory>,
    );

    FixtureLivePlane {
        advertise,
        local_addr,
        factory,
        serve_task,
    }
}

/// The ADJ-P6B-16 install: construct the facade-owned member live host
/// over the fixture's own composition (the DEC-P6B-L10 config shape) and
/// hang it on the machine slot. Realm facts stay `None` — the fixture
/// realm never exercises recovery-from-cold on the live path; members are
/// resident when a live verb arrives. Isolated here so at most ONE fn
/// tracks any upstream constructor motion.
fn install_member_live_host(
    service: &Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    adapter: &Arc<meerkat_runtime::meerkat_machine::MeerkatMachine>,
    host: Arc<meerkat_live::LiveAdapterHost>,
    ws_state: Arc<meerkat_live::LiveWsState>,
    base_url: String,
    factory: Arc<dyn meerkat_client::RealtimeSessionFactory>,
) {
    let live_host = meerkat::surface::ServiceMemberLiveHost::new(
        meerkat::surface::ServiceMemberLiveHostConfig {
            service: Arc::clone(service),
            runtime_adapter: Arc::clone(adapter),
            host,
            ws_state: Some(ws_state),
            base_url: Some(base_url),
            session_factory: factory,
            realm_id: None,
            instance_id: None,
            backend: None,
        },
    );
    adapter.set_member_live_host(Arc::new(live_host));
}
