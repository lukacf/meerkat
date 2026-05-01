//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory depends on this crate directly and owns the value passed across the
//! core build seam.

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
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
    pub(super) struct AuthoritySeal {
        pub(super) words: [u64; 4],
        pub(super) guard: u128,
        pub(super) checksum: u64,
    }

    const CANONICAL_AUTHORITY_WORDS: [u64; 4] = [
        0xf4_22_2f_48_41_5f_d0_3b,
        0x91_7c_40_22_7a_8a_61_d9,
        0x5c_c6_93_13_d4_89_a2_7e,
        0xaa_d5_0e_b8_20_64_7f_11,
    ];
    const CANONICAL_AUTHORITY_GUARD: u128 = 0x006d_6565_726b_6174_2d61_6765_6e74_2121;

    const fn authority_checksum(words: [u64; 4], guard: u128) -> u64 {
        let mut checksum = 0x6d6b_7421_fade_beef_u64;
        let mut index = 0;
        while index < words.len() {
            checksum ^= words[index].rotate_left((index as u32 + 1) * 11);
            checksum = checksum.wrapping_mul(0x0000_0100_0000_01b3);
            index += 1;
        }
        checksum ^= (guard >> 64) as u64;
        checksum = checksum.wrapping_mul(0x0000_0100_0000_01b3);
        checksum ^ guard as u64
    }

    pub(super) const fn canonical_factory_seal() -> AuthoritySeal {
        AuthoritySeal {
            words: CANONICAL_AUTHORITY_WORDS,
            guard: CANONICAL_AUTHORITY_GUARD,
            checksum: authority_checksum(CANONICAL_AUTHORITY_WORDS, CANONICAL_AUTHORITY_GUARD),
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
        let authority = authority_from_seal(private::AuthoritySeal {
            words: [0; 4],
            guard: 0,
            checksum: 0,
        });

        assert!(!authority.is_canonical_factory_authority());
    }
}
