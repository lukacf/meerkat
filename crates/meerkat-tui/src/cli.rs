//! Command-line arguments for `rkat-tui`.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Interactive coding harness for Meerkat.
#[derive(Parser, Debug, Clone)]
#[command(name = "rkat-tui", version)]
#[command(about = "Interactive terminal coding harness for Meerkat")]
#[command(
    after_help = "Examples:\n  rkat-tui\n  rkat-tui \"fix the failing test\"\n  rkat-tui --resume last\n  rkat-tui --model claude-sonnet-4-6 --classifier-model claude-haiku-5-5\n  rkat-tui --endpoint-def litellm=openai=http://localhost:4000,key_env=LITELLM_KEY --model litellm:gpt-5.5\n  rkat-tui --allow 'shell=cargo nextest run*' --set auto.compact=auto\n\nEvery tui.toml setting can be given with --set <dotted.key>=<value> (TOML value; bare words are strings).\nCommand-line settings apply after the global and realm tui.toml, before the project file's deny rules."
)]
pub struct Args {
    /// Optional initial prompt to send as the first turn.
    pub prompt: Option<String>,

    /// Model for the session: `<endpoint>:<id>`, or a bare id looked up in the
    /// endpoints' model lists (then inferred from its prefix). Defaults to the
    /// configured or provider default model.
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Endpoint for a bare `--model` id and the default endpoint (a built-in
    /// `anthropic`/`openai`/`gemini`, or a `[endpoints.<name>]` from tui.toml).
    #[arg(long, conflicts_with = "provider")]
    pub endpoint: Option<String>,

    /// Built-in provider endpoint (anthropic, openai, gemini); same as `--endpoint`.
    #[arg(long)]
    pub provider: Option<String>,

    /// Set any tui.toml key for this run: `--set auto.compact=auto`,
    /// `--set endpoints.litellm.base_url=http://host:4000`,
    /// `--set 'allow=[{tool="shell",arg="cargo nextest run*"}]'`. Tables merge,
    /// scalars replace, arrays append. Repeatable.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,

    /// Add an allow rule for this run: `<tool>[=<glob>]` (e.g.
    /// `shell=cargo nextest run*`). Repeatable.
    #[arg(long, value_name = "TOOL[=PATTERN]")]
    pub allow: Vec<String>,

    /// Add a deny rule for this run: `<tool>[=<glob>]`. Repeatable.
    #[arg(long, value_name = "TOOL[=PATTERN]")]
    pub deny: Vec<String>,

    /// Define an endpoint for this run: `<name>=<dialect>=<url>[,key_env=VAR]`
    /// (dialect: anthropic, openai, gemini). Repeatable.
    #[arg(long = "endpoint-def", value_name = "NAME=DIALECT=URL[,key_env=VAR]")]
    pub endpoint_def: Vec<String>,

    /// Treat `cargo build|test|nextest|check|clippy` as tier 1 (`[auto]
    /// trust_project_builds = true`).
    #[arg(long, conflicts_with = "no_trust_project_builds")]
    pub trust_project_builds: bool,

    /// Review project builds like any other command (`[auto]
    /// trust_project_builds = false`).
    #[arg(long)]
    pub no_trust_project_builds: bool,

    /// Resume an existing session: `last`, `~N`, a session ID, or an ID prefix.
    #[arg(long, conflicts_with = "continue_last")]
    pub resume: Option<String>,

    /// Continue the most recent session of this workspace (same as `--resume last`).
    #[arg(short = 'c', long = "continue")]
    pub continue_last: bool,

    /// Workspace directory: the project root for path policy and conventions
    /// (AGENTS.md, .rkat/), and where session state lives (`.rkat/tui/`).
    /// Defaults to the nearest ancestor of the current directory that already
    /// has `.rkat/tui/`, else the current directory.
    #[arg(short = 'w', long = "workspace", alias = "context-root")]
    pub context_root: Option<PathBuf>,

    /// User-global conventions root (defaults to the home directory).
    #[arg(long)]
    pub user_config_root: Option<PathBuf>,

    /// Directory for user-global rkat-tui files: AGENTS.md, SYSTEM.md, tui.toml
    /// rules, notes, history. Defaults to $XDG_CONFIG_HOME/rkat-tui
    /// (~/.config/rkat-tui); on Windows the platform config dir.
    #[arg(long)]
    pub config_dir: Option<PathBuf>,

    /// Join a shared realm by ID instead of using the workspace-local state
    /// directory (the realm model `rkat` uses).
    #[arg(long)]
    pub realm: Option<String>,

    /// Start in an isolated, throwaway realm.
    #[arg(long)]
    pub isolated: bool,

    /// Backend when creating a new realm or workspace state directory
    /// (default: sqlite). An existing realm keeps the backend it was created
    /// with; naming a different one is an error.
    #[arg(long, value_enum)]
    pub realm_backend: Option<RealmBackendArg>,

    /// Store state under this realms root instead of `<workspace>/.rkat`.
    #[arg(long)]
    pub state_root: Option<PathBuf>,

    /// OS process confinement of the shell commands the session runs:
    /// `workspace` lets them write only the workspace, the temp dir, the
    /// Cargo home and the cache dir (network allowed), `strict` additionally
    /// denies IP network access, `off` runs them unconfined. A mode this host
    /// cannot enforce (e.g. no unprivileged user namespaces) refuses to start.
    #[arg(long, value_enum, value_name = "MODE", default_value_t = ConfinementArg::Workspace)]
    pub confinement: ConfinementArg,

    /// Launch processes unconfined (same as `--confinement off`).
    #[arg(long)]
    pub no_confinement: bool,

    /// Disable auto mode: every tier-3 action is denied instead of classified.
    #[arg(long)]
    pub no_auto: bool,

    /// Model used by the auto-mode classifier (defaults per provider).
    #[arg(long)]
    pub classifier_model: Option<String>,

    /// Do not capture mouse events. The mouse is captured by default for wheel
    /// scrolling and click-to-expand tool output; most terminals still allow
    /// native text selection with Shift held. This flag restores plain selection.
    #[arg(long, conflicts_with = "mouse")]
    pub no_mouse: bool,

    /// Accepted for compatibility: mouse capture is now the default.
    #[arg(long, hide = true)]
    pub mouse: bool,

    /// Block until every configured MCP server has finished connecting before
    /// the first turn (default: connect in the background).
    #[arg(long)]
    pub wait_for_mcp: bool,

    /// Compact the context automatically once it reaches the `[auto]`
    /// `compact_auto_percent` (overrides `compact` in tui.toml).
    #[arg(long, conflicts_with = "no_auto_compact")]
    pub auto_compact: bool,

    /// Only warn when the context fills up; never compact automatically
    /// (overrides `compact` in tui.toml).
    #[arg(long)]
    pub no_auto_compact: bool,

    /// Do not download the community price list for the cost readout; use
    /// the cached copy and `[pricing]` from tui.toml only.
    #[arg(long)]
    pub no_pricing_fetch: bool,
}

impl Args {
    /// Whether the UI captures mouse events (on unless `--no-mouse`).
    #[must_use]
    pub fn mouse_capture(&self) -> bool {
        !self.no_mouse
    }

