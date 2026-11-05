//! Command-line overlay for `tui.toml`.
//!
//! Every `--set <dotted.key>=<value>` becomes a small TOML document; the
//! convenience flags (`--allow`, `--deny`, `--endpoint-def`, `--no-auto`,
//! `--classifier-model`, `--[no-]auto-compact`, `--[no-]trust-project-builds`)
//! desugar to the same documents, so there is one code path. The documents are
//! merged (tables recursively, scalars replace, arrays append) into one
//! [`RulesFile`] that [`crate::approval::rules::load_layered_with`] applies as
//! the `cli` layer: after the global and realm files, before the project
//! file's deny-only/lower-only rules.

use std::fmt;

use toml::{Table, Value};

use crate::approval::rules::{RuleOrigin, RulesFile};
use crate::cli::Args;
use crate::endpoints::{Dialect, is_valid_endpoint_name};

/// A command-line setting that cannot be applied. Fatal at start-up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{flag} {arg}: {reason}")]
pub struct OverlayError {
    /// The flag (`--set`, `--allow`, …).
    pub flag: &'static str,
    /// The offending argument as given.
    pub arg: String,
    /// What is wrong with it.
    pub reason: String,
}

impl OverlayError {
    fn new(flag: &'static str, arg: &str, reason: impl fmt::Display) -> Self {
        Self {
            flag,
            arg: arg.to_string(),
            reason: reason.to_string(),
        }
    }
}

/// The merged command-line layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConfigOverlay {
    table: Table,
}

impl ConfigOverlay {
    /// Build the overlay from every `--set` (in order) and then the
    /// convenience flags, validating each argument on its own.
    ///
    /// # Errors
    ///
    /// The first argument that has a bad path, does not fit the `tui.toml`
    /// schema, or fails validation.
    pub fn from_args(args: &Args) -> Result<Self, OverlayError> {
        let mut overlay = Self::default();
        for arg in &args.set {
            overlay.add("--set", arg, parse_set(arg)?)?;
        }
        for arg in &args.allow {
            overlay.add("--allow", arg, rule_table("allow", arg, "--allow")?)?;
        }
        for arg in &args.deny {
            overlay.add("--deny", arg, rule_table("deny", arg, "--deny")?)?;
        }
        for arg in &args.endpoint_def {
            overlay.add("--endpoint-def", arg, endpoint_def_table(arg)?)?;
        }
        let auto = |key: &str, value: Value| nested(&["auto", key], value);
        if args.no_auto {
            overlay.add("--no-auto", "", auto("enabled", Value::Boolean(false)))?;
        }
        if let Some(model) = &args.classifier_model {
            overlay.add(
                "--classifier-model",
                model,
                auto("classifier_model", Value::String(model.clone())),
            )?;
        }
        if args.auto_compact {
            overlay.add(
                "--auto-compact",
                "",
                auto("compact", Value::String("auto".into())),
            )?;
        }
        if args.no_auto_compact {
            overlay.add(
                "--no-auto-compact",
                "",
                auto("compact", Value::String("warn".into())),
            )?;
        }
        if args.trust_project_builds {
            overlay.add(
                "--trust-project-builds",
                "",
                auto("trust_project_builds", Value::Boolean(true)),
            )?;
        }
        if args.no_trust_project_builds {
            overlay.add(
                "--no-trust-project-builds",
                "",
                auto("trust_project_builds", Value::Boolean(false)),
            )?;
        }
        Ok(overlay)
    }

    /// Validate `piece` on its own, then merge it in.
    fn add(&mut self, flag: &'static str, arg: &str, piece: Table) -> Result<(), OverlayError> {
        validate(&piece).map_err(|reason| OverlayError::new(flag, arg, reason))?;
        merge(&mut self.table, piece);
        Ok(())
    }

