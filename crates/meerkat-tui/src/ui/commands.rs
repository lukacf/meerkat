//! Slash-command parsing.

use std::fmt;

/// Whether a rule allows or denies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleEffect {
    /// Allow matching calls without review.
    Allow,
    /// Deny matching calls.
    Deny,
}

/// `/rules` subcommands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesCommand {
    /// List the active rules.
    List,
    /// Add a rule.
    Add {
        /// Allow or deny.
        effect: RuleEffect,
        /// Tool name.
        tool: String,
        /// Optional argument pattern (rest of the line).
        pattern: Option<String>,
    },
    /// Remove rule number `n` (as shown by `/rules list`).
    Remove(usize),
}

/// `/auto` subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCommand {
    /// Show the auto-mode state.
    Status,
    /// Enable auto mode.
    On,
    /// Disable auto mode (every non-trivial call escalates).
    Off,
    /// Resume after a denial-limit pause.
    Resume,
}

/// `/model` subcommands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCommand {
    /// `/model`: open the picker (from the cached provider list).
    Pick,
    /// `/model refresh`: refetch the provider list, then open the picker.
    Refresh,
    /// `/model <id>`: switch to this model.
    Set(String),
}

/// A parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `/quit`, `/exit`.
    Quit,
    /// `/clear`: clear the transcript and start a new session.
    Clear,
    /// `/resume [ref]`.
    Resume(Option<String>),
    /// `/rules ...`.
    Rules(RulesCommand),
    /// `/auto [on|off|resume]`.
    Auto(AutoCommand),
    /// `/model [id | refresh]`.
    Model(ModelCommand),
    /// `/help`.
    Help,
    /// `/remember <text>` or a single-line `#text` input: append a line to the
    /// workspace AGENTS.md.
    Remember(String),
    /// `/compact`: summarise the session's context.
    Compact,
    /// `/cost`: the session's token cost by model.
    Cost,
}

/// Slash-command parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// The command name is not known.
    Unknown(String),
    /// The command is known but its arguments are wrong.
    Usage(&'static str),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(f, "unknown command /{name} (try /help)"),
            Self::Usage(usage) => write!(f, "usage: {usage}"),
        }
    }
}

const RULES_USAGE: &str = "/rules [list | add allow|deny <tool> [pattern] | rm <n>]";
const AUTO_USAGE: &str = "/auto [on | off | resume]";

