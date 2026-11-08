//! Inherited host-only request channel. No MCP argument can select this peer.
use super::*;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
const REQUEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) struct Channel {
    pub(super) input: Option<ChildStdin>,
    pub(super) output: BufReader<ChildStdout>,
    sequence: u64,
    pending: bool,
}
impl Channel {
    pub(super) fn new(input: ChildStdin, output: BufReader<ChildStdout>) -> Self {
        Self {
            input: Some(input),
            output,
            sequence: 0,
            pending: false,
        }
    }
}

/// Transport-only observation. Only Joined proves this peer's adapter and
/// socket owner drained; it says nothing about P or physical worker completion.
#[cfg(any(test, feature = "console"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PeerRetirement {
    NotRequested,
    Pending,
    Joined,
    Failed,
}

#[cfg(any(test, feature = "console"))]
fn retirement_reply(
    value: serde_json::Value,
    peer: usize,
    close: bool,
) -> Result<PeerRetirement, ConnectError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        scope: String,
        peer: usize,
        state: PeerRetirement,
    }
    let reply: Reply = serde_json::from_value(value).map_err(|_| unavailable())?;
    if reply.scope != "host_shared_peer_retirement"
        || reply.peer != peer
        || (close && reply.state == PeerRetirement::NotRequested)
    {
        return Err(unavailable());
    }
    Ok(reply.state)
}

/// One connection selected by the resource owner. It has no decoder and is
/// transferred with that connection into the native helper composition.
#[derive(Clone)]
pub struct SharedSetupPeer {
    pub(super) channel: Arc<tokio::sync::Mutex<Channel>>,
    pub(super) index: usize,
    acquisition: Option<toolkit_package::Id>,
    pub(super) physical: Option<Arc<super::physical::PhysicalLease>>,
    pub(super) configuration: meerkat_core::McpServerConfig,
    pub(super) entries: Arc<super::entry::EntryTasks>,
    #[cfg(feature = "console")]
    pub(super) attempt: Arc<super::attempt::PeerAttempt>,
}
impl SharedSetupPeer {
    #[cfg(feature = "console")]
    pub(crate) async fn settled_preparation(
        self: &Arc<Self>,
        member: &meerkat_toolkit::setup::SharedPreparationMember,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
        session: &str,
    ) -> Result<Arc<dyn toolkit_mobkit::private_originals::SharedPreparationResource>, ConnectError>
    {
        if !self.attempt.current_registration(member.helper(), session) {
            return Err(unavailable());
        }
        let peer = self.for_acquisition(
            toolkit_package::Id::new(member.acquisition().id.clone()).map_err(|_| unavailable())?,
        );
        let value = peer
            .request(serde_json::json!({"operation":"observe_closed_selection",
            "preparation":member.key().id, "selection":selection}))
            .await?;
        if value
            .as_object()
            .is_none_or(|value| value.len() != 1 || !value.contains_key("completion"))
            || !self.attempt.current_registration(member.helper(), session)
        {
            return Err(unavailable());
        }
        let completion = meerkat_toolkit::setup::SharedPreparationCompletion::from_controller(
            value["completion"].clone(),
        )
        .map_err(|_| unavailable())?;
        if value["completion"]["job"]["status"] != "succeeded"
            || !value["completion"]["job"]["error"].is_null()
        {
            return Err(unavailable());
        }
        Ok(Arc::new(SettledPreparation {
            entries: self.entries.clone(),
            preparation: member.key().id.clone(),
            selection: selection.clone(),
            completion,
            job: value["completion"]["job"].clone(),
        }))
    }
    pub(super) fn new(
        channel: Arc<tokio::sync::Mutex<Channel>>,
        index: usize,
        configuration: meerkat_core::McpServerConfig,
        entries: Arc<super::entry::EntryTasks>,
    ) -> Self {
        Self {
            channel,
            index,
            acquisition: None,
            physical: None,
            configuration,
            entries,
            #[cfg(feature = "console")]
            attempt: Arc::new(super::attempt::PeerAttempt::default()),
        }
    }

    pub(super) fn with_physical(mut self, physical: Arc<super::physical::PhysicalLease>) -> Self {
        self.physical = Some(physical);
        self
    }

