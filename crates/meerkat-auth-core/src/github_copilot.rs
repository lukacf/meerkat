//! Target-neutral GitHub Copilot identity and endpoint declaration.

pub const GITHUB_COPILOT_CLIENT_ID: &str = concat!("Iv1.", "b507a08c87ecfe98");
pub const GITHUB_COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.35.0";
pub const GITHUB_COPILOT_AUTHORIZE_URL: &str = "https://github.com/login/device";
pub const GITHUB_COPILOT_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
pub const GITHUB_COPILOT_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
pub const GITHUB_COPILOT_TOKEN_EXCHANGE_URL: &str =
    "https://api.github.com/copilot_internal/v2/token";
/// Copilot identity of a GitHub token: login, plan and the per-plan Copilot
/// API host. The operator-token mode reads it with the token itself as the
/// bearer.
pub const GITHUB_COPILOT_USER_URL: &str = "https://api.github.com/copilot_internal/user";
pub const GITHUB_COPILOT_SCOPES: &[&str] = &["read:user"];
