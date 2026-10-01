# ADR-001 r2 source citation verification

The authority reviewer independently checked all 16 pinned commit/path/line
references using local Git objects. All exist and support the associated source
observations. No broken source reference was found. This verifies static code
citations, not running behavior or coverage beyond the stated source regions.

Nonblocking precision notes for the next revision:

- The blob endpoint applies configured console authentication policy; when
  authentication is disabled it permits anonymous callers. Avoid wording that
  implies authentication is always required.
- Anchor `el-context` at handlers/mod.rs:134 for the `mcp_tool` default.
- Add crates/mcp/src/handlers/knowledge_views.rs:31-48 as direct support for
  literal `manage:wiki` and unrestricted-subject commissioning checks. The
  existing wiki_editorial.rs:1004-1051 reference supports space-bound revocable
  grants. Binding processor, operation and output destination is a proposed
  requirement, not a claim about the current wiki grant implementation.

Checked candidate SHA-256:
`44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca`.