    /// Whether no flag contributed anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// The merged document.
    #[must_use]
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// The overlay as a rules file (`None` when empty).
    ///
    /// # Errors
    ///
    /// When the merged document does not fit the schema (each piece did).
    pub fn rules_file(&self) -> Result<Option<RulesFile>, OverlayError> {
        if self.is_empty() {
            return Ok(None);
        }
        to_rules_file(&self.table)
            .map(Some)
            .map_err(|reason| OverlayError::new("--set", "(combined)", reason))
    }
}

/// Merge `overlay` into `base`: tables merge recursively, arrays append,
/// everything else replaces.
pub fn merge(base: &mut Table, overlay: Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(existing)), Value::Table(incoming)) => merge(existing, incoming),
            (Some(Value::Array(existing)), Value::Array(incoming)) => existing.extend(incoming),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Parse `--set key.path=value` into a one-entry document.
///
/// A key segment may be double-quoted to hold dots or other characters
/// (`pricing."gpt-5.4".input=1.25`).
///
/// The value is TOML (`true`, `85`, `"x"`, `[{tool = "shell"}]`); anything
/// that does not parse as TOML is taken as a string, so `auto.compact=auto`
/// needs no quotes.
///
/// # Errors
///
/// A missing `=` or an invalid key path.
pub fn parse_set(arg: &str) -> Result<Table, OverlayError> {
    let (path, raw) = split_key_path(arg)
        .ok_or_else(|| OverlayError::new("--set", arg, "expected <dotted.key>=<value>"))?;
    if let Some(bad) = path.iter().find(|segment| match segment {
        KeySegment::Bare(name) => !is_valid_endpoint_name(name),
        KeySegment::Quoted(name) => name.is_empty(),
    }) {
        return Err(OverlayError::new(
            "--set",
            arg,
            format!(
                "invalid key segment `{}` (letters, digits, `_`, `-`, or \"quoted\"; `.` separates)",
                bad.as_str()
            ),
        ));
    }
    let path: Vec<&str> = path.iter().map(KeySegment::as_str).collect();
    Ok(nested(&path, parse_value(raw)))
}

/// One segment of a `--set` key path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySegment<'a> {
    /// `name`
    Bare(&'a str),
    /// `"name"` (may contain `.`, e.g. a model id like `gpt-5.4`).
    Quoted(&'a str),
}

impl<'a> KeySegment<'a> {
    fn as_str(&self) -> &'a str {
        match *self {
            Self::Bare(name) | Self::Quoted(name) => name,
        }
    }
}

/// Split `a."b.c".d=value` into its key segments and the raw value. A quoted
/// segment runs to the next `"` (no escapes). `None` without a `=` after the
/// key, or with an unterminated quote.
fn split_key_path(arg: &str) -> Option<(Vec<KeySegment<'_>>, &str)> {
    let mut segments = Vec::new();
    let mut rest = arg.trim_start();
    loop {
        let (segment, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            (
                KeySegment::Quoted(quoted.get(..end)?),
                quoted.get(end + 1..)?,
            )
        } else {
            let end = rest.find(['.', '='])?;
            (KeySegment::Bare(rest.get(..end)?.trim()), rest.get(end..)?)
        };
        segments.push(segment);
        let after = after.trim_start();
        if let Some(next) = after.strip_prefix('.') {
            rest = next.trim_start();
        } else {
            return after.strip_prefix('=').map(|raw| (segments, raw));
        }
    }
}

/// `raw` as a TOML value, or a string when it is not one.
fn parse_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    match format!("v = {trimmed}").parse::<Table>() {
        Ok(mut table) => table
            .remove("v")
            .unwrap_or_else(|| Value::String(raw.to_string())),
        Err(_) => Value::String(raw.to_string()),
    }
}

/// `{a = {b = {c = value}}}` for the path `a.b.c`.
fn nested(path: &[&str], value: Value) -> Table {
    let mut value = value;
    for key in path.iter().skip(1).rev() {
        let mut table = Table::new();
        table.insert((*key).to_string(), value);
        value = Value::Table(table);
    }
    let mut root = Table::new();
    if let Some(first) = path.first() {
        root.insert((*first).to_string(), value);
    }
    root
}

