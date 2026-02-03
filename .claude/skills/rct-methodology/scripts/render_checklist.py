#!/usr/bin/env python3
"""Render .rct/checklist.yaml to .rct/outputs/CHECKLIST.md.

Requires PyYAML (pip install pyyaml).
"""
from pathlib import Path
import sys

try:
    import yaml  # type: ignore
except Exception:
    print("PyYAML not installed. Run: pip install pyyaml", file=sys.stderr)
    sys.exit(1)

repo = Path.cwd()
checklist = repo / ".rct" / "checklist.yaml"
out_md = repo / ".rct" / "outputs" / "CHECKLIST.md"

if not checklist.exists():
    print(f"Missing checklist: {checklist}", file=sys.stderr)
    sys.exit(1)

with checklist.open() as f:
    data = yaml.safe_load(f)

phases = data.get("phases", [])

lines = []
lines.append(f"# {data.get('project', 'Checklist')}")
lines.append("")
if "last_updated" in data:
    lines.append(f"**Last Updated:** {data['last_updated']}")
    lines.append("")

for phase in phases:
    pid = phase.get("id")
    title = phase.get("title", "")
    lines.append(f"## Phase {pid}: {title}")
    lines.append("")

    if phase.get("approved_marker"):
        lines.append(f"<!-- approved_marker: {phase['approved_marker']} -->")
        lines.append("")

    lines.append("### Gate Results")
    lines.append("<!-- GATE_RESULTS_START -->")
    gate = phase.get("gate_results", {}) or {}
    if gate:
        if gate.get("updated_at"):
            lines.append(f"<!-- updated: {gate['updated_at']} -->")
        if gate.get("stage"):
            lines.append(f"<!-- stage: {gate['stage']} -->")
        for v in gate.get("verdicts", []) or []:
            lines.append(f"- {v.get('reviewer')}: {v.get('verdict')} - {v.get('summary', 'ok')}")
    lines.append("<!-- GATE_RESULTS_END -->")
    lines.append("")

    vcmds = phase.get("verification_commands", []) or []
    if vcmds:
        lines.append("### Verification Commands")
        lines.append("```bash")
        for cmd in vcmds:
            lines.append(str(cmd))
        lines.append("```")
        lines.append("")

    reviewers = phase.get("reviewers", []) or []
    if reviewers:
        lines.append("### Reviewers")
        for r in reviewers:
            lines.append(f"- {r}")
        lines.append("")

    lines.append("### Tasks")
    for t in phase.get("tasks", []) or []:
        done = "x" if t.get("done") else " "
        text = t.get("text", "")
        done_when = t.get("done_when", "")
        tid = t.get("id", "")
        spec_id = t.get("spec_id", "")
        if tid:
            text = f"{tid} {text}".strip()
        if spec_id:
            text = f"{text} (spec: {spec_id})"
        if done_when:
            lines.append(f"- [{done}] {text} (Done when: {done_when})")
        else:
            lines.append(f"- [{done}] {text}")
    lines.append("")

out_md.parent.mkdir(parents=True, exist_ok=True)
out_md.write_text("\n".join(lines) + "\n")
print(f"Wrote {out_md}")
