#!/usr/bin/env python3
"""Resolve CHANGELOG.md rebase conflicts where both sides added entries under the
same [Unreleased] headers. Per conflict hunk, keep every block main added (not in
base), keep every block theirs kept or added, drop blocks theirs removed/edited."""
import re, subprocess, pathlib
base = subprocess.check_output(['git','show',':1:CHANGELOG.md'], text=True)
theirs_full = subprocess.check_output(['git','show',':3:CHANGELOG.md'], text=True)
text = pathlib.Path('CHANGELOG.md').read_text()
def blocks(lines):
    out, cur = [], []
    for l in lines:
        starts = l.startswith('- ') or l.startswith('#') or l.startswith('<<<<<<<') or l.startswith('=======') or l.startswith('>>>>>>>') or (l.strip() and not l.startswith(' '))
        if starts and cur:
            out.append(''.join(cur)); cur = []
        cur.append(l)
    if cur: out.append(''.join(cur))
    return out
def norm(b): return b.strip('\n')
pat = re.compile(r'^<<<<<<< [^\n]*\n(.*?)^=======\n(.*?)^>>>>>>> [^\n]*\n', re.S | re.M)
count = 0
def repl(m):
    global count; count += 1
    ours_b = blocks(m.group(1).splitlines(keepends=True))
    theirs_b = blocks(m.group(2).splitlines(keepends=True))
    result = []
    for b in ours_b:
        if not norm(b): continue
        in_base = norm(b) in base
        kept_by_theirs = norm(b) in theirs_full
        if (not in_base) or kept_by_theirs:
            result.append(b)
    def is_marker(b):
        return re.match(r'^(<<<<<<< |=======\s*$|>>>>>>> )', b) is not None
    result = [b for b in result if not is_marker(b)]
    for b in theirs_b:
        if norm(b) and not is_marker(b) and norm(b) not in [norm(x) for x in result]:
            result.append(b)
    out = ''.join(x if x.endswith('\n') else x + '\n' for x in result)
    # keep bullet blocks separated by one blank line
    out = re.sub(r'\n{3,}', '\n\n', out)
    return out
resolved = pat.sub(repl, text)
assert not re.search(r'^(<<<<<<< |=======$|>>>>>>> )', resolved, re.M), "unresolved markers remain"
pathlib.Path('CHANGELOG.md').write_text(resolved)
print(f"resolved {count} hunk(s)")
