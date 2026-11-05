//! [`ReadOnlyProjectTools`]: the `read_file` / `glob` / `grep` subset of the
//! file tools, confined to the project root, for the approval classifier.
//!
//! It wraps its own [`FileToolDispatcher`] (no diff channel, private
//! read-state), so classifier reads never satisfy the agent's
//! write-before-read check. Unlike the agent's tools it refuses every path
//! outside the project root, and every environment file (`.env`, `.env.*`,
//! `.envrc`, see [`is_secret_file_name`]): those are refused by path and
//! skipped by `glob`/`grep`, so secrets never reach the classifier's
//! provider. Other protected paths inside the root stay readable.

use super::{FileToolDispatcher, GLOB, GREP, READ_FILE};
use crate::paths::{ProjectRoot, is_secret_file_name};
use async_trait::async_trait;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{
    AgentToolDispatcher, ToolCallView, ToolDef, ToolError, ToolMutationClass, ToolResult,
};
use serde::Deserialize;
use std::sync::Arc;

/// Names of the tools exposed by [`ReadOnlyProjectTools`].
pub const READ_ONLY_TOOL_NAMES: &[&str] = &[READ_FILE, GLOB, GREP];

/// The `path` argument shared by the three read-only tools.
#[derive(Debug, Deserialize)]
struct PathArg {
    #[serde(default)]
    path: Option<String>,
}

/// Read-only, root-confined project tools for the classifier.
pub struct ReadOnlyProjectTools {
    inner: FileToolDispatcher,
    tools: Arc<[Arc<ToolDef>]>,
}

impl ReadOnlyProjectTools {
    /// Tools rooted at `root`, backed by a fresh [`FileToolDispatcher`].
    pub fn new(root: ProjectRoot) -> Self {
        let inner = FileToolDispatcher::hiding_secrets(root);
        let tools: Arc<[Arc<ToolDef>]> = inner
            .tools()
            .iter()
            .filter(|def| READ_ONLY_TOOL_NAMES.contains(&def.name.as_str()))
            .cloned()
            .collect();
        Self { inner, tools }
    }

    /// The project root paths are confined to.
    pub fn root(&self) -> &ProjectRoot {
        self.inner.root()
    }

    /// Why `call`'s `path` may not be used, if it may not.
    fn refusal(&self, call: ToolCallView<'_>) -> Option<String> {
        let raw = call.parse_args::<PathArg>().ok()?.path?;
        // Unresolvable paths are left to the inner tool's own error handling.
        let resolved = self.root().resolve(&raw).ok()?;
        if !resolved.inside() {
            return Some(format!(
                "refused: {raw} is outside the project root ({}); only project files may be read",
                self.root().as_path().display()
            ));
        }
        // `resolved` is canonical, so a symlink to an environment file is
        // caught by its target's name.
        let relative = resolved
            .abs
            .strip_prefix(self.root().as_path())
            .unwrap_or(&resolved.abs);
        relative
            .components()
            .any(|c| is_secret_file_name(c.as_os_str()))
            .then(|| format!("refused: {raw} is an environment file, which may hold secrets"))
    }
}