    /// Retain a projection of this same physical peer for one source selection.
    /// The caller caches this handle so canonical resource replay keeps identity.
    pub(crate) fn for_acquisition(&self, acquisition: toolkit_package::Id) -> Arc<Self> {
        Arc::new(Self {
            channel: self.channel.clone(),
            index: self.index,
            acquisition: Some(acquisition),
            configuration: self.configuration.clone(),
            entries: self.entries.clone(),
            physical: self.physical.clone(),
            #[cfg(feature = "console")]
            attempt: self.attempt.clone(),
        })
    }

    /// Fence this exact peer and retain its transport drain at the existing
    /// Python owner. Pending is an observation, not successful cleanup. This
    /// neither leaves canonical P membership nor stops the shared Controller.
    #[cfg(any(test, feature = "console"))]
    pub(crate) async fn close_peer(&self) -> Result<PeerRetirement, ConnectError> {
        self.peer_retirement(true).await
    }

    /// Observe the same retained close task without starting another close.
    #[cfg(any(test, feature = "console"))]
    pub(crate) async fn observe_peer(&self) -> Result<PeerRetirement, ConnectError> {
        self.peer_retirement(false).await
    }

    #[cfg(feature = "console")]
    pub(crate) async fn join_retirement(&self) -> Result<(), ConnectError> {
        let mut observed = self.close_peer().await?;
        loop {
            match observed {
                PeerRetirement::Joined => return Ok(()),
                PeerRetirement::Pending => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    observed = self.observe_peer().await?;
                }
                _ => return Err(unavailable()),
            }
        }
    }

    /// Replace only this exact joined attempt in the same inherited Controller.
    /// The caller retains the whole operation, including the returned custody,
    /// before awaiting it. Old indices, registrations and claims never change.
    #[cfg(feature = "console")]
    pub(crate) async fn renew(
        &self,
        member: &meerkat_core::MobMemberBinding,
        session: &str,
    ) -> Result<PreparedSharedConnection, ConnectError> {
        if self.acquisition.is_some() || !self.attempt.renewable(member, session) {
            return Err(unavailable());
        }
        let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) =
            &self.configuration.transport
        else {
            return Err(unavailable());
        };
        if !matches!(launch.args.len(), 9 | 13) || launch.args[7] != "--shared-peer" {
            return Err(unavailable());
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Renewal {
            scope: String,
            predecessor: usize,
            peer: usize,
            path: PathBuf,
        }
        let value = self
            .request(serde_json::json!({"operation": "renew_peer"}))
            .await?;
        let renewed: Renewal = serde_json::from_value(value).map_err(|_| unavailable())?;
        let filename = format!("{}.sock", renewed.peer);
        if renewed.scope != "host_shared_peer_renewed"
            || renewed.predecessor != self.index
            || renewed.peer <= self.index
            || renewed.peer >= 8
            || renewed.path.parent() != Path::new(&launch.args[8]).parent()
            || renewed.path.file_name().and_then(|name| name.to_str()) != Some(filename.as_str())
        {
            return Err(unavailable());
        }
        let mut args = launch.args.clone();
        args[8] = renewed.path.to_str().ok_or_else(unavailable)?.into();
        let bytes = serde_json::to_vec(&serde_json::json!({"mcpServers": {"toolkit-setup": {
            "command": launch.command, "args": args,
        }}}))
        .map_err(|_| unavailable())?;
        let connection = PreparedSetupConnection::from_preparer_output(&bytes)?;
        let configuration = PreparedSetupConnection::from_preparer_output(&bytes)?.into_server();
        let mut peer = Self::new(
            self.channel.clone(),
            renewed.peer,
            configuration,
            self.entries.clone(),
        );
        peer.physical = self.physical.clone();
        Ok(PreparedSharedConnection { connection, peer })
    }

    #[cfg(any(test, feature = "console"))]
    async fn peer_retirement(&self, close: bool) -> Result<PeerRetirement, ConnectError> {
        let value = self
            .request_owned(
                serde_json::json!({
                    "operation": if close { "close_peer" } else { "observe_peer" },
                }),
                true,
            )
            .await
            .map_err(|_| unavailable())?;
        retirement_reply(value, self.index, close)
    }

    pub(crate) async fn inspect(&self, ticket: &str) -> Result<InspectedSelection, ConnectError> {
        let value = self
            .request(serde_json::json!({
                "operation": "inspect_selection", "ticket": ticket,
            }))
            .await?;
        let selected: SelectionReply = serde_json::from_value(value).map_err(|_| unavailable())?;
        // This digest names the Python owner's exact encoding. The inherited
        // response binds it to the matching command; Rust validates the typed
        // selection independently instead of inventing a cross-language codec.
        if !digest_text(&selected.selection_sha256) {
            return Err(unavailable());
        }
        Ok(InspectedSelection {
            ticket: ticket.into(),
            sha256: selected.selection_sha256,
            selection: selected.selection,
        })
    }

    pub(crate) async fn release(
        &self,
        selected: &InspectedSelection,
        preparation: &str,
    ) -> Result<serde_json::Value, ConnectError> {
        self.request(serde_json::json!({"operation": "release_selection",
            "ticket": selected.ticket, "selection_sha256": selected.sha256,
            "preparation": preparation}))
            .await
    }

    pub(super) async fn prepare_entry(
        &self,
        connection: &str,
        tool: &str,
        arguments: &serde_json::Value,
        preparation: Option<&str>,
        native_request: &serde_json::Value,
    ) -> Result<String, RequestFailure> {
        let mut request = serde_json::json!({"operation":"prepare_entry", "peer":self.index,
            "connection":connection, "tool":tool, "arguments":arguments,
            "preparation":preparation, "native_request":native_request});
        if let Some(physical) = &self.physical {
            // Write before the private await or token publication. An interrupted
            // intent is retained and prevents a cold nonentry classification.
            let pin = physical
                .intent(self.index, &request)
                .map_err(|_| RequestFailure::NotSent)?;
            request["physical_intent"] = pin;
        }
        let value = self.request_owned(request, false).await?;
        if value.as_object().is_none_or(|object| object.len() != 1) {
            return Err(RequestFailure::Unknown);
        }
        let entry = value["entry"].as_str().ok_or(RequestFailure::Unknown)?;
        if !digest_text(entry) {
            return Err(RequestFailure::Unknown);
        }
        Ok(entry.into())
    }
    pub(super) async fn close_entry(&self, entry: &str) -> Result<(), ConnectError> {
        let value = self
            .request(serde_json::json!({"operation":"close_entry", "entry":entry}))
            .await?;
        if value.as_object().is_none_or(|object| object.len() != 1)
            || !matches!(value["decision"].as_str(), Some("entered" | "not_entered"))
        {
            return Err(unavailable());
        }
        if let Some(physical) = &self.physical {
            physical.observed(entry, value["decision"].as_str().ok_or_else(unavailable)?)?;
        }
        Ok(())
    }
    pub(super) async fn finish_entry(&self, entry: &str) -> Result<(), ConnectError> {
        let value = self
            .request(serde_json::json!({"operation":"finish_entry", "entry":entry}))
            .await?;
        if value.as_object().is_none_or(|object| object.len() != 1)
            || !matches!(value["decision"].as_str(), Some("entered" | "not_entered"))
        {
            return Err(unavailable());
        }
        Ok(())
    }

    async fn request(&self, command: serde_json::Value) -> Result<serde_json::Value, ConnectError> {
        let cleanup = matches!(
            command["operation"].as_str(),
            Some("close_entry" | "finish_entry")
        );
        self.request_owned(command, cleanup)
            .await
            .map_err(|_| unavailable())
    }

    async fn request_owned(
        &self,
        mut command: serde_json::Value,
        cleanup: bool,
    ) -> Result<serde_json::Value, RequestFailure> {
        command["peer"] = serde_json::json!(self.index);
        if matches!(
            command["operation"].as_str(),
            Some("observe_completion" | "leave_preparation" | "observe_closed_selection")
        ) && let Some(acquisition) = &self.acquisition
        {
            command["setup_acquisition"] = acquisition.as_str().into();
        }
        let channel = self.channel.clone();
        let deadline = tokio::time::Instant::now() + REQUEST_BUDGET;
        let (reply, result) = tokio::sync::oneshot::channel();
        // The existing resource owner retains the command, not the native
        // caller's future. Dropping that caller only drops this receiver; the
        // exact response is still drained before another command can enter.
        self.entries
            .spawn_command(
                async move {
                    let observed = exchange(&channel, command, deadline).await;
                    let unknown = matches!(observed, Err(RequestFailure::Unknown));
                    let _ = reply.send(observed);
                    if unknown { Err(unavailable()) } else { Ok(()) }
                },
                cleanup,
            )
            .map_err(|_| RequestFailure::NotSent)?;
        result.await.map_err(|_| RequestFailure::Unknown)?
    }
}

