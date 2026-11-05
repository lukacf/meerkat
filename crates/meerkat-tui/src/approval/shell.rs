//! Tier-1 shell analysis: a hand-written POSIX-subset lexer plus a read-only allowlist.
//!
//! The lexer deliberately understands only a small, predictable subset of POSIX
//! shell syntax. Anything that could hide a second command or expand at runtime
//! (command substitution, process substitution, parameter expansion, heredocs,
//! subshells, brace groups) is reported as [`Unsupported`], which routes the
//! command to the classifier instead of auto-allowing it.
//!
//! [`allowlisted`] then requires *every* segment of a `&&`/`||`/`;`/`|` chain to
//! pass the allowlist; one unknown segment sends the whole command to Tier 3.

use super::policy::PolicyContext;
use std::fmt;
use std::path::PathBuf;

/// Control operators separating simple commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `;`
    Semi,
    /// `|` (also `|&`)
    Pipe,
    /// `&` (background)
    Amp,
    /// An unescaped newline.
    Newline,
}

/// Redirection operator kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `<`
    In,
    /// `>`
    Out,
    /// `>>`
    Append,
    /// `>|`
    Clobber,
    /// `>&`
    DupOut,
    /// `<&`
    DupIn,
    /// `<>`
    ReadWrite,
    /// `&>` / `&>>`
    OutErr,
}

/// A redirection with its (unexpanded) target word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// Explicit file descriptor (`2>`), if any.
    pub fd: Option<u32>,
    /// Operator kind.
    pub kind: RedirectKind,
    /// Target word after quote removal.
    pub target: String,
    /// Whether the target contains an unquoted glob character.
    pub target_glob: bool,
}

/// A word after quote removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// Text after quote removal and escape processing.
    pub text: String,
    /// Whether the word contains an unquoted glob character (`*`, `?`, `[`).
    pub unquoted_glob: bool,
}

/// A lexer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    /// A word.
    Word(Word),
    /// A control operator.
    Op(Op),
    /// A redirection including its target.
    Redirect(Redirect),
}

/// The command uses syntax outside the supported subset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported shell syntax: {0}")]
pub struct Unsupported(pub &'static str);

/// The command is not on the Tier-1 allowlist; the reason is human readable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not allowlisted: {0}")]
pub struct NotAllowlisted(pub String);

impl NotAllowlisted {
    fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

impl From<Unsupported> for NotAllowlisted {
    fn from(err: Unsupported) -> Self {
        Self(err.to_string())
    }
}

/// Raw tokens before redirect targets are attached.
enum Raw {
    Word(Word),
    Op(Op),
    Redir { fd: Option<u32>, kind: RedirectKind },
}

#[derive(Default)]
struct WordBuf {
    text: String,
    active: bool,
    quoted: bool,
    unquoted_glob: bool,
}

impl WordBuf {
    fn push_unquoted(&mut self, c: char) {
        self.active = true;
        if matches!(c, '*' | '?' | '[') {
            self.unquoted_glob = true;
        }
        self.text.push(c);
    }

    fn push_quoted(&mut self, c: char) {
        self.active = true;
        self.quoted = true;
        self.text.push(c);
    }

    fn mark_quoted(&mut self) {
        self.active = true;
        self.quoted = true;
    }

    /// The pending word is an unquoted run of digits (a possible fd prefix).
    fn as_fd(&self) -> Option<u32> {
        if self.active
            && !self.quoted
            && !self.text.is_empty()
            && self.text.bytes().all(|b| b.is_ascii_digit())
        {
            return self.text.parse().ok();
        }
        None
    }

    fn flush(&mut self, out: &mut Vec<Raw>) {
        if self.active {
            let buf = std::mem::take(self);
            out.push(Raw::Word(Word {
                text: buf.text,
                unquoted_glob: buf.unquoted_glob,
            }));
        }
    }
}

/// Lex a command string into tokens.
///
/// # Errors
///
/// Returns [`Unsupported`] for any syntax outside the supported subset.
pub fn lex(input: &str) -> Result<Vec<Tok>, Unsupported> {
    let raw = lex_raw(input)?;
    attach_redirect_targets(raw)
}

fn lex_raw(input: &str) -> Result<Vec<Raw>, Unsupported> {
    let mut out = Vec::new();
    let mut word = WordBuf::default();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' => word.flush(&mut out),
            '\n' => {
                word.flush(&mut out);
                out.push(Raw::Op(Op::Newline));
            }
            '#' if !word.active => {
                // Comment until end of line.
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '\'' => {
                word.mark_quoted();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(q) => word.push_quoted(q),
                        None => return Err(Unsupported("unterminated single quote")),
                    }
                }
            }
            '"' => {
                word.mark_quoted();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('$' | '`' | '"' | '\\')) => word.push_quoted(e),
                            Some('\n') => {}
                            Some(other) => {
                                word.push_quoted('\\');
                                word.push_quoted(other);
                            }
                            None => return Err(Unsupported("unterminated double quote")),
                        },
                        Some('$') => return Err(Unsupported("parameter expansion")),
                        Some('`') => return Err(Unsupported("command substitution")),
                        Some(q) => word.push_quoted(q),
                        None => return Err(Unsupported("unterminated double quote")),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(e) => word.push_quoted(e),
                None => return Err(Unsupported("trailing backslash")),
            },
            '$' => return Err(Unsupported("parameter expansion")),
            '`' => return Err(Unsupported("command substitution")),
            '(' | ')' => return Err(Unsupported("subshell or grouping")),
            '{' | '}' => return Err(Unsupported("brace group or expansion")),
            ';' => {
                word.flush(&mut out);
                if chars.peek() == Some(&';') {
                    return Err(Unsupported("case terminator"));
                }
                out.push(Raw::Op(Op::Semi));
            }
            '|' => {
                word.flush(&mut out);
                match chars.peek() {
                    Some('|') => {
                        chars.next();
                        out.push(Raw::Op(Op::OrOr));
                    }
                    Some('&') => {
                        chars.next();
                        out.push(Raw::Op(Op::Pipe));
                    }
                    _ => out.push(Raw::Op(Op::Pipe)),
                }
            }
            '&' => {
                word.flush(&mut out);
                match chars.peek() {
                    Some('&') => {
                        chars.next();
                        out.push(Raw::Op(Op::AndAnd));
                    }
                    Some('>') => {
                        chars.next();
                        if chars.peek() == Some(&'>') {
                            chars.next();
                        }
                        out.push(Raw::Redir {
                            fd: None,
                            kind: RedirectKind::OutErr,
                        });
                    }
                    _ => out.push(Raw::Op(Op::Amp)),
                }
            }
            '<' | '>' => {
                let fd = word.as_fd();
                if fd.is_some() {
                    word = WordBuf::default();
                } else {
                    word.flush(&mut out);
                }
                let kind = if c == '<' {
                    match chars.peek() {
                        Some('<') => return Err(Unsupported("heredoc or here-string")),
                        Some('(') => return Err(Unsupported("process substitution")),
                        Some('&') => {
                            chars.next();
                            RedirectKind::DupIn
                        }
                        Some('>') => {
                            chars.next();
                            RedirectKind::ReadWrite
                        }
                        _ => RedirectKind::In,
                    }
                } else {
                    match chars.peek() {
                        Some('(') => return Err(Unsupported("process substitution")),
                        Some('>') => {
                            chars.next();
                            RedirectKind::Append
                        }
                        Some('&') => {
                            chars.next();
                            RedirectKind::DupOut
                        }
                        Some('|') => {
                            chars.next();
                            RedirectKind::Clobber
                        }
                        _ => RedirectKind::Out,
                    }
                };
                out.push(Raw::Redir { fd, kind });
            }
            other => word.push_unquoted(other),
        }
    }
    word.flush(&mut out);
    Ok(out)
}

fn attach_redirect_targets(raw: Vec<Raw>) -> Result<Vec<Tok>, Unsupported> {
    let mut out = Vec::with_capacity(raw.len());
    let mut iter = raw.into_iter();
    while let Some(tok) = iter.next() {
        match tok {
            Raw::Word(w) => out.push(Tok::Word(w)),
            Raw::Op(op) => out.push(Tok::Op(op)),
            Raw::Redir { fd, kind } => match iter.next() {
                Some(Raw::Word(target)) => out.push(Tok::Redirect(Redirect {
                    fd,
                    kind,
                    target: target.text,
                    target_glob: target.unquoted_glob,
                })),
                _ => return Err(Unsupported("redirect without a target")),
            },
        }
    }
    Ok(out)
}

/// One simple command: its words, redirects, and the operator that ended it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    /// Words in order (command name first).
    pub words: Vec<Word>,
    /// Redirections attached to this command.
    pub redirects: Vec<Redirect>,
    /// Operator that terminated the segment (`None` at end of input).
    pub terminator: Option<Op>,
}

impl Segment {
    fn is_empty(&self) -> bool {
        self.words.is_empty() && self.redirects.is_empty()
    }
}