impl std::fmt::Debug for ReadOnlyProjectTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadOnlyProjectTools")
            .field("root", self.root())
            .finish_non_exhaustive()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for ReadOnlyProjectTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_mutation_class(&self, tool_name: &str) -> ToolMutationClass {
        if READ_ONLY_TOOL_NAMES.contains(&tool_name) {
            ToolMutationClass::ReadOnly
        } else {
            ToolMutationClass::Unknown
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !READ_ONLY_TOOL_NAMES.contains(&call.name) {
            return Err(ToolError::AccessDenied {
                name: call.name.to_string(),
            });
        }
        if let Some(message) = self.refusal(call) {
            return Ok(ToolResult::new(call.id.to_string(), message, true).into());
        }
        self.inner.dispatch(call).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::value::{RawValue, to_raw_value};

    fn setup() -> (tempfile::TempDir, ReadOnlyProjectTools) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".rkat")).unwrap();
        std::fs::write(dir.path().join(".rkat/tui.toml"), "PROTECTED-CONTENT\n").unwrap();
        std::fs::write(dir.path().join("build.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join(".env"), "API_KEY=SECRET-ENV fn main\n").unwrap();
        std::fs::write(dir.path().join(".env.local"), "SECRET-ENV fn main\n").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/.envrc"), "SECRET-ENV fn main\n").unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        (dir, ReadOnlyProjectTools::new(root))
    }

    fn args(value: &serde_json::Value) -> Box<RawValue> {
        to_raw_value(value).unwrap()
    }

    async fn call(
        tools: &ReadOnlyProjectTools,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let raw = args(value);
        tools
            .dispatch(ToolCallView {
                id: "c1",
                name,
                args: &raw,
            })
            .await
            .map(|outcome| outcome.result)
    }

    #[test]
    fn lists_exactly_three_tools() {
        let (_dir, tools) = setup();
        let names: Vec<String> = tools.tools().iter().map(|d| d.name.to_string()).collect();
        assert_eq!(names, vec!["read_file", "glob", "grep"]);
    }

    #[tokio::test]
    async fn write_file_is_refused() {
        let (dir, tools) = setup();
        let err = call(
            &tools,
            "write_file",
            &serde_json::json!({"path": "x.txt", "content": "hi"}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::AccessDenied { .. }), "{err:?}");
        assert!(!dir.path().join("x.txt").exists());
        let err = call(&tools, "edit_file", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::AccessDenied { .. }));
    }

    #[tokio::test]
    async fn protected_path_is_readable() {
        let (_dir, tools) = setup();
        let result = call(
            &tools,
            "read_file",
            &serde_json::json!({"path": ".rkat/tui.toml"}),
        )
        .await
        .unwrap();
        assert!(!result.is_error, "{}", result.text_content());
        assert!(result.text_content().contains("PROTECTED-CONTENT"));
    }

    #[tokio::test]
    async fn outside_root_is_refused() {
        let (dir, tools) = setup();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "SECRET").unwrap();
        let path = outside.path().join("secret");
        for value in [
            serde_json::json!({"path": path.to_str().unwrap()}),
            serde_json::json!({"path": "../secret"}),
        ] {
            let result = call(&tools, "read_file", &value).await.unwrap();
            assert!(result.is_error);
            assert!(result.text_content().contains("outside the project root"));
            assert!(!result.text_content().contains("SECRET"));
        }
        for name in ["glob", "grep"] {
            let result = call(
                &tools,
                name,
                &serde_json::json!({"pattern": "*", "path": outside.path().to_str().unwrap()}),
            )
            .await
            .unwrap();
            assert!(result.is_error, "{name}");
        }
        drop(dir);
    }

    #[tokio::test]
    async fn glob_and_grep_inside_root_work() {
        let (_dir, tools) = setup();
        let result = call(&tools, "glob", &serde_json::json!({"pattern": "*.rs"}))
            .await
            .unwrap();
        assert!(result.text_content().contains("build.rs"));
        let result = call(&tools, "grep", &serde_json::json!({"pattern": "fn main"}))
            .await
            .unwrap();
        assert!(result.text_content().contains("build.rs"));
    }

    #[tokio::test]
    async fn environment_files_are_never_read_listed_or_searched() {
        let (dir, tools) = setup();
        for path in [".env", ".env.local", "sub/.envrc", "./sub/../.env"] {
            let result = call(&tools, "read_file", &serde_json::json!({ "path": path }))
                .await
                .unwrap();
            assert!(result.is_error, "{path}");
            assert!(result.text_content().contains("environment file"), "{path}");
            assert!(!result.text_content().contains("SECRET-ENV"), "{path}");
        }
        // A symlink to an environment file is refused by its target.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join(".env"), dir.path().join("notes.txt"))
                .unwrap();
            let result = call(
                &tools,
                "read_file",
                &serde_json::json!({"path": "notes.txt"}),
            )
            .await
            .unwrap();
            assert!(result.is_error);
            assert!(!result.text_content().contains("SECRET-ENV"));
        }
        // No output line names an environment file or shows its content
        // (the "no match" messages echo the pattern, so check paths).
        let leaks = |text: &str| {
            text.lines().any(|l| {
                l.starts_with(".env") || l.starts_with("sub/.envrc") || l.contains("API_KEY=")
            })
        };
        let result = call(&tools, "glob", &serde_json::json!({"pattern": "**/.env*"}))
            .await
            .unwrap();
        assert!(!leaks(&result.text_content()), "{}", result.text_content());
        for (pattern, mode) in [
            ("SECRET-ENV", "content"),
            ("SECRET-ENV", "files_with_matches"),
            ("SECRET-ENV", "count"),
            ("fn main", "content"),
        ] {
            let result = call(
                &tools,
                "grep",
                &serde_json::json!({"pattern": pattern, "output_mode": mode}),
            )
            .await
            .unwrap();
            let text = result.text_content();
            assert!(!leaks(&text), "{pattern} {mode}: {text}");
        }
        // Searching an environment file directly is refused too.
        let result = call(
            &tools,
            "grep",
            &serde_json::json!({"pattern": "KEY", "path": ".env"}),
        )
        .await
        .unwrap();
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn agent_tools_still_see_environment_files() {
        let (dir, _tools) = setup();
        let agent = FileToolDispatcher::new(ProjectRoot::new(dir.path()).unwrap(), None);
        let raw = args(&serde_json::json!({"pattern": "SECRET-ENV"}));
        let result = agent
            .dispatch(ToolCallView {
                id: "c1",
                name: "grep",
                args: &raw,
            })
            .await
            .unwrap()
            .result;
        assert!(result.text_content().contains(".env"));
    }
}
