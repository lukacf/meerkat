# Compiler-owned candidate mode. Ordinary packages do not include this overlay.
# The original official download/cache path is never a candidate fallback.
def verified_cache(directory, name, target):
    if name not in BINARIES or Path(directory).absolute() != (ROOT / ".bin").absolute():
        return False
    try:
        from install_candidate import verify_installed_candidate_runtime
        proof = verify_installed_candidate_runtime(ROOT / "runtime-candidate.json",
            expected_native_version=VERSION, expected_platform=target)
        return (proof["source"] == "signed-candidate" and proof["version"] == VERSION
            and proof["platform"] == target and name in proof["native"])
    except (OSError, ValueError, RuntimeError, KeyError, TypeError):
        return False


def main():
    from install_candidate import install_candidate_native_tools
    install_candidate_native_tools(ROOT / "runtime-candidate.json",
        expected_native_version=VERSION, expected_platform=platform_target())
    print(f"Installed explicit signed-candidate MobKit {VERSION} tools and exact source-built SDK.", flush=True)


if __name__ == "__main__":
    main()
