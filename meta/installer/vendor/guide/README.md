# Guide client SDK

This directory carries the Guide's Python client: the published `meerkat-meta`
wheel of the Meta release the Guide runs (`SELECTED_VERSION` in
`launchkit/guide_sdk.py`). It is pure Python, contains no gateway and does not
replace an application's separately selected SDK or native runtime.

- Release: Meta `v0.9.3`, https://github.com/lukacf/meerkat-meta/releases/tag/v0.9.3
- Release commit (the tag peels to it): `3b0073a834b93725c20cfa97bbd6db099348ece6`
- `sdk/python` tree at that commit: `d41781b733c506ed0355bd7aad4323dc77ebcda6`
- Package version: `0.9.3`
- Published file: https://files.pythonhosted.org/packages/bc/21/ce4e7167e06ab90efde50d3447b2127f14699bab9107d254ddf0c838a886/meerkat_meta-0.9.3-py3-none-any.whl
- Wheel bytes: 166988
- Wheel SHA-256: `3e87446e560b94ab6d9d0b3dfac5868b2603db7739316e251aefb5e3b4b4d610`
- Sorted member-list SHA-256: `90c99483dcd8f67fda4b91cbaee9d20fa0702eaf7f18f9a05b53a2cedba87121`

These are the bytes PyPI serves for the release, with the size and SHA-256 that
PyPI reports. Each of the 24 `meerkat_meta/` members equals the same path under
`sdk/python` at the release commit, and the wheel's `RECORD` matches every
member. To compute the member-list digest, hash each non-directory member, sort
by path, concatenate `sha256`, two spaces, path and a newline for every member,
then hash that UTF-8 list.

`launchkit/guide_sdk.py` pins the release commit, the tree, the SHA-256 and the
size; packaging and product verification refuse any other bytes.
`requirements-guide-sdk.txt` pins the same SHA-256 for the development
environment and the Forge workflow, which install this file with
`--require-hashes`, so the suite imports the installed wheel, not SDK source.

The release includes the public client transport waits for `spawn_helper` and
`fork_helper`, which refuse invalid values before writing a request. They do not
send a native timeout option. Forge uses the remaining total review window.
Timeout leaves the native outcome unknown and does not authorize a retry.

Selecting another Meta release replaces this wheel with that release's
published wheel and fills the four pin fields together. Until that release
publishes, the pin stays empty and every use of it refuses. Generated
applications keep the exact SDK/native pair in their own selected product.
