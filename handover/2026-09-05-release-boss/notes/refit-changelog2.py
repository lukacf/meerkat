#!/usr/bin/env python3
"""Rewrite CHANGELOG.md in the working tree so [Unreleased] = <onto>'s [Unreleased]
plus the delta of <old_commit>'s [Unreleased] over <old_base>'s; everything from the
first release header down is <onto>'s verbatim. Args: old_base old_commit onto."""
import re, subprocess, sys, pathlib
old_base, old_commit, onto = sys.argv[1:4]
def show(rev): return subprocess.check_output(['git','show',f'{rev}:CHANGELOG.md'], text=True)
def section(text):
    i = text.index('## [Unreleased]\n') + len('## [Unreleased]\n')
    m = re.search(r'^## \[\d', text[i:], re.M); return text[i:i+m.start()]
def by_header(sec):
    out, hdr = {}, None
    for block in re.split(r'(?m)(?=^### |^- )', sec):
        if block.startswith('### '): hdr = block.strip(); out.setdefault(hdr, [])
        elif block.startswith('- '):
            b = block.rstrip('\n') + '\n'
            if re.match(r'^(<<<<<<< |=======\s*$|>>>>>>> )', b): continue
            out.setdefault(hdr, []).append(b)
    return out
mine, base, onto_sec = by_header(section(show(old_commit))), by_header(section(show(old_base))), by_header(section(show(onto)))
base_all = {b for v in base.values() for b in v}
merged = {h: list(v) for h, v in onto_sec.items()}
for h, v in mine.items():
    for b in v:
        if b not in base_all and b not in merged.setdefault(h, []): merged[h].append(b)
order = ['### Breaking', '### Changed - BREAKING for host/SDK implementors', '### Added', '### Changed', '### Deprecated', '### Removed', '### Fixed', '### Security']
rank = lambda h: order.index(h) if h in order else len(order)
parts = [f'{h}\n\n' + '\n'.join(merged[h]) for h in sorted([h for h in merged if merged[h]], key=rank)]
onto_text = show(onto)
head = onto_text[:onto_text.index('## [Unreleased]\n') + len('## [Unreleased]\n')]
m = re.search(r'^## \[\d', onto_text[len(head):], re.M); tail = onto_text[len(head)+m.start():]
pathlib.Path('CHANGELOG.md').write_text(re.sub(r'\n{3,}', '\n\n', head + ('\n' + '\n'.join(parts) + '\n' if parts else '\n') + tail))
