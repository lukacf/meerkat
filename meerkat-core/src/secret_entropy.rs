//! Secret-quality OS entropy.
//!
//! One owner for "this value must be unguessable bearer authority" randomness.
//! Callers that need a capability/bearer identity draw from here instead of
//! reaching for a UUID: a UUIDv4 carries 122 bits of entropy and advertises a
//! *uniqueness* contract, not a secrecy one.
//!
//! The draw is fallible and panic-free: an OS entropy failure is returned as a
//! typed error rather than unwrapped.

use thiserror::Error;

/// Failure to draw OS entropy for a secret-quality value.
#[derive(Debug, Error)]
#[error("failed to draw {requested} bytes of OS entropy: {detail}")]
pub struct SecretEntropyError {
    /// Number of bytes requested by the caller.
    pub requested: usize,
    /// Platform detail for the failed draw.
    pub detail: String,
}

/// Fill `buffer` with cryptographic OS entropy.
pub fn fill_secret_entropy(buffer: &mut [u8]) -> Result<(), SecretEntropyError> {
    getrandom::fill(buffer).map_err(|error| SecretEntropyError {
        requested: buffer.len(),
        detail: error.to_string(),
    })
}

/// Draw `N` bytes of OS entropy and render them lowercase hex.
///
/// Hex keeps the rendered token URL-, log- and SQL-safe without adding an
/// encoding dependency, and keeps the full drawn entropy (two hex characters
/// per byte).
pub fn secret_entropy_hex<const N: usize>() -> Result<String, SecretEntropyError> {
    let mut bytes = [0_u8; N];
    fill_secret_entropy(&mut bytes)?;
    let mut rendered = String::with_capacity(N * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        // Writing to a String is infallible; the formatter error is discarded
        // deliberately rather than unwrapped.
        let _ = write!(rendered, "{byte:02x}");
    }
    Ok(rendered)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn secret_entropy_hex_renders_full_width_and_differs_per_draw() {
        let first = secret_entropy_hex::<32>().expect("entropy");
        let second = secret_entropy_hex::<32>().expect("entropy");
        assert_eq!(first.len(), 64, "32 bytes must render as 64 hex characters");
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "independent draws must differ");
    }

    #[test]
    fn fill_secret_entropy_fills_the_whole_buffer() {
        let mut buffer = [0_u8; 32];
        fill_secret_entropy(&mut buffer).expect("entropy");
        assert!(
            buffer.iter().any(|byte| *byte != 0),
            "an all-zero 32-byte draw is not a credible OS entropy result"
        );
    }
}