/// Split a token stream on control operators.
pub fn segments(tokens: Vec<Tok>) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut current = Segment::default();
    for tok in tokens {
        match tok {
            Tok::Word(w) => current.words.push(w),
            Tok::Redirect(r) => current.redirects.push(r),
            Tok::Op(op) => {
                current.terminator = Some(op);
                out.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Commands that run another command; never Tier 1.
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "xargs", "exec", "eval", "nohup", "timeout", "nice", "time", "su",
    "setsid", "stdbuf", "chroot", "watch", "parallel",
];

/// How positional words of a command are checked against the project root.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathArgs {
    /// Options are checked against the per-command lists, and every positional
    /// (and every path-valued option) must resolve inside the project.
    InsideProject,
    /// Words are plain strings (e.g. `echo`), never touched as paths or options.
    Ignore,
}

/// Command-specific checks beyond option parsing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Extra {
    None,
    /// `uniq` with at most one positional (the two-positional form writes a file).
    UniqSingleInput,
    /// `git branch`: listing only.
    GitBranchList,
    /// `git remote`: only `-v`/`--verbose`.
    GitRemoteList,
    /// `cargo fmt` must carry `--check`.
    CargoFmtCheck,
    /// `find`: a fixed set of read-only primaries.
    Find,
}

/// What an option does with its argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arg {
    /// No argument. An `--opt=value` value is still path-checked.
    Flag,
    /// Required argument that is not a path (pattern, number, format).
    Plain,
    /// Optional argument, only ever attached (`--color=never`, `-uno`).
    Optional,
    /// Required argument naming a file; checked like a positional.
    Path,
}

struct Allowed {
    name: &'static str,
    /// Subcommand that must directly follow the command name (`git status`).
    sub: Option<&'static str>,
    /// Positive list of accepted long options. Anything else (including GNU
    /// abbreviations such as `--outp`) is rejected.
    longs: &'static [(&'static str, Arg)],
    /// Short options that take an argument. Short options not listed here are
    /// treated as plain flags.
    short_args: &'static [(char, Arg)],
    /// Short options that are never accepted, alone or in a cluster.
    forbidden_short: &'static [char],
    paths: PathArgs,
    extra: Extra,
    /// Runs project code (build scripts, proc macros, `rustc-wrapper`, toolchain
    /// overrides); requires `trust_project_builds`.
    project_build: bool,
}

const HELP: [(&str, Arg); 2] = [("--help", Arg::Flag), ("--version", Arg::Flag)];

const fn ignore(name: &'static str) -> Allowed {
    Allowed {
        name,
        sub: None,
        longs: &[],
        short_args: &[],
        forbidden_short: &[],
        paths: PathArgs::Ignore,
        extra: Extra::None,
        project_build: false,
    }
}

const fn tool(
    name: &'static str,
    longs: &'static [(&'static str, Arg)],
    short_args: &'static [(char, Arg)],
    forbidden_short: &'static [char],
) -> Allowed {
    Allowed {
        name,
        sub: None,
        longs,
        short_args,
        forbidden_short,
        paths: PathArgs::InsideProject,
        extra: Extra::None,
        project_build: false,
    }
}

const fn with_extra(mut a: Allowed, extra: Extra) -> Allowed {
    a.extra = extra;
    a
}

const CAT_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--number", Arg::Flag),
    ("--number-nonblank", Arg::Flag),
    ("--show-all", Arg::Flag),
    ("--show-ends", Arg::Flag),
    ("--show-tabs", Arg::Flag),
    ("--show-nonprinting", Arg::Flag),
    ("--squeeze-blank", Arg::Flag),
];
const HEAD_TAIL_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--lines", Arg::Plain),
    ("--bytes", Arg::Plain),
    ("--quiet", Arg::Flag),
    ("--silent", Arg::Flag),
    ("--verbose", Arg::Flag),
    ("--zero-terminated", Arg::Flag),
];
const HEAD_TAIL_SHORT: &[(char, Arg)] = &[('n', Arg::Plain), ('c', Arg::Plain)];
const WC_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--lines", Arg::Flag),
    ("--words", Arg::Flag),
    ("--bytes", Arg::Flag),
    ("--chars", Arg::Flag),
    ("--max-line-length", Arg::Flag),
];
const LS_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--all", Arg::Flag),
    ("--almost-all", Arg::Flag),
    ("--classify", Arg::Flag),
    ("--color", Arg::Optional),
    ("--directory", Arg::Flag),
    ("--group-directories-first", Arg::Flag),
    ("--human-readable", Arg::Flag),
    ("--inode", Arg::Flag),
    ("--recursive", Arg::Flag),
    ("--reverse", Arg::Flag),
    ("--size", Arg::Flag),
    ("--sort", Arg::Plain),
    ("--time", Arg::Plain),
    ("--full-time", Arg::Flag),
];
const LS_SHORT: &[(char, Arg)] = &[('I', Arg::Plain), ('w', Arg::Plain), ('T', Arg::Plain)];
const FILE_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--brief", Arg::Flag),
    ("--mime", Arg::Flag),
    ("--mime-type", Arg::Flag),
    ("--mime-encoding", Arg::Flag),
    ("--dereference", Arg::Flag),
    ("--no-dereference", Arg::Flag),
    ("--magic-file", Arg::Path),
    ("--files-from", Arg::Path),
    ("--separator", Arg::Plain),
];
const FILE_SHORT: &[(char, Arg)] = &[
    ('m', Arg::Path),
    ('f', Arg::Path),
    ('F', Arg::Plain),
    ('e', Arg::Plain),
    ('P', Arg::Plain),
];
const STAT_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--dereference", Arg::Flag),
    ("--file-system", Arg::Flag),
    ("--format", Arg::Plain),
    ("--printf", Arg::Plain),
    ("--terse", Arg::Flag),
];
const DU_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--all", Arg::Flag),
    ("--apparent-size", Arg::Flag),
    ("--bytes", Arg::Flag),
    ("--human-readable", Arg::Flag),
    ("--max-depth", Arg::Plain),
    ("--summarize", Arg::Flag),
    ("--total", Arg::Flag),
];
const DU_SHORT: &[(char, Arg)] = &[('d', Arg::Plain), ('B', Arg::Plain), ('t', Arg::Plain)];
const TREE_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--dirsfirst", Arg::Flag),
    ("--gitignore", Arg::Flag),
    ("--noreport", Arg::Flag),
    ("--prune", Arg::Flag),
    ("--charset", Arg::Plain),
    ("--filelimit", Arg::Plain),
];
const TREE_SHORT: &[(char, Arg)] = &[('L', Arg::Plain), ('I', Arg::Plain), ('P', Arg::Plain)];
const DIFF_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--brief", Arg::Flag),
    ("--color", Arg::Optional),
    ("--context", Arg::Optional),
    ("--ignore-all-space", Arg::Flag),
    ("--ignore-blank-lines", Arg::Flag),
    ("--ignore-case", Arg::Flag),
    ("--ignore-space-change", Arg::Flag),
    ("--new-file", Arg::Flag),
    ("--recursive", Arg::Flag),
    ("--report-identical-files", Arg::Flag),
    ("--side-by-side", Arg::Flag),
    ("--unified", Arg::Optional),
];
const DIFF_SHORT: &[(char, Arg)] = &[
    ('U', Arg::Plain),
    ('C', Arg::Plain),
    ('W', Arg::Plain),
    ('x', Arg::Plain),
    ('I', Arg::Plain),
];
const CMP_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--bytes", Arg::Plain),
    ("--ignore-initial", Arg::Plain),
    ("--print-bytes", Arg::Flag),
    ("--quiet", Arg::Flag),
    ("--silent", Arg::Flag),
    ("--verbose", Arg::Flag),
];
const CMP_SHORT: &[(char, Arg)] = &[('i', Arg::Plain), ('n', Arg::Plain)];
const REALPATH_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--canonicalize-existing", Arg::Flag),
    ("--canonicalize-missing", Arg::Flag),
    ("--no-symlinks", Arg::Flag),
    ("--quiet", Arg::Flag),
    ("--relative-to", Arg::Path),
    ("--relative-base", Arg::Path),
    ("--zero", Arg::Flag),
];
const SORT_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--check", Arg::Flag),
    ("--dictionary-order", Arg::Flag),
    ("--field-separator", Arg::Plain),
    ("--general-numeric-sort", Arg::Flag),
    ("--human-numeric-sort", Arg::Flag),
    ("--ignore-case", Arg::Flag),
    ("--ignore-leading-blanks", Arg::Flag),
    ("--key", Arg::Plain),
    ("--merge", Arg::Flag),
    ("--month-sort", Arg::Flag),
    ("--numeric-sort", Arg::Flag),
    ("--reverse", Arg::Flag),
    ("--stable", Arg::Flag),
    ("--unique", Arg::Flag),
    ("--version-sort", Arg::Flag),
    ("--zero-terminated", Arg::Flag),
];
const SORT_SHORT: &[(char, Arg)] = &[('k', Arg::Plain), ('t', Arg::Plain), ('S', Arg::Plain)];
const UNIQ_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--count", Arg::Flag),
    ("--repeated", Arg::Flag),
    ("--unique", Arg::Flag),
    ("--ignore-case", Arg::Flag),
    ("--skip-fields", Arg::Plain),
    ("--skip-chars", Arg::Plain),
    ("--check-chars", Arg::Plain),
    ("--zero-terminated", Arg::Flag),
];
const UNIQ_SHORT: &[(char, Arg)] = &[('f', Arg::Plain), ('s', Arg::Plain), ('w', Arg::Plain)];
const RG_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--case-sensitive", Arg::Flag),
    ("--color", Arg::Plain),
    ("--column", Arg::Flag),
    ("--context", Arg::Plain),
    ("--after-context", Arg::Plain),
    ("--before-context", Arg::Plain),
    ("--count", Arg::Flag),
    ("--count-matches", Arg::Flag),
    ("--file", Arg::Path),
    ("--files", Arg::Flag),
    ("--files-with-matches", Arg::Flag),
    ("--files-without-match", Arg::Flag),
    ("--fixed-strings", Arg::Flag),
    ("--glob", Arg::Plain),
    ("--heading", Arg::Flag),
    ("--hidden", Arg::Flag),
    ("--iglob", Arg::Plain),
    ("--ignore-case", Arg::Flag),
    ("--invert-match", Arg::Flag),
    ("--json", Arg::Flag),
    ("--line-number", Arg::Flag),
    ("--max-count", Arg::Plain),
    ("--max-depth", Arg::Plain),
    ("--multiline", Arg::Flag),
    ("--no-filename", Arg::Flag),
    ("--no-heading", Arg::Flag),
    ("--no-ignore", Arg::Flag),
    ("--null", Arg::Flag),
    ("--only-matching", Arg::Flag),
    ("--quiet", Arg::Flag),
    ("--regexp", Arg::Plain),
    ("--smart-case", Arg::Flag),
    ("--sort", Arg::Plain),
    ("--stats", Arg::Flag),
    ("--trim", Arg::Flag),
    ("--type", Arg::Plain),
    ("--type-list", Arg::Flag),
    ("--type-not", Arg::Plain),
    ("--unrestricted", Arg::Flag),
    ("--vimgrep", Arg::Flag),
    ("--with-filename", Arg::Flag),
    ("--word-regexp", Arg::Flag),
];
const RG_SHORT: &[(char, Arg)] = &[
    ('e', Arg::Plain),
    ('f', Arg::Path),
    ('g', Arg::Plain),
    ('t', Arg::Plain),
    ('T', Arg::Plain),
    ('A', Arg::Plain),
    ('B', Arg::Plain),
    ('C', Arg::Plain),
    ('m', Arg::Plain),
    ('d', Arg::Plain),
    ('M', Arg::Plain),
    ('j', Arg::Plain),
    ('E', Arg::Plain),
    ('r', Arg::Plain),
];
const GREP_LONGS: &[(&str, Arg)] = &[
    HELP[0],
    HELP[1],
    ("--after-context", Arg::Plain),
    ("--before-context", Arg::Plain),
    ("--color", Arg::Optional),
    ("--context", Arg::Optional),
    ("--count", Arg::Flag),
    ("--exclude", Arg::Plain),
    ("--exclude-dir", Arg::Plain),
    ("--extended-regexp", Arg::Flag),
    ("--file", Arg::Path),
    ("--files-with-matches", Arg::Flag),
    ("--files-without-match", Arg::Flag),
    ("--fixed-strings", Arg::Flag),
    ("--ignore-case", Arg::Flag),
    ("--include", Arg::Plain),
    ("--invert-match", Arg::Flag),
    ("--line-number", Arg::Flag),
    ("--max-count", Arg::Plain),
    ("--no-filename", Arg::Flag),
    ("--no-messages", Arg::Flag),
    ("--null", Arg::Flag),
    ("--only-matching", Arg::Flag),
    ("--perl-regexp", Arg::Flag),
    ("--quiet", Arg::Flag),
    ("--recursive", Arg::Flag),
    ("--regexp", Arg::Plain),
    ("--silent", Arg::Flag),
    ("--text", Arg::Flag),
    ("--with-filename", Arg::Flag),
    ("--word-regexp", Arg::Flag),
];
const GREP_SHORT: &[(char, Arg)] = &[
    ('e', Arg::Plain),
    ('f', Arg::Path),
    ('A', Arg::Plain),
    ('B', Arg::Plain),
    ('C', Arg::Plain),
    ('m', Arg::Plain),
];