/// An exact joined predecessor observation. Only the existing task owner is
/// retained, so this attachment cannot dispatch against the successor peer.
#[cfg(feature = "console")]
struct SettledPreparation {
    entries: Arc<super::entry::EntryTasks>,
    preparation: String,
    selection: meerkat_toolkit::setup::SharedPreparationSelection,
    completion: meerkat_toolkit::setup::SharedPreparationCompletion,
    job: serde_json::Value,
}
#[cfg(feature = "console")]
#[async_trait::async_trait]
impl toolkit_mobkit::private_originals::SharedPreparationResource for SettledPreparation {
    async fn completion(
        &self,
        preparation: &str,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
    ) -> Result<
        Option<meerkat_toolkit::setup::SharedPreparationCompletion>,
        toolkit_mobkit::private_originals::PrivateOriginalError,
    > {
        if preparation != self.preparation || selection != &self.selection {
            return Err(toolkit_mobkit::private_originals::PrivateOriginalError::Conflict);
        }
        Ok(Some(self.completion.clone()))
    }
    async fn retain_leave(
        &self,
        leave: toolkit_mobkit::private_originals::SharedPreparationLeave,
    ) -> Result<serde_json::Value, toolkit_mobkit::private_originals::PrivateOriginalError> {
        retain_preparation_leave(&self.entries, leave).await
    }
    async fn leave(
        &self,
        preparation: &str,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
        last: bool,
    ) -> Result<serde_json::Value, toolkit_mobkit::private_originals::PrivateOriginalError> {
        if preparation != self.preparation || selection != &self.selection {
            return Err(toolkit_mobkit::private_originals::PrivateOriginalError::Conflict);
        }
        Ok(
            serde_json::json!({"scope":"host_shared_member_left", "last_member":last,
            "physical_stop_requested":false, "worker_joined":true, "job":self.job}),
        )
    }
}

