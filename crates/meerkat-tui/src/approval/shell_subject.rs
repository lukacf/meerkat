//! Shell commands as rule subjects.
//!
//! Allow rules must cover *every* simple command of a `;`/`&&`/`||`/`|` chain,
//! so `cargo nextest run*` does not allow `cargo nextest run; curl x | sh`.
//! A segment with a redirection other than an fd duplication (`2>&1`, `<&0`,
//! `>&-`) never matches an allow rule, so `echo*` does not allow
//! `echo hi >&out.txt` (which writes `out.txt`).
//!
//! Deny rules match when *any* simple command (or the whole command) matches,
//! either as written (whitespace-normalised, leading `\` stripped from the
//! command word) or in the canonical form produced by `canonical_text`, so
//! `true && git push`, `git  push`, `\git push`, `env FOO=1 /usr/bin/git -C x
//! push` are all caught by `git push*`.
//!
//! # What deny normalisation covers
//!
//! The canonical form repeatedly strips, from the front of each simple command:
//!
//! - a leading `\` on the command word;
//! - the directory of a command word containing `/` (`/usr/bin/git`,
//!   `./bin/git` become `git`);
//! - leading `NAME=value` assignments;
//! - the prefixes `command`, `builtin`, `env`, `exec`, `nice`, `nohup`, `time`,
//!   `timeout <duration>`, `stdbuf`, `sudo`, `doas`, `setsid`, `xargs`, `eval`,
//!   `watch` and `chroot <dir>`, with their options (and option values, for
//!   the options listed in `PREFIXES`); `env -S '...'` / `--split-string`
//!   is split into words;
//!
//! and, when the resulting command is `git`, the git global options before the
//! subcommand (`-C <dir>`, `-c <k=v>`, `--git-dir[=]<x>`, `--work-tree[=]<x>`,
//! `--namespace[=]<x>`, `--config-env[=]<x>`, `--attr-source[=]<x>`,
//! `--no-pager`, `-P`, `-p` and any other `-`/`--` word).
//!
//! # What it does not cover
//!
//! Deny rules are a best-effort text match, not a sandbox. They do **not** see
//! through encodings (quoting tricks beyond quote removal, `\x..` escapes,
//! base64), `sh -c '...'` / `bash -c` and other interpreters, `su -c`, aliases,
//! shell functions, variables or substitutions expanding to a command
//! (`$GIT push`), git aliases, or scripts that run the command. Such commands
//! are not matched by deny rules; they fall through to the remaining approval
//! tiers (the classifier) instead.

use std::collections::VecDeque;

use super::rules::ArgPattern;
use crate::approval::shell::{Op, RedirectKind, Segment, is_assignment, lex, segments};

/// Command words of a segment joined by single spaces, without a leading `\`.
fn segment_text(seg: &Segment) -> String {
    let mut out = String::new();
    for (i, word) in seg.words.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let text = if i == 0 {
            word.text.trim_start_matches('\\')
        } else {
            word.text.as_str()
        };
        out.push_str(text);
    }
    out
}

/// Whitespace-normalised text with a leading `\` stripped.
fn normalise(raw: &str) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_start_matches('\\').to_string()
}

/// A command prefix that runs its operand as another command.
struct Prefix {
    /// Command name.
    name: &'static str,
    /// Short options whose value is attached or in the next word.
    short_args: &'static [char],
    /// Long options whose value is `--opt=value` or in the next word.
    long_args: &'static [&'static str],
    /// Positional operands before the command (`timeout 5`, `chroot /x`).
    operands: usize,
    /// Whether `-S` / `--split-string` carries the command as one word (`env`).
    split_string: bool,
}

const fn prefix(
    name: &'static str,
    short_args: &'static [char],
    long_args: &'static [&'static str],
    operands: usize,
) -> Prefix {
    Prefix {
        name,
        short_args,
        long_args,
        operands,
        split_string: false,
    }
}