const GIT_FORBIDDEN_SHORT: &[char] = &['o'];
const GIT_STATUS_LONGS: &[(&str, Arg)] = &[
    ("--ahead-behind", Arg::Flag),
    ("--branch", Arg::Flag),
    ("--ignored", Arg::Optional),
    ("--long", Arg::Flag),
    ("--no-ahead-behind", Arg::Flag),
    ("--porcelain", Arg::Optional),
    ("--short", Arg::Flag),
    ("--show-stash", Arg::Flag),
    ("--untracked-files", Arg::Optional),
    ("--verbose", Arg::Flag),
];
const GIT_STATUS_SHORT: &[(char, Arg)] = &[('u', Arg::Optional)];
/// Shared by `git diff`, `git log` and `git show`.
const GIT_LOG_LONGS: &[(&str, Arg)] = &[
    ("--abbrev", Arg::Optional),
    ("--abbrev-commit", Arg::Flag),
    ("--after", Arg::Plain),
    ("--all", Arg::Flag),
    ("--author", Arg::Plain),
    ("--before", Arg::Plain),
    ("--cached", Arg::Flag),
    ("--check", Arg::Flag),
    ("--color", Arg::Optional),
    ("--date", Arg::Plain),
    ("--decorate", Arg::Optional),
    ("--diff-filter", Arg::Optional),
    ("--exit-code", Arg::Flag),
    ("--find-renames", Arg::Optional),
    ("--first-parent", Arg::Flag),
    ("--follow", Arg::Flag),
    ("--format", Arg::Optional),
    ("--graph", Arg::Flag),
    ("--grep", Arg::Plain),
    ("--histogram", Arg::Flag),
    ("--ignore-all-space", Arg::Flag),
    ("--ignore-space-change", Arg::Flag),
    ("--max-count", Arg::Plain),
    ("--merge-base", Arg::Flag),
    ("--merges", Arg::Flag),
    ("--minimal", Arg::Flag),
    ("--name-only", Arg::Flag),
    ("--name-status", Arg::Flag),
    ("--no-color", Arg::Flag),
    ("--no-ext-diff", Arg::Flag),
    ("--no-merges", Arg::Flag),
    ("--no-patch", Arg::Flag),
    ("--no-textconv", Arg::Flag),
    ("--numstat", Arg::Flag),
    ("--oneline", Arg::Flag),
    ("--patch", Arg::Flag),
    ("--patience", Arg::Flag),
    ("--pretty", Arg::Optional),
    ("--quiet", Arg::Flag),
    ("--raw", Arg::Flag),
    ("--reverse", Arg::Flag),
    ("--shortstat", Arg::Flag),
    ("--since", Arg::Plain),
    ("--skip", Arg::Plain),
    ("--staged", Arg::Flag),
    ("--stat", Arg::Optional),
    ("--summary", Arg::Flag),
    ("--unified", Arg::Optional),
    ("--until", Arg::Plain),
    ("--word-diff", Arg::Optional),
];
const GIT_LOG_SHORT: &[(char, Arg)] = &[
    ('n', Arg::Plain),
    ('O', Arg::Path),
    ('U', Arg::Optional),
    ('S', Arg::Plain),
    ('G', Arg::Plain),
    ('M', Arg::Optional),
    ('C', Arg::Optional),
    ('B', Arg::Optional),
    ('l', Arg::Optional),
];
const GIT_BRANCH_LONGS: &[(&str, Arg)] = &[
    ("--all", Arg::Flag),
    ("--color", Arg::Optional),
    ("--contains", Arg::Plain),
    ("--format", Arg::Plain),
    ("--list", Arg::Flag),
    ("--merged", Arg::Plain),
    ("--no-color", Arg::Flag),
    ("--no-contains", Arg::Plain),
    ("--no-merged", Arg::Plain),
    ("--points-at", Arg::Plain),
    ("--remotes", Arg::Flag),
    ("--show-current", Arg::Flag),
    ("--sort", Arg::Plain),
    ("--verbose", Arg::Flag),
];
const GIT_REV_PARSE_LONGS: &[(&str, Arg)] = &[
    ("--abbrev-ref", Arg::Optional),
    ("--git-dir", Arg::Flag),
    ("--is-inside-work-tree", Arg::Flag),
    ("--quiet", Arg::Flag),
    ("--short", Arg::Optional),
    ("--show-cdup", Arg::Flag),
    ("--show-prefix", Arg::Flag),
    ("--show-toplevel", Arg::Flag),
    ("--symbolic-full-name", Arg::Flag),
    ("--verify", Arg::Flag),
];
const GIT_LS_FILES_LONGS: &[(&str, Arg)] = &[
    ("--cached", Arg::Flag),
    ("--deleted", Arg::Flag),
    ("--directory", Arg::Flag),
    ("--error-unmatch", Arg::Flag),
    ("--exclude-standard", Arg::Flag),
    ("--full-name", Arg::Flag),
    ("--ignored", Arg::Flag),
    ("--modified", Arg::Flag),
    ("--others", Arg::Flag),
    ("--stage", Arg::Flag),
];
const GIT_BLAME_LONGS: &[(&str, Arg)] = &[
    ("--date", Arg::Plain),
    ("--line-porcelain", Arg::Flag),
    ("--porcelain", Arg::Flag),
    ("--root", Arg::Flag),
    ("--show-email", Arg::Flag),
    ("--show-name", Arg::Flag),
    ("--show-number", Arg::Flag),
];
const GIT_BLAME_SHORT: &[(char, Arg)] = &[('L', Arg::Plain)];
const GIT_REMOTE_LONGS: &[(&str, Arg)] = &[("--verbose", Arg::Flag)];