#[async_trait::async_trait]
impl toolkit_mobkit::private_originals::SharedPreparationResource for SharedSetupPeer {
    async fn completion(
        &self,
        preparation: &str,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
    ) -> Result<
        Option<meerkat_toolkit::setup::SharedPreparationCompletion>,
        toolkit_mobkit::private_originals::PrivateOriginalError,
    > {
        use toolkit_mobkit::private_originals::PrivateOriginalError;
        let value = self.request(serde_json::json!({"operation":"observe_completion",
            "preparation":preparation,
            "validation_id":selection.validation_id().map_err(|_| PrivateOriginalError::Conflict)?
        })).await.map_err(|_| PrivateOriginalError::Unavailable)?;
        if value
            .as_object()
            .is_none_or(|value| value.len() != 1 || !value.contains_key("completion"))
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        if value["completion"].is_null() {
            return Ok(None);
        }
        meerkat_toolkit::setup::SharedPreparationCompletion::from_controller(
            value["completion"].clone(),
        )
        .map(Some)
        .map_err(|_| PrivateOriginalError::Conflict)
    }

    async fn retain_leave(
        &self,
        leave: toolkit_mobkit::private_originals::SharedPreparationLeave,
    ) -> Result<serde_json::Value, toolkit_mobkit::private_originals::PrivateOriginalError> {
        retain_preparation_leave(&self.entries, leave).await
    }