/// Prefixes stripped by `canonical_text`.
const PREFIXES: &[Prefix] = &[
    prefix("command", &[], &[], 0),
    prefix("builtin", &[], &[], 0),
    Prefix {
        name: "env",
        short_args: &['u', 'C', 'S', 'P'],
        long_args: &["--unset", "--chdir", "--split-string"],
        operands: 0,
        split_string: true,
    },
    prefix("exec", &['a'], &[], 0),
    prefix("nice", &['n'], &["--adjustment"], 0),
    prefix("nohup", &[], &[], 0),
    prefix("time", &['f', 'o'], &["--format", "--output"], 0),
    prefix("timeout", &['s', 'k'], &["--signal", "--kill-after"], 1),
    prefix(
        "stdbuf",
        &['i', 'o', 'e'],
        &["--input", "--output", "--error"],
        0,
    ),
    prefix(
        "sudo",
        &['u', 'g', 'C', 'D', 'h', 'p', 'r', 't', 'U', 'T', 'R'],
        &[
            "--user",
            "--group",
            "--chdir",
            "--chroot",
            "--host",
            "--prompt",
            "--role",
            "--type",
            "--other-user",
            "--command-timeout",
            "--close-from",
        ],
        0,
    ),
    prefix("doas", &['u', 'C'], &[], 0),
    prefix("setsid", &[], &[], 0),
    prefix(
        "xargs",
        &['a', 'd', 'E', 'I', 'L', 'n', 'P', 's'],
        &[
            "--arg-file",
            "--delimiter",
            "--max-lines",
            "--max-args",
            "--max-procs",
            "--max-chars",
            "--process-slot-var",
        ],
        0,
    ),
    prefix("eval", &[], &[], 0),
    prefix("watch", &['n', 'q'], &["--interval", "--equexit"], 0),
    prefix("chroot", &[], &["--userspec", "--groups"], 1),
];

/// Git global options that take a separate value word.
const GIT_GLOBAL_ARGS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--attr-source",
];

/// Strip `prefix`'s options from the front of `rest`. Returns the command words
/// carried by `-S`/`--split-string` when the prefix has them.
fn strip_options(prefix: &Prefix, rest: &mut VecDeque<String>) -> Option<String> {
    let mut split = None;
    while let Some(word) = rest.front() {
        if word == "--" {
            rest.pop_front();
            break;
        }
        if !word.starts_with('-') {
            break;
        }
        let Some(word) = rest.pop_front() else {
            break;
        };
        if word == "-" {
            continue;
        }
        if let Some(long) = word.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let full = format!("--{name}");
            if !prefix.long_args.contains(&full.as_str()) {
                continue;
            }
            let value = attached.or_else(|| rest.pop_front());
            if prefix.split_string && full == "--split-string" {
                split = value;
            }
            continue;
        }
        let cluster = word.get(1..).unwrap_or_default();
        for (idx, c) in cluster.char_indices() {
            if !prefix.short_args.contains(&c) {
                continue;
            }
            let attached = cluster.get(idx + c.len_utf8()..).unwrap_or_default();
            let value = if attached.is_empty() {
                rest.pop_front()
            } else {
                Some(attached.to_string())
            };
            if prefix.split_string && c == 'S' {
                split = value;
            }
            break;
        }
    }
    split
}

/// Strip git global options between `git` and its subcommand.
fn strip_git_globals(rest: &mut VecDeque<String>) {
    while let Some(word) = rest.front() {
        if !word.starts_with('-') || word == "-" {
            break;
        }
        let takes_value = GIT_GLOBAL_ARGS.contains(&word.as_str());
        rest.pop_front();
        if takes_value {
            rest.pop_front();
        }
    }
}

/// The canonical text of one simple command for deny matching: prefixes,
/// assignments, command-word directories and git global options removed (see
/// the module documentation for the exact list), words joined by single
/// spaces.
fn canonical_text(words: impl IntoIterator<Item = String>) -> String {
    let mut rest: VecDeque<String> = words.into_iter().collect();
    while let Some(first) = rest.pop_front() {
        let unescaped = first.trim_start_matches('\\');
        if is_assignment(unescaped) {
            continue;
        }
        let name = unescaped.rsplit('/').next().unwrap_or(unescaped);
        let Some(prefix) = PREFIXES.iter().find(|p| p.name == name) else {
            rest.push_front(name.to_string());
            break;
        };
        if let Some(split) = strip_options(prefix, &mut rest) {
            for word in split.split_whitespace().rev() {
                rest.push_front(word.to_string());
            }
            continue;
        }
        for _ in 0..prefix.operands {
            rest.pop_front();
        }
    }
    if rest.front().is_some_and(|w| w == "git") {
        let git = rest.pop_front();
        strip_git_globals(&mut rest);
        if let Some(git) = git {
            rest.push_front(git);
        }
    }
    Vec::from(rest).join(" ")
}