const CARGO_LONGS: &[(&str, Arg)] = &[
    ("--all", Arg::Flag),
    ("--all-features", Arg::Flag),
    ("--all-targets", Arg::Flag),
    ("--bench", Arg::Plain),
    ("--benches", Arg::Flag),
    ("--bin", Arg::Plain),
    ("--bins", Arg::Flag),
    ("--check", Arg::Flag),
    ("--color", Arg::Plain),
    ("--depth", Arg::Plain),
    ("--doc", Arg::Flag),
    ("--duplicates", Arg::Flag),
    ("--edges", Arg::Plain),
    ("--example", Arg::Plain),
    ("--examples", Arg::Flag),
    ("--exclude", Arg::Plain),
    ("--features", Arg::Plain),
    ("--format-version", Arg::Plain),
    ("--frozen", Arg::Flag),
    ("--invert", Arg::Plain),
    ("--jobs", Arg::Plain),
    ("--lib", Arg::Flag),
    ("--locked", Arg::Flag),
    ("--message-format", Arg::Plain),
    ("--no-capture", Arg::Flag),
    ("--no-default-features", Arg::Flag),
    ("--no-deps", Arg::Flag),
    ("--no-fail-fast", Arg::Flag),
    ("--no-run", Arg::Flag),
    ("--offline", Arg::Flag),
    ("--package", Arg::Plain),
    ("--prefix", Arg::Plain),
    ("--profile", Arg::Plain),
    ("--quiet", Arg::Flag),
    ("--release", Arg::Flag),
    ("--target", Arg::Plain),
    ("--test", Arg::Plain),
    ("--tests", Arg::Flag),
    ("--verbose", Arg::Flag),
    ("--workspace", Arg::Flag),
];
const CARGO_SHORT: &[(char, Arg)] = &[
    ('p', Arg::Plain),
    ('j', Arg::Plain),
    ('F', Arg::Plain),
    ('e', Arg::Plain),
    ('i', Arg::Plain),
    ('E', Arg::Plain),
];
const CARGO_FORBIDDEN_SHORT: &[char] = &['Z', 'C'];

const fn git(
    sub: &'static str,
    longs: &'static [(&'static str, Arg)],
    short_args: &'static [(char, Arg)],
    extra: Extra,
) -> Allowed {
    Allowed {
        name: "git",
        sub: Some(sub),
        longs,
        short_args,
        forbidden_short: GIT_FORBIDDEN_SHORT,
        paths: PathArgs::InsideProject,
        extra,
        project_build: false,
    }
}

/// Every `cargo` subcommand runs project-controlled code: `.cargo/config.toml`
/// can set `rustc-wrapper`, and `rust-toolchain.toml` can point at any binary.
const fn cargo(sub: &'static str, extra: Extra) -> Allowed {
    Allowed {
        name: "cargo",
        sub: Some(sub),
        longs: CARGO_LONGS,
        short_args: CARGO_SHORT,
        forbidden_short: CARGO_FORBIDDEN_SHORT,
        paths: PathArgs::InsideProject,
        extra,
        project_build: true,
    }
}

/// The Tier-1 allowlist.
const ALLOWLIST: &[Allowed] = &[
    tool("cat", CAT_LONGS, &[], &[]),
    tool("head", HEAD_TAIL_LONGS, HEAD_TAIL_SHORT, &[]),
    tool("tail", HEAD_TAIL_LONGS, HEAD_TAIL_SHORT, &['f', 'F']),
    tool("wc", WC_LONGS, &[], &[]),
    tool("ls", LS_LONGS, LS_SHORT, &[]),
    ignore("pwd"),
    ignore("echo"),
    ignore("which"),
    ignore("basename"),
    ignore("dirname"),
    tool("file", FILE_LONGS, FILE_SHORT, &['C', 'z', 'Z']),
    tool("stat", STAT_LONGS, &[('c', Arg::Plain)], &[]),
    tool("du", DU_LONGS, DU_SHORT, &['X']),
    tool("tree", TREE_LONGS, TREE_SHORT, &['o', 'H', 'R', 'T']),
    tool("diff", DIFF_LONGS, DIFF_SHORT, &['X']),
    tool("cmp", CMP_LONGS, CMP_SHORT, &[]),
    tool("realpath", REALPATH_LONGS, &[], &[]),
    tool("sort", SORT_LONGS, SORT_SHORT, &['o', 'T']),
    with_extra(
        tool("uniq", UNIQ_LONGS, UNIQ_SHORT, &[]),
        Extra::UniqSingleInput,
    ),
    tool("rg", RG_LONGS, RG_SHORT, &['z', 'L']),
    tool("grep", GREP_LONGS, GREP_SHORT, &[]),
    with_extra(tool("find", &[], &[], &[]), Extra::Find),
    git("status", GIT_STATUS_LONGS, GIT_STATUS_SHORT, Extra::None),
    git("diff", GIT_LOG_LONGS, GIT_LOG_SHORT, Extra::None),
    git("log", GIT_LOG_LONGS, GIT_LOG_SHORT, Extra::None),
    git("show", GIT_LOG_LONGS, GIT_LOG_SHORT, Extra::None),
    git("branch", GIT_BRANCH_LONGS, &[], Extra::GitBranchList),
    git("rev-parse", GIT_REV_PARSE_LONGS, &[], Extra::None),
    git("ls-files", GIT_LS_FILES_LONGS, &[], Extra::None),
    git("blame", GIT_BLAME_LONGS, GIT_BLAME_SHORT, Extra::None),
    git("remote", GIT_REMOTE_LONGS, &[], Extra::GitRemoteList),
    cargo("check", Extra::None),
    cargo("clippy", Extra::None),
    cargo("fmt", Extra::CargoFmtCheck),
    cargo("tree", Extra::None),
    cargo("metadata", Extra::None),
    cargo("build", Extra::None),
    cargo("test", Extra::None),
    cargo("nextest", Extra::None),
];

/// Global options accepted between `git` and its subcommand.
const GIT_GLOBAL_OK: &[&str] = &["--no-pager"];

/// `cargo` invocations that only print and never read project configuration.
const CARGO_INFO: &[&str] = &["--version", "-V", "--list"];

/// Whether `command` passes the Tier-1 read-only allowlist, with relative paths
/// resolved against the project root.
///
/// # Errors
///
/// Returns [`NotAllowlisted`] with a reason when any segment fails.
pub fn allowlisted(command: &str, ctx: &PolicyContext<'_>) -> Result<(), NotAllowlisted> {
    allowlisted_in(command, None, ctx)
}

/// Like [`allowlisted`], with an explicit working directory (the shell tool's
/// `working_dir`, relative to the project root). The working directory itself
/// must lie inside the project.
///
/// # Errors
///
/// Returns [`NotAllowlisted`] with a reason when any segment fails.
pub fn allowlisted_in(
    command: &str,
    working_dir: Option<&str>,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    if !ctx.posix_shell {
        return Err(NotAllowlisted::new(
            "shell analysis is disabled on non-POSIX shells",
        ));
    }
    let base = match working_dir {
        Some(dir) => {
            let resolved = ctx
                .root
                .resolve(dir)
                .map_err(|e| NotAllowlisted::new(format!("working directory: {e}")))?;
            if !resolved.inside() {
                return Err(NotAllowlisted::new(
                    "working directory is outside the project",
                ));
            }
            resolved.abs
        }
        None => ctx.root.as_path().to_path_buf(),
    };
    let segs = segments(lex(command)?);
    let mut any = false;
    for seg in &segs {
        if seg.terminator == Some(Op::Amp) {
            return Err(NotAllowlisted::new("background execution (`&`)"));
        }
        if seg.is_empty() {
            continue;
        }
        any = true;
        check_segment(seg, &base, ctx)?;
    }
    if any {
        Ok(())
    } else {
        Err(NotAllowlisted::new("empty command"))
    }
}

