//! Shell security engine
//!
//! Provides a robust parse-then-validate pipeline for shell commands using
//! POSIX-compliant word splitting and glob pattern matching.

use crate::builtin::shell::config::ShellError;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
pub use meerkat_core::types::SecurityMode;

/// POSIX-compliant command invocation
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    /// The executable being called
    pub executable: String,
    /// Arguments passed to the executable
    pub arguments: Vec<String>,
}

impl CommandInvocation {
    /// Parse a raw command string into a structured invocation.
    ///
    /// Fails if the command has malformed shell syntax (e.g., unclosed quotes).
    pub fn parse(raw_command: &str) -> Result<Self, ShellError> {
        let words = shlex::split(raw_command).ok_or_else(|| {
            ShellError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Malformed shell command: unclosed quotes or invalid escape sequences",
            ))
        })?;

        if words.is_empty() {
            return Ok(Self {
                executable: String::new(),
                arguments: Vec::new(),
            });
        }

        let mut iter = words.into_iter();
        let executable = iter.next().unwrap_or_default();
        let arguments = iter.collect();

        Ok(Self {
            executable,
            arguments,
        })
    }

    /// Reconstruct a canonicalized command string
    pub fn to_canonical_string(&self) -> String {
        if self.executable.is_empty() {
            return String::new();
        }

        let mut out = shlex::try_quote(&self.executable)
            .map(|q| q.into_owned())
            .unwrap_or_else(|_| self.executable.clone());

        for arg in &self.arguments {
            out.push(' ');
            out.push_str(
                &shlex::try_quote(arg)
                    .map(|q| q.into_owned())
                    .unwrap_or_else(|_| arg.clone()),
            );
        }
        out
    }
}

/// A compiled security policy
pub struct SecurityEngine {
    mode: SecurityMode,
    matcher: Option<GlobSet>,
}

