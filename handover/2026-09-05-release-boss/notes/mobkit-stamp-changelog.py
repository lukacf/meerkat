#!/usr/bin/env python3
"""Stamp MobKit CHANGELOG.md: [Unreleased] -> [VERSION] - DATE, fresh [Unreleased], pin note."""
import re, sys, pathlib, datetime
version, meerkat = sys.argv[1], sys.argv[2]
p = pathlib.Path('CHANGELOG.md'); s = p.read_text()
today = datetime.date.today().isoformat()
assert s.count('## [Unreleased]\n') == 1
note = (f"### Changed\n\n- **Pinned to Meerkat {meerkat}.** All exact Meerkat dependency sites move to\n"
        f"  `={meerkat}`; this release pairs with Meerkat {meerkat}, which makes the\n"
        f"  reopened-gateway resume path wait for a slow stale-session teardown instead\n"
        f"  of failing with `UnregisterInProgress`.\n\n")
s = s.replace('## [Unreleased]\n', f'## [Unreleased]\n\n## [{version}] - {today}\n\n' + note, 1)
# tidy: collapse 3+ newlines
s = re.sub(r'\n{3,}', '\n\n', s)
p.write_text(s)
print(f"stamped {version} ({today}); unreleased section now empty")