    async fn leave(
        &self,
        preparation: &str,
        selection: &meerkat_toolkit::setup::SharedPreparationSelection,
        last: bool,
    ) -> Result<serde_json::Value, toolkit_mobkit::private_originals::PrivateOriginalError> {
        use toolkit_mobkit::private_originals::PrivateOriginalError;
        let validation = selection
            .validation_id()
            .map_err(|_| PrivateOriginalError::Conflict)?
            .to_owned();
        let preparation = preparation.to_owned();
        // This is a retained command handle for the same peer, never another
        // native connection or public attachment. A dropped cancellation or
        // setup_stop caller cannot discard a last-member stop/join obligation.
        let owned = Self {
            channel: self.channel.clone(),
            index: self.index,
            acquisition: self.acquisition.clone(),
            physical: self.physical.clone(),
            configuration: self.configuration.clone(),
            entries: self.entries.clone(),
            #[cfg(feature = "console")]
            attempt: self.attempt.clone(),
        };
        let (reply, result) = tokio::sync::oneshot::channel();
        self.entries
            .spawn_command(
                async move {
                    let observed = owned.leave_owned(&preparation, &validation, last).await;
                    let failed = observed.is_err();
                    let _ = reply.send(observed);
                    if failed { Err(unavailable()) } else { Ok(()) }
                },
                true,
            )
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        result
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?
    }
}

async fn retain_preparation_leave(
    entries: &super::entry::EntryTasks,
    leave: toolkit_mobkit::private_originals::SharedPreparationLeave,
) -> Result<serde_json::Value, toolkit_mobkit::private_originals::PrivateOriginalError> {
    use toolkit_mobkit::private_originals::PrivateOriginalError;
    let (reply, result) = tokio::sync::oneshot::channel();
    entries
        .spawn_command(
            async move {
                let observed = leave.settle().await;
                let failed = observed.is_err();
                let _ = reply.send(observed);
                if failed { Err(unavailable()) } else { Ok(()) }
            },
            true,
        )
        .map_err(|_| PrivateOriginalError::Unavailable)?;
    result
        .await
        .map_err(|_| PrivateOriginalError::Unavailable)?
}

impl SharedSetupPeer {
    async fn leave_owned(
        &self,
        preparation: &str,
        validation: &str,
        last: bool,
    ) -> Result<serde_json::Value, toolkit_mobkit::private_originals::PrivateOriginalError> {
        use toolkit_mobkit::private_originals::PrivateOriginalError;
        let deadline = tokio::time::Instant::now() + REQUEST_BUDGET;
        loop {
            let value = tokio::time::timeout_at(
                deadline,
                self.request_owned(
                    serde_json::json!({
                        "operation": "leave_preparation", "validation_id": validation,
                        "preparation": preparation, "last": last,
                    }),
                    true,
                ),
            )
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .map_err(|_| PrivateOriginalError::Unavailable)?;
            if value.as_object().is_none_or(|object| object.len() != 5)
                || value["scope"] != "host_shared_member_left"
                || value["last_member"] != last
                || !value["physical_stop_requested"].is_boolean()
                || !value["worker_joined"].is_boolean()
                || !(value["job"].is_null() || value["job"].is_object())
                || (!last && value["physical_stop_requested"] != false)
            {
                return Err(PrivateOriginalError::Unavailable);
            }
            if !last || value["worker_joined"] == true {
                return Ok(value);
            }
            // A retained successful pending response only permits another
            // observation of the same accepted stop, not an effect retry.
            tokio::time::timeout_at(
                deadline,
                tokio::time::sleep(std::time::Duration::from_millis(50)),
            )
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        }
    }
}

/// Only the preparation caller interprets a retained Refused response as no
/// token issued. Other command failures do not prove their effect outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestFailure {
    NotSent,
    Refused,
    Unknown,
}

