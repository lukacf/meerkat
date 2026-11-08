//! Positive effect declarations bound to already verified installations.
//! Fixed compiled profiles or trusted collection programs own classification;
//! package claims cannot supply it or replace retained credential custody.
use super::*;
use meerkat_core::{McpServerConfig, mcp_config::McpTransportConfig};
use tokio::io::AsyncReadExt;
mod selected_python;

const REVISION: &str = "b98d115bbb080e9555466a96656ec4df605aea21";
const ARTIFACT: &str = "13608f06cb754be101a87ea5ebeb9ed15e632d3f709ba3b3e5866f3fc7970c70";
// get_me is context_tools.go:GetMe in REVISION, using Users.Get(ctx, "") on
// the selected client. Its availability never widens installed/native access.
pub(super) const TOOLS: [&str; 9] = [
    "list_pull_requests",
    "pull_request_read",
    "list_commits",
    "get_commit",
    "actions_list",
    "actions_get",
    "list_issues",
    "issue_read",
    "get_me",
];

/// No public constructor. Possession comes from the current checked software
/// owner's hash-bound receipt and this compiled fixed profile, never help text.
pub(crate) struct InstalledReadOnly {
    config: McpServerConfig,
    operation: String,
}
impl InstalledReadOnly {
    pub(crate) fn matches(&self, config: &McpServerConfig, operation: &str) -> bool {
        &self.config == config && self.operation == operation
    }
}
pub(crate) async fn selected(
    installed: &InstalledSelection,
    config: &McpServerConfig,
    operation: &str,
    inputs: Option<&SelectedServiceInputs>,
    program: &dyn crate::service_collection::CollectionProgram,
) -> Result<Option<InstalledReadOnly>, crate::ConnectError> {
    if installed.family == "selected-python" && installed.profile == "python" {
        return selected_python::selected(installed, config, operation, inputs, program).await;
    }
    if installed.family != "official-github" || installed.profile != "enterprise-selected-reads" {
        return Ok(None);
    }
    let refused = crate::service_probe::unavailable;
    let file = tokio::fs::File::open(&installed.receipt_path)
        .await
        .map_err(|_| refused())?;
    if !file.metadata().await.map_err(|_| refused())?.is_file() {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| refused())?;
    if bytes.len() > MAX_DOCUMENT
        || installed.receipt_sha256.as_deref()
            != Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
    {
        return Err(refused());
    }
    serde_json::from_slice::<crate::forge::UniqueJson>(&bytes).map_err(|_| refused())?;
    let receipt: Value = serde_json::from_slice(&bytes).map_err(|_| refused())?;
    declare(installed, config, operation, &receipt)
        .ok_or_else(refused)
        .map(Some)
}
fn declare(
    installed: &InstalledSelection,
    config: &McpServerConfig,
    operation: &str,
    receipt: &Value,
) -> Option<InstalledReadOnly> {
    if installed.family != "official-github"
        || installed.profile != "enterprise-selected-reads"
        || receipt["schema_version"] != 1
        || receipt["scope"] != "observed_software_installation_only"
        || receipt["family"] != installed.family
        || receipt["profile"] != installed.profile
        || receipt["component"] != "github"
        || receipt["source_revision"] != REVISION
        || receipt["artifact_sha256"] != ARTIFACT
        || receipt["target"] != "linux-x86_64"
        || receipt["configuration"]["sha256"].as_str() != installed.configuration_sha256.as_deref()
        || installed.configuration_sha256.is_none()
        || receipt["configuration"]["server_name"] != installed.server_name
        || config.connector.is_some()
        || !TOOLS.contains(&operation)
    {
        return None;
    }
    let operations = &config.access.as_ref()?.allowed_operations;
    if !operations.contains(operation) || operations.iter().any(|op| !TOOLS.contains(&op.as_str()))
    {
        return None;
    }
    let mut projected =
        public_launch_configuration(installed, &receipt["mcp_configuration"], operations).ok()?;
    let McpTransportConfig::Stdio(launch) = &mut projected.transport else {
        return None;
    };
    let destination = receipt["destination"].as_str()?;
    if !std::path::Path::new(destination).is_absolute()
        || launch.command
            != std::path::Path::new(destination)
                .join("github-mcp-server")
                .to_str()?
        || launch.cwd.as_deref() != Some(destination)
        || launch.args.len() != 7
        || launch.args[0] != "stdio"
        || launch.args[1] != "--read-only"
        || launch.args[2] != "--gh-host"
        || !enterprise_origin(&launch.args[3])
        || launch.args[4] != "--toolsets="
        || launch.args[5] != "--tools"
    {
        return None;
    }
    let tools: Vec<_> = launch.args[6].split(',').collect();
    if tools.is_empty()
        || tools
            != TOOLS
                .iter()
                .copied()
                .filter(|tool| tools.contains(tool))
                .collect::<Vec<_>>()
        || operations.iter().any(|op| !tools.contains(&op.as_str()))
    {
        return None;
    }
    let command: Vec<_> = std::iter::once(launch.command.clone())
        .chain(launch.args.iter().cloned())
        .collect();
    if receipt["command"] != json!(command) {
        return None;
    }
    launch.secret_env = ["GITHUB_PERSONAL_ACCESS_TOKEN".into()]
        .into_iter()
        .collect();
    if &projected != config {
        return None;
    }
    Some(InstalledReadOnly {
        config: config.clone(),
        operation: operation.into(),
    })
}
fn enterprise_origin(host: &str) -> bool {
    let Some(domain) = host.strip_prefix("https://") else {
        return false;
    };
    let labels: Vec<_> = domain.split('.').collect();
    domain.len() <= 253
        && labels.len() >= 2
        && !matches!(domain, "github.com" | "api.github.com" | "localhost")
        && !domain.ends_with(".localhost")
        && labels
            .last()
            .is_some_and(|label| label.as_bytes().first().is_some_and(u8::is_ascii_lowercase))
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoDeclaration;
    impl crate::service_collection::CollectionProgram for NoDeclaration {
        fn accepts_target(&self, _: &crate::service_collection::CollectionTarget<'_>) -> bool {
            false
        }
        fn start(
            &self,
            _: &crate::service_collection::CollectionTarget<'_>,
        ) -> Box<dyn crate::service_collection::CollectionCursor> {
            panic!("classification control does not start a cursor")
        }
    }
    // Contract controls use synthetic receipt bytes, never installation proof.
    fn fixture(root: &std::path::Path) -> (InstalledSelection, McpServerConfig, Value) {
        let storage = crate::RealmOptions {
            context_root: root.into(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("read-declaration")
        .unwrap();
        let destination = root.join("installed");
        let command = destination.join("github-mcp-server");
        let args = vec![
            "stdio",
            "--read-only",
            "--gh-host",
            "https://ghe.example",
            "--toolsets=",
            "--tools",
            "pull_request_read,issue_read",
        ];
        let launch = json!({"mcpServers":{"ghe":{"command":command,"args":args,"cwd":destination,"env":{}}}});
        let installed = InstalledSelection {
            realm: storage.locator,
            session: SessionId::new(),
            job_id: "job".into(),
            validation_id: "validation".into(),
            family: "official-github".into(),
            profile: "enterprise-selected-reads".into(),
            mcp_path: destination.join("ready/mcp.json"),
            receipt_path: root.join("receipt.json"),
            receipt_sha256: None,
            configuration_sha256: Some("a".repeat(64)),
            server_name: "ghe".into(),
            mcp_document_sha256: "b".repeat(64),
            recovery_request_sha256: None,
        };
        let operations = ["pull_request_read".to_owned(), "issue_read".to_owned()]
            .into_iter()
            .collect();
        let mut config = public_launch_configuration(&installed, &launch, &operations).unwrap();
        let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
            panic!("stdio");
        };
        stdio.secret_env = ["GITHUB_PERSONAL_ACCESS_TOKEN".into()]
            .into_iter()
            .collect();
        let command: Vec<_> = std::iter::once(command.to_str().unwrap())
            .chain(args.iter().copied())
            .collect();
        let receipt = json!({"schema_version":1,"scope":"observed_software_installation_only","family":installed.family,
            "profile":installed.profile,"component":"github","source_revision":REVISION,"artifact_sha256":ARTIFACT,
            "target":"linux-x86_64","configuration":{"sha256":"a".repeat(64),"server_name":"ghe"},
            "destination":destination,"command":command,"mcp_configuration":launch});
        (installed, config, receipt)
    }
    #[test]
    fn only_fixed_verified_profile_and_exact_selected_launch_can_declare_reads() {
        let root = tempfile::tempdir().unwrap();
        let (installed, config, receipt) = fixture(root.path());
        let declared = declare(&installed, &config, "pull_request_read", &receipt).unwrap();
        assert!(declared.matches(&config, "pull_request_read"));
        assert!(!declared.matches(&config, "issue_read"));
        for field in [
            "source_revision",
            "artifact_sha256",
            "target",
            "profile",
            "family",
            "scope",
        ] {
            let mut changed = receipt.clone();
            changed[field] = json!("changed");
            assert!(
                declare(&installed, &config, "pull_request_read", &changed).is_none(),
                "{field}"
            );
        }
        let mut changed = config.clone();
        changed.name = "same-tool-foreign-installation".into();
        assert!(declare(&installed, &changed, "pull_request_read", &receipt).is_none());
        assert!(declare(&installed, &config, "create_issue", &receipt).is_none());
        assert!(
            declare(&installed, &config, "get_commit", &receipt).is_none(),
            "catalog membership cannot widen selected operations"
        );
        let mut changed = receipt.clone();
        changed["configuration"]["sha256"] = json!("c".repeat(64));
        assert!(declare(&installed, &config, "pull_request_read", &changed).is_none());
    }
    #[test]
    fn host_toolsets_environment_and_operation_order_are_closed_contract_fields() {
        let root = tempfile::tempdir().unwrap();
        let (installed, config, receipt) = fixture(root.path());
        for (index, value) in [
            (1, "--read-write"),
            (3, "https://github.com"),
            (4, "--toolsets=all"),
            (6, "issue_read,pull_request_read"),
            (6, "pull_request_read,pull_request_read"),
            (6, "pull_request_read,create_issue"),
        ] {
            let mut selected = config.clone();
            let mut changed = receipt.clone();
            let McpTransportConfig::Stdio(stdio) = &mut selected.transport else {
                panic!("stdio");
            };
            stdio.args[index] = value.into();
            changed["mcp_configuration"]["mcpServers"]["ghe"]["args"][index] = json!(value);
            changed["command"][index + 1] = json!(value);
            assert!(declare(&installed, &selected, "pull_request_read", &changed).is_none());
        }
        for host in [
            "https://ghe.example/",
            "https://127.0.0.1",
            "https://ghe.localhost",
            "https://GHE.example",
            "https://ghe.example:443",
        ] {
            assert!(!enterprise_origin(host));
        }
        let mut selected = config.clone();
        let McpTransportConfig::Stdio(stdio) = &mut selected.transport else {
            panic!("stdio");
        };
        stdio.env.insert("GITHUB_TOOLSETS".into(), "all".into());
        assert!(declare(&installed, &selected, "pull_request_read", &receipt).is_none());
    }
    #[test]
    fn authenticated_identity_requires_exact_selected_receipt_and_native_access() {
        let root = tempfile::tempdir().unwrap();
        let (installed, mut config, mut receipt) = fixture(root.path());
        assert!(declare(&installed, &config, "get_me", &receipt).is_none());
        let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
            panic!("stdio");
        };
        stdio.args[6] = "get_me".into();
        receipt["mcp_configuration"]["mcpServers"]["ghe"]["args"][6] = json!("get_me");
        receipt["command"][7] = json!("get_me");
        config.access.as_mut().unwrap().allowed_operations =
            ["get_me".to_owned()].into_iter().collect();
        let declared = declare(&installed, &config, "get_me", &receipt).unwrap();
        assert!(declared.matches(&config, "get_me"));
        assert!(declare(&installed, &config, "issue_read", &receipt).is_none());
        assert!(declare(&installed, &config, "get_repository", &receipt).is_none());
        let mut widened = config.clone();
        widened
            .access
            .as_mut()
            .unwrap()
            .allowed_operations
            .insert("issue_read".into());
        assert!(declare(&installed, &widened, "get_me", &receipt).is_none());
        let mut different = receipt.clone();
        different["artifact_sha256"] = json!("0".repeat(64));
        assert!(declare(&installed, &config, "get_me", &different).is_none());
    }
    #[tokio::test]
    async fn declaration_requires_current_exact_receipt_bytes_not_profile_or_help_alone() {
        let root = tempfile::tempdir().unwrap();
        let (mut installed, config, receipt) = fixture(root.path());
        let bytes = serde_json::to_vec(&receipt).unwrap();
        tokio::fs::write(&installed.receipt_path, &bytes)
            .await
            .unwrap();
        assert!(
            selected(
                &installed,
                &config,
                "pull_request_read",
                None,
                &NoDeclaration
            )
            .await
            .is_err()
        );
        installed.receipt_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
        assert!(
            selected(
                &installed,
                &config,
                "pull_request_read",
                None,
                &NoDeclaration
            )
            .await
            .unwrap()
            .is_some()
        );
        let mut altered = receipt;
        altered["help"] = json!({"effect":"read"});
        tokio::fs::write(
            &installed.receipt_path,
            serde_json::to_vec(&altered).unwrap(),
        )
        .await
        .unwrap();
        assert!(
            selected(
                &installed,
                &config,
                "pull_request_read",
                None,
                &NoDeclaration
            )
            .await
            .is_err()
        );
        installed.profile = "selected-reads".into();
        assert!(
            selected(
                &installed,
                &config,
                "pull_request_read",
                None,
                &NoDeclaration
            )
            .await
            .unwrap()
            .is_none()
        );
    }
}