fn check_segment(
    seg: &Segment,
    base: &std::path::Path,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    for redirect in &seg.redirects {
        check_redirect(redirect, base, ctx)?;
    }
    let mut words: &[Word] = &seg.words;
    while let Some((first, rest)) = words.split_first() {
        if first.text == "command" || first.text == "builtin" {
            words = rest;
        } else {
            break;
        }
    }
    let Some((name, args)) = words.split_first() else {
        return Err(NotAllowlisted::new("redirect without a command"));
    };
    if name.unquoted_glob {
        return Err(NotAllowlisted::new("glob in command name"));
    }
    if is_assignment(&name.text) {
        return Err(NotAllowlisted::new("leading variable assignment"));
    }
    if WRAPPERS.contains(&name.text.as_str()) {
        return Err(NotAllowlisted::new(format!(
            "`{}` runs another command",
            name.text
        )));
    }
    let candidates: Vec<&Allowed> = ALLOWLIST.iter().filter(|a| a.name == name.text).collect();
    if candidates.is_empty() {
        return Err(NotAllowlisted::new(format!(
            "`{}` is not on the read-only allowlist",
            name.text
        )));
    }
    if name.text == "cargo"
        && let [only] = args
        && CARGO_INFO.contains(&only.text.as_str())
    {
        return Ok(());
    }

    let (entry, rest) = match_subcommand(&candidates, &name.text, args)?;
    check_entry(entry, rest, base, ctx)
}

/// Pick the allowlist entry for this invocation and return the args after the subcommand.
fn match_subcommand<'e, 'a>(
    candidates: &[&'e Allowed],
    name: &str,
    args: &'a [Word],
) -> Result<(&'e Allowed, &'a [Word]), NotAllowlisted> {
    let Some(first) = candidates.first() else {
        return Err(NotAllowlisted::new(format!("`{name}` is not allowlisted")));
    };
    if first.sub.is_none() {
        return Ok((first, args));
    }
    let mut idx = 0;
    if name == "git" {
        while args
            .get(idx)
            .is_some_and(|a| GIT_GLOBAL_OK.contains(&a.text.as_str()))
        {
            idx += 1;
        }
    }
    let Some(sub) = args.get(idx).map(|w| w.text.as_str()) else {
        return Err(NotAllowlisted::new(format!(
            "`{name}` without a subcommand"
        )));
    };
    if sub.starts_with('-') || sub.starts_with('+') {
        return Err(NotAllowlisted::new(format!(
            "`{name}` option `{sub}` before the subcommand"
        )));
    }
    let entry = candidates
        .iter()
        .find(|c| c.sub == Some(sub))
        .ok_or_else(|| {
            NotAllowlisted::new(format!("`{name} {sub}` is not on the read-only allowlist"))
        })?;
    Ok((entry, args.get(idx + 1..).unwrap_or(&[])))
}

/// A word (or the attached part of one) that must name a path inside the project.
#[derive(Clone, Copy)]
struct PathValue<'a> {
    text: &'a str,
    unquoted_glob: bool,
}

impl<'a> From<&'a Word> for PathValue<'a> {
    fn from(w: &'a Word) -> Self {
        Self {
            text: &w.text,
            unquoted_glob: w.unquoted_glob,
        }
    }
}

/// Arguments split by the command's option table.
#[derive(Default)]
struct Parsed<'a> {
    positionals: Vec<&'a Word>,
    path_values: Vec<PathValue<'a>>,
}

/// Walk `args` with the entry's option tables. Unknown long options, forbidden
/// short options and unexpected characters in short clusters are rejected.
fn parse_args<'a>(
    entry: &Allowed,
    label: &str,
    args: &'a [Word],
) -> Result<Parsed<'a>, NotAllowlisted> {
    let mut out = Parsed::default();
    let mut iter = args.iter();
    let mut options_done = false;
    while let Some(word) = iter.next() {
        let text = word.text.as_str();
        if options_done || text == "-" || !text.starts_with('-') {
            out.positionals.push(word);
            continue;
        }
        if text == "--" {
            options_done = true;
            continue;
        }
        let takes = if text.starts_with("--") {
            parse_long(entry, label, word, &mut out)?
        } else {
            parse_short(entry, label, word, &mut out)?
        };
        match takes {
            Some(Arg::Path) => {
                let value = iter.next().ok_or_else(|| {
                    NotAllowlisted::new(format!("`{label} {text}` without a value"))
                })?;
                out.path_values.push(value.into());
            }
            Some(Arg::Plain) => {
                let value = iter.next().ok_or_else(|| {
                    NotAllowlisted::new(format!("`{label} {text}` without a value"))
                })?;
                reject_option_glob(label, value)?;
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Check one `--long[=value]` option. Returns the argument kind when the value
/// is in the next word.
fn parse_long<'a>(
    entry: &Allowed,
    label: &str,
    word: &'a Word,
    out: &mut Parsed<'a>,
) -> Result<Option<Arg>, NotAllowlisted> {
    let text = word.text.as_str();
    let (name, value) = match text.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (text, None),
    };
    let Some(&(_, kind)) = entry.longs.iter().find(|(n, _)| *n == name) else {
        return Err(NotAllowlisted::new(format!(
            "`{label}` option `{name}` is not on the read-only allowlist"
        )));
    };
    match (kind, value) {
        (Arg::Plain | Arg::Optional, Some(_)) => {
            reject_option_glob(label, word)?;
            Ok(None)
        }
        (Arg::Path | Arg::Flag, Some(v)) => {
            out.path_values.push(PathValue {
                text: v,
                unquoted_glob: word.unquoted_glob,
            });
            Ok(None)
        }
        (Arg::Plain | Arg::Path, None) => Ok(Some(kind)),
        (Arg::Flag | Arg::Optional, None) => Ok(None),
    }
}

/// Check one `-abc` short-option cluster. Returns the argument kind when the
/// last option's value is in the next word.
fn parse_short<'a>(
    entry: &Allowed,
    label: &str,
    word: &'a Word,
    out: &mut Parsed<'a>,
) -> Result<Option<Arg>, NotAllowlisted> {
    let text = word.text.as_str();
    let cluster = text.get(1..).unwrap_or_default();
    for (idx, c) in cluster.char_indices() {
        if entry.forbidden_short.contains(&c) {
            return Err(NotAllowlisted::new(format!("`{label}` with `-{c}`")));
        }
        if let Some(&(_, kind)) = entry.short_args.iter().find(|(s, _)| *s == c) {
            let attached = cluster.get(idx + c.len_utf8()..).unwrap_or_default();
            if attached.is_empty() {
                return Ok(matches!(kind, Arg::Plain | Arg::Path).then_some(kind));
            }
            if kind == Arg::Path {
                out.path_values.push(PathValue {
                    text: attached,
                    unquoted_glob: word.unquoted_glob,
                });
            } else {
                reject_option_glob(label, word)?;
            }
            return Ok(None);
        }
        if !c.is_ascii_alphanumeric() {
            return Err(NotAllowlisted::new(format!(
                "`{label}` option `{text}` has an unexpected attached value"
            )));
        }
    }
    Ok(None)
}

/// Refuse an option value (`-k*`, `--exclude=*.x`, or the word after `-e`) that
/// carries an unquoted glob: its expansion is chosen by file names in the
/// working directory, so a file named `-oevil` could become an extra option.
fn reject_option_glob(label: &str, word: &Word) -> Result<(), NotAllowlisted> {
    if word.unquoted_glob {
        return Err(NotAllowlisted::new(format!(
            "`{label}` option value `{}` is an unquoted glob",
            word.text
        )));
    }
    Ok(())
}

fn check_entry(
    entry: &Allowed,
    args: &[Word],
    base: &std::path::Path,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    let label = match entry.sub {
        Some(sub) => format!("{} {sub}", entry.name),
        None => entry.name.to_string(),
    };
    if entry.project_build && !ctx.trust_project_builds {
        return Err(NotAllowlisted::new(format!(
            "`{label}` executes project code and trust_project_builds is off"
        )));
    }
    if entry.paths == PathArgs::Ignore {
        return Ok(());
    }
    if entry.extra == Extra::Find {
        return check_find(args, base, ctx);
    }

    let parsed = parse_args(entry, &label, args)?;
    match entry.extra {
        Extra::None | Extra::Find => {}
        Extra::UniqSingleInput => {
            if parsed.positionals.len() > 1 {
                return Err(NotAllowlisted::new("`uniq` with an output file"));
            }
        }
        Extra::GitBranchList => check_git_branch(args)?,
        Extra::GitRemoteList => {
            if args
                .iter()
                .any(|a| !matches!(a.text.as_str(), "-v" | "--verbose"))
            {
                return Err(NotAllowlisted::new("`git remote` beyond listing"));
            }
        }
        Extra::CargoFmtCheck => {
            if !args.iter().any(|a| a.text == "--check") {
                return Err(NotAllowlisted::new("`cargo fmt` without `--check`"));
            }
        }
    }

    for value in parsed
        .positionals
        .iter()
        .map(|w| PathValue::from(*w))
        .chain(parsed.path_values.iter().copied())
    {
        check_path_value(value, base, ctx)?;
    }
    Ok(())
}