/// `--allow tool[=pattern]` → `allow = [{tool, arg}]`.
fn rule_table(list: &str, arg: &str, flag: &'static str) -> Result<Table, OverlayError> {
    let (tool, pattern) = match arg.split_once('=') {
        Some((tool, pattern)) => (tool.trim(), Some(pattern)),
        None => (arg.trim(), None),
    };
    if tool.is_empty() {
        return Err(OverlayError::new(flag, arg, "expected <tool>[=<pattern>]"));
    }
    let mut entry = Table::new();
    entry.insert("tool".into(), Value::String(tool.to_string()));
    if let Some(pattern) = pattern {
        entry.insert("arg".into(), Value::String(pattern.to_string()));
    }
    let mut root = Table::new();
    root.insert(list.into(), Value::Array(vec![Value::Table(entry)]));
    Ok(root)
}

/// `--endpoint-def name=dialect=url[,key_env=VAR]` → `[endpoints.name]`.
fn endpoint_def_table(arg: &str) -> Result<Table, OverlayError> {
    let bad = |reason: &str| OverlayError::new("--endpoint-def", arg, reason);
    let (name, rest) = arg
        .split_once('=')
        .ok_or_else(|| bad("expected <name>=<dialect>=<url>[,key_env=VAR]"))?;
    let (dialect, rest) = rest
        .split_once('=')
        .ok_or_else(|| bad("expected <name>=<dialect>=<url>[,key_env=VAR]"))?;
    let (url, key_env) = match rest.rsplit_once(",key_env=") {
        Some((url, var)) => (url, Some(var)),
        None => (rest, None),
    };
    if url.trim().is_empty() {
        return Err(bad("the base URL is empty"));
    }
    if key_env.is_some_and(|var| var.trim().is_empty()) {
        return Err(bad("key_env is empty"));
    }
    let mut section = Table::new();
    section.insert("dialect".into(), Value::String(dialect.trim().to_string()));
    section.insert("base_url".into(), Value::String(url.trim().to_string()));
    if let Some(var) = key_env {
        section.insert("api_key_env".into(), Value::String(var.trim().to_string()));
    }
    Ok(nested(&["endpoints", name.trim()], Value::Table(section)))
}

fn to_rules_file(table: &Table) -> Result<RulesFile, String> {
    Value::Table(table.clone())
        .try_into::<RulesFile>()
        .map_err(|err| err.to_string().trim().to_string())
}