impl SecurityEngine {
    /// Create a new engine from a mode and list of patterns.
    ///
    /// Fails if any glob pattern is invalid.
    pub fn new(mode: SecurityMode, patterns: &[String]) -> Result<Self, ShellError> {
        if mode == SecurityMode::Unrestricted || patterns.is_empty() {
            return Ok(Self {
                mode,
                matcher: None,
            });
        }

        let mut builder = GlobSetBuilder::new();
        for pat in patterns {
            // Use literal_separator(false) to get command-line glob semantics
            // where `*` matches `/` (e.g., `cat *` matches `cat /tmp/foo`)
            let glob = GlobBuilder::new(pat)
                .literal_separator(false)
                .build()
                .map_err(|e| {
                    ShellError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Invalid glob pattern '{}': {}", pat, e),
                    ))
                })?;
            builder.add(glob);
        }

        let matcher = builder.build().map_err(|e| {
            ShellError::Io(std::io::Error::other(format!(
                "Failed to compile security patterns: {}",
                e
            )))
        })?;

        Ok(Self {
            mode,
            matcher: Some(matcher),
        })
    }

    /// Check if a raw command string is allowed by this policy.
    pub fn check(&self, raw_command: &str) -> Result<(), ShellError> {
        if self.mode == SecurityMode::Unrestricted {
            return Ok(());
        }

        let invocation = CommandInvocation::parse(raw_command)?;
        self.check_invocation(&invocation)
    }

    /// Check if a command invocation is allowed by this policy.
    pub fn check_invocation(&self, invocation: &CommandInvocation) -> Result<(), ShellError> {
        if self.mode == SecurityMode::Unrestricted {
            return Ok(());
        }

        if invocation.executable.is_empty() {
            return Ok(());
        }

        // We match against the canonical string to ensure the whole command line is validated.
        // This prevents "command smuggling" via arguments.
        let canonical = invocation.to_canonical_string();

        let is_match = self
            .matcher
            .as_ref()
            .is_some_and(|m| m.is_match(&canonical));

        match self.mode {
            SecurityMode::AllowList => {
                if is_match {
                    Ok(())
                } else {
                    Err(ShellError::BlockedCommand(format!(
                        "Command '{}' does not match any allowed patterns",
                        canonical
                    )))
                }
            }
            SecurityMode::DenyList => {
                if is_match {
                    Err(ShellError::BlockedCommand(format!(
                        "Command '{}' matches a denied pattern",
                        canonical
                    )))
                } else {
                    Ok(())
                }
            }
            SecurityMode::Unrestricted => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invocation_parsing() {
        let inv = CommandInvocation::parse("  ls  -la  'my dir'  ").unwrap();
        assert_eq!(inv.executable, "ls");
        assert_eq!(inv.arguments, vec!["-la", "my dir"]);

        // Canonicalization check
        assert_eq!(inv.to_canonical_string(), "ls -la 'my dir'");
    }

    #[test]
    fn test_parsing_failure() {
        let result = CommandInvocation::parse("rm -rf \"unclosed");
        assert!(result.is_err());
    }

    #[test]
    fn test_unrestricted_mode() {
        let engine = SecurityEngine::new(SecurityMode::Unrestricted, &[]).unwrap();
        assert!(engine.check("rm -rf /").is_ok());
    }

    #[test]
    fn test_allow_list() {
        let patterns = vec!["git *".to_string(), "ls".to_string()];
        let engine = SecurityEngine::new(SecurityMode::AllowList, &patterns).unwrap();

        assert!(engine.check("ls").is_ok());
        assert!(engine.check("git status").is_ok());
        assert!(engine.check("git checkout main").is_ok());

        // Blocked: not in list
        assert!(engine.check("rm -rf /").is_err());
        // Blocked: ls with args is not 'ls' exactly
        assert!(engine.check("ls -la").is_err());
    }

    #[test]
    fn test_deny_list() {
        let patterns = vec!["rm -rf /".to_string(), "curl *".to_string()];
        let engine = SecurityEngine::new(SecurityMode::DenyList, &patterns).unwrap();

        assert!(engine.check("ls -la").is_ok());

        // Blocked: denied exactly
        assert!(engine.check("rm -rf /").is_err());
        // Blocked: denied via glob
        assert!(engine.check("curl http://malicious.com").is_err());
    }

    #[test]
    fn test_glob_matches_slashes_in_paths() {
        // Command-line globs should match paths with slashes
        // e.g., `cat *` should match `cat /tmp/foo`
        let patterns = vec!["cat *".to_string()];
        let engine = SecurityEngine::new(SecurityMode::AllowList, &patterns).unwrap();

        // These should all be allowed since `*` should match anything including `/`
        assert!(
            engine.check("cat /tmp/foo").is_ok(),
            "cat * should match cat /tmp/foo"
        );
        assert!(
            engine.check("cat /path/to/file.txt").is_ok(),
            "cat * should match paths with multiple slashes"
        );
        assert!(
            engine.check("cat somefile").is_ok(),
            "cat * should match simple filenames"
        );
    }

    #[test]
    fn test_glob_matches_urls() {
        // Command-line globs should match URLs with slashes
        // e.g., `curl *` should match `curl http://example.com/path`
        let patterns = vec!["curl *".to_string()];
        let engine = SecurityEngine::new(SecurityMode::AllowList, &patterns).unwrap();

        assert!(
            engine.check("curl http://example.com").is_ok(),
            "curl * should match simple URLs"
        );
        assert!(
            engine.check("curl http://example.com/path").is_ok(),
            "curl * should match URLs with paths"
        );
        assert!(
            engine
                .check("curl http://example.com/deep/nested/path")
                .is_ok(),
            "curl * should match URLs with deep paths"
        );
    }

    #[test]
    fn test_deny_list_glob_matches_slashes() {
        // Deny list globs should also match paths with slashes
        let patterns = vec!["rm *".to_string()];
        let engine = SecurityEngine::new(SecurityMode::DenyList, &patterns).unwrap();

        // These should all be denied since `rm *` should match any rm command
        assert!(
            engine.check("rm /etc/passwd").is_err(),
            "rm * should deny rm /etc/passwd"
        );
        assert!(
            engine.check("rm /path/to/file").is_err(),
            "rm * should deny paths with slashes"
        );
    }

    #[test]
    fn test_command_smuggling_prevention() {
        // If someone tries to smuggle: git commit; rm -rf /
        let smuggled = "git commit; rm -rf /";
        let invocation = CommandInvocation::parse(smuggled).unwrap();

        // Shlex will treat ';' as part of the previous word if not separated by space,
        // or as a separate word if it is.
        // In the canonical string, EVERYTHING is quoted.
        let canonical = invocation.to_canonical_string();

        // The resulting string should NOT contain a raw ';' that would be interpreted by a shell.
        // It should look like: 'git' 'commit;' 'rm' '-rf' '/'
        assert!(canonical.contains("'commit;'") || canonical.contains("';'"));
        assert!(!canonical.contains("commit; rm")); // Raw sequence must be broken by quotes
    }
}