/// `find` primaries accepted in Tier 1, with whether they take an argument.
const FIND_PRIMARIES: &[(&str, Arg)] = &[
    ("-name", Arg::Plain),
    ("-iname", Arg::Plain),
    ("-path", Arg::Plain),
    ("-type", Arg::Plain),
    ("-maxdepth", Arg::Plain),
    ("-mindepth", Arg::Plain),
    ("-newer", Arg::Path),
    ("-size", Arg::Plain),
    ("-print", Arg::Flag),
    ("-print0", Arg::Flag),
    ("-prune", Arg::Flag),
    ("-not", Arg::Flag),
    ("!", Arg::Flag),
    ("-o", Arg::Flag),
    ("-a", Arg::Flag),
    ("(", Arg::Flag),
    (")", Arg::Flag),
];

/// `find PATH... EXPRESSION`: starting points must be inside the project and the
/// expression may only use [`FIND_PRIMARIES`].
fn check_find(
    args: &[Word],
    base: &std::path::Path,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    let mut iter = args.iter();
    let mut in_expression = false;
    while let Some(word) = iter.next() {
        let text = word.text.as_str();
        let is_primary = text.starts_with('-') || matches!(text, "!" | "(" | ")");
        if !in_expression && !is_primary {
            check_path_value(word.into(), base, ctx)?;
            continue;
        }
        in_expression = true;
        reject_option_glob("find", word)?;
        let Some(&(_, kind)) = FIND_PRIMARIES.iter().find(|(p, _)| *p == text) else {
            return Err(NotAllowlisted::new(format!(
                "`find` with `{text}` is not on the read-only allowlist"
            )));
        };
        match kind {
            Arg::Flag | Arg::Optional => {}
            Arg::Plain | Arg::Path => {
                let value = iter
                    .next()
                    .ok_or_else(|| NotAllowlisted::new(format!("`find {text}` without a value")))?;
                reject_option_glob("find", value)?;
                if kind == Arg::Path {
                    check_path_value(value.into(), base, ctx)?;
                }
            }
        }
    }
    Ok(())
}

fn check_git_branch(args: &[Word]) -> Result<(), NotAllowlisted> {
    const MUTATING_SHORT: &[char] = &['d', 'D', 'm', 'M', 'c', 'C', 'f', 'u', 't'];
    const LISTING: &[&str] = &[
        "-l",
        "--list",
        "--contains",
        "--no-contains",
        "--merged",
        "--no-merged",
        "--points-at",
    ];
    for arg in args.iter().map(|w| w.text.as_str()) {
        if is_short_cluster(arg) && arg.chars().skip(1).any(|c| MUTATING_SHORT.contains(&c)) {
            return Err(NotAllowlisted::new(format!("`git branch {arg}` mutates")));
        }
    }
    let listing = args.iter().any(|a| {
        LISTING
            .iter()
            .any(|l| a.text == *l || a.text.starts_with(&format!("{l}=")))
    });
    let has_positional = args
        .iter()
        .any(|a| !(a.text.starts_with('-') && a.text.len() > 1));
    if has_positional && !listing {
        return Err(NotAllowlisted::new("`git branch <name>` creates a branch"));
    }
    Ok(())
}

/// Single-dash short-flag cluster such as `-o` or `-nru` (not `--long`, not `-`).
fn is_short_cluster(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--")
}

/// Whether an unquoted glob word has the narrow `dir/[**/]*.<ext>` shape that
/// can only match ordinary files below a literal directory.
///
/// Returns the literal directory prefix on success. The prefix is never empty:
/// a bare `*.md` or `**/*.md` expands to file names chosen by whoever created
/// the files, and a file named `-oevil.md` would turn into an option. A leading
/// `./` counts as a literal prefix (`./*.md` expands to `./-oevil.md`, which is
/// never an option); `.` is accepted only as the first component.
fn simple_glob_prefix(text: &str) -> Option<String> {
    let mut parts: Vec<&str> = text.split('/').collect();
    let last = parts.pop()?;
    let ext = last.strip_prefix("*.")?;
    if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let mut prefix = Vec::new();
    let mut seen_globstar = false;
    for (idx, part) in parts.into_iter().enumerate() {
        if part == "**" {
            if prefix.is_empty() {
                return None;
            }
            seen_globstar = true;
            continue;
        }
        let leading_dot = idx == 0 && part == ".";
        if seen_globstar
            || part.is_empty()
            || (part == "." && !leading_dot)
            || part == ".."
            || part.contains(['*', '?', '['])
        {
            return None;
        }
        prefix.push(part);
    }
    if prefix.is_empty() {
        return None;
    }
    Some(prefix.join("/"))
}

/// Check a path-position word: no `~user` forms, only narrow globs, and the
/// resolved path must lie inside the project.
fn check_path_value(
    value: PathValue<'_>,
    base: &std::path::Path,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    let text = value.text;
    if text.starts_with('~') && text != "~" && !text.starts_with("~/") {
        return Err(NotAllowlisted::new(format!(
            "path `{text}` uses `~user` expansion"
        )));
    }
    if value.unquoted_glob {
        let Some(prefix) = simple_glob_prefix(text) else {
            return Err(NotAllowlisted::new(format!(
                "glob `{text}` is not of the form `dir/*.ext`, `dir/**/*.ext` or `./*.ext`"
            )));
        };
        return check_inside(&prefix, base, ctx);
    }
    check_inside(text, base, ctx)
}

fn check_inside(
    value: &str,
    base: &std::path::Path,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    if value == "-" {
        return Ok(());
    }
    let candidate: PathBuf = if value.starts_with('/') || value == "~" || value.starts_with("~/") {
        PathBuf::from(value)
    } else if value.starts_with('~') {
        return Err(NotAllowlisted::new(format!(
            "path `{value}` uses `~user` expansion"
        )));
    } else {
        base.join(value)
    };
    let Some(as_str) = candidate.to_str() else {
        return Err(NotAllowlisted::new("non UTF-8 path argument"));
    };
    let resolved = ctx
        .root
        .resolve(as_str)
        .map_err(|e| NotAllowlisted::new(format!("path `{value}`: {e}")))?;
    if resolved.inside() {
        Ok(())
    } else {
        Err(NotAllowlisted::new(format!(
            "path `{value}` is outside the project"
        )))
    }
}

fn check_redirect(
    redirect: &Redirect,
    base: &std::path::Path,
    ctx: &PolicyContext<'_>,
) -> Result<(), NotAllowlisted> {
    let ok = match (redirect.kind, redirect.fd) {
        (RedirectKind::DupOut, Some(2)) => redirect.target == "1",
        (RedirectKind::Out, None | Some(1 | 2)) => redirect.target == "/dev/null",
        (RedirectKind::In, None | Some(0)) => {
            if redirect.target_glob {
                return Err(NotAllowlisted::new(format!(
                    "glob in redirect target {redirect}"
                )));
            }
            check_path_value(
                PathValue {
                    text: &redirect.target,
                    unquoted_glob: false,
                },
                base,
                ctx,
            )?;
            true
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(NotAllowlisted::new(format!("redirect {redirect}")))
    }
}

impl fmt::Display for Redirect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(fd) = self.fd {
            write!(f, "{fd}")?;
        }
        let op = match self.kind {
            RedirectKind::In => "<",
            RedirectKind::Out => ">",
            RedirectKind::Append => ">>",
            RedirectKind::Clobber => ">|",
            RedirectKind::DupOut => ">&",
            RedirectKind::DupIn => "<&",
            RedirectKind::ReadWrite => "<>",
            RedirectKind::OutErr => "&>",
        };
        write!(f, "{op}{}", self.target)
    }
}

/// Whether `word` is a `NAME=value` variable assignment.
pub(crate) fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Interpreters whose first positional argument is a script file.
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "python", "python3", "node", "ruby", "perl", "deno", "bun",
];

