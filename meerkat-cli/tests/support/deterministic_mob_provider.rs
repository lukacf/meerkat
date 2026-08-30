use std::path::Path;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;

pub const MODEL: &str = "deterministic-mob-stall";
pub const PROVIDER: &str = "self_hosted";

const PROVIDER_API_KEY_ENV: &[&str] = &[
    "RKAT_ANTHROPIC_API_KEY",
    "ANTHROPIC_API_KEY",
    "RKAT_OPENAI_API_KEY",
    "OPENAI_API_KEY",
    "RKAT_GEMINI_API_KEY",
    "GEMINI_API_KEY",
    "RKAT_GOOGLE_API_KEY",
    "GOOGLE_API_KEY",
    "RKAT_AZURE_OPENAI_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "RKAT_SELF_HOSTED_API_KEY",
];

pub struct DeterministicMobProvider {
    server_task: JoinHandle<()>,
}

impl Drop for DeterministicMobProvider {
    fn drop(&mut self) {
        self.server_task.abort();
    }
}

pub fn remove_ambient_provider_credentials(command: &mut Command) {
    for name in PROVIDER_API_KEY_ENV {
        command.env_remove(name);
    }
}

pub async fn install(
    state_root: &Path,
    config_realm: &str,
    mob_id: &str,
) -> (
    DeterministicMobProvider,
    tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind deterministic mob provider");
    let port = listener
        .local_addr()
        .expect("deterministic mob provider address")
        .port();
    let (request_tx, requests) = tokio::sync::mpsc::unbounded_channel::<()>();
    let server_task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let request_tx = request_tx.clone();
            connections.spawn(async move {
                let mut request_bytes = [0u8; 1024];
                if matches!(socket.read(&mut request_bytes).await, Ok(read) if read > 0) {
                    let _ = request_tx.send(());
                }
                std::future::pending::<()>().await;
                drop(socket);
            });
        }
    });

    let member_realm = meerkat_core::mob_realm_id(mob_id)
        .expect("valid fixture mob realm")
        .to_string();
    let realm_doc_dir = state_root.join(config_realm);
    tokio::fs::create_dir_all(&realm_doc_dir)
        .await
        .expect("realm config dir");
    tokio::fs::write(
        realm_doc_dir.join("config.toml"),
        format!(
            r#"[self_hosted.servers.deterministic_mob]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{MODEL}"]
server = "deterministic_mob"
remote_model = "{MODEL}"

[realm."{member_realm}"]
default_binding = "deterministic_mob"

[realm."{member_realm}".backend.deterministic_mob]
provider = "{PROVIDER}"
backend_kind = "{PROVIDER}"
server = "deterministic_mob"

[realm."{member_realm}".auth.deterministic_mob]
provider = "{PROVIDER}"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{member_realm}".binding.deterministic_mob]
backend_profile = "deterministic_mob"
auth_profile = "deterministic_mob"
"#
        ),
    )
    .await
    .expect("write deterministic mob provider config");

    (DeterministicMobProvider { server_task }, requests)
}
