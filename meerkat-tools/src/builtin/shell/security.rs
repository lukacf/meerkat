//! Shell security module
//!
//! This module provides command validation and sanitization for shell execution.
//! It implements defense-in-depth strategies to prevent command injection and
//! other security vulnerabilities.

use super::config::ShellError;

/// Patterns that indicate shell chaining/piping (allowed but logged)
const CHAINING_CHARS: &[char] = &[
    '|', // Pipe
    ';', // Command separator
    '&', // Background/AND
];

/// Dangerous command patterns that are always blocked
const BLOCKED_PATTERNS: &[&str] = &["rm -rf /", "rm -rf ~", "> /dev/sd", "dd if="];

/// Security validation result
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    /// Command is safe to execute
    Safe,
    /// Command has potential risks but is allowed (with warning)
    Warning { reason: String },
    /// Command is blocked
    Blocked { reason: String },
}

/// Validate a shell command for security issues
///
/// Returns:
/// - `Safe` if the command appears safe
/// - `Warning` if the command has potential risks but is allowed
/// - `Blocked` if the command matches a dangerous pattern
pub fn validate_command(command: &str) -> ValidationResult {
    // Check for blocked patterns first (case-insensitive for some)
    let lower = command.to_lowercase();
    for pattern in BLOCKED_PATTERNS {
        if lower.contains(&pattern.to_lowercase()) {
            return ValidationResult::Blocked {
                reason: format!("Command contains blocked pattern: '{}'", pattern),
            };
        }
    }

    // Check for chaining (allowed but warned)
    for ch in CHAINING_CHARS {
        if command.contains(*ch) {
            return ValidationResult::Warning {
                reason: format!(
                    "Command contains shell chaining character '{}' - ensure this is intentional",
                    ch
                ),
            };
        }
    }

    ValidationResult::Safe
}

/// Validate command and return error if blocked
pub fn validate_command_or_error(command: &str) -> Result<ValidationResult, ShellError> {
    match validate_command(command) {
        ValidationResult::Blocked { reason } => Err(ShellError::BlockedCommand(reason)),
        other => Ok(other),
    }
}

/// Security configuration for shell execution
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// Maximum number of concurrent shell processes
    pub max_concurrent_processes: usize,
    /// Maximum output size in bytes
    pub max_output_bytes: usize,
    /// Block commands with dangerous characters
    pub block_dangerous_chars: bool,
    /// Block commands matching dangerous patterns
    pub block_dangerous_patterns: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_concurrent_processes: 10,
            max_output_bytes: 10 * 1024 * 1024, // 10MB
            block_dangerous_chars: true,
            block_dangerous_patterns: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== validate_command Tests ====================

    #[test]
    fn test_safe_commands() {
        // Common safe commands should pass
        assert_eq!(validate_command("ls -la"), ValidationResult::Safe);
        assert_eq!(validate_command("cat file.txt"), ValidationResult::Safe);
        assert_eq!(
            validate_command("grep pattern file"),
            ValidationResult::Safe
        );
        assert_eq!(validate_command("pwd"), ValidationResult::Safe);
        assert_eq!(validate_command("echo hello"), ValidationResult::Safe);
        assert_eq!(validate_command("wc -l file.txt"), ValidationResult::Safe);
    }

    #[test]
    fn test_blocked_dangerous_patterns() {
        // rm -rf /
        let result = validate_command("rm -rf /");
        assert!(matches!(result, ValidationResult::Blocked { .. }));

        // rm -rf ~
        let result = validate_command("rm -rf ~");
        assert!(matches!(result, ValidationResult::Blocked { .. }));

        // Overwrite disk
        let result = validate_command("> /dev/sda");
        assert!(matches!(result, ValidationResult::Blocked { .. }));

        // dd to disk
        let result = validate_command("dd if=/dev/zero of=/dev/sda");
        assert!(matches!(result, ValidationResult::Blocked { .. }));
    }

    #[test]
    fn test_warning_for_chaining() {
        // Pipe
        let result = validate_command("cat file | grep pattern");
        assert!(matches!(result, ValidationResult::Warning { .. }));

        // Semicolon
        let result = validate_command("cd dir; ls");
        assert!(matches!(result, ValidationResult::Warning { .. }));

        // Ampersand (background)
        let result = validate_command("long_command &");
        assert!(matches!(result, ValidationResult::Warning { .. }));
    }

    #[test]
    fn test_validate_command_or_error() {
        // Safe command
        let result = validate_command_or_error("ls -la");
        assert!(result.is_ok());

        // Blocked command
        let result = validate_command_or_error("rm -rf /");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ShellError::BlockedCommand(_)));
    }

    #[test]
    fn test_case_insensitive_pattern_matching() {
        // Should block regardless of case
        let result = validate_command("RM -RF /");
        assert!(matches!(result, ValidationResult::Blocked { .. }));

        let result = validate_command("Rm -Rf /");
        assert!(matches!(result, ValidationResult::Blocked { .. }));
    }

    // ==================== SecurityConfig Tests ====================

    #[test]
    fn test_security_config_defaults() {
        let config = SecurityConfig::default();
        assert_eq!(config.max_concurrent_processes, 10);
        assert_eq!(config.max_output_bytes, 10 * 1024 * 1024);
        assert!(config.block_dangerous_chars);
        assert!(config.block_dangerous_patterns);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_empty_command() {
        assert_eq!(validate_command(""), ValidationResult::Safe);
    }

    #[test]
    fn test_whitespace_command() {
        assert_eq!(validate_command("   "), ValidationResult::Safe);
    }

    #[test]
    fn test_safe_file_operations() {
        // Normal file operations should be safe
        assert_eq!(validate_command("cp file1 file2"), ValidationResult::Safe);
        assert_eq!(validate_command("mv old new"), ValidationResult::Safe);
        assert_eq!(validate_command("mkdir -p dir"), ValidationResult::Safe);
        assert_eq!(validate_command("touch newfile"), ValidationResult::Safe);
    }

    #[test]
    fn test_safe_git_commands() {
        assert_eq!(validate_command("git status"), ValidationResult::Safe);
        assert_eq!(
            validate_command("git log --oneline"),
            ValidationResult::Safe
        );
        assert_eq!(validate_command("git diff HEAD~1"), ValidationResult::Safe);
    }

    #[test]
    fn test_blocked_pattern_in_larger_command() {
        // Pattern embedded in larger command should still be blocked
        let result = validate_command("sudo rm -rf / --no-preserve-root");
        assert!(matches!(result, ValidationResult::Blocked { .. }));
    }
}
