# Official components for macOS arm64

This directory holds two separately selected components. Neither is part of the
library families under `consumers/`; each is enabled only by naming its family.

| Family | Component | What is carried |
| --- | --- | --- |
| `official-github` | `github/` | the GitHub MCP server executable from its publisher's macOS arm64 release archive, that archive's licence, a profile text and a launch template |
| `official-playwright` | `playwright/` | the three Playwright registry archives, a project file, a lock and a closure record written from them, their licence and notice files, a profile text and a launch template |

`delivery.json` records, for each component, its catalog identifier, profile,
target, source revision and the exact files it consists of. `CONTENTS.json` lists
every file here with its size and SHA-256, and `SHA256SUMS` repeats the digests
in checksum format. The library guide prints the SHA-256 of `CONTENTS.json`; the
setup installer checks the complete declared contents against that value before
it reads anything else.

Read `github/UPSTREAM-PROFILE.md` and `playwright/UPSTREAM-PROFILE.md` before
enabling a family. They state what each route fixes, what the host must supply
and what is not checked.

The components are inputs. Packaging, configuration checking and readback do
not execute the server, Node, npm or a browser, do not authenticate and do not
establish that a host can run them. Installation requires macOS arm64.