/// Whether a redirection only duplicates or closes a file descriptor
/// (`2>&1`, `<&0`, `>&-`). `>&file` writes `file` and is not one.
fn is_fd_dup(kind: RedirectKind, target: &str) -> bool {
    matches!(kind, RedirectKind::DupOut | RedirectKind::DupIn)
        && (target == "-" || (!target.is_empty() && target.bytes().all(|b| b.is_ascii_digit())))
}

/// Allow semantics: every segment matches. Unsupported syntax, background
/// jobs and file redirections never match.
pub(crate) fn allow_matches(arg: &ArgPattern, command: &str) -> bool {
    if let ArgPattern::Literal(text) = arg {
        return text == command;
    }
    let Ok(tokens) = lex(command) else {
        return false;
    };
    let mut any = false;
    for seg in segments(tokens) {
        if seg.terminator == Some(Op::Amp) {
            return false;
        }
        if seg.words.is_empty() && seg.redirects.is_empty() {
            continue;
        }
        let fd_dup_only = seg.redirects.iter().all(|r| is_fd_dup(r.kind, &r.target));
        if !fd_dup_only || !arg.matches(&segment_text(&seg)) {
            return false;
        }
        any = true;
    }
    any
}

/// Deny semantics: the whole command or any segment matches, as written or in
/// its `canonical_text` form.
pub(crate) fn deny_matches(arg: &ArgPattern, command: &str) -> bool {
    if arg.matches(command) || arg.matches(&normalise(command)) {
        return true;
    }
    match lex(command) {
        Ok(tokens) => segments(tokens).iter().any(|seg| {
            arg.matches(&segment_text(seg))
                || arg.matches(&canonical_text(seg.words.iter().map(|w| w.text.clone())))
        }),
        // Best effort for syntax the lexer refuses: split on anything that
        // can separate or nest commands.
        Err(_) => command
            .split([';', '&', '|', '\n', '(', ')', '`', '{', '}'])
            .map(|piece| normalise(piece.trim_start_matches('$')))
            .any(|piece| {
                !piece.is_empty()
                    && (arg.matches(&piece)
                        || arg.matches(&canonical_text(
                            piece.split_whitespace().map(str::to_string),
                        )))
            }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn pat(raw: &str) -> ArgPattern {
        ArgPattern::new(raw).unwrap()
    }

    #[test]
    fn allow_rule_does_not_match_fd_dup_to_file() {
        let echo = pat("echo*");
        assert!(!allow_matches(&echo, "echo hi >&out.txt"));
        assert!(!allow_matches(&echo, "echo hi >& out.txt"));
        assert!(!allow_matches(&echo, "echo hi 2>&out.txt"));
        assert!(allow_matches(&echo, "echo hi 2>&1"));
        assert!(allow_matches(&echo, "echo hi >&-"));
        assert!(allow_matches(&echo, "echo hi"));
    }

    #[test]
    fn deny_sees_through_prefixes_and_git_globals() {
        let push = pat("git push*");
        for cmd in [
            "git push",
            "\\git push",
            "command git push",
            "builtin git push",
            "env git push origin",
            "env -i FOO=1 git push",
            "env -u HOME -- git push",
            "env -S 'git push'",
            "FOO=1 git push",
            "FOO=1 BAR=/x/y git push",
            "/usr/bin/git push",
            "./bin/git push",
            "git -C sub push",
            "git -c a=b --no-pager push",
            "git --git-dir .git --work-tree=. -P push",
            "true && env /usr/bin/git -C . push --force",
            "env FOO=1 command /usr/bin/git -C x push",
            "nice -n 5 git push",
            "nohup git push",
            "time -p git push",
            "timeout 5 git push",
            "timeout -s KILL 5 git push",
            "sudo -u root git push",
            "exec git push",
            "stdbuf -oL git push",
            "echo x | xargs -n 1 git push",
            "FOO=$x git push",
        ] {
            assert!(deny_matches(&push, cmd), "{cmd} should be denied");
        }
        for cmd in [
            "git status",
            "echo git push",
            "env git status",
            "git -C push status",
        ] {
            assert!(!deny_matches(&push, cmd), "{cmd} should not be denied");
        }
    }

    #[test]
    fn canonical_text_keeps_plain_commands() {
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(canonical_text(words("ls -la src")), "ls -la src");
        assert_eq!(canonical_text(words("echo git push")), "echo git push");
        assert_eq!(canonical_text(words("env")), "");
    }
}