async fn exchange(
    channel: &tokio::sync::Mutex<Channel>,
    command: serde_json::Value,
    deadline: tokio::time::Instant,
) -> Result<serde_json::Value, RequestFailure> {
    let mut channel = tokio::time::timeout_at(deadline, channel.lock())
        .await
        .map_err(|_| RequestFailure::NotSent)?;
    if channel.input.is_none() || channel.pending {
        return Err(RequestFailure::NotSent);
    }
    let id = channel
        .sequence
        .checked_add(1)
        .ok_or(RequestFailure::NotSent)?;
    let mut bytes = serde_json::to_vec(&serde_json::json!({"id": id, "command": command}))
        .map_err(|_| RequestFailure::NotSent)?;
    bytes.push(b'\n');
    if bytes.len() > 256 * 1024 {
        return Err(RequestFailure::NotSent);
    }
    channel.sequence = id;
    channel.pending = true;
    let pending = PendingRequest(&mut channel);
    let received = tokio::time::timeout_at(deadline, async {
        let input = pending.0.input.as_mut().ok_or_else(unavailable)?;
        input.write_all(&bytes).await.map_err(|_| unavailable())?;
        input.flush().await.map_err(|_| unavailable())?;
        read_response(&mut pending.0.output, id).await
    })
    .await
    .map_err(|_| RequestFailure::Unknown)?
    .map_err(|_| RequestFailure::Unknown)?;
    pending.0.pending = false;
    match received {
        Reply::Value(value) => Ok(value),
        Reply::Refused => Err(RequestFailure::Refused),
    }
}

// Only resource-owner failure/timeout can now drop a partially settled
// command. Close that failed transport so a later command cannot consume its
// response. Native caller cancellation does not drop this owned exchange.
struct PendingRequest<'a>(&'a mut Channel);
impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        if self.0.pending {
            self.0.input.take();
        }
    }
}

