#!/usr/bin/env python3
"""Resolve a rebase conflict in CHANGELOG.md where main stamped the old
[Unreleased] section into a release and the branch commit added/edited entries
under the old [Unreleased]. Result: ours (main side) with the [Unreleased]
section replaced by the branch commit's [Unreleased] section minus the
pre-existing main content that was stamped into the release."""
import subprocess, sys, re
def stage(n): return subprocess.check_output(['git','show',f':{n}:CHANGELOG.md'], text=True)
base, ours, theirs = stage(1), stage(2), stage(3)
UNREL = '## [Unreleased]\n'
def section(text):
    i = text.index(UNREL) + len(UNREL)
    m = re.search(r'^## \[\d', text[i:], re.M)
    return i, i + m.start()
# the pre-existing content of the old [Unreleased] on the branch base's base:
# everything in base's section that is NOT branch-authored is whatever also
# appears in ours' stamped release. We identify it as the lines of base's
# section that are absent from the difference base->theirs and that also
# appear verbatim in ours' release section right after [Unreleased].
bs, be = section(base); ts, te = section(theirs); os_, oe = section(ours)
base_sec, theirs_sec, ours_sec = base[bs:be], theirs[ts:te], ours[os_:oe]
release_start = oe  # ours: start of "## [0.8.xx]" header
# stamped block = ours' release section body up to its next "## [" header
m = re.search(r'^## \[\d[^\n]*\n', ours[release_start:], re.M)
rel_body_start = release_start + m.end()
m2 = re.search(r'^## \[\d', ours[rel_body_start:], re.M)
stamped = ours[rel_body_start: rel_body_start + m2.start()]
# The stamped body must contain, verbatim, the non-branch part of base's section.
# Compute non-branch part: blocks of base_sec that appear in stamped.
def blocks(sec):
    # split into header lines and bullet blocks
    out, cur = [], []
    for line in sec.splitlines(keepends=True):
        if line.startswith('### ') or line.startswith('- ') or (line.strip()=='' and cur and cur[-1].strip()==''):
            if cur: out.append(''.join(cur)); cur=[]
        cur.append(line)
    if cur: out.append(''.join(cur))
    return out
pre_existing = [b for b in blocks(base_sec) if b.strip() and b.strip() in stamped]
new_sec = theirs_sec
for b in pre_existing:
    if b.startswith('### '):
        continue
    assert b in new_sec, f"branch edited pre-existing block; manual resolution needed:\n{b[:200]}"
    new_sec = new_sec.replace(b, '', 1)
# drop headers that now have no bullets, collapse blank runs
parts = re.split(r'(?m)(?=^### )', new_sec)
kept = []
for p in parts:
    if p.startswith('### ') and not re.search(r'(?m)^- ', p):
        continue
    kept.append(p)
new_sec = ''.join(kept)
new_sec = re.sub(r'\n{3,}', '\n\n', new_sec)
if not new_sec.startswith('\n'): new_sec = '\n' + new_sec
if not new_sec.endswith('\n\n'): new_sec = new_sec.rstrip('\n') + '\n\n'
resolved = ours[:os_] + new_sec + ours[oe:]
open('CHANGELOG.md','w').write(resolved)
print(f"resolved: removed {len([b for b in pre_existing if not b.startswith('### ')])} pre-existing block(s); unreleased section now {new_sec.count(chr(10))} lines")
