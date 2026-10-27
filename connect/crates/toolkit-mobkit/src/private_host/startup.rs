//! Startup phases on the existing inherited private connection.
//!
//! This owns the only command reader throughout preparation and adoption. It
//! carries artifact references, never a replacement native finish or readiness
//! observation. The executable's actual held driver owns those operations.

use super::*;

/// Selected by the trusted replacement-process launcher from its existing
/// update transaction. Paths and hashes do not authenticate that transaction;
/// association_directory is output custody, never authority or completion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationUpdateStartup {
    pub selection: SelectedHostArtifact,
    pub prior_tool_policy: SelectedHostArtifact,
    pub association_directory: std::path::PathBuf,
}

#[derive(Clone, Debug)]
pub enum PrivateHostStartupIntent {
    Installed(HostBootstrapSelection),
    PrepareApplication(SelectedHostArtifact),
    ReplaceApplication(ApplicationUpdateStartup),
}

/// The pending first request, or the final Bootstrap on that same connection.
/// Until adoption no ordinary host command can be consumed by another reader.
pub struct PrivateHostStartup {
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
    frame: Vec<u8>,
    request_id: u64,
    intent: PrivateHostStartupIntent,
    lost: bool,
}

impl PrivateHostStartup {
    pub async fn read(pipes: InheritedHostPipes) -> Result<Self, PrivateHostError> {
        let (commands, writer) = pipes.into_async()?;
        let mut reader = BufReader::new(commands);
        let mut frame = Vec::new();
        let request = read_request(&mut reader, &mut frame).await?
            .ok_or(PrivateHostError::Protocol)?;
        if request.id != 1 { return Err(PrivateHostError::Protocol); }
        let intent = match request.command {
            Command::Bootstrap { selection } => PrivateHostStartupIntent::Installed(selection),
            Command::PrepareApplication { plan } => PrivateHostStartupIntent::PrepareApplication(plan),
            Command::ReplaceApplication { startup } => PrivateHostStartupIntent::ReplaceApplication(startup),
            _ => return Err(PrivateHostError::Protocol),
        };
        Ok(Self { reader, writer, frame, request_id: 1, intent, lost: false })
    }

    pub fn intent(&self) -> &PrivateHostStartupIntent { &self.intent }

    /// The trusted composition must validate the update and supply its original
    /// predecessor before opening any owner. This consumes request 1 without
    /// another reader, command, preparation result or activation publication.
    pub fn replace_with_original(mut self, predecessor: HostBootstrapSelection) -> Result<Self, PrivateHostError> {
        if self.lost || !self.frame.is_empty() || self.request_id != 1
            || !matches!(self.intent, PrivateHostStartupIntent::ReplaceApplication(_))
        { return Err(PrivateHostError::Protocol); }
        self.intent = PrivateHostStartupIntent::Installed(predecessor);
        Ok(self)
    }

    /// Observe loss while the actual driver is working. Cancelling this wait
    /// retains any partial frame in this same owner. Any early command is a
    /// protocol failure; it cannot publish an activation before a result exists.
    pub async fn wait_lost(&mut self) -> PrivateHostError {
        let result = read_request(&mut self.reader, &mut self.frame).await;
        self.lost = true;
        match result {
            Err(error) => error,
            Ok(_) => PrivateHostError::Protocol,
        }
    }

    /// Publish only an actual complete_bundle result. The held completion must
    /// remain owned by the caller until the returned final selection is adopted.
    pub async fn application_prepared(
        mut self,
        manifest: SelectedHostArtifact,
    ) -> Result<Self, PrivateHostError> {
        if self.lost || self.request_id != 1
            || !matches!(self.intent, PrivateHostStartupIntent::PrepareApplication(_))
            || !self.frame.is_empty()
        { return Err(PrivateHostError::Protocol); }
        write_response(&mut self.writer, &response(1, Ok(Reply::ApplicationPrepared { manifest }))).await?;
        let request = read_request(&mut self.reader, &mut self.frame).await?
            .ok_or(PrivateHostError::Protocol)?;
        if request.id != 2 { return Err(PrivateHostError::Protocol); }
        let Command::Bootstrap { selection } = request.command else {
            return Err(PrivateHostError::Protocol);
        };
        self.request_id = 2;
        self.intent = PrivateHostStartupIntent::Installed(selection);
        Ok(self)
    }

    /// A failed compiler/owner operation keeps its actual error and cleanup
    /// with the driver. This response does not assert native rollback or Ready.
    pub async fn reject(mut self, error: HostLifetimeError) -> Result<(), PrivateHostError> {
        write_response(&mut self.writer, &response(self.request_id, Err(error))).await
    }

    pub(super) fn into_installed(self) -> Result<(
        BufReader<Box<dyn AsyncRead + Unpin + Send>>,
        Box<dyn AsyncWrite + Unpin + Send>,
        u64,
        HostBootstrapSelection,
    ), PrivateHostError> {
        let PrivateHostStartupIntent::Installed(selection) = self.intent else {
            return Err(PrivateHostError::Protocol);
        };
        if self.lost || !self.frame.is_empty() { return Err(PrivateHostError::Protocol); }
        Ok((self.reader, self.writer, self.request_id, selection))
    }
}
