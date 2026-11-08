use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(super) const MAX_FRAME: usize = 1024 * 1024;
pub(super) const PROTOCOL: &str = "connect-provider-setup/1";
pub(super) const NATIVE_ENVIRONMENT: &[&str] = &["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"];
pub(super) const ENVIRONMENT: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SYSTEMROOT",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Initialize {
    pub protocol: String,
    pub application_id: String,
    pub instance_id: String,
    pub principal_id: String,
    pub login_scope_id: String,
    pub expires_at_ms: u64,
    pub realm_id: String,
    pub context_root: PathBuf,
    pub state_root: PathBuf,
    pub user_config_root: PathBuf,
    pub credential_scope: super::CredentialScope,
    pub browser: BrowserTransport,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum BrowserTransport {
    LocalLoopback,
    ManualReturn,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Initialize {
        id: String,
        binding: Initialize,
    },
    Answer {
        id: String,
        prompt_id: String,
        answer: Answer,
    },
    Cancel {
        id: String,
    },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Answer {
    Choice { choice_id: String },
    Confirm,
    Action { action: Action },
    Key { key: String },
    Callback { url: String },
    Cancel,
}
#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Use,
    EnterKey,
    SignIn,
    Back,
}

pub(super) fn opaque(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|t| u64::try_from(t.as_millis()).ok())
        .unwrap_or(u64::MAX)
}
impl Initialize {
    pub(super) fn valid(&self) -> bool {
        self.protocol == PROTOCOL
            && self.credential_scope.valid()
            && [
                &self.application_id,
                &self.instance_id,
                &self.login_scope_id,
            ]
            .iter()
            .all(|s| opaque(s))
            && meerkat_core::ApprovalPrincipalId::new(&self.principal_id).is_ok()
            && meerkat_core::runtime_bootstrap::validate_explicit_realm_id(&self.realm_id).is_ok()
            && self.expires_at_ms > now()
            && self.expires_at_ms <= 9_007_199_254_740_991
            && [&self.context_root, &self.state_root, &self.user_config_root]
                .iter()
                .all(|p| Path::is_absolute(p) && p.to_str().is_some_and(|s| !s.contains('\0')))
    }
}
pub(super) fn valid_answer(prompt: &serde_json::Value, answer: &Answer) -> bool {
    match (prompt["kind"].as_str(), answer) {
        (_, Answer::Cancel) => true,
        (Some("choices"), Answer::Choice { choice_id }) => {
            prompt["choices"].as_array().is_some_and(|xs| {
                xs.iter()
                    .any(|x| x["choice_id"] == *choice_id && x["selectable"] == true)
            })
        }
        (Some("target_review"), Answer::Confirm) => true,
        (Some("action"), Answer::Action { action }) => {
            serde_json::to_value(action).ok().is_some_and(|a| {
                prompt["actions"]
                    .as_array()
                    .is_some_and(|xs| xs.contains(&a))
            })
        }
        (Some("key"), Answer::Key { key }) => {
            !key.trim().is_empty() && key.len() <= 8192 && !key.chars().any(char::is_control)
        }
        (Some("browser_manual"), Answer::Callback { url }) => {
            !url.is_empty() && url.len() <= 16384 && !url.chars().any(char::is_control)
        }
        _ => false,
    }
}