/// Whether one piece fits the schema and its rules and endpoints are valid.
fn validate(piece: &Table) -> Result<(), String> {
    let file = to_rules_file(piece)?;
    file.rules(RuleOrigin::Cli).map_err(|err| err.to_string())?;
    for (name, section) in &file.endpoints {
        if !is_valid_endpoint_name(name) {
            return Err(format!("invalid endpoint name `{name}`"));
        }
        if let Some(dialect) = &section.dialect
            && Dialect::parse(dialect).is_none()
        {
            return Err(format!(
                "unknown dialect `{dialect}` (expected anthropic, openai or gemini)"
            ));
        }
    }
    for (model, section) in &file.pricing {
        if let Some(field) = section.invalid_field() {
            return Err(format!(
                "pricing.\"{model}\".{field} must be a non-negative number"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ui::compaction::CompactMode;
    use clap::Parser;

    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["rkat-tui"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).unwrap()
    }

    fn overlay(extra: &[&str]) -> Result<ConfigOverlay, OverlayError> {
        ConfigOverlay::from_args(&args(extra))
    }

    fn file(extra: &[&str]) -> RulesFile {
        overlay(extra)
            .unwrap()
            .rules_file()
            .unwrap()
            .unwrap_or_default()
    }

    fn doc(text: &str) -> Table {
        text.parse().unwrap()
    }

    #[test]
    fn merge_table() {
        type Case<'a> = (&'a str, &'a str, &'a str);
        let cases: Vec<Case<'_>> = vec![
            // scalar replace
            ("a = 1", "a = 2", "a = 2"),
            ("a = 1", "b = \"x\"", "a = 1\nb = \"x\""),
            // table merge
            (
                "[auto]\nenabled = true\ncompact = \"warn\"",
                "[auto]\ncompact = \"auto\"",
                "[auto]\nenabled = true\ncompact = \"auto\"",
            ),
            // array append
            (
                "allow = [{tool = \"read\"}]",
                "allow = [{tool = \"shell\", arg = \"ls*\"}]",
                "allow = [{tool = \"read\"}, {tool = \"shell\", arg = \"ls*\"}]",
            ),
            // endpoints by name
            (
                "[endpoints.litellm]\ndialect = \"openai\"\nbase_url = \"http://a\"\n[endpoints.other]\ndialect = \"gemini\"",
                "[endpoints.litellm]\nbase_url = \"http://host:4000\"",
                "[endpoints.litellm]\ndialect = \"openai\"\nbase_url = \"http://host:4000\"\n[endpoints.other]\ndialect = \"gemini\"",
            ),
            // a table replaces a scalar of the same name
            ("a = 1", "[a]\nb = 2", "[a]\nb = 2"),
        ];
        for (base, over, want) in cases {
            let mut merged = doc(base);
            merge(&mut merged, doc(over));
            assert_eq!(merged, doc(want), "{base} + {over}");
        }
    }

    #[test]
    fn set_parses_values_and_falls_back_to_strings() {
        let f = file(&[
            "--set",
            "auto.compact=auto",
            "--set",
            "auto.compact_warn_percent=60",
            "--set",
            "auto.enabled=false",
            "--set",
            "auto.classifier_model=claude-haiku-4-5",
            "--set",
            "endpoints.litellm.base_url=http://host:4000",
            "--set",
            "endpoints.litellm.priority=3",
        ]);
        let auto = f.auto.unwrap();
        assert_eq!(auto.compact, Some(CompactMode::Auto));
        assert_eq!(auto.compact_warn_percent, Some(60));
        assert_eq!(auto.enabled, Some(false));
        assert_eq!(auto.classifier_model.as_deref(), Some("claude-haiku-4-5"));
        let litellm = &f.endpoints["litellm"];
        assert_eq!(litellm.base_url.as_deref(), Some("http://host:4000"));
        assert_eq!(litellm.priority, Some(3));
    }

    #[test]
    fn set_quoted_segments_take_model_ids() {
        let f = file(&[
            "--set",
            r#"pricing."claude-opus-5-5".input=15"#,
            "--set",
            r#"pricing."gpt-5.4".output=10.5"#,
            "--set",
            r#"pricing."claude-opus-5-5".cached_input=1.5"#,
        ]);
        let opus = f.pricing["claude-opus-5-5"];
        assert_eq!(opus.input, Some(15.0));
        assert_eq!(opus.cached_input, Some(1.5));
        assert_eq!(f.pricing["gpt-5.4"].output, Some(10.5));
        for (argv, needle) in [
            (["--set", r#"pricing."x.input=1"#], "expected <dotted.key>"),
            (["--set", r#"pricing."".input=1"#], "invalid key segment"),
            (["--set", r#"pricing."m".input=-1"#], "non-negative"),
            (["--set", r#"pricing."m".bogus=1"#], "unknown field"),
        ] {
            let err = overlay(&argv).expect_err(needle).to_string();
            assert!(err.contains(needle), "{argv:?}: {err}");
        }
    }

    #[test]
    fn set_arrays_append_across_flags() {
        let f = file(&[
            "--set",
            r#"allow=[{tool="shell",arg="cargo nextest run*"}]"#,
            "--set",
            r#"allow=[{tool="read_file"}]"#,
            "--allow",
            "shell=git status*",
        ]);
        let tools: Vec<(&str, Option<&str>)> = f
            .allow
            .iter()
            .map(|e| (e.tool.as_str(), e.arg.as_deref()))
            .collect();
        assert_eq!(
            tools,
            vec![
                ("shell", Some("cargo nextest run*")),
                ("read_file", None),
                ("shell", Some("git status*")),
            ]
        );
    }

    #[test]
    fn invalid_settings_name_the_argument() {
        let cases: Vec<(&[&str], &str)> = vec![
            (&["--set", "auto.enabled"], "--set auto.enabled"),
            (&["--set", "auto..enabled=true"], "invalid key segment"),
            (&["--set", "auto.nope=1"], "unknown field"),
            (&["--set", "bogus=1"], "unknown field"),
            (
                &["--set", "auto.compact_warn_percent=300"],
                "--set auto.compact_warn_percent=300",
            ),
            (&["--set", "auto.enabled=maybe"], "invalid type"),
            (&["--set", "auto.compact=sometimes"], "sometimes"),
            (&["--set", "endpoints.x.dialect=cohere"], "unknown dialect"),
            (&["--allow", "shell=[unclosed"], "--allow shell=[unclosed"),
            (&["--deny", "=x"], "expected <tool>"),
            (
                &["--endpoint-def", "litellm=openai"],
                "--endpoint-def litellm=openai",
            ),
            (&["--endpoint-def", "x=cohere=http://h"], "unknown dialect"),
            (
                &["--endpoint-def", "bad.name=openai=http://h"],
                "invalid endpoint name",
            ),
        ];
        for (argv, needle) in cases {
            let err = overlay(argv).expect_err(needle).to_string();
            assert!(err.contains(needle), "{argv:?}: {err}");
        }
    }

    #[test]
    fn convenience_flags_desugar() {
        let f = file(&[
            "--allow",
            "shell=cargo nextest run*",
            "--allow",
            "read_file",
            "--deny",
            "shell=git push*",
            "--endpoint-def",
            "litellm=openai=http://localhost:4000,key_env=LITELLM_KEY",
            "--endpoint-def",
            "local=anthropic=http://127.0.0.1:9000/x?a=b",
            "--no-auto",
            "--classifier-model",
            "gpt-5.2",
            "--auto-compact",
            "--trust-project-builds",
        ]);
        assert_eq!(f.allow.len(), 2);
        assert_eq!(f.allow[0].arg.as_deref(), Some("cargo nextest run*"));
        assert_eq!(f.allow[1].arg, None);
        assert_eq!(f.deny[0].arg.as_deref(), Some("git push*"));
        let litellm = &f.endpoints["litellm"];
        assert_eq!(litellm.dialect.as_deref(), Some("openai"));
        assert_eq!(litellm.base_url.as_deref(), Some("http://localhost:4000"));
        assert_eq!(litellm.api_key_env.as_deref(), Some("LITELLM_KEY"));
        let local = &f.endpoints["local"];
        assert_eq!(
            local.base_url.as_deref(),
            Some("http://127.0.0.1:9000/x?a=b")
        );
        assert_eq!(local.api_key_env, None);
        let auto = f.auto.unwrap();
        assert_eq!(auto.enabled, Some(false));
        assert_eq!(auto.classifier_model.as_deref(), Some("gpt-5.2"));
        assert_eq!(auto.compact, Some(CompactMode::Auto));
        assert_eq!(auto.trust_project_builds, Some(true));

        let f = file(&["--no-auto-compact", "--no-trust-project-builds"]);
        let auto = f.auto.unwrap();
        assert_eq!(auto.compact, Some(CompactMode::Warn));
        assert_eq!(auto.trust_project_builds, Some(false));
        assert!(overlay(&[]).unwrap().rules_file().unwrap().is_none());
    }

    #[test]
    fn flags_win_over_set() {
        let f = file(&["--set", "auto.enabled=true", "--no-auto"]);
        assert_eq!(f.auto.unwrap().enabled, Some(false));
    }
}