pub(crate) struct InspectedSelection {
    ticket: String,
    pub(crate) sha256: String,
    pub(crate) selection: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionReply {
    selection_sha256: String,
    selection: serde_json::Value,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    id: u64,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}
enum Reply {
    Value(serde_json::Value),
    Refused,
}

fn digest_text(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

async fn read_response(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    expected: u64,
) -> Result<Reply, ConnectError> {
    let mut bytes = Vec::new();
    for _ in 0..RESPONSE_LIMIT {
        let byte = reader.read_u8().await.map_err(|_| unavailable())?;
        bytes.push(byte);
        if byte != b'\n' {
            continue;
        }
        let response: Response = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if response.id != expected {
            return Err(unavailable());
        }
        return match (response.result, response.error) {
            (Some(value), None) => Ok(Reply::Value(value)),
            (None, Some(error)) if error == "shared_selection_unavailable" => Ok(Reply::Refused),
            _ => Err(unavailable()),
        };
    }
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_retirement_reply_requires_exact_peer_scope_and_state() {
        use serde_json::json;
        let reply = json!({"scope":"host_shared_peer_retirement", "peer":0, "state":"joined"});
        assert_eq!(
            retirement_reply(reply.clone(), 0, true).unwrap(),
            PeerRetirement::Joined
        );
        for (name, state) in [
            ("not_requested", PeerRetirement::NotRequested),
            ("pending", PeerRetirement::Pending),
            ("failed", PeerRetirement::Failed),
        ] {
            let mut value = reply.clone();
            value["state"] = json!(name);
            assert_eq!(retirement_reply(value.clone(), 0, false).unwrap(), state);
            if state == PeerRetirement::NotRequested {
                assert!(retirement_reply(value, 0, true).is_err());
            }
        }
        for invalid in [
            json!({"scope":"host_shared_peer_retirement", "peer":1, "state":"joined"}),
            json!({"scope":"physical_worker", "peer":0, "state":"joined"}),
            json!({"scope":"host_shared_peer_retirement", "peer":0, "state":"completed"}),
            json!({"scope":"host_shared_peer_retirement", "peer":0, "state":"joined", "extra":true}),
            json!({"scope":"host_shared_peer_retirement", "peer":true, "state":"joined"}),
            json!({"scope":"host_shared_peer_retirement", "peer":-1, "state":"joined"}),
            json!({"peer":0, "state":"joined"}),
            json!({"state":"joined"}),
            json!(null),
        ] {
            assert!(retirement_reply(invalid, 0, false).is_err());
        }
    }

    #[tokio::test]
    async fn cancelled_peer_close_receiver_keeps_drain_reply_and_b_readback_correlated() {
        use futures_util::FutureExt;
        use std::process::Stdio;
        use tokio::io::AsyncBufReadExt;
        let directory = tempfile::tempdir().unwrap();
        // Synthetic inherited channel only. The Python socket/worker control
        // separately exercises the real per-peer close task and B survival.
        let script = r#"
IFS= read -r first || exit 1
case "$first" in *'"operation":"close_peer"'*'"peer":0'*) ;; *) exit 2;; esac
printf 'closing-a\n' >&2
attempt=0
while [ ! -f "$1/release" ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 1000 ] || exit 3
    sleep 0.01
done
printf '{"id":1,"result":{"scope":"host_shared_peer_retirement","peer":0,"state":"pending"}}\n'
IFS= read -r second || exit 4
case "$second" in *'"operation":"observe_peer"'*'"peer":1'*) ;; *) exit 5;; esac
printf '{"id":2,"result":{"scope":"host_shared_peer_retirement","peer":1,"state":"not_requested"}}\n'
IFS= read -r third || exit 6
case "$third" in *'"operation":"observe_peer"'*'"peer":0'*) ;; *) exit 7;; esac
printf '{"id":3,"result":{"scope":"host_shared_peer_retirement","peer":0,"state":"joined"}}\n'
if IFS= read -r unexpected; then exit 8; fi
"#;
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", script, "peer-retirement-channel"])
            .arg(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut progress = BufReader::new(child.stderr.take().unwrap());
        let channel = Arc::new(tokio::sync::Mutex::new(Channel::new(
            child.stdin.take().unwrap(),
            BufReader::new(child.stdout.take().unwrap()),
        )));
        let entries = Arc::new(super::super::entry::EntryTasks::default());
        let peer = |index| {
            Arc::new(SharedSetupPeer::new(
                channel.clone(),
                index,
                meerkat_core::McpServerConfig::stdio(
                    "toolkit-setup",
                    "not-executed",
                    vec![],
                    Default::default(),
                ),
                entries.clone(),
            ))
        };
        let a = peer(0);
        let b = peer(1);
        let (mut waiting_a, mut waiting_b) = (None, None);
        let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
            std::time::Duration::from_secs(5),
            async {
                let close_a = a.clone();
                waiting_a = Some(tokio::spawn(async move { close_a.close_peer().await }));
                let mut line = String::new();
                progress.read_line(&mut line).await.unwrap();
                assert_eq!(
                    line, "closing-a\n",
                    "exact A close emitted before caller loss"
                );
                waiting_a.as_ref().unwrap().abort();
                assert!(waiting_a.take().unwrap().await.unwrap_err().is_cancelled());
                assert!(
                    channel.try_lock().is_err(),
                    "existing owner still drains A reply"
                );
                waiting_b = Some(tokio::spawn(async move { b.observe_peer().await }));
                assert!(!waiting_b.as_ref().unwrap().is_finished());
                std::fs::write(directory.path().join("release"), b"release").unwrap();
                assert_eq!(
                    waiting_b.as_mut().unwrap().await.unwrap().unwrap(),
                    PeerRetirement::NotRequested
                );
                waiting_b.take();
                assert_eq!(a.observe_peer().await.unwrap(), PeerRetirement::Joined);
                let selected = channel.lock().await;
                assert_eq!(selected.sequence, 3);
                assert!(selected.input.is_some() && !selected.pending);
            },
        ))
        .catch_unwind()
        .await;
        // Caller panic/timeout still drains every retained exchange, closes
        // the inherited channel and joins the one bounded child owner.
        std::fs::write(directory.path().join("release"), b"release").unwrap();
        let joined = entries.close().await;
        channel.lock().await.input.take();
        for caller in [waiting_a, waiting_b].into_iter().flatten() {
            let _ = caller.await;
        }
        let exit = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
        assert!(joined.is_ok(), "{joined:?}");
        assert!(exit.unwrap().unwrap().success());
        match checked {
            Ok(Ok(())) => (),
            Ok(Err(_)) => panic!("peer retirement exceeded bounded transport control"),
            Err(error) => std::panic::resume_unwind(error),
        }
    }

    #[tokio::test]
    async fn cancelled_peer_receiver_keeps_owned_command_and_next_peer_reply() {
        use futures_util::FutureExt;
        use std::process::Stdio;
        use tokio::io::AsyncBufReadExt;
        let directory = tempfile::tempdir().unwrap();
        // Synthetic inherited transport only. Native authority and the real
        // Controller remain separate acceptance requirements.
        let script = r#"
IFS= read -r first || exit 1
case "$first" in *'"peer":0'*) ;; *) exit 2;; esac
printf 'first\n' >&2
attempt=0
while [ ! -f "$1/release" ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 1000 ] || exit 3
    sleep 0.01
