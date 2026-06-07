use crate::vocabulary::ExecPolicy;

pub fn normalize_executable_bit(path: &str, bytes: &[u8]) -> bool {
    ExecPolicy::classify(path, bytes).is_executable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_bit_normalization() {
        assert!(normalize_executable_bit("hooks/run.sh", b"echo hi\n"));
        assert!(!normalize_executable_bit(
            "skills/review.md",
            b"#!/bin/sh\n"
        ));
        assert!(normalize_executable_bit(
            "hooks/setup",
            b"#!/usr/bin/env bash\nset -e\n"
        ));
    }
}