/// Help text shown by `/help`: every slash command and key binding.
pub const HELP_TEXT: &str = "\
Commands:
  /help, /?                      show this help
  /quit, /exit, /q               leave rkat-tui (asks again while a turn runs or messages are queued)
  /clear, /new                   clear the transcript and start a new session (idle only)
  /resume [ref]                  resume a session (last, ~N, or id prefix); no ref opens a picker
  /rules [list]                  list approval rules
  /rules add allow|deny <tool> [pattern]
                                 add an approval rule
  /rules rm <n>                  remove rule n
  /auto                          show auto-mode state and denial counters
  /auto on|off|resume            enable, disable, or resume auto mode after a pause
  /model                         pick a model from every endpoint's list (type to filter)
  /model [endpoint:]<id>         switch to a model (idle only); a bare id is looked up
  /model refresh                 refetch every endpoint's model list and pick
  /remember <text>, #<text>      add a line to the workspace AGENTS.md (\\# sends a literal #)
  /compact                       summarise the conversation to free context (idle only)
                                 (tui.toml [auto] compact = \"warn\" (default) warns at 75%;
                                 \"auto\" or --auto-compact runs it at 85%)
  /cost                          token usage and cost per model (footer: $0.42, ~ estimate, $? unpriced)
Keys:
  Enter                          send; while the agent is working, queues the message
                                 and delivers it at the next step
  Alt+Enter, Shift+Enter, Ctrl+J insert a newline
  Ctrl-C                         interrupt the turn · clear the input · press twice to quit
  Esc                            recall queued messages (interrupts when nothing is queued)
  Up (empty input)               edit the newest queued message
  Ctrl-D                         quit (on empty input; press again within 3 s while a turn
                                 runs or messages are queued: the turn is interrupted and
                                 undelivered messages are saved)
  Ctrl-O                         toggle the last reasoning block
  Ctrl-R                         search history (Ctrl-R again: older · Enter accept · Esc cancel)
  Ctrl-X Ctrl-S                  interrupt and send the input (sends normally when idle)
  Up/Down                        move between lines · recall prompt history
  PgUp/PgDn, Shift+Up/Down       scroll the transcript
  Home/End                       scroll to top/bottom when the input is empty (Ctrl+Home/End always)
Approval prompt:
  y allow once · a allow and add a rule · n/Esc deny · s stop the turn (Ctrl-C stops all)
Start a prompt with // to send a literal leading slash.";

/// A single-line `#text` input (not `##…`) as a remember request: returns the
/// text after `#`, trimmed. Multi-line input and `\#` escapes are prompts.
#[must_use]
pub fn parse_remember(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.contains('\n') || trimmed.starts_with("##") {
        return None;
    }
    let text = trimmed.strip_prefix('#')?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Undo a leading `\#` escape (`\#foo` sends the prompt `#foo`).
#[must_use]
pub fn unescape_hash(input: &str) -> Option<String> {
    let start = input.len() - input.trim_start().len();
    let rest = input.get(start..)?.strip_prefix("\\#")?;
    Some(format!("#{rest}"))
}

/// Parse a submitted line. Returns `None` when the line is not a slash command
/// (no leading `/`, or a `//` escape).
pub fn parse(input: &str) -> Option<Result<Command, CommandError>> {
    let trimmed = input.trim();
    let body = trimmed.strip_prefix('/')?;
    if body.starts_with('/') {
        return None;
    }
    let mut words = body.split_whitespace();
    let name = words.next().unwrap_or_default();
    if name == "remember" {
        // Keep the text verbatim (inner whitespace included).
        let text = body
            .trim_start()
            .get(name.len()..)
            .unwrap_or_default()
            .trim();
        return Some(if text.is_empty() {
            Err(CommandError::Usage("/remember <text>"))
        } else {
            Ok(Command::Remember(text.to_string()))
        });
    }
    let args: Vec<&str> = words.collect();
    Some(parse_command(name, &args))
}

fn parse_command(name: &str, args: &[&str]) -> Result<Command, CommandError> {
    match name {
        "quit" | "exit" | "q" => Ok(Command::Quit),
        "clear" | "new" => Ok(Command::Clear),
        "help" | "?" => Ok(Command::Help),
        "cost" => match args {
            [] => Ok(Command::Cost),
            _ => Err(CommandError::Usage("/cost")),
        },
        "compact" => match args {
            [] => Ok(Command::Compact),
            _ => Err(CommandError::Usage("/compact")),
        },
        "resume" => match args {
            [] => Ok(Command::Resume(None)),
            [r] => Ok(Command::Resume(Some((*r).to_string()))),
            _ => Err(CommandError::Usage("/resume [last | ~N | id-prefix]")),
        },
        "model" => match args {
            [] => Ok(Command::Model(ModelCommand::Pick)),
            ["refresh"] => Ok(Command::Model(ModelCommand::Refresh)),
            [m] => Ok(Command::Model(ModelCommand::Set((*m).to_string()))),
            _ => Err(CommandError::Usage("/model [[endpoint:]id | refresh]")),
        },
        "auto" => match args {
            [] => Ok(Command::Auto(AutoCommand::Status)),
            ["on"] => Ok(Command::Auto(AutoCommand::On)),
            ["off"] => Ok(Command::Auto(AutoCommand::Off)),
            ["resume"] => Ok(Command::Auto(AutoCommand::Resume)),
            _ => Err(CommandError::Usage(AUTO_USAGE)),
        },
        "rules" => parse_rules(args).map(Command::Rules),
        other => Err(CommandError::Unknown(other.to_string())),
    }
}

fn parse_rules(args: &[&str]) -> Result<RulesCommand, CommandError> {
    match args {
        [] | ["list"] => Ok(RulesCommand::List),
        ["add", effect, tool, pattern @ ..] => {
            let effect = match *effect {
                "allow" => RuleEffect::Allow,
                "deny" => RuleEffect::Deny,
                _ => return Err(CommandError::Usage(RULES_USAGE)),
            };
            let pattern = (!pattern.is_empty()).then(|| pattern.join(" "));
            Ok(RulesCommand::Add {
                effect,
                tool: (*tool).to_string(),
                pattern,
            })
        }
        ["rm" | "remove", n] => n
            .parse::<usize>()
            .map(RulesCommand::Remove)
            .map_err(|_| CommandError::Usage(RULES_USAGE)),
        _ => Err(CommandError::Usage(RULES_USAGE)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Command {
        parse(s).unwrap().unwrap()
    }

    #[test]
    fn not_commands() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("//literal"), None);
    }

    #[test]
    fn simple_commands() {
        assert_eq!(ok("/quit"), Command::Quit);
        assert_eq!(ok(" /exit "), Command::Quit);
        assert_eq!(ok("/clear"), Command::Clear);
        assert_eq!(ok("/help"), Command::Help);
        assert_eq!(ok("/resume"), Command::Resume(None));
        assert_eq!(ok("/resume ~2"), Command::Resume(Some("~2".into())));
        assert_eq!(
            ok("/model gpt-5.4"),
            Command::Model(ModelCommand::Set("gpt-5.4".into()))
        );
        assert_eq!(ok("/model"), Command::Model(ModelCommand::Pick));
        assert_eq!(ok("/model refresh"), Command::Model(ModelCommand::Refresh));
        assert!(matches!(
            parse("/model a b"),
            Some(Err(CommandError::Usage(_)))
        ));
    }

    #[test]
    fn remember_and_compact_commands() {
        assert_eq!(
            ok("/remember use  cargo nextest"),
            Command::Remember("use  cargo nextest".into())
        );
        assert_eq!(
            parse("/remember"),
            Some(Err(CommandError::Usage("/remember <text>")))
        );
        assert_eq!(ok("/compact"), Command::Compact);
        assert_eq!(ok("/cost"), Command::Cost);
        assert_eq!(parse("/cost now"), Some(Err(CommandError::Usage("/cost"))));
        assert!(matches!(
            parse("/compact now"),
            Some(Err(CommandError::Usage(_)))
        ));
    }

    #[test]
    fn hash_remember_and_escape() {
        assert_eq!(parse_remember("#use tabs "), Some("use tabs".into()));
        assert_eq!(parse_remember("  #  use tabs"), Some("use tabs".into()));
        assert_eq!(parse_remember("#"), None);
        assert_eq!(parse_remember("#   "), None);
        assert_eq!(parse_remember("## heading"), None);
        assert_eq!(parse_remember("#one\ntwo"), None);
        assert_eq!(parse_remember("\\#tag"), None);
        assert_eq!(parse_remember("plain"), None);
        assert_eq!(unescape_hash("\\#tag it"), Some("#tag it".into()));
        assert_eq!(unescape_hash("#tag"), None);
    }

    #[test]
    fn auto_commands() {
        assert_eq!(ok("/auto"), Command::Auto(AutoCommand::Status));
        assert_eq!(ok("/auto on"), Command::Auto(AutoCommand::On));
        assert_eq!(ok("/auto off"), Command::Auto(AutoCommand::Off));
        assert_eq!(ok("/auto resume"), Command::Auto(AutoCommand::Resume));
        assert!(matches!(
            parse("/auto maybe"),
            Some(Err(CommandError::Usage(_)))
        ));
    }

    #[test]
    fn rules_commands() {
        assert_eq!(ok("/rules"), Command::Rules(RulesCommand::List));
        assert_eq!(ok("/rules list"), Command::Rules(RulesCommand::List));
        assert_eq!(
            ok("/rules add allow shell cargo test *"),
            Command::Rules(RulesCommand::Add {
                effect: RuleEffect::Allow,
                tool: "shell".into(),
                pattern: Some("cargo test *".into()),
            })
        );
        assert_eq!(
            ok("/rules add deny web_fetch"),
            Command::Rules(RulesCommand::Add {
                effect: RuleEffect::Deny,
                tool: "web_fetch".into(),
                pattern: None,
            })
        );
        assert_eq!(ok("/rules rm 3"), Command::Rules(RulesCommand::Remove(3)));
        assert!(matches!(
            parse("/rules rm x"),
            Some(Err(CommandError::Usage(_)))
        ));
        assert!(matches!(
            parse("/rules add maybe t"),
            Some(Err(CommandError::Usage(_)))
        ));
    }

    #[test]
    fn help_mentions_every_command_and_key() {
        for needle in [
            "/help",
            "/quit",
            "/exit",
            "/clear",
            "/resume",
            "/rules",
            "/auto",
            "/model",
            "/remember",
            "#<text>",
            "/compact",
            "/cost",
            "Enter",
            "Alt+Enter",
            "Ctrl+J",
            "Ctrl-C",
            "Esc",
            "Ctrl-D",
            "Ctrl-O",
            "Ctrl-R",
            "Ctrl-X Ctrl-S",
            "PgUp",
            "PgDn",
            "Home",
            "End",
            "y allow",
            "a allow",
            "n/Esc deny",
            "s stop",
            "recall queued messages",
            "newest queued message",
            "delivers it at the next step",
            "press again",
            "switch to a model (idle only)",
            "endpoint:",
            "/model refresh",
        ] {
            assert!(HELP_TEXT.contains(needle), "help is missing {needle}");
        }
    }

    #[test]
    fn unknown_command() {
        assert_eq!(
            parse("/frob"),
            Some(Err(CommandError::Unknown("frob".into())))
        );
        assert_eq!(
            CommandError::Unknown("frob".into()).to_string(),
            "unknown command /frob (try /help)"
        );
    }
}