    /// The selected process confinement: `--no-confinement` wins over
    /// `--confinement`.
    #[must_use]
    pub fn effective_confinement(&self) -> ConfinementArg {
        if self.no_confinement {
            ConfinementArg::Off
        } else {
            self.confinement
        }
    }
}

/// `--confinement`: how the session confines the processes it launches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ConfinementArg {
    /// Confine each launch to the workspace directory; network allowed.
    Workspace,
    /// Confine each launch to the workspace directory; no IP network access.
    Strict,
    /// Launch as the trusted host, with no OS confinement.
    Off,
}

/// Session store backend for a newly created realm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RealmBackendArg {
    /// Append-only JSONL files.
    Jsonl,
    /// SQLite database (the default for the workspace-local realm).
    Sqlite,
    /// In-process only: nothing survives the process (throwaway sessions).
    Memory,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_invocation() {
        let args = Args::try_parse_from(["rkat-tui"]).unwrap();
        assert!(args.prompt.is_none());
        assert!(!args.no_pricing_fetch);
        assert!(
            Args::try_parse_from(["rkat-tui", "--no-pricing-fetch"])
                .unwrap()
                .no_pricing_fetch
        );
        assert!(!args.no_auto);
        assert!(!args.isolated);
    }

    #[test]
    fn parses_prompt_and_flags() {
        let args = Args::try_parse_from([
            "rkat-tui",
            "--model",
            "claude-sonnet-4-6",
            "--resume",
            "last",
            "--no-auto",
            "--realm-backend",
            "sqlite",
            "fix the tests",
        ])
        .unwrap();
        assert_eq!(args.prompt.as_deref(), Some("fix the tests"));
        assert_eq!(args.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(args.resume.as_deref(), Some("last"));
        assert!(args.no_auto);
        assert_eq!(args.realm_backend, Some(RealmBackendArg::Sqlite));
    }

    #[test]
    fn continue_is_short_c_and_conflicts_with_resume() {
        let args = Args::try_parse_from(["rkat-tui", "-c"]).unwrap();
        assert!(args.continue_last);
        assert!(Args::try_parse_from(["rkat-tui", "-c", "--resume", "last"]).is_err());
        let args = Args::try_parse_from(["rkat-tui", "-w", "/tmp/x"]).unwrap();
        assert_eq!(
            args.context_root.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
        let args = Args::try_parse_from(["rkat-tui", "--context-root", "/tmp/y"]).unwrap();
        assert_eq!(
            args.context_root.as_deref(),
            Some(std::path::Path::new("/tmp/y"))
        );
    }

    #[test]
    fn parses_config_dir() {
        let args = Args::try_parse_from(["rkat-tui", "--config-dir", "/tmp/cfg"]).unwrap();
        assert_eq!(
            args.config_dir.as_deref(),
            Some(std::path::Path::new("/tmp/cfg"))
        );
        let args = Args::try_parse_from(["rkat-tui"]).unwrap();
        assert!(args.config_dir.is_none());
    }

    #[test]
    fn exclusive_flag_pairs() {
        assert!(Args::try_parse_from(["rkat-tui", "--auto-compact", "--no-auto-compact"]).is_err());
        assert!(
            Args::try_parse_from([
                "rkat-tui",
                "--trust-project-builds",
                "--no-trust-project-builds"
            ])
            .is_err()
        );
        assert!(
            Args::try_parse_from(["rkat-tui", "--endpoint", "x", "--provider", "openai"]).is_err()
        );
        let args = Args::try_parse_from([
            "rkat-tui", "--set", "a=1", "--set", "b=2", "--allow", "x", "--deny", "y",
        ])
        .unwrap();
        assert_eq!(args.set, vec!["a=1", "b=2"]);
        assert_eq!(args.allow, vec!["x"]);
        assert_eq!(args.deny, vec!["y"]);
    }

    #[test]
    fn confinement_defaults_to_workspace_and_no_confinement_wins() {
        let args = Args::try_parse_from(["rkat-tui"]).unwrap();
        assert_eq!(args.effective_confinement(), ConfinementArg::Workspace);
        let args = Args::try_parse_from(["rkat-tui", "--confinement", "strict"]).unwrap();
        assert_eq!(args.effective_confinement(), ConfinementArg::Strict);
        let args = Args::try_parse_from(["rkat-tui", "--confinement", "off"]).unwrap();
        assert_eq!(args.effective_confinement(), ConfinementArg::Off);
        let args =
            Args::try_parse_from(["rkat-tui", "--confinement", "strict", "--no-confinement"])
                .unwrap();
        assert_eq!(args.effective_confinement(), ConfinementArg::Off);
        assert!(Args::try_parse_from(["rkat-tui", "--confinement", "bogus"]).is_err());
    }

    #[test]
    fn help_renders_without_terminal() {
        let err = Args::try_parse_from(["rkat-tui", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }
}