done
printf '{"id":1,"result":{"peer":0}}\n'
IFS= read -r second || exit 4
case "$second" in *'"peer":1'*) ;; *) exit 5;; esac
printf '{"id":2,"result":{"peer":1}}\n'
if IFS= read -r unexpected; then exit 6; fi
"#;
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", script, "owned-channel"])
            .arg(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut progress = BufReader::new(child.stderr.take().unwrap());
        let channel = Arc::new(tokio::sync::Mutex::new(Channel::new(
            child.stdin.take().unwrap(),
            BufReader::new(child.stdout.take().unwrap()),
        )));
        let entries = Arc::new(super::super::entry::EntryTasks::default());
        let peer = |index| {
            Arc::new(SharedSetupPeer::new(
                channel.clone(),
                index,
                meerkat_core::McpServerConfig::stdio(
                    "toolkit-setup",
                    "not-executed",
                    vec![],
                    Default::default(),
                ),
                entries.clone(),
            ))
        };
        let a = peer(0);
        let b = peer(1);
        let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
            std::time::Duration::from_secs(5),
            async {
                // Definite local rejection sends no frame or sequence number.
                assert!(matches!(
                    a.request_owned(
                        serde_json::json!({
                            "operation": "probe", "oversized": "x".repeat(256 * 1024),
                        }),
                        false
                    )
                    .await,
                    Err(RequestFailure::NotSent)
                ));
                assert_eq!(channel.lock().await.sequence, 0);
                let waiting_a = tokio::spawn(async move {
                    a.request(serde_json::json!({"operation": "probe"})).await
                });
                let mut line = String::new();
                progress.read_line(&mut line).await.unwrap();
                assert_eq!(line, "first\n", "A's exact frame reached the owner");
                waiting_a.abort();
                assert!(waiting_a.await.unwrap_err().is_cancelled());
                // The resource owner still owns A's reply and the shared pipe.
                assert!(channel.try_lock().is_err());
                let waiting_b = tokio::spawn(async move {
                    b.request(serde_json::json!({"operation": "probe"})).await
                });
                assert!(!waiting_b.is_finished());
                std::fs::write(directory.path().join("release"), b"release").unwrap();
                assert_eq!(
                    waiting_b.await.unwrap().unwrap(),
                    serde_json::json!({"peer": 1})
                );
                let selected = channel.lock().await;
                assert_eq!(selected.sequence, 2);
                assert!(selected.input.is_some() && !selected.pending);
            },
        ))
        .catch_unwind()
        .await;
        // Always release the finite synthetic owner, then join every Rust
        // request and close its exact inherited pipe before child join.
        std::fs::write(directory.path().join("release"), b"release").unwrap();
        let joined = entries.close().await;
        channel.lock().await.input.take();
        let exit = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
        assert!(joined.is_ok(), "{joined:?}");
        assert!(exit.unwrap().unwrap().success());
        match checked {
            Ok(Ok(())) => (),
            Ok(Err(_)) => panic!("owned private command exceeded bounded transport control"),
            Err(error) => std::panic::resume_unwind(error),
        }
    }

    #[tokio::test]
    async fn owner_response_requires_exact_id_and_one_bounded_result() {
        let mut good = b"{\"id\":1,\"result\":{\"selected\":true}}\n".as_slice();
        assert!(matches!(
            read_response(&mut good, 1).await,
            Ok(Reply::Value(_))
        ));
        for bytes in [
            b"{\"id\":2,\"result\":{}}\n".as_slice(),
            b"{\"id\":1,\"id\":1,\"result\":{}}\n",
            b"{\"id\":true,\"result\":{}}\n",
            b"{\"id\":1,\"result\":{},\"error\":\"shared_selection_unavailable\"}\n",
            b"{\"id\":1,\"error\":\"unrelated\"}\n",
        ] {
            assert!(read_response(&mut &bytes[..], 1).await.is_err());
        }
        let bytes = vec![b'x'; RESPONSE_LIMIT];
        assert!(read_response(&mut bytes.as_slice(), 1).await.is_err());
    }
}