/// If the command runs a script file (`./x.sh`, `bash x.sh`, `python3 tool.py`),
/// return the script path as written. Used to attach in-project script content to
/// the classifier payload. Returns `None` when the command cannot be lexed.
pub fn script_path(command: &str) -> Option<String> {
    let segs = segments(lex(command).ok()?);
    for seg in segs {
        let mut words = seg.words.iter().map(|w| w.text.as_str());
        let Some(name) = words.next() else {
            continue;
        };
        if name.contains('/') {
            return Some(name.to_string());
        }
        let base = name.rsplit('/').next().unwrap_or(name);
        if INTERPRETERS.contains(&base) {
            // `deno run x.ts`, `bun run x.ts`
            let found = words.find(|w| !w.starts_with('-') && *w != "run");
            if let Some(path) = found {
                return Some(path.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::paths::ProjectRoot;

    fn project() -> (tempfile::TempDir, ProjectRoot) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("input.txt"), "x\n").unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        (dir, root)
    }

    fn ctx(root: &ProjectRoot, trust: bool) -> PolicyContext<'_> {
        PolicyContext {
            root,
            trust_project_builds: trust,
            posix_shell: true,
        }
    }

    fn words(cmd: &str) -> Vec<String> {
        lex(cmd)
            .unwrap()
            .into_iter()
            .filter_map(|t| match t {
                Tok::Word(w) => Some(w.text),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn lexer_quotes_and_escapes() {
        assert_eq!(
            words("echo 'a b' \"c d\" e\\ f"),
            ["echo", "a b", "c d", "e f"]
        );
        assert_eq!(words("echo 'a$b'"), ["echo", "a$b"]);
        assert_eq!(words(r#"echo "a\"b""#), ["echo", "a\"b"]);
        assert_eq!(words(r#"echo "a\nb""#), ["echo", "a\\nb"]);
        assert_eq!(words("echo ''"), ["echo", ""]);
        assert_eq!(words("ls # comment $(x)"), ["ls"]);
        assert_eq!(words("echo a#b"), ["echo", "a#b"]);
    }

    #[test]
    fn lexer_operators() {
        let toks = lex("a && b || c; d | e & f\ng |& h").unwrap();
        let ops: Vec<Op> = toks
            .iter()
            .filter_map(|t| match t {
                Tok::Op(o) => Some(*o),
                _ => None,
            })
            .collect();
        assert_eq!(
            ops,
            [
                Op::AndAnd,
                Op::OrOr,
                Op::Semi,
                Op::Pipe,
                Op::Amp,
                Op::Newline,
                Op::Pipe
            ]
        );
    }

    #[test]
    fn lexer_redirects() {
        let toks = lex("cmd 2>&1 >/dev/null 2>/dev/null <in.txt >>log &>all 3<>rw").unwrap();
        let redirects: Vec<Redirect> = toks
            .into_iter()
            .filter_map(|t| match t {
                Tok::Redirect(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(redirects.len(), 7);
        assert_eq!(
            redirects[0],
            Redirect {
                fd: Some(2),
                kind: RedirectKind::DupOut,
                target: "1".into(),
                target_glob: false,
            }
        );
        assert_eq!(redirects[1].kind, RedirectKind::Out);
        assert_eq!(redirects[1].fd, None);
        assert_eq!(redirects[2].fd, Some(2));
        assert_eq!(redirects[3].kind, RedirectKind::In);
        assert_eq!(redirects[4].kind, RedirectKind::Append);
        assert_eq!(redirects[5].kind, RedirectKind::OutErr);
        assert_eq!(redirects[6].kind, RedirectKind::ReadWrite);
        assert_eq!(redirects[6].fd, Some(3));
    }

    #[test]
    fn quoted_digits_are_not_fds() {
        let toks = lex("echo '2'>x").unwrap();
        assert!(toks.contains(&Tok::Word(Word {
            text: "2".into(),
            unquoted_glob: false
        })));
    }

    #[test]
    fn lexer_unsupported() {
        for cmd in [
            "echo $(id)",
            "echo `id`",
            "echo $HOME",
            "echo \"$HOME\"",
            "echo \"`id`\"",
            "diff <(ls) <(ls a)",
            "tee >(cat)",
            "(ls)",
            "{ ls; }",
            "echo {a,b}",
            "cat <<EOF",
            "cat <<<word",
            "echo 'unterminated",
            "echo \"unterminated",
            "echo trailing\\",
            "ls >",
            "case x in a) ;; esac",
        ] {
            assert!(lex(cmd).is_err(), "{cmd} should be unsupported");
        }
    }

    #[test]
    fn segments_split_on_ops() {
        let segs = segments(lex("ls -la && cat a | rg x; pwd").unwrap());
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0].terminator, Some(Op::AndAnd));
        assert_eq!(segs[1].terminator, Some(Op::Pipe));
        assert_eq!(segs[2].terminator, Some(Op::Semi));
        assert_eq!(segs[3].terminator, None);
        assert_eq!(segs[3].words[0].text, "pwd");
    }

    #[test]
    fn allowlist_table() {
        let (dir, root) = project();
        let outside = dir.path().parent().unwrap().join("elsewhere.txt");
        let outside_cat = format!("cat {}", outside.display());
        let inside_abs = format!("cat {}", dir.path().join("src/main.rs").display());
        let cases: Vec<(&str, bool)> = vec![
            // Plan examples
            ("ls && rm -rf /", false),
            ("git -c core.pager=sh status", false),
            ("cat a | rg x", true),
            ("echo $(id)", false),
            ("find . -delete", false),
            ("cargo test 2>&1", true),
            // Simple read-only commands
            ("ls", true),
            ("ls -la src", true),
            ("pwd", true),
            ("echo hello world", true),
            ("echo /etc/passwd", true),
            ("which cargo", true),
            ("cat src/main.rs", true),
            ("head -n 20 src/main.rs", true),
            ("tail -n 5 src/main.rs", true),
            ("wc -l src/main.rs", true),
            ("file src/main.rs", true),
            ("file -C -m magic", false),
            ("stat src/main.rs", true),
            ("du -sh src", true),
            ("tree src", true),
            ("tree -o out.txt", false),
            ("diff src/main.rs input.txt", true),
            ("cmp src/main.rs input.txt", true),
            ("basename /a/b/c", true),
            ("dirname /a/b/c", true),
            ("realpath src", true),
            ("sort input.txt", true),
            ("sort -o out.txt input.txt", false),
            ("sort -uo out.txt input.txt", false),
            ("sort --output=out.txt input.txt", false),
            ("uniq input.txt", true),
            ("uniq input.txt out.txt", false),
            ("rg TODO src", true),
            ("rg --pre cat TODO", false),
            ("rg --pre-glob '*.gz' TODO", false),
            ("grep -rn foo src", true),
            ("find . -name '*.rs'", true),
            ("find . -exec rm {} \\;", false),
            ("find . -execdir ls ';'", false),
            ("find . -ok rm ';'", false),
            ("find . -fprint out", false),
            ("find . -fls out", false),
            ("find / -name passwd", false),
            // Paths
            ("cat ../secret", false),
            ("cat ~/.ssh/id_rsa", false),
            ("cat /etc/passwd", false),
            (outside_cat.as_str(), false),
            (inside_abs.as_str(), true),
            ("rg -f /etc/patterns x", false),
            ("ls --directory=/etc", false),
            // Chains
            ("ls && pwd", true),
            ("ls || pwd", true),
            ("ls; pwd", true),
            ("ls\npwd", true),
            ("ls;", true),
            ("cat a | sort | uniq", true),
            ("ls | xargs rm", false),
            ("ls & pwd", false),
            ("ls &", false),
            ("", false),
            ("   ", false),
            // Redirects
            ("ls >/dev/null", true),
            ("ls 2>/dev/null", true),
            ("ls >/dev/null 2>&1", true),
            ("ls > out.txt", false),
            ("ls >> out.txt", false),
            ("ls 1>&2", false),
            ("ls &>/dev/null", false),
            ("cat < input.txt", true),
            ("cat < /etc/passwd", false),
            // Prefixes and wrappers
            ("FOO=bar ls", false),
            ("command ls", true),
            ("builtin echo hi", true),
            ("sudo ls", false),
            ("env ls", false),
            ("exec ls", false),
            ("eval ls", false),
            ("nohup ls", false),
            ("timeout 5 ls", false),
            ("nice ls", false),
            ("time ls", false),
            ("l* src", false),
            ("/bin/ls", false),
            ("./script.sh", false),
            ("rm -rf target", false),
            ("python3 -c 'print(1)'", false),
            // git
            ("git status", true),
            ("git --no-pager log --oneline -5", true),
            ("git diff HEAD~1", true),
            ("git diff --ext-diff", false),
            ("git diff --output=patch.txt", false),
            ("git log -o x", false),
            ("git show HEAD:src/main.rs", true),
            ("git branch", true),
            ("git branch -a", true),
            ("git branch -D main", false),
            ("git branch newbranch", false),
            ("git branch --contains abc123", true),
            ("git rev-parse HEAD", true),
            ("git ls-files", true),
            ("git blame src/main.rs", true),
            ("git remote -v", true),
            ("git remote add evil https://x", false),
            ("git push --force", false),
            ("git commit -m x", false),
            ("git -C /tmp status", false),
            ("git", false),
            // cargo
            ("cargo fmt --check", true),
            ("cargo fmt", false),
            ("cargo tree", true),
            ("cargo metadata --format-version 1", true),
            ("cargo check --workspace", true),
            ("cargo clippy --fix", false),
            ("cargo check --config build.rustc-wrapper=x", false),
            ("cargo +nightly check", false),
            ("cargo build --release", true),
            ("cargo nextest run", true),
            ("cargo run", false),
            ("cargo install ripgrep", false),
            ("cargo build --target-dir /tmp/x", false),
            ("cargo --version", true),
            ("cargo --list", true),
            ("cargo check 2>&1", true),
            ("cargo test", true),
            ("cargo test -- --nocapture", true),
            ("cargo clippy --all-targets -- -D warnings", true),
            ("cargo -Zunstable-options check", false),
            ("cargo check -Zbuild-std", false),
            ("cargo check --manifest-path /tmp/Cargo.toml", false),
            // GNU long-option abbreviations and unknown long options
            ("sort --compress-prog=./x.sh big", false),
            ("sort --outp=src/main.rs big", false),
            ("file --comp x", false),
            ("rg --hostname-bin=./x .", false),
            ("rg --hyperlink-format=x .", false),
            ("rg --pr cat x", false),
            ("cat --unknown src/main.rs", false),
            ("git log --out=x", false),
            ("git diff --ext", false),
            ("wc --files0-from=list", false),
            // Other writers
            ("tree -H .", false),
            ("tree -fH . src", false),
            ("sort -T /tmp x", false),
            ("sort -rT/tmp x", false),
            ("tail -f input.txt", false),
            ("find . -exec rm {} ;", false),
            ("find . -name '*.rs' -newer src/main.rs", true),
            ("find . -newer /etc/passwd", false),
            ("find . -type f -name x -print0", true),
            ("find . -fprintf out x", false),
            ("find -L . -name x", false),
            ("find . -maxdepth 2 -not -path './target/*'", true),
            // Tilde and globs
            ("cat ~root/.bashrc", false),
            ("cat ~x/.aws/credentials", false),
            ("cat .*/.*/etc/passwd", false),
            ("cat *", false),
            ("cat ?", false),
            ("ls *.rs", false),
            ("ls src/*.rs", true),
            ("ls **/*.rs", false),
            ("ls src/**/*.rs", true),
            ("ls ../*.rs", false),
            ("ls ./*.rs", true),
            ("ls ./**/*.rs", true),
            ("ls src/./*.rs", false),
            ("ls /etc/*.conf", false),
            ("cat < *", false),
            ("cat '*'", true),
            // Attached option values
            ("grep -f/etc/shadow -r .", false),
            ("grep -rf/etc/shadow .", false),
            ("grep --file=/etc/shadow -r .", false),
            ("grep -f /etc/shadow -r .", false),
            ("grep -e foo -r src", true),
            ("grep -e/etc/x -r src", true),
            ("git log -O/abs", false),
            ("git log -O /abs", false),
            ("git log -p -n5", true),
            ("git log --format='%h %s' -n 3", true),
            ("ls -x/etc", false),
            ("head -n20 src/main.rs", true),
            ("head -20 src/main.rs", true),
            ("sort -t / -k2 input.txt", true),
            ("rg -n foo src", true),
            ("rg -g '*.rs' foo", true),
            ("rg -n --glob '!target' foo src", true),
            ("grep -rn x src", true),
            ("ls -la", true),
            ("head -n 20 Cargo.toml", true),
            ("git log --oneline -5", true),
            ("git status --porcelain=v2", true),
            ("git status -uno", true),
            ("uniq -f 1 input.txt", true),
        ];
        let c = ctx(&root, true);
        let mut failures = Vec::new();
        for (cmd, expected) in &cases {
            let got = allowlisted(cmd, &c);
            if got.is_ok() != *expected {
                failures.push(format!("{cmd:?}: expected {expected}, got {got:?}"));
            }
        }
        assert!(cases.len() >= 60, "table has {} cases", cases.len());
        assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
    }

    #[test]
    fn project_builds_require_trust() {
        let (_dir, root) = project();
        let untrusted = ctx(&root, false);
        for cmd in [
            "cargo test",
            "cargo build",
            "cargo nextest run",
            "cargo check",
            "cargo clippy",
        ] {
            assert!(allowlisted(cmd, &untrusted).is_err(), "{cmd}");
        }
        for cmd in [
            "cargo fmt --check",
            "cargo tree",
            "cargo metadata",
            "cargo metadata --format-version 1",
            "cargo check 2>&1",
        ] {
            assert!(allowlisted(cmd, &untrusted).is_err(), "{cmd}");
        }
        assert!(allowlisted("cargo --version", &untrusted).is_ok());
        assert!(allowlisted("cargo --list", &untrusted).is_ok());
        assert!(allowlisted("cargo --version check", &untrusted).is_err());
        assert!(allowlisted("cat src/main.rs", &untrusted).is_ok());
    }

    #[test]
    fn non_posix_shell_never_allowlists() {
        let (_dir, root) = project();
        let c = PolicyContext {
            root: &root,
            trust_project_builds: true,
            posix_shell: false,
        };
        assert!(allowlisted("ls", &c).is_err());
    }

    #[test]
    fn working_dir_must_be_inside() {
        let (_dir, root) = project();
        let c = ctx(&root, true);
        assert!(allowlisted_in("ls", Some("src"), &c).is_ok());
        assert!(allowlisted_in("ls", Some("/etc"), &c).is_err());
        assert!(allowlisted_in("cat ../input.txt", Some("src"), &c).is_ok());
        assert!(allowlisted_in("cat ../../x", Some("src"), &c).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_argument_escaping_is_rejected() {
        let (dir, root) = project();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("s"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("s"), dir.path().join("link")).unwrap();
        assert!(allowlisted("cat link", &ctx(&root, true)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn dotdot_through_symlinked_directory_is_rejected() {
        let (dir, root) = project();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("inner")).unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("inner"), dir.path().join("lnk")).unwrap();
        let c = ctx(&root, true);
        assert!(allowlisted("cat lnk/../secret", &c).is_err());
        assert!(allowlisted("cat src/../input.txt", &c).is_ok());
    }

    #[test]
    fn unprefixed_globs_are_refused() {
        let (dir, root) = project();
        // `sort *.md` with a file named `-oevil.md` expands to `sort -oevil.md`.
        std::fs::write(dir.path().join("-oevil.md"), "x").unwrap();
        std::fs::write(dir.path().join("a.md"), "x").unwrap();
        let c = ctx(&root, true);
        assert!(allowlisted("sort *.md", &c).is_err());
        assert!(allowlisted("sort **/*.md", &c).is_err());
        assert!(allowlisted("sort ./*.md", &c).is_ok());
        assert!(allowlisted("ls src/*.rs", &c).is_ok());
        assert!(allowlisted("ls src/**/*.rs", &c).is_ok());
    }

    #[test]
    fn option_value_globs_are_refused() {
        let (_dir, root) = project();
        let c = ctx(&root, true);
        // Plain values, attached (short and long) and in the next word.
        assert!(allowlisted("sort -k* input.txt", &c).is_err());
        assert!(allowlisted("sort -k *.txt input.txt", &c).is_err());
        assert!(allowlisted("sort --key=* input.txt", &c).is_err());
        assert!(allowlisted("grep -r --exclude=*.x foo src", &c).is_err());
        assert!(allowlisted("grep -r --exclude='*.x' foo src", &c).is_ok());
        assert!(allowlisted("sort -k2 input.txt", &c).is_ok());
    }

    #[test]
    fn find_expression_globs_are_refused() {
        let (_dir, root) = project();
        let c = ctx(&root, true);
        assert!(allowlisted("find . -name *.rs", &c).is_err());
        assert!(allowlisted("find . -path ./src/*", &c).is_err());
        assert!(allowlisted("find . -name '*.rs'", &c).is_ok());
        assert!(allowlisted("find src/*.rs -name x", &c).is_ok());
        assert!(allowlisted("find *.rs -name x", &c).is_err());
    }

    #[test]
    fn fd_dup_to_a_file_is_refused() {
        let (_dir, root) = project();
        let c = ctx(&root, true);
        assert!(allowlisted("echo hi >&out.txt", &c).is_err());
        assert!(allowlisted("echo hi >& out.txt", &c).is_err());
        assert!(allowlisted("echo hi 2>&out.txt", &c).is_err());
        assert!(allowlisted("echo hi 1>&2", &c).is_err());
        assert!(allowlisted("echo hi 2>&1", &c).is_ok());
    }

    #[test]
    fn script_paths() {
        assert_eq!(
            script_path("./build.sh --fast").as_deref(),
            Some("./build.sh")
        );
        assert_eq!(
            script_path("bash scripts/x.sh").as_deref(),
            Some("scripts/x.sh")
        );
        assert_eq!(
            script_path("python3 -u tool.py").as_deref(),
            Some("tool.py")
        );
        assert_eq!(script_path("deno run app.ts").as_deref(), Some("app.ts"));
        assert_eq!(script_path("ls -la"), None);
        assert_eq!(script_path("echo $(x)"), None);
    }
}
