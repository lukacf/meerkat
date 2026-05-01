//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory depends on this crate directly and owns the value passed across the
//! core build seam.

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AgentFactoryBuildAuthority {
    seal: private::AuthoritySeal,
}

impl AgentFactoryBuildAuthority {
    /// Validate that this value was minted by the internal authority bridge.
    #[doc(hidden)]
    pub fn is_canonical_factory_authority(&self) -> bool {
        self.seal == private::canonical_factory_seal()
    }
}

mod private {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(C)]
    pub(super) struct AuthoritySeal {
        pub(super) words: [u64; 4],
    }

    const FALLBACK_AUTHORITY_WORDS: [u64; 4] = [
        0xf4_22_2f_48_41_5f_d0_3b,
        0x91_7c_40_22_7a_8a_61_d9,
        0x5c_c6_93_13_d4_89_a2_7e,
        0xaa_d5_0e_b8_20_64_7f_11,
    ];

    const fn hex_nibble(byte: u8) -> u64 {
        match byte {
            b'0'..=b'9' => (byte - b'0') as u64,
            b'a'..=b'f' => (byte - b'a' + 10) as u64,
            b'A'..=b'F' => (byte - b'A' + 10) as u64,
            _ => 0,
        }
    }

    const fn authority_word(value: Option<&str>, fallback: u64) -> u64 {
        let Some(value) = value else {
            return fallback;
        };
        let bytes = value.as_bytes();
        let mut index = 0;
        let mut word = 0_u64;
        while index < bytes.len() {
            word = (word << 4) | hex_nibble(bytes[index]);
            index += 1;
        }
        word
    }

    const CANONICAL_AUTHORITY_WORDS: [u64; 4] = [
        authority_word(
            option_env!("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_0"),
            FALLBACK_AUTHORITY_WORDS[0],
        ),
        authority_word(
            option_env!("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_1"),
            FALLBACK_AUTHORITY_WORDS[1],
        ),
        authority_word(
            option_env!("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_2"),
            FALLBACK_AUTHORITY_WORDS[2],
        ),
        authority_word(
            option_env!("MEERKAT_AGENT_BUILD_AUTHORITY_WORD_3"),
            FALLBACK_AUTHORITY_WORDS[3],
        ),
    ];

    pub(super) const fn canonical_factory_seal() -> AuthoritySeal {
        AuthoritySeal {
            words: CANONICAL_AUTHORITY_WORDS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentFactoryBuildAuthority, private};

    fn authority_from_seal(seal: private::AuthoritySeal) -> AgentFactoryBuildAuthority {
        AgentFactoryBuildAuthority { seal }
    }

    #[test]
    fn canonical_seal_validates() {
        let authority = authority_from_seal(private::canonical_factory_seal());

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn non_canonical_seal_is_rejected() {
        let authority = authority_from_seal(private::AuthoritySeal { words: [0; 4] });

        assert!(!authority.is_canonical_factory_authority());
    }
}
