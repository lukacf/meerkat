# Native package compatibility

Native agent-team packages with saved organization context use manifest
**protocol 2**, with a required lowercase 64-character SHA256
`organization_hash`. Install them with a Forge reader that supports protocol 2.
The older readers in Meta v0.9.3/v0.9.4, whose Forge package metadata is 0.2.0,
refuse these packages. The source version or decoder digest identifies support
more precisely than the reused Forge 0.2.0 metadata version.

Packages without organization context continue to use protocol 1. For identical
inputs, their generated output is byte-identical to the producer before the
organization change. Protocol 1 keeps its exact keys and meaning. Missing or
null organization context does not opt a package into protocol 2.

The new reader accepts both closed manifest shapes. Protocol 1 with an added
organization key, protocol 2 without that key, malformed digests, unknown
protocols and non-integer protocol values are refused. The source identity
hashes the entire manifest except its own identity field, binding the protocol
and organization fingerprint alongside the existing source inputs.

This compatibility applies to the organization change. Each historical reader's
existing closed source-file allowlist still applies; a newer compiler's other
changes can independently affect which packages it accepts. No historical
reader, package, fixture or installed source proof is rewritten.

## Receipts

New local application and native receipts retain the organization fingerprint
from their admitted package. A receipt that claims an organization must match
the decoded package. Protocol-2 native receipts must carry the fingerprint;
their source identity binds the same value.

Older application receipts that omit the field remain readable. Reading them
does not backfill the field or make them current for a different organization.
Changing or removing the saved organization context makes a previously bound
installation differ from the current guide; it does not alter that installation.

Updates and source rollback copy the organization fingerprint from the selected
package's report, or remove the receipt field when that report has none. Recovery
to the accepted candidate uses the candidate's report; rollback uses the previous
package's report. Native teams do not have an application update path.

An older Forge does not maintain this field when updating an application.
Downgrading Forge, updating an application to a different organization or to no
organization, then upgrading Forge can therefore leave a stale fingerprint in
the receipt. The newer reader refuses the mismatch. Reinstall the application
after that downgrade-update-upgrade cycle; do not edit the receipt to bypass
source verification. This rule does not backfill older receipts or automatically
alter any retained installation.

## Inspected readers

This is the source inventory inspected for the organization change on
2026-10-23. Results below describe decoder behavior from source review, not
executed compatibility tests or host qualification.

There is one maintained decoder implementation,
`launchkit.deployment_sources.read_package_source`. Packaged Forge copies that
decoder. Meta v0.9.0, v0.9.1 and v0.9.2 do not contain this module and are not
classified as alternative native readers.

| Reader source | Exact commit | Decoder | Protocol 2 |
| --- | --- | --- | --- |
| Meta v0.9.3, Forge metadata 0.2.0 | `3b0073a834b93725c20cfa97bbd6db099348ece6` | A | Refuses |
| Meta v0.9.4, Forge metadata 0.2.0 | `3c3af34f0a12bfab3d33183f65da9fd2578d58e3` | A | Refuses |
| Retained Forge source, metadata 0.2.0 | `3f91e4873023ce4143b72ed274b5db822d76e3a0` | A | Refuses |
| Packaged Forge source, metadata 0.2.0 | `9d61a72ffbebfd6438fbf79d6ca0591323309d90` | B | Refuses |
| Incoming-information follow-up, metadata 0.2.0 | `021b8ad0c7b31e9cddef209fbbf087ce3a89b869` | A | Refuses |
| Brief-binding follow-up, metadata 0.2.0 | `22eaed019afaa80826a93bb31c14cfee06e0bae1` | C | Refuses |
| Acceptance-container follow-up, metadata 0.2.0 | `5c9dfcdf8e179914bb62b4ad878343280ed008fb` | C | Refuses |
| Application-organization follow-up, metadata 0.2.0 | `d348f001848b59de438fd99a98f087eea7c34126` | D | Refuses |
| Native-organization implementation, metadata 0.2.0 | `9695e5f9b15a13a1c8e30c72bd523eb4afbbdfb8` | E | Accepts both closed shapes with existing checks |

Decoder SHA256 values refer to `installer/launchkit/deployment_sources.py`:

| Decoder | SHA256 |
| --- | --- |
| A | `32050f3abfdbd53b990d9990c97d3ab829f102652b6080900997e99914609d47` |
| B | `fe2e3b2d1aaad03ca2f96f4185bb212257df05875104e163aa48a0ee95786bf5` |
| C | `baa24deb21398d75f375cc0c446e25868ad0fbba93e9077a287a8fc3ac21d458` |
| D | `0915396b94e68c029df875e4adf862747ab1041d6ac4b2da5c8dddec31c7432d` |
| E | `2729654db42b77fbabf6a01e7dd59a80a01dde18ee72578dd6ed6dd910480af9` |

Both retained source archives below also refuse protocol 2. Their member
`launchkit/deployment_sources.py` has SHA256
`4b6be43d71ada0cbd189bbeaa6634b15ac8dec3cf6d499560a6488ef3e5ecbfc`:

- `installer/tests/fixtures/runtime_history_v1_candidate/public-sources.zip`
- `installer/tests/fixtures/runtime_history_v2/public-sources.zip`

The history fixture's `v2` name does not identify native manifest protocol 2 or
a published Forge version. Arbitrary private repackaging versions cannot be
inferred from project metadata; inspect the packaged decoder's digest or source
commit instead.

## Decoder entry points

The inspected implementation has these 20 direct calls to the canonical reader.
All paths are relative to `installer/launchkit`. Application-only owners can
refuse a native package before reaching this reader.

| Module | Calling functions |
| --- | --- |
| `deployment_sources.py` | `compile_native_source` |
| `deployment.py` | `interactive_source_binding`, `package_files`, `_installed_configuration`, `_installed_package_read`, `options`, `submit`, `_destination_control_package`, `guide_status.verified_source` |
| `local_application_registry.py` | `_receipt_host`, `continues_installed_source`, `_recover_installation` (two call sites), `_reconnect_status` |
| `ordinary_application_source.py` | `recover_ordinary_application_source` |
| `destination_product_install.py` | `_host_source` |
| `local_application_recovery.py` | `recover_local_application.inspect_source` |
| `connect_accepted_source.py` | `_retain` |
| `deployment_application_bootstrap.py` | `_read_source` |
| `product_installed_source.py` | `_open_files` |

The implementation and regression cases are source-review candidates until
the validation owner executes their consolidated candidate. This inventory
does not establish successful installation or recovery on a retained host.
