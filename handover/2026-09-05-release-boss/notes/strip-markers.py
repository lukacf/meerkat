import re, pathlib
p = pathlib.Path('CHANGELOG.md'); lines = p.read_text().splitlines(keepends=True)
kept = [l for l in lines if not re.match(r'^(<<<<<<< |=======\s*$|>>>>>>> )', l)]
if len(kept) != len(lines): p.write_text(''.join(kept)); print(f"stripped {len(lines)-len(kept)} marker line(s)")
