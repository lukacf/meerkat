use crate::archive_path::MobpackArchivePath;

pub fn normalize_executable_bit(path: &MobpackArchivePath, bytes: &[u8]) -> bool {
    path.executable_policy(bytes).is_executable()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_bit_normalization() {
        let hook_script = MobpackArchivePath::parse("hooks/run.sh").unwrap();
        assert!(normalize_executable_bit(&hook_script, b"echo hi\n"));
        let skill_script = MobpackArchivePath::parse("skills/review.md").unwrap();
        assert!(!normalize_executable_bit(&skill_script, b"#!/bin/sh\n"));
        let hook_shebang = MobpackArchivePath::parse("hooks/setup").unwrap();
        assert!(normalize_executable_bit(
            &hook_shebang,
            b"#!/usr/bin/env bash\nset -e\n"
        ));
    }
}
