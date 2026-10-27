# Native Connect workspace

This workspace contains the Connect CLI, integrated Console gateway, native
host adapters and `toolkit-store`. The directory name is retained from development.
The portable root workspace owns `toolkit-package`; its lockfile remains separate.

The current dependency selection and matching SDKs are documented in
[the build guide](../BUILDING.md). `Cargo.toml` and `Cargo.lock` select the
native graph; the portable root workspace has its own lock. The retained
`vendor/core-codegen` tree belongs to the v0.9.0 release and is not selected by
the current native workspace.

See [the build guide](../BUILDING.md) for locked source-only builds and complete
distribution prerequisites. This development selection does not replace the
published v0.9.0 archives, resources or SDK assets. Checking out `v0.9.0` selects
the released registry/vendor composition described by that tag's build guide.
Resolution and compilation do not establish installed-host or business success.
