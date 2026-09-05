#!/usr/bin/env python3
"""During `git rebase --exec`: rewrite HEAD's CHANGELOG.md so that [Unreleased] =
<onto>'s [Unreleased] plus this commit's own delta (its old-branch counterpart's
[Unreleased] minus <old_base>'s [Unreleased]); everything from the first
release header down is taken verbatim from <onto>. Args: old_base old_tip onto."""
import re, subprocess, sys, pathlib
old_base, old_tip, onto = sys.argv[1:4]
def show(rev): return subprocess.check_output(['git','show',f'{rev}:CHANGELOG.md'], text=True)
def section(text):
    i = text.index('## [Unreleased]\n') + len('## [Unreleased]\n')
    m = re.search(r'^## \[\d', text[i:], re.M); return text[i:i+m.start()]
def bullets_by_header(sec):
    out, hdr = {}, None
    for block in re.split(r'(?m)(?=^### |^- )', sec):
        if block.startswith('### '): hdr = block.strip(); out.setdefault(hdr, [])
        elif block.startswith('- '):
            b = block.rstrip('\n') + '\n'
            if re.match(r'^(<<<<<<< |=======\s*$|>>>>>>> )', b): continue
            out.setdefault(hdr, []).append(b)
    return out
subject = subprocess.check_output(['git','log','-1','--format=%s','HEAD'], text=True).strip()
olds = subprocess.check_output(['git','log','--format=%H%x00%s', f'{old_base}..{old_tip}'], text=True).strip().split('\n')
old_commit = next(h for line in olds for h, s in [line.split('\x00',1)] if s == subject)
mine = bullets_by_header(section(show(old_commit)))
base = bullets_by_header(section(show(old_base)))
onto_sec = bullets_by_header(section(show(onto)))
base_all = {b for v in base.values() for b in v}
merged = {h: list(v) for h, v in onto_sec.items()}
for h, v in mine.items():
    for b in v:
        if b not in base_all and b not in merged.setdefault(h, []): merged[h].append(b)
order = ['### Breaking', '### Changed - BREAKING for host/SDK implementors', '### Added', '### Changed', '### Deprecated', '### Removed', '### Fixed', '### Security']
def rank(h): return order.index(h) if h in order else len(order)
parts = []
for h in sorted([h for h in merged if merged[h]], key=rank):
    parts.append(f'{h}\n\n' + '\n'.join(merged[h]) )
onto_text = show(onto)
head = onto_text[:onto_text.index('## [Unreleased]\n') + len('## [Unreleased]\n')]
m = re.search(r'^## \[\d', onto_text[len(head):], re.M); tail = onto_text[len(head)+m.start():]
new = head + ('\n' + '\n'.join(parts) + '\n' if parts else '\n') + tail
pathlib.Path('CHANGELOG.md').write_text(re.sub(r'\n{3,}', '\n\n', new))
